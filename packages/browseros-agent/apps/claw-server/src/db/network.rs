//! Durable network mock state: handoff snapshots, rules, revisions and hits.
//!
//! Capture records never reach this module; they are session-scoped memory plus
//! temporary files (D3).
//!
//! Every write takes a `ConnectionTrait` so callers can run them inside a
//! transaction. Reads use the repository's own connection.

use crate::{
    db::Database,
    error::{AppError, AppResult},
};
use browseros_core::network::{EndpointMatcher, HeaderPair, RuleAction};
use futures_util::future::BoxFuture;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult,
    Statement, TransactionTrait, TryGetable, Value,
};
use serde_json::Value as Json;

fn db(error: DbErr) -> AppError {
    AppError::Db(Box::new(error))
}

fn get<T: TryGetable>(row: &QueryResult, column: &str) -> AppResult<T> {
    row.try_get("", column)
        .map_err(|error| AppError::Db(Box::new(error)))
}

fn get_opt<T: TryGetable>(row: &QueryResult, column: &str) -> AppResult<Option<T>> {
    row.try_get::<Option<T>>("", column)
        .map_err(|error| AppError::Db(Box::new(error)))
}

/// Runtime state of a rule. Paired with `enabled`, which carries the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleState {
    /// The browser acknowledged Fetch interception for this rule.
    Active,
    /// Saved, but not confirmed yet; see [`StateReason`].
    Pending,
    /// The bound tab is gone, so the rule cannot be applied.
    Unbound,
    /// The stored rule cannot be used (missing body, bad script, ...).
    Failed,
    /// Intent is off, or the rule lost a replacement or a stop-all.
    Disabled,
}

impl RuleState {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Unbound => "unbound",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
        }
    }

    /// Unknown values are a hard error: silently mapping them hides data bugs.
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "active" => Ok(Self::Active),
            "pending" => Ok(Self::Pending),
            "unbound" => Ok(Self::Unbound),
            "failed" => Ok(Self::Failed),
            "disabled" => Ok(Self::Disabled),
            other => Err(AppError::Internal(format!(
                "unknown net_rules.state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateReason {
    NoBrowser,
    NoTab,
    ApplyTimeout,
    TabClosed,
    BodyMissing,
    BodyTooLarge,
    ProcessorInvalid,
    User,
    StoppedAll,
    ReplacedBy(String),
    Superseded,
}

impl StateReason {
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::NoBrowser => "no_browser".to_string(),
            Self::NoTab => "no_tab".to_string(),
            Self::ApplyTimeout => "apply_timeout".to_string(),
            Self::TabClosed => "tab_closed".to_string(),
            Self::BodyMissing => "body_missing".to_string(),
            Self::BodyTooLarge => "body_too_large".to_string(),
            Self::ProcessorInvalid => "processor_invalid".to_string(),
            Self::User => "user".to_string(),
            Self::StoppedAll => "stopped_all".to_string(),
            Self::ReplacedBy(id) => format!("replaced_by:{id}"),
            Self::Superseded => "superseded".to_string(),
        }
    }

    pub fn parse(value: &str) -> AppResult<Self> {
        let replaced = value.strip_prefix("replaced_by:");
        match value {
            "no_browser" => Ok(Self::NoBrowser),
            "no_tab" => Ok(Self::NoTab),
            "apply_timeout" => Ok(Self::ApplyTimeout),
            "tab_closed" => Ok(Self::TabClosed),
            "body_missing" => Ok(Self::BodyMissing),
            "body_too_large" => Ok(Self::BodyTooLarge),
            "processor_invalid" => Ok(Self::ProcessorInvalid),
            "user" => Ok(Self::User),
            "stopped_all" => Ok(Self::StoppedAll),
            "superseded" => Ok(Self::Superseded),
            other => match replaced {
                Some(id) => Ok(Self::ReplacedBy(id.to_string())),
                None => Err(AppError::Internal(format!(
                    "unknown net_rules.state_reason: {other}"
                ))),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct HandoffRow {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub record_id: String,
    pub source_meta: Json,
    pub request_headers: Vec<HeaderPair>,
    pub response_headers: Vec<HeaderPair>,
    pub request_body: Option<String>,
    pub body_id: Option<String>,
    pub body_bytes: u64,
    pub body_state: String,
    pub body_reason: Option<String>,
    pub matcher_seed: Json,
    pub suggested_ignore: Json,
    pub related: Json,
    pub rule_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    AllTabs,
    Tab,
}

impl ScopeKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AllTabs => "all_tabs",
            Self::Tab => "tab",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        if value == "tab" {
            Self::Tab
        } else {
            Self::AllTabs
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuleRow {
    pub id: String,
    pub name: String,
    pub scope_kind: ScopeKind,
    pub scope_tab: Option<i64>,
    /// Intent: does the user or the AI want this rule on?
    pub enabled: bool,
    pub state: RuleState,
    pub state_reason: Option<StateReason>,
    pub current_revision: u32,
    pub hit_count: u64,
    pub last_hit_at: Option<i64>,
    pub author_client: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RuleRevisionRow {
    pub rule_id: String,
    pub revision: u32,
    pub matcher: EndpointMatcher,
    /// The stored action; bodies live in the body store, never inline.
    pub action: RuleAction,
    pub body_id: Option<String>,
    /// Revision whose sample file holds the rule's sample body.
    pub sample_rev: u32,
    /// `false` when the seeding handoff had no body.
    pub sample_available: bool,
    pub sample_status: Option<i64>,
    pub sample_headers: Vec<HeaderPair>,
    /// `{method, url, headers, body}` captured when the sample was seeded.
    pub sample_request: Json,
    pub source_handoff_id: Option<String>,
    pub source_record_id: Option<String>,
    pub source_tab_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct HitRow {
    pub id: String,
    pub rule_id: String,
    pub revision: u32,
    pub at_ms: i64,
    pub tab_title: Option<String>,
    pub url: String,
    pub method: String,
    pub outcome: String,
    pub reason: Option<String>,
    pub network_id: Option<String>,
}

#[derive(Clone)]
pub struct NetworkRepository {
    db: Database,
}

impl NetworkRepository {
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Runs one statement group on the pooled connection.
    ///
    /// The pool owns a single connection and `Database::open` already holds
    /// it, so this hands that connection over instead of acquiring another.
    /// Nesting one statement group inside another would deadlock.
    pub async fn stmt<F, T>(&self, work: F) -> AppResult<T>
    where
        F: for<'a> FnOnce(&'a DatabaseConnection) -> BoxFuture<'a, AppResult<T>>,
    {
        work(self.db.connection()).await
    }

    pub async fn begin(&self) -> AppResult<DatabaseTransaction> {
        self.db
            .connection()
            .begin()
            .await
            .map_err(|error| AppError::Db(Box::new(error)))
    }

    // ---- rules ---------------------------------------------------------

    pub async fn list_rules(&self, include_deleted: bool) -> AppResult<Vec<RuleRow>> {
        let sql = if include_deleted {
            "SELECT * FROM net_rules ORDER BY updated_at DESC"
        } else {
            "SELECT * FROM net_rules WHERE deleted_at IS NULL ORDER BY updated_at DESC"
        };
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                sql.to_string(),
            ))
            .await
            .map_err(db)?;
        rows.iter().map(rule_from_row).collect()
    }

    pub async fn get_rule(&self, id: &str) -> AppResult<Option<RuleRow>> {
        Self::get_rule_from(self.db.connection(), id).await
    }

    pub async fn get_rule_from<C: ConnectionTrait>(
        connection: &C,
        id: &str,
    ) -> AppResult<Option<RuleRow>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rules WHERE id = ?",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.first().map(rule_from_row).transpose()
    }

    /// Names are unique among live rules, ASCII-case-insensitively.
    pub async fn find_rule_by_name(&self, name: &str) -> AppResult<Option<RuleRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rules WHERE deleted_at IS NULL AND name = ? COLLATE NOCASE",
                [Value::from(name.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.first().map(rule_from_row).transpose()
    }

    /// Lower-cased names of every live rule, for default-name de-duplication.
    pub async fn rule_names_lowercase(&self) -> AppResult<Vec<String>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT name FROM net_rules WHERE deleted_at IS NULL".to_string(),
            ))
            .await
            .map_err(db)?;
        rows.iter()
            .map(|row| get::<String>(row, "name").map(|name| name.to_ascii_lowercase()))
            .collect()
    }

    pub async fn insert_rule<C: ConnectionTrait>(
        &self,
        connection: &C,
        rule: &RuleRow,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_rules
                 (id, name, scope_kind, scope_tab, enabled, state, state_reason, current_revision,
                  hit_count, last_hit_at, author_client, created_at, updated_at, deleted_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(rule.id.clone()),
                    Value::from(rule.name.clone()),
                    Value::from(rule.scope_kind.as_str().to_owned()),
                    Value::from(rule.scope_tab),
                    Value::from(i64::from(rule.enabled)),
                    Value::from(rule.state.as_str().to_owned()),
                    Value::from(rule.state_reason.as_ref().map(StateReason::as_str)),
                    Value::from(i64::from(rule.current_revision)),
                    Value::from(rule.hit_count as i64),
                    Value::from(rule.last_hit_at),
                    Value::from(rule.author_client.clone()),
                    Value::from(rule.created_at),
                    Value::from(rule.updated_at),
                    Value::from(rule.deleted_at),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn update_rule_scope<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        scope_kind: &ScopeKind,
        scope_tab: Option<i64>,
        updated_at: i64,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET scope_kind = ?, scope_tab = ?, updated_at = ? WHERE id = ?",
                [
                    Value::from(scope_kind.as_str().to_owned()),
                    Value::from(scope_tab),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn update_rule_name<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        name: &str,
        updated_at: i64,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET name = ?, updated_at = ? WHERE id = ?",
                [
                    Value::from(name.to_owned()),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn update_rule_intent<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        enabled: bool,
        state: RuleState,
        reason: Option<&StateReason>,
        updated_at: i64,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET enabled = ?, state = ?, state_reason = ?, updated_at = ?
                 WHERE id = ?",
                [
                    Value::from(i64::from(enabled)),
                    Value::from(state.as_str().to_owned()),
                    Value::from(reason.map(StateReason::as_str)),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Rewrites revision, scope and author in one statement.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_rule_revision<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        revision: u32,
        scope_kind: &ScopeKind,
        scope_tab: Option<i64>,
        author_client: Option<&str>,
        updated_at: i64,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET current_revision = ?, scope_kind = ?, scope_tab = ?,
                  author_client = ?, updated_at = ? WHERE id = ?",
                [
                    Value::from(i64::from(revision)),
                    Value::from(scope_kind.as_str().to_owned()),
                    Value::from(scope_tab),
                    Value::from(author_client.map(ToString::to_string)),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Sets the same (state, reason) on many rules; returns the ids updated.
    pub async fn set_state_for<C: ConnectionTrait>(
        &self,
        connection: &C,
        ids: &[String],
        state: RuleState,
        reason: Option<&StateReason>,
        updated_at: i64,
    ) -> AppResult<Vec<String>> {
        let mut updated = Vec::new();
        for id in ids {
            let rows = connection
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    // `enabled = 1` matters: a stop-all landing while the
                    // snapshot is being applied must not be reverted.
                    "UPDATE net_rules SET state = ?, state_reason = ?, updated_at = ?
                     WHERE id = ? AND deleted_at IS NULL AND enabled = 1",
                    [
                        Value::from(state.as_str().to_owned()),
                        Value::from(reason.map(StateReason::as_str)),
                        Value::from(updated_at),
                        Value::from(id.clone()),
                    ],
                ))
                .await
                .map_err(db)?
                .rows_affected();
            if rows > 0 {
                updated.push(id.clone());
            }
        }
        Ok(updated)
    }

    /// `pending` → `active` for rules inside the current snapshot, once the
    /// runtime confirms it. Returns the ids that changed.
    pub async fn promote_pending<C: ConnectionTrait>(
        &self,
        connection: &C,
        ids: &[String],
        updated_at: i64,
    ) -> AppResult<Vec<String>> {
        let mut updated = Vec::new();
        for id in ids {
            let rows = connection
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    "UPDATE net_rules SET state = 'active', state_reason = NULL, updated_at = ?
                     WHERE id = ? AND deleted_at IS NULL AND enabled = 1 AND state = 'pending'",
                    [Value::from(updated_at), Value::from(id.clone())],
                ))
                .await
                .map_err(db)?
                .rows_affected();
            if rows > 0 {
                updated.push(id.clone());
            }
        }
        Ok(updated)
    }

    /// The browser disconnected: nothing is confirmed any more.
    pub async fn demote_active_to_pending<C: ConnectionTrait>(
        &self,
        connection: &C,
        updated_at: i64,
    ) -> AppResult<Vec<String>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT id FROM net_rules WHERE deleted_at IS NULL AND enabled = 1
                 AND state = 'active'",
                [],
            ))
            .await
            .map_err(db)?;
        let ids = rows
            .iter()
            .map(|row| get::<String>(row, "id"))
            .collect::<AppResult<Vec<_>>>()?;
        if ids.is_empty() {
            return Ok(ids);
        }
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET state = 'pending', state_reason = 'no_browser',
                  updated_at = ?
                 WHERE deleted_at IS NULL AND enabled = 1 AND state = 'active'",
                [Value::from(updated_at)],
            ))
            .await
            .map_err(db)?;
        Ok(ids)
    }

    /// Disables every enabled rule; used by "stop all mocks".
    pub async fn disable_all_rules<C: ConnectionTrait>(
        &self,
        connection: &C,
        updated_at: i64,
    ) -> AppResult<Vec<String>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT id FROM net_rules WHERE deleted_at IS NULL AND enabled = 1",
                [],
            ))
            .await
            .map_err(db)?;
        let ids = rows
            .iter()
            .map(|row| get::<String>(row, "id"))
            .collect::<AppResult<Vec<_>>>()?;
        if ids.is_empty() {
            return Ok(ids);
        }
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET enabled = 0, state = 'disabled',
                  state_reason = 'stopped_all', updated_at = ?
                 WHERE deleted_at IS NULL AND enabled = 1",
                [Value::from(updated_at)],
            ))
            .await
            .map_err(db)?;
        Ok(ids)
    }

    pub async fn soft_delete_rule<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        at: i64,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
                [Value::from(at), Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Marks every live rule deleted. Returns the ids so the caller can undo.
    pub async fn soft_delete_all_rules<C: ConnectionTrait>(
        &self,
        connection: &C,
        at: i64,
    ) -> AppResult<Vec<String>> {
        let rows = connection
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT id FROM net_rules WHERE deleted_at IS NULL".to_string(),
            ))
            .await
            .map_err(db)?;
        let ids = rows
            .iter()
            .map(|row| get::<String>(row, "id"))
            .collect::<AppResult<Vec<_>>>()?;
        if ids.is_empty() {
            return Ok(ids);
        }
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET deleted_at = ? WHERE deleted_at IS NULL",
                [Value::from(at)],
            ))
            .await
            .map_err(db)?;
        Ok(ids)
    }

    /// Undeletes without touching intent or state: whatever the user chose
    /// before deleting is what comes back.
    pub async fn restore_rule<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        updated_at: i64,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET deleted_at = NULL, updated_at = ?
                 WHERE id = ? AND deleted_at IS NOT NULL",
                [Value::from(updated_at), Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Deletes rules whose undo window closed. Returns the ids so the caller
    /// can release their body directories.
    pub async fn purge_deleted_rules<C: ConnectionTrait>(
        &self,
        connection: &C,
        before: i64,
    ) -> AppResult<Vec<String>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT id FROM net_rules WHERE deleted_at IS NOT NULL AND deleted_at < ?",
                [Value::from(before)],
            ))
            .await
            .map_err(db)?;
        let ids = rows
            .iter()
            .map(|row| get::<String>(row, "id"))
            .collect::<AppResult<Vec<_>>>()?;
        for id in &ids {
            Self::hard_delete_rule(connection, id).await?;
        }
        Ok(ids)
    }

    pub async fn hard_delete_rule<C: ConnectionTrait>(connection: &C, id: &str) -> AppResult<()> {
        for sql in [
            "DELETE FROM net_rule_hits WHERE rule_id = ?",
            "DELETE FROM net_rule_revisions WHERE rule_id = ?",
            "DELETE FROM net_rules WHERE id = ?",
        ] {
            connection
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    sql,
                    [Value::from(id.to_owned())],
                ))
                .await
                .map_err(db)?;
        }
        Ok(())
    }

    // ---- revisions -----------------------------------------------------

    pub async fn insert_revision<C: ConnectionTrait>(
        &self,
        connection: &C,
        revision: &RuleRevisionRow,
    ) -> AppResult<()> {
        let action = serde_json::to_string(&revision.action)
            .map_err(|error| AppError::Internal(format!("serialize action: {error}")))?;
        let matcher = serde_json::to_string(&revision.matcher)
            .map_err(|error| AppError::Internal(format!("serialize matcher: {error}")))?;
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_rule_revisions
                 (rule_id, revision, matcher_json, action_json, body_id, sample_rev,
                  sample_available, sample_status, sample_headers_json, sample_request_json,
                  source_handoff_id, source_record_id, source_tab_id, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(revision.rule_id.clone()),
                    Value::from(i64::from(revision.revision)),
                    Value::from(matcher),
                    Value::from(action),
                    Value::from(revision.body_id.clone()),
                    Value::from(i64::from(revision.sample_rev)),
                    Value::from(i64::from(revision.sample_available)),
                    Value::from(revision.sample_status),
                    Value::from(
                        serde_json::to_string(&revision.sample_headers).unwrap_or_default(),
                    ),
                    Value::from(revision.sample_request.to_string()),
                    Value::from(revision.source_handoff_id.clone()),
                    Value::from(revision.source_record_id.clone()),
                    Value::from(revision.source_tab_id),
                    Value::from(revision.created_at),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_revision(
        &self,
        rule_id: &str,
        revision: u32,
    ) -> AppResult<Option<RuleRevisionRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rule_revisions WHERE rule_id = ? AND revision = ?",
                [
                    Value::from(rule_id.to_owned()),
                    Value::from(i64::from(revision)),
                ],
            ))
            .await
            .map_err(db)?;
        rows.first().map(revision_from_row).transpose()
    }

    pub async fn list_revisions(&self, rule_id: &str) -> AppResult<Vec<RuleRevisionRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rule_revisions WHERE rule_id = ? ORDER BY revision DESC",
                [Value::from(rule_id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(revision_from_row).collect()
    }

    /// The current revision of every live rule, in one query.
    pub async fn list_current_revisions(
        &self,
        rule_ids: &[String],
    ) -> AppResult<Vec<RuleRevisionRow>> {
        if rule_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = rule_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let values = rule_ids
            .iter()
            .map(|id| Value::from(id.clone()))
            .collect::<Vec<_>>();
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                format!(
                    "SELECT r.* FROM net_rule_revisions r
                     JOIN net_rules n ON n.id = r.rule_id
                     WHERE n.deleted_at IS NULL AND r.rule_id IN ({placeholders})
                       AND r.revision = n.current_revision"
                ),
                values,
            ))
            .await
            .map_err(db)?;
        rows.iter().map(revision_from_row).collect()
    }

    // ---- handoffs ------------------------------------------------------

    pub async fn insert_handoff<C: ConnectionTrait>(
        &self,
        connection: &C,
        handoff: &HandoffRow,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_handoffs
                 (id, created_at, expires_at, record_id, source_meta_json, request_headers_json,
                  response_headers_json, request_body, body_id, body_bytes, body_state,
                  body_reason, matcher_seed_json, suggested_ignore_json, related_json, rule_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(handoff.id.clone()),
                    Value::from(handoff.created_at),
                    Value::from(handoff.expires_at),
                    Value::from(handoff.record_id.clone()),
                    Value::from(handoff.source_meta.to_string()),
                    Value::from(
                        serde_json::to_string(&handoff.request_headers).unwrap_or_default(),
                    ),
                    Value::from(
                        serde_json::to_string(&handoff.response_headers).unwrap_or_default(),
                    ),
                    Value::from(handoff.request_body.clone()),
                    Value::from(handoff.body_id.clone()),
                    Value::from(handoff.body_bytes as i64),
                    Value::from(handoff.body_state.clone()),
                    Value::from(handoff.body_reason.clone()),
                    Value::from(handoff.matcher_seed.to_string()),
                    Value::from(handoff.suggested_ignore.to_string()),
                    Value::from(handoff.related.to_string()),
                    Value::from(handoff.rule_id.clone()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_handoff(&self, id: &str) -> AppResult<Option<HandoffRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs WHERE id = ?",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.first().map(handoff_from_row).transpose()
    }

    /// Copying a reference twice must return the same one.
    pub async fn find_handoff_by_record(
        &self,
        record_id: &str,
        now: i64,
    ) -> AppResult<Option<HandoffRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs WHERE record_id = ? AND expires_at > ?
                 ORDER BY created_at DESC LIMIT 1",
                [Value::from(record_id.to_owned()), Value::from(now)],
            ))
            .await
            .map_err(db)?;
        rows.first().map(handoff_from_row).transpose()
    }

    pub async fn list_handoffs(&self) -> AppResult<Vec<HandoffRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs ORDER BY created_at DESC".to_string(),
            ))
            .await
            .map_err(db)?;
        rows.iter().map(handoff_from_row).collect()
    }

    /// Oldest live handoffs first: the recycle order for both quotas.
    pub async fn list_handoffs_by_age<C: ConnectionTrait>(
        &self,
        connection: &C,
        now: i64,
    ) -> AppResult<Vec<HandoffRow>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs WHERE expires_at > ? ORDER BY created_at ASC",
                [Value::from(now)],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(handoff_from_row).collect()
    }

    pub async fn list_expired_handoffs<C: ConnectionTrait>(
        &self,
        connection: &C,
        now: i64,
    ) -> AppResult<Vec<HandoffRow>> {
        let rows = connection
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs WHERE expires_at <= ?",
                [Value::from(now)],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(handoff_from_row).collect()
    }

    pub async fn total_handoff_body_bytes(&self) -> AppResult<u64> {
        let row = self
            .db
            .connection()
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COALESCE(SUM(body_bytes), 0) AS total FROM net_handoffs".to_string(),
            ))
            .await
            .map_err(db)?;
        let Some(row) = row else {
            return Ok(0);
        };
        let total: i64 = get(&row, "total")?;
        Ok(total.max(0) as u64)
    }

    pub async fn delete_handoff<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM net_handoffs WHERE id = ?",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn set_handoff_rule<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: &str,
        rule_id: Option<&str>,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_handoffs SET rule_id = ? WHERE id = ?",
                [
                    Value::from(rule_id.map(ToString::to_string)),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    // ---- hits ----------------------------------------------------------

    pub async fn insert_hit<C: ConnectionTrait>(
        &self,
        connection: &C,
        hit: &HitRow,
    ) -> AppResult<()> {
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_rule_hits
                 (id, rule_id, revision, at_ms, tab_title, url, method, outcome, reason, network_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(hit.id.clone()),
                    Value::from(hit.rule_id.clone()),
                    Value::from(i64::from(hit.revision)),
                    Value::from(hit.at_ms),
                    Value::from(hit.tab_title.clone()),
                    Value::from(hit.url.clone()),
                    Value::from(hit.method.clone()),
                    Value::from(hit.outcome.clone()),
                    Value::from(hit.reason.clone()),
                    Value::from(hit.network_id.clone()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// `counted` is true for the outcomes that really hit (fulfilled,
    /// processed); follow-up outcomes only refresh `last_hit_at`.
    pub async fn increment_hit<C: ConnectionTrait>(
        &self,
        connection: &C,
        rule_id: &str,
        at: i64,
        counted: bool,
    ) -> AppResult<()> {
        let sql = if counted {
            "UPDATE net_rules SET hit_count = hit_count + 1, last_hit_at = ? WHERE id = ?"
        } else {
            "UPDATE net_rules SET last_hit_at = ? WHERE id = ?"
        };
        connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                sql,
                [Value::from(at), Value::from(rule_id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Rewrites the outcome of an already recorded hit (blocked, failed, ...).
    /// Returns the number of rows changed; `0` means "insert instead".
    pub async fn update_hit_outcome<C: ConnectionTrait>(
        &self,
        connection: &C,
        rule_id: &str,
        network_id: &str,
        outcome: &str,
        reason: Option<&str>,
    ) -> AppResult<u64> {
        let result = connection
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rule_hits SET outcome = ?, reason = ?
                 WHERE rule_id = ? AND network_id = ?",
                [
                    Value::from(outcome.to_owned()),
                    Value::from(reason.map(ToString::to_string)),
                    Value::from(rule_id.to_owned()),
                    Value::from(network_id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn list_hits<C: ConnectionTrait>(
        &self,
        connection: &C,
        rule_id: Option<&str>,
        limit: u32,
    ) -> AppResult<Vec<HitRow>> {
        let rows = match rule_id {
            Some(rule_id) => connection
                .query_all(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    "SELECT * FROM net_rule_hits WHERE rule_id = ?
                         ORDER BY at_ms DESC LIMIT ?",
                    [
                        Value::from(rule_id.to_owned()),
                        Value::from(i64::from(limit)),
                    ],
                ))
                .await
                .map_err(db)?,
            None => self
                .db
                .connection()
                .query_all(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    "SELECT * FROM net_rule_hits ORDER BY at_ms DESC LIMIT ?",
                    [Value::from(i64::from(limit))],
                ))
                .await
                .map_err(db)?,
        };
        rows.iter().map(hit_from_row).collect()
    }

    pub async fn delete_hits<C: ConnectionTrait>(
        &self,
        connection: &C,
        rule_id: Option<&str>,
    ) -> AppResult<u64> {
        let result = match rule_id {
            Some(rule_id) => connection
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    "DELETE FROM net_rule_hits WHERE rule_id = ?",
                    [Value::from(rule_id.to_owned())],
                ))
                .await
                .map_err(db)?,
            None => connection
                .execute(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    "DELETE FROM net_rule_hits".to_string(),
                ))
                .await
                .map_err(db)?,
        };
        Ok(result.rows_affected())
    }
}

fn rule_from_row(row: &QueryResult) -> AppResult<RuleRow> {
    let state = RuleState::parse(&get::<String>(row, "state")?)?;
    let reason = get_opt::<String>(row, "state_reason")?
        .map(|value| StateReason::parse(&value))
        .transpose()?;
    Ok(RuleRow {
        id: get(row, "id")?,
        name: get(row, "name")?,
        scope_kind: ScopeKind::parse(&get::<String>(row, "scope_kind")?),
        scope_tab: get_opt(row, "scope_tab")?,
        enabled: get::<i64>(row, "enabled")? != 0,
        state,
        state_reason: reason,
        current_revision: get::<i64>(row, "current_revision")? as u32,
        hit_count: get::<i64>(row, "hit_count")?.max(0) as u64,
        last_hit_at: get_opt(row, "last_hit_at")?,
        author_client: get_opt(row, "author_client")?,
        created_at: get(row, "created_at")?,
        updated_at: get(row, "updated_at")?,
        deleted_at: get_opt(row, "deleted_at")?,
    })
}

fn revision_from_row(row: &QueryResult) -> AppResult<RuleRevisionRow> {
    let matcher: EndpointMatcher =
        serde_json::from_str(&get::<String>(row, "matcher_json")?).unwrap_or_default();
    let action: RuleAction = serde_json::from_str(&get::<String>(row, "action_json")?)
        .map_err(|error| AppError::Internal(format!("stored rule action is invalid: {error}")))?;
    Ok(RuleRevisionRow {
        rule_id: get(row, "rule_id")?,
        revision: get::<i64>(row, "revision")? as u32,
        matcher,
        action,
        body_id: get_opt(row, "body_id")?,
        sample_rev: get::<i64>(row, "sample_rev")? as u32,
        sample_available: get::<i64>(row, "sample_available")? != 0,
        sample_status: get_opt(row, "sample_status")?,
        sample_headers: serde_json::from_str(&get::<String>(row, "sample_headers_json")?)
            .unwrap_or_default(),
        sample_request: serde_json::from_str(&get::<String>(row, "sample_request_json")?)
            .unwrap_or(Json::Null),
        source_handoff_id: get_opt(row, "source_handoff_id")?,
        source_record_id: get_opt(row, "source_record_id")?,
        source_tab_id: get_opt(row, "source_tab_id")?,
        created_at: get(row, "created_at")?,
    })
}

fn handoff_from_row(row: &QueryResult) -> AppResult<HandoffRow> {
    Ok(HandoffRow {
        id: get(row, "id")?,
        created_at: get(row, "created_at")?,
        expires_at: get(row, "expires_at")?,
        record_id: get(row, "record_id")?,
        source_meta: serde_json::from_str(&get::<String>(row, "source_meta_json")?)
            .unwrap_or(Json::Null),
        request_headers: serde_json::from_str(&get::<String>(row, "request_headers_json")?)
            .unwrap_or_default(),
        response_headers: serde_json::from_str(&get::<String>(row, "response_headers_json")?)
            .unwrap_or_default(),
        request_body: get_opt(row, "request_body")?,
        body_id: get_opt(row, "body_id")?,
        body_bytes: get::<i64>(row, "body_bytes")?.max(0) as u64,
        body_state: get(row, "body_state")?,
        body_reason: get_opt(row, "body_reason")?,
        matcher_seed: serde_json::from_str(&get::<String>(row, "matcher_seed_json")?)
            .unwrap_or(Json::Null),
        suggested_ignore: serde_json::from_str(&get::<String>(row, "suggested_ignore_json")?)
            .unwrap_or(Json::Null),
        related: serde_json::from_str(&get::<String>(row, "related_json")?).unwrap_or(Json::Null),
        rule_id: get_opt(row, "rule_id")?,
    })
}

fn hit_from_row(row: &QueryResult) -> AppResult<HitRow> {
    Ok(HitRow {
        id: get(row, "id")?,
        rule_id: get(row, "rule_id")?,
        revision: get::<i64>(row, "revision")? as u32,
        at_ms: get(row, "at_ms")?,
        tab_title: get_opt(row, "tab_title")?,
        url: get(row, "url")?,
        method: get(row, "method")?,
        outcome: get(row, "outcome")?,
        reason: get_opt(row, "reason")?,
        network_id: get_opt(row, "network_id")?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::db::Database;
    use tempfile::tempdir;

    async fn repository() -> anyhow::Result<(tempfile::TempDir, NetworkRepository)> {
        let directory = tempdir()?;
        let database = Database::open(directory.path().join("browserclaw.sqlite")).await?;
        Ok((directory, NetworkRepository::new(database)))
    }

    fn rule(id: &str, name: &str) -> RuleRow {
        RuleRow {
            id: id.to_string(),
            name: name.to_string(),
            scope_kind: ScopeKind::AllTabs,
            scope_tab: None,
            enabled: true,
            state: RuleState::Pending,
            state_reason: Some(StateReason::NoBrowser),
            current_revision: 1,
            hit_count: 0,
            last_hit_at: None,
            author_client: None,
            created_at: 1,
            updated_at: 1,
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn name_unique_index_is_case_insensitive_and_ignores_deleted() -> anyhow::Result<()> {
        let (_directory, repository) = repository().await?;
        let transaction = repository.begin().await?;
        let inserted = rule("rule_a", "Foo");
        repository.insert_rule(&transaction, &inserted).await?;
        transaction.commit().await?;

        let transaction = repository.begin().await?;
        let duplicate = repository
            .insert_rule(&transaction, &rule("rule_b", "foo"))
            .await;
        assert!(duplicate.is_err(), "names collide case-insensitively");
        transaction.rollback().await?;

        let transaction = repository.begin().await?;
        assert_eq!(
            repository
                .soft_delete_rule(&transaction, "rule_a", 9)
                .await?,
            1
        );
        transaction.commit().await?;

        let transaction = repository.begin().await?;
        repository
            .insert_rule(&transaction, &rule("rule_b", "foo"))
            .await?;
        transaction.commit().await?;
        assert_eq!(repository.list_rules(false).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn state_and_reason_round_trip_through_the_row_types() -> anyhow::Result<()> {
        let (_directory, repository) = repository().await?;
        let transaction = repository.begin().await?;
        let mut row = rule("rule_a", "Foo");
        row.state = RuleState::Disabled;
        row.state_reason = Some(StateReason::ReplacedBy("rule_z".to_string()));
        repository.insert_rule(&transaction, &row).await?;
        let stored = NetworkRepository::get_rule_from(&transaction, "rule_a")
            .await?
            .expect("stored rule");
        transaction.commit().await?;
        assert_eq!(stored.state, RuleState::Disabled);
        assert_eq!(
            stored.state_reason,
            Some(StateReason::ReplacedBy("rule_z".to_string()))
        );
        assert_eq!(
            StateReason::parse("no_tab")?,
            StateReason::NoTab,
            "reasons round trip"
        );
        assert!(
            RuleState::parse("legacy").is_err(),
            "unknown states are not silently mapped"
        );
        Ok(())
    }

    #[tokio::test]
    async fn hit_follow_up_updates_the_existing_row() -> anyhow::Result<()> {
        let (_directory, repository) = repository().await?;
        let seed = rule("rule_a", "Foo");
        let row = HitRow {
            id: "hit_1".to_string(),
            rule_id: "rule_a".to_string(),
            revision: 1,
            at_ms: 100,
            tab_title: None,
            url: "https://example.test/x".to_string(),
            method: "GET".to_string(),
            outcome: "fulfilled".to_string(),
            reason: None,
            network_id: Some("1.2".to_string()),
        };
        let (updated, hits, stored) = repository
            .stmt(|connection| {
                let repository = repository.clone();
                let seed = seed.clone();
                let row = row.clone();
                Box::pin(async move {
                    repository.insert_rule(connection, &seed).await?;
                    repository
                        .increment_hit(connection, "rule_a", 100, true)
                        .await?;
                    repository
                        .increment_hit(connection, "rule_a", 110, false)
                        .await?;
                    repository.insert_hit(connection, &row).await?;
                    let updated = repository
                        .update_hit_outcome(connection, "rule_a", "1.2", "blocked", Some("cors"))
                        .await?;
                    let hits = repository.list_hits(connection, Some("rule_a"), 10).await?;
                    let stored = NetworkRepository::get_rule_from(connection, "rule_a").await?;
                    Ok((updated, hits, stored))
                })
            })
            .await?;
        assert_eq!(updated, 1);
        assert_eq!(hits.len(), 1, "the follow-up rewrote, not appended");
        assert_eq!(hits[0].outcome, "blocked");
        let stored = stored.expect("stored rule");
        assert_eq!(stored.hit_count, 1, "only counted outcomes increment");
        assert_eq!(stored.last_hit_at, Some(110));
        Ok(())
    }

    #[tokio::test]
    async fn handoff_record_lookup_is_idempotent_and_physical_delete_works() -> anyhow::Result<()> {
        let (_directory, repository) = repository().await?;
        let handoff = HandoffRow {
            id: "netref_a".to_string(),
            created_at: 1,
            expires_at: 100,
            record_id: "netrec_a".to_string(),
            source_meta: Json::Null,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            request_body: None,
            body_id: None,
            body_bytes: 10,
            body_state: "available".to_string(),
            body_reason: None,
            matcher_seed: Json::Null,
            suggested_ignore: Json::Null,
            related: Json::Null,
            rule_id: None,
        };
        let transaction = repository.begin().await?;
        repository.insert_handoff(&transaction, &handoff).await?;
        transaction.commit().await?;

        let found = repository
            .find_handoff_by_record("netrec_a", 5)
            .await?
            .expect("reused handoff");
        assert_eq!(found.id, "netref_a");
        assert!(
            repository
                .find_handoff_by_record("netrec_a", 200)
                .await?
                .is_none()
        );
        assert_eq!(repository.total_handoff_body_bytes().await?, 10);

        let transaction = repository.begin().await?;
        assert_eq!(
            repository.delete_handoff(&transaction, "netref_a").await?,
            1
        );
        transaction.commit().await?;
        assert!(repository.get_handoff("netref_a").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn state_batches_only_move_rules_that_match() -> anyhow::Result<()> {
        let (_directory, repository) = repository().await?;
        let transaction = repository.begin().await?;
        repository
            .insert_rule(&transaction, &rule("rule_a", "A"))
            .await?;
        let mut disabled = rule("rule_b", "B");
        disabled.enabled = false;
        disabled.state = RuleState::Disabled;
        disabled.state_reason = Some(StateReason::User);
        repository.insert_rule(&transaction, &disabled).await?;
        transaction.commit().await?;

        let transaction = repository.begin().await?;
        let ids = vec!["rule_a".to_string(), "rule_b".to_string()];
        let promoted = repository.promote_pending(&transaction, &ids, 2).await?;
        transaction.commit().await?;
        assert_eq!(promoted, vec!["rule_a".to_string()], "only enabled+pending");

        let transaction = repository.begin().await?;
        let demoted = repository.demote_active_to_pending(&transaction, 3).await?;
        transaction.commit().await?;
        assert_eq!(demoted, vec!["rule_a".to_string()]);

        let transaction = repository.begin().await?;
        let disabled_ids = repository.disable_all_rules(&transaction, 4).await?;
        transaction.commit().await?;
        assert_eq!(disabled_ids, vec!["rule_a".to_string()]);
        Ok(())
    }
}
