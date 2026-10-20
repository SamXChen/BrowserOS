//! Durable network mock state: handoff snapshots, rules, revisions and hits.
//!
//! Capture records never reach this module; they are session-scoped memory plus
//! temporary files (D3).

use crate::{
    db::Database,
    error::{AppError, AppResult},
};
use browseros_core::network::{EndpointMatcher, HeaderPair, MockResponse};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, QueryResult, Statement, TryGetable, Value};
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

#[derive(Debug, Clone)]
pub struct HandoffRow {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub source_meta: Json,
    pub request_headers: Vec<HeaderPair>,
    pub response_headers: Vec<HeaderPair>,
    pub request_body: Option<String>,
    pub body_id: Option<String>,
    pub body_state: String,
    pub body_reason: Option<String>,
    pub matcher_seed: Json,
    pub suggested_ignore: Json,
    pub related: Json,
    pub state: String,
    pub rule_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub scope_kind: ScopeKind,
    pub scope_tab: Option<i64>,
    pub enabled: bool,
    pub state: String,
    pub state_reason: Option<String>,
    pub current_revision: u32,
    pub generation: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub author_client: Option<String>,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RuleRevisionRow {
    pub rule_id: String,
    pub revision: u32,
    pub matcher: EndpointMatcher,
    pub response: MockResponse,
    pub sample_headers: Vec<HeaderPair>,
    pub body_id: Option<String>,
    pub source_handoff_id: Option<String>,
    pub source_record_id: Option<String>,
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
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rules WHERE id = ?",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.first().map(rule_from_row).transpose()
    }

    pub async fn insert_rule(&self, rule: &RuleRow) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_rules \
                 (id, scope_kind, scope_tab, enabled, state, state_reason, current_revision, \
                  generation, created_at, updated_at, author_client, deleted_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(rule.id.clone()),
                    Value::from(rule.scope_kind.as_str().to_owned()),
                    Value::from(rule.scope_tab),
                    Value::from(i64::from(rule.enabled)),
                    Value::from(rule.state.clone()),
                    Value::from(rule.state_reason.clone()),
                    Value::from(i64::from(rule.current_revision)),
                    Value::from(rule.generation as i64),
                    Value::from(rule.created_at),
                    Value::from(rule.updated_at),
                    Value::from(rule.author_client.clone()),
                    Value::from(rule.deleted_at),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn update_rule_scope(
        &self,
        id: &str,
        scope_kind: &ScopeKind,
        scope_tab: Option<i64>,
        updated_at: i64,
    ) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET scope_kind = ?, scope_tab = ?, updated_at = ? WHERE id = ?",
                [
                    Value::from(scope_kind.as_str()),
                    Value::from(scope_tab),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn update_rule_state(
        &self,
        id: &str,
        enabled: bool,
        state: &str,
        reason: Option<&str>,
        updated_at: i64,
    ) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET enabled = ?, state = ?, state_reason = ?, updated_at = ? \
                 WHERE id = ?",
                [
                    Value::from(i64::from(enabled)),
                    Value::from(state.to_owned()),
                    Value::from(reason.map(ToString::to_string)),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn update_rule_revision(
        &self,
        id: &str,
        revision: u32,
        scope_kind: &ScopeKind,
        scope_tab: Option<i64>,
        generation: u64,
        updated_at: i64,
    ) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET current_revision = ?, scope_kind = ?, scope_tab = ?, \
                  generation = ?, updated_at = ? WHERE id = ?",
                [
                    Value::from(i64::from(revision)),
                    Value::from(scope_kind.as_str().to_owned()),
                    Value::from(scope_tab),
                    Value::from(generation as i64),
                    Value::from(updated_at),
                    Value::from(id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn soft_delete_rule(&self, id: &str, at: i64) -> AppResult<u64> {
        let result = self
            .db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET deleted_at = ?, enabled = 0, state = 'disabled', \
                  state_reason = 'deleted' WHERE id = ? AND deleted_at IS NULL",
                [Value::from(at), Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn restore_rule(&self, id: &str) -> AppResult<u64> {
        let result = self
            .db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_rules SET deleted_at = NULL, state = 'applying' WHERE id = ? \
                  AND deleted_at IS NOT NULL",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Deletes rules whose undo window closed. Returns the rule IDs so the
    /// caller can release their body directories.
    pub async fn purge_deleted_rules(&self, before: i64) -> AppResult<Vec<String>> {
        let rows = self
            .db
            .connection()
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
            self.hard_delete_rule(id).await?;
        }
        Ok(ids)
    }

    pub async fn hard_delete_rule(&self, id: &str) -> AppResult<()> {
        let connection = self.db.connection();
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

    pub async fn insert_revision(&self, revision: &RuleRevisionRow) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR REPLACE INTO net_rule_revisions \
                 (rule_id, revision, matcher_json, response_json, sample_headers_json, body_id, \
                  source_handoff_id, source_record_id, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    Value::from(revision.rule_id.clone()),
                    Value::from(i64::from(revision.revision)),
                    Value::from(serde_json::to_string(&revision.matcher)?),
                    Value::from(serde_json::to_string(&revision.response)?),
                    Value::from(serde_json::to_string(&revision.sample_headers)?),
                    Value::from(revision.body_id.clone()),
                    Value::from(revision.source_handoff_id.clone()),
                    Value::from(revision.source_record_id.clone()),
                    Value::from(revision.created_at),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn list_revisions(&self, rule_id: &str) -> AppResult<Vec<RuleRevisionRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_rule_revisions WHERE rule_id = ? ORDER BY revision ASC",
                [Value::from(rule_id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(revision_from_row).collect()
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

    pub async fn insert_hit(&self, hit: &HitRow) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_rule_hits \
                 (id, rule_id, revision, at_ms, tab_title, url, method, outcome, reason, network_id) \
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
        // Keep the most recent 200 rows per rule; the UI only needs a short
        // diagnostic window and hits must not grow without bound.
        let _ = self
            .db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM net_rule_hits WHERE rule_id = ? AND id NOT IN (\
                 SELECT id FROM net_rule_hits WHERE rule_id = ? \
                 ORDER BY at_ms DESC LIMIT 200)",
                [
                    Value::from(hit.rule_id.clone()),
                    Value::from(hit.rule_id.clone()),
                ],
            ))
            .await;
        Ok(())
    }

    pub async fn list_hits(&self, rule_id: Option<&str>, limit: u32) -> AppResult<Vec<HitRow>> {
        let (sql, params) = match rule_id {
            Some(rule_id) => (
                "SELECT * FROM net_rule_hits WHERE rule_id = ? ORDER BY at_ms DESC LIMIT ?",
                vec![
                    Value::from(rule_id.to_owned()),
                    Value::from(i64::from(limit)),
                ],
            ),
            None => (
                "SELECT * FROM net_rule_hits ORDER BY at_ms DESC LIMIT ?",
                vec![Value::from(i64::from(limit))],
            ),
        };
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                sql,
                params,
            ))
            .await
            .map_err(db)?;
        rows.iter().map(hit_from_row).collect()
    }

    pub async fn delete_hits(&self, rule_id: &str) -> AppResult<u64> {
        let result = self
            .db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM net_rule_hits WHERE rule_id = ?",
                [Value::from(rule_id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn insert_handoff(&self, handoff: &HandoffRow) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO net_handoffs \
                 (id, created_at, expires_at, source_meta_json, request_headers_json, \
                  response_headers_json, request_body, body_id, body_state, body_reason, \
                  matcher_seed_json, suggested_ignore_json, related_json, state, rule_id, deleted_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
                [
                    Value::from(handoff.id.clone()),
                    Value::from(handoff.created_at),
                    Value::from(handoff.expires_at),
                    Value::from(serde_json::to_string(&handoff.source_meta)?),
                    Value::from(serde_json::to_string(&handoff.request_headers)?),
                    Value::from(serde_json::to_string(&handoff.response_headers)?),
                    Value::from(handoff.request_body.clone()),
                    Value::from(handoff.body_id.clone()),
                    Value::from(handoff.body_state.clone()),
                    Value::from(handoff.body_reason.clone()),
                    Value::from(serde_json::to_string(&handoff.matcher_seed)?),
                    Value::from(serde_json::to_string(&handoff.suggested_ignore)?),
                    Value::from(serde_json::to_string(&handoff.related)?),
                    Value::from(handoff.state.clone()),
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

    pub async fn list_handoffs(&self) -> AppResult<Vec<HandoffRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM net_handoffs WHERE deleted_at IS NULL ORDER BY created_at DESC"
                    .to_string(),
            ))
            .await
            .map_err(db)?;
        rows.iter().map(handoff_from_row).collect()
    }

    pub async fn delete_handoff(&self, id: &str) -> AppResult<u64> {
        let result = self
            .db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM net_handoffs WHERE id = ?",
                [Value::from(id.to_owned())],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    pub async fn set_handoff_rule(&self, handoff_id: &str, rule_id: &str) -> AppResult<()> {
        self.db
            .connection()
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE net_handoffs SET rule_id = ?, state = 'used' WHERE id = ?",
                [
                    Value::from(rule_id.to_owned()),
                    Value::from(handoff_id.to_owned()),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn find_rule_by_handoff(&self, handoff_id: &str) -> AppResult<Option<RuleRow>> {
        let rows = self
            .db
            .connection()
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT r.* FROM net_rules r \
                 JOIN net_rule_revisions v ON v.rule_id = r.id AND v.revision = r.current_revision \
                 WHERE v.source_handoff_id = ? AND r.deleted_at IS NULL \
                 ORDER BY r.updated_at DESC LIMIT 1",
                [Value::from(handoff_id.to_owned())],
            ))
            .await
            .map_err(db)?;
        rows.first().map(rule_from_row).transpose()
    }
}

fn rule_from_row(row: &QueryResult) -> AppResult<RuleRow> {
    Ok(RuleRow {
        id: get(row, "id")?,
        scope_kind: ScopeKind::parse(&get::<String>(row, "scope_kind")?),
        scope_tab: get_opt(row, "scope_tab")?,
        enabled: get::<i64>(row, "enabled")? != 0,
        state: get(row, "state")?,
        state_reason: get_opt(row, "state_reason")?,
        current_revision: get::<i64>(row, "current_revision")? as u32,
        generation: get::<i64>(row, "generation")? as u64,
        created_at: get(row, "created_at")?,
        updated_at: get(row, "updated_at")?,
        author_client: get_opt(row, "author_client")?,
        deleted_at: get_opt(row, "deleted_at")?,
    })
}

fn revision_from_row(row: &QueryResult) -> AppResult<RuleRevisionRow> {
    Ok(RuleRevisionRow {
        rule_id: get(row, "rule_id")?,
        revision: get::<i64>(row, "revision")? as u32,
        matcher: serde_json::from_str(&get::<String>(row, "matcher_json")?)?,
        response: serde_json::from_str(&get::<String>(row, "response_json")?)?,
        sample_headers: serde_json::from_str(&get::<String>(row, "sample_headers_json")?)?,
        body_id: get_opt(row, "body_id")?,
        source_handoff_id: get_opt(row, "source_handoff_id")?,
        source_record_id: get_opt(row, "source_record_id")?,
        created_at: get(row, "created_at")?,
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

fn handoff_from_row(row: &QueryResult) -> AppResult<HandoffRow> {
    Ok(HandoffRow {
        id: get(row, "id")?,
        created_at: get(row, "created_at")?,
        expires_at: get(row, "expires_at")?,
        source_meta: serde_json::from_str(&get::<String>(row, "source_meta_json")?)?,
        request_headers: serde_json::from_str(&get::<String>(row, "request_headers_json")?)?,
        response_headers: serde_json::from_str(&get::<String>(row, "response_headers_json")?)?,
        request_body: get_opt(row, "request_body")?,
        body_id: get_opt(row, "body_id")?,
        body_state: get(row, "body_state")?,
        body_reason: get_opt(row, "body_reason")?,
        matcher_seed: serde_json::from_str(&get::<String>(row, "matcher_seed_json")?)?,
        suggested_ignore: serde_json::from_str(&get::<String>(row, "suggested_ignore_json")?)?,
        related: serde_json::from_str(&get::<String>(row, "related_json")?)?,
        state: get(row, "state")?,
        rule_id: get_opt(row, "rule_id")?,
    })
}
