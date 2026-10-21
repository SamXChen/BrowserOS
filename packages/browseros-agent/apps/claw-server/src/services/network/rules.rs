//! The single pipeline for every rule change.
//!
//! Nothing else in the service writes rule rows or pushes to the runtime. All
//! operations take `rules_lock`, persist inside one transaction, rebuild the
//! runtime snapshot and derive the resulting state, so intent and runtime state
//! can never disagree.

use super::{
    apply::{RULE_BODY_LIMIT, default_response, matcher_from_seed, resolve_static_input},
    body_store::BodyStore,
    naming::{candidates, default_rule_name, suffixed_name, validate_rule_name},
    runtime_port::RuntimeSlot,
};
use crate::db::DbConnection;
use crate::{
    db::network::{NetworkRepository, RuleRevisionRow, RuleRow, RuleState, ScopeKind, StateReason},
    error::{AppError, AppResult},
    services::network::service::SseEvent,
};
use browseros_core::network::{
    ApplyReport, BodyField, EndpointMatcher, HeaderPatch, NetworkRecord, ProcessorHooks,
    ProcessorInput, ProcessorResult, ProcessorSpec, QueryMode, ResponseInput, ResponseProcessor,
    RuleAction, RuntimeAction, RuntimeRule, RuntimeRuleSet, StaticResponse, TabSummary,
    build_processed, matches,
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

pub const UNDO_WINDOW_MS: i64 = 8_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyRequest {
    #[serde(default)]
    pub handoff_id: Option<String>,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub tab_id: Option<i64>,
    #[serde(default)]
    pub ignore_keys: Option<Vec<String>>,
    #[serde(default)]
    pub strict_query: Option<bool>,
    #[serde(default)]
    pub body_fields: Option<Vec<BodyField>>,
    #[serde(default)]
    pub exact_url: Option<String>,
    #[serde(default)]
    pub response: Option<ResponseInput>,
    #[serde(default)]
    pub processor: Option<ProcessorArg>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub author_client: Option<String>,
}

/// A response-stage script supplied by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorArg {
    pub script: String,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
}

/// A rule whose matcher is identical is replaced by the new one (D9).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replacement {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub rule_id: String,
    pub name: String,
    pub revision: u32,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `METHOD pathname`, so the agent can report what the rule covers.
    pub endpoint: String,
    pub scope: &'static str,
    pub action: &'static str,
    pub ignore_keys: Vec<String>,
    pub strict_query: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<Replacement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preview: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_preview: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<&'static str>,
}

/// Result of a snapshot rebuild: the rules whose state changed.
pub struct Rebuild {
    pub changed: Vec<String>,
    /// Whether the runtime confirmed the narrowing; disable paths care.
    pub narrowed: bool,
}

pub struct RuleEngine {
    repo: NetworkRepository,
    bodies: Arc<BodyStore>,
    runtime: Arc<RuntimeSlot>,
    processor: Arc<dyn ResponseProcessor>,
    tabs: RwLock<Vec<TabSummary>>,
    /// Preview needs captured records; the service injects a reader so this
    /// module stays free of capture plumbing.
    records: RwLock<Option<RecordReader>>,
    sse: broadcast::Sender<SseEvent>,
    rules_lock: Mutex<()>,
    stop_gate: Mutex<()>,
    generation: AtomicU64,
    stop_epoch: AtomicU64,
    snapshot_ids: RwLock<Vec<String>>,
}

/// Reads the capture buffer for `dryRun` previews.
pub type RecordReader =
    Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, Vec<NetworkRecord>> + Send + Sync>;

impl RuleEngine {
    pub fn new(
        repo: NetworkRepository,
        bodies: Arc<BodyStore>,
        runtime: Arc<RuntimeSlot>,
        sse: broadcast::Sender<SseEvent>,
        processor: Arc<dyn ResponseProcessor>,
    ) -> Self {
        Self {
            repo,
            bodies,
            runtime,
            processor,
            tabs: RwLock::new(Vec::new()),
            records: RwLock::new(None),
            sse,
            rules_lock: Mutex::new(()),
            stop_gate: Mutex::new(()),
            generation: AtomicU64::new(0),
            stop_epoch: AtomicU64::new(0),
            snapshot_ids: RwLock::new(Vec::new()),
        }
    }

    pub async fn set_tabs(&self, tabs: Vec<TabSummary>) {
        *self.tabs.write().await = tabs;
    }

    pub async fn set_record_reader(&self, reader: RecordReader) {
        *self.records.write().await = Some(reader);
    }

    /// Rules currently in the runtime snapshot, for `pending` → `active`.
    pub async fn snapshot_ids(&self) -> Vec<String> {
        self.snapshot_ids.read().await.clone()
    }

    fn broadcast(&self, event: &'static str, data: Value) {
        let _ = self.sse.send(SseEvent { event, data });
    }

    // ---- apply ---------------------------------------------------------

    pub async fn apply(&self, request: ApplyRequest) -> AppResult<ApplyOutcome> {
        if request.handoff_id.is_none() && request.rule.is_none() {
            return Err(AppError::bad_request(
                "invalid_request: rule_apply requires handoffId or rule",
            ));
        }
        let epoch_at_call = self.stop_epoch.load(Ordering::SeqCst);
        let _guard = self.rules_lock.lock().await;
        let now = now_ms();

        let handoff = match &request.handoff_id {
            Some(id) => {
                let handoff = self
                    .repo
                    .get_handoff(id)
                    .await?
                    .ok_or_else(|| AppError::not_found("handoff_not_found"))?;
                if handoff.expires_at < now {
                    return Err(AppError::gone("handoff_expired"));
                }
                Some(handoff)
            }
            None => None,
        };

        let existing = self.resolve_target(&request, handoff.as_ref()).await?;
        let current_revision = match &existing {
            Some(rule) => Some(self.current_revision(rule).await?),
            None => None,
        };
        let matcher = self.build_matcher(&request, handoff.as_ref(), current_revision.as_ref())?;
        let (resolved, sample_preview) = self
            .build_action(&request, handoff.as_ref(), current_revision.as_ref())
            .await?;
        let (scope_kind, scope_tab) = self.resolve_scope(&request, existing.as_ref()).await?;
        let enabled = request
            .enabled
            .unwrap_or_else(|| existing.as_ref().is_none_or(|rule| rule.enabled));

        if request.dry_run {
            let preview = self
                .preview_matches(&matcher, scope_kind, scope_tab)
                .await?;
            return Ok(ApplyOutcome {
                rule_id: existing
                    .as_ref()
                    .map(|rule| rule.id.clone())
                    .unwrap_or_default(),
                name: match existing.as_ref() {
                    Some(rule) => rule.name.clone(),
                    None => self.placeholder_name(&matcher).await,
                },
                revision: current_revision
                    .as_ref()
                    .map(|revision| revision.revision)
                    .unwrap_or(0),
                state: "dry_run",
                reason: None,
                endpoint: format!("{} {}", matcher.method, matcher.pathname),
                scope: scope_kind.as_str(),
                action: resolved.kind(),
                ignore_keys: matcher.query_policy.ignore_keys.clone(),
                strict_query: matches!(matcher.query_policy.mode, QueryMode::Strict),
                replaced: self
                    .replaced_by(&matcher, existing.as_ref().map(|rule| rule.id.as_str()))
                    .await?,
                preview,
                sample_preview,
                next_action: None,
            });
        }

        let rule_id = existing
            .as_ref()
            .map(|rule| rule.id.clone())
            .unwrap_or_else(|| format!("rule_{}", Uuid::new_v4().simple()));
        let revision = current_revision
            .as_ref()
            .map(|revision| revision.revision + 1)
            .unwrap_or(1);
        let name = self
            .resolve_name(&request, existing.as_ref(), &matcher)
            .await?;
        if let ResolvedAction::Static { body, .. } = &resolved {
            self.bodies.write_rule(&rule_id, revision, body).await?;
        }
        // The rule owns its sample: a deleted or expired handoff must not
        // take the processor's sample, or the dry-run body, with it.
        if let Some(sample) = self
            .sample_bytes(handoff.as_ref(), current_revision.as_ref())
            .await?
        {
            self.bodies
                .write_rule_sample(&rule_id, revision, &sample)
                .await?;
        }
        let replaced = self
            .replaced_by(&matcher, existing.as_ref().map(|rule| rule.id.as_str()))
            .await?;

        let transaction = self.repo.begin().await?;
        {
            let repo = self.repo.clone();
            let connection = &transaction;
            let sample = self.sample_fields(handoff.as_ref(), current_revision.as_ref(), revision);
            let revision_row = RuleRevisionRow {
                rule_id: rule_id.clone(),
                revision,
                matcher: matcher.clone(),
                action: resolved.action(),
                body_id: resolved.body_id(&rule_id, revision),
                sample_rev: sample.0,
                sample_available: sample.1,
                sample_status: sample.2,
                sample_headers: sample.3,
                sample_request: sample.4,
                source_handoff_id: handoff.as_ref().map(|handoff| handoff.id.clone()),
                source_record_id: handoff.as_ref().map(|handoff| handoff.record_id.clone()),
                source_tab_id: handoff
                    .as_ref()
                    .and_then(|handoff| handoff.source_meta.get("tabId"))
                    .and_then(serde_json::Value::as_i64),
                created_at: now,
            };
            self.repo.insert_revision(connection, &revision_row).await?;

            let (state, reason) = if enabled {
                (RuleState::Pending, None)
            } else {
                (RuleState::Disabled, Some(StateReason::User))
            };
            if existing.is_none() {
                repo.insert_rule(
                    connection,
                    &RuleRow {
                        id: rule_id.clone(),
                        name: name.clone(),
                        scope_kind,
                        scope_tab,
                        enabled,
                        state,
                        state_reason: reason,
                        current_revision: revision,
                        hit_count: 0,
                        last_hit_at: None,
                        author_client: request.author_client.clone(),
                        created_at: now,
                        updated_at: now,
                        deleted_at: None,
                    },
                )
                .await?;
            } else {
                repo.update_rule_revision(
                    connection,
                    &rule_id,
                    revision,
                    &scope_kind,
                    scope_tab,
                    request.author_client.as_deref(),
                    now,
                )
                .await?;
                repo.update_rule_intent(connection, &rule_id, enabled, state, reason.as_ref(), now)
                    .await?;
                repo.update_rule_name(connection, &rule_id, &name, now)
                    .await?;
            }
            for victim in &replaced {
                repo.update_rule_intent(
                    connection,
                    &victim.id,
                    false,
                    RuleState::Disabled,
                    Some(&StateReason::ReplacedBy(rule_id.clone())),
                    now,
                )
                .await?;
            }
            if let Some(handoff) = &handoff {
                repo.set_handoff_rule(connection, &handoff.id, Some(&rule_id))
                    .await?;
            }
        }

        // A stop-all that landed meanwhile wins: the check and the commit must
        // not interleave with the stop-all's own write.
        let superseded = {
            let _gate = self.stop_gate.lock().await;
            let changed = self.stop_epoch.load(Ordering::SeqCst) != epoch_at_call;
            if changed && enabled {
                self.repo
                    .update_rule_intent(
                        &transaction,
                        &rule_id,
                        false,
                        RuleState::Disabled,
                        Some(&StateReason::Superseded),
                        now,
                    )
                    .await?;
            }
            transaction
                .commit()
                .await
                .map_err(|error| AppError::Db(Box::new(error)))?;
            changed && enabled
        };

        let rebuild = self.rebuild_and_apply().await?;
        let (state, reason) = match self.repo.get_rule(&rule_id).await? {
            Some(rule) => (rule.state, rule.state_reason),
            None => (RuleState::Pending, None),
        };
        // `superseded` is its own outcome state, not `disabled`
        // with a reason: the caller must not report the rule as live.
        let (state, reason) = if superseded {
            ("superseded", Some(StateReason::Superseded))
        } else {
            (state.as_str(), reason)
        };
        self.broadcast_rules(&rebuild).await;

        Ok(ApplyOutcome {
            rule_id,
            name,
            revision,
            state,
            reason: reason.map(|reason| reason.as_str()),
            endpoint: format!("{} {}", matcher.method, matcher.pathname),
            scope: scope_kind.as_str(),
            action: resolved.kind(),
            ignore_keys: matcher.query_policy.ignore_keys.clone(),
            strict_query: matches!(matcher.query_policy.mode, QueryMode::Strict),
            replaced,
            preview: Vec::new(),
            sample_preview,
            next_action: Some("refresh"),
        })
    }

    // ---- simple operations ---------------------------------------------

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> AppResult<RuleRow> {
        let epoch_at_call = self.stop_epoch.load(Ordering::SeqCst);
        let _guard = self.rules_lock.lock().await;
        let now = now_ms();
        let rule = self.live_rule(id).await?;

        if enabled {
            if let Some(StateReason::ReplacedBy(other)) = &rule.state_reason {
                // Turning a replaced rule back on replaces the rule that won.
                let other_id = other.clone();
                let rule_id = rule.id.clone();
                let repo = self.repo.clone();
                self.stmt(move |connection| {
                    Box::pin(async move {
                        repo.update_rule_intent(
                            connection,
                            &other_id,
                            false,
                            RuleState::Disabled,
                            Some(&StateReason::ReplacedBy(rule_id)),
                            now,
                        )
                        .await
                    })
                })
                .await?;
            }
            if self.stop_epoch.load(Ordering::SeqCst) != epoch_at_call {
                return Err(AppError::conflict("superseded"));
            }
        }

        let (state, reason) = if enabled {
            (RuleState::Pending, None)
        } else {
            (RuleState::Disabled, Some(StateReason::User))
        };
        let applied = {
            let rule_id = id.to_string();
            let reason = reason.clone();
            let repo = self.repo.clone();
            self.stmt(|connection| {
                Box::pin(async move {
                    repo.update_rule_intent(
                        connection,
                        &rule_id,
                        enabled,
                        state,
                        reason.as_ref(),
                        now,
                    )
                    .await
                })
            })
            .await?
        };
        if applied == 0 {
            return Err(AppError::not_found("rule_not_found"));
        }

        let rebuild = self.rebuild_and_apply().await?;
        if !enabled && !rebuild.narrowed {
            // The browser still intercepts this rule: put the intent back.
            let rule_id = id.to_string();
            let previous_state = rule.state;
            let previous_reason = rule.state_reason.clone();
            let previous_enabled = rule.enabled;
            let repo = self.repo.clone();
            self.stmt(|connection| {
                Box::pin(async move {
                    repo.update_rule_intent(
                        connection,
                        &rule_id,
                        previous_enabled,
                        previous_state,
                        previous_reason.as_ref(),
                        now,
                    )
                    .await
                })
            })
            .await?;
            let rebuild = self.rebuild_and_apply().await?;
            self.broadcast_rules(&rebuild).await;
            return Err(AppError::conflict("fetch_not_disabled"));
        }
        self.broadcast_rules(&rebuild).await;
        self.live_rule(id).await
    }

    pub async fn set_scope(
        &self,
        id: &str,
        scope: ScopeKind,
        tab: Option<i64>,
    ) -> AppResult<RuleRow> {
        let _guard = self.rules_lock.lock().await;
        let rule = self.live_rule(id).await?;
        let now = now_ms();
        let tab = match scope {
            ScopeKind::AllTabs => None,
            ScopeKind::Tab => {
                let tab = tab
                    .or(rule.scope_tab)
                    .ok_or_else(|| AppError::bad_request("tab_required"))?;
                if !self.tab_exists(tab).await {
                    return Err(AppError::bad_request(format!("tab_unbound: {tab}")));
                }
                Some(tab)
            }
        };
        let rule_id = id.to_string();
        let unbound = rule.state == RuleState::Unbound;
        let rule_enabled = rule.enabled;
        let repo = self.repo.clone();
        self.stmt(|connection| {
            Box::pin(async move {
                repo.update_rule_scope(connection, &rule_id, &scope, tab, now)
                    .await?;
                if unbound {
                    repo.update_rule_intent(
                        connection,
                        &rule_id,
                        rule_enabled,
                        RuleState::Pending,
                        None,
                        now,
                    )
                    .await?;
                }
                Ok(())
            })
        })
        .await?;
        if rule.enabled {
            let rebuild = self.rebuild_and_apply().await?;
            self.broadcast_rules(&rebuild).await;
        } else {
            self.broadcast_rule(id).await;
        }
        self.live_rule(id).await
    }

    pub async fn rename(&self, id: &str, name: &str) -> AppResult<RuleRow> {
        let _guard = self.rules_lock.lock().await;
        let name = validate_rule_name(name)?;
        let rule = self.live_rule(id).await?;
        if let Some(other) = self.repo.find_rule_by_name(&name).await?
            && other.id != rule.id
        {
            return Err(AppError::conflict(format!(
                "rule_name_taken: 「{name}」 is used by another rule"
            )));
        }
        let rule_id = id.to_string();
        let new_name = name.clone();
        let repo = self.repo.clone();
        self.stmt(|connection| {
            Box::pin(async move {
                repo.update_rule_name(connection, &rule_id, &new_name, now_ms())
                    .await
            })
        })
        .await?;
        self.broadcast_rule(id).await;
        self.live_rule(id).await
    }

    pub async fn restore_revision(&self, id: &str, revision: u32) -> AppResult<RuleRow> {
        let _guard = self.rules_lock.lock().await;
        let rule = self.live_rule(id).await?;
        let current = self.current_revision(&rule).await?;
        if current.revision == revision {
            return Err(AppError::bad_request(
                "invalid_request: revision is current",
            ));
        }
        let source = self
            .repo
            .get_revision(id, revision)
            .await?
            .ok_or_else(|| AppError::not_found("revision_not_found"))?;
        let now = now_ms();
        let next = current.revision + 1;

        let body = match &source.action {
            RuleAction::Static(_) => {
                self.bodies
                    .read_rule(&source.body_id.clone().unwrap_or_default())
                    .await?
            }
            RuleAction::Processor(_) => None,
        };
        if let Some(body) = &body {
            self.bodies.write_rule(id, next, body).await?;
        }

        let repo = self.repo.clone();
        let next_body_id = match &source.action {
            RuleAction::Static(_) => Some(format!("{id}/{next}")),
            RuleAction::Processor(_) => None,
        };
        let source_revision = source.clone();
        let rule_id = id.to_string();
        let scope_kind = rule.scope_kind;
        let scope_tab = rule.scope_tab;
        let author_client = rule.author_client.clone();
        self.stmt(|connection| {
            let source_revision = source_revision.clone();
            let next_body_id = next_body_id.clone();
            let author_client = author_client.clone();
            Box::pin(async move {
                let repo = repo.clone();
                repo.insert_revision(
                    connection,
                    &RuleRevisionRow {
                        rule_id: rule_id.clone(),
                        revision: next,
                        matcher: source_revision.matcher.clone(),
                        action: source_revision.action.clone(),
                        body_id: next_body_id,
                        sample_rev: source_revision.sample_rev,
                        sample_available: source_revision.sample_available,
                        sample_status: source_revision.sample_status,
                        sample_headers: source_revision.sample_headers.clone(),
                        sample_request: source_revision.sample_request.clone(),
                        source_handoff_id: source_revision.source_handoff_id.clone(),
                        source_record_id: source_revision.source_record_id.clone(),
                        source_tab_id: source_revision.source_tab_id,
                        created_at: now,
                    },
                )
                .await?;
                repo.update_rule_revision(
                    connection,
                    &rule_id,
                    next,
                    &scope_kind,
                    scope_tab,
                    author_client.as_deref(),
                    now,
                )
                .await
            })
        })
        .await?;

        let rebuild = self.rebuild_and_apply().await?;
        self.broadcast_rules(&rebuild).await;
        self.live_rule(id).await
    }

    pub async fn delete(&self, id: &str) -> AppResult<Value> {
        let _guard = self.rules_lock.lock().await;
        self.live_rule(id).await?;
        let at = now_ms();
        let rule_id = id.to_string();
        let removed = self
            .stmt(|connection| {
                let repo = self.repo.clone();
                Box::pin(async move { repo.soft_delete_rule(connection, &rule_id, at).await })
            })
            .await?;
        if removed == 0 {
            return Err(AppError::not_found("rule_not_found"));
        }
        let rebuild = self.rebuild_and_apply().await?;
        self.broadcast_rules(&rebuild).await;
        let deleted = json!({
            "kind": "rule",
            "id": id,
            "undoToken": id,
            "undoUntil": at + UNDO_WINDOW_MS,
        });
        let _ = self.sse.send(SseEvent {
            event: "deleted",
            data: deleted.clone(),
        });
        Ok(json!({
            "deleted": true,
            "undoToken": id,
            "undoUntil": at + UNDO_WINDOW_MS,
        }))
    }

    pub async fn delete_all(&self) -> AppResult<Value> {
        let _guard = self.rules_lock.lock().await;
        let at = now_ms();
        let ids = self
            .stmt(|connection| {
                let repo = self.repo.clone();
                Box::pin(async move { repo.soft_delete_all_rules(connection, at).await })
            })
            .await?;
        let rebuild = self.rebuild_and_apply().await?;
        self.broadcast_rules(&rebuild).await;
        for id in &ids {
            let _ = self.sse.send(SseEvent {
                event: "deleted",
                data: json!({
                    "kind": "rule",
                    "id": id,
                    "undoToken": id,
                    "undoUntil": at + UNDO_WINDOW_MS,
                }),
            });
        }
        Ok(json!({ "deleted": ids.len() }))
    }

    pub async fn restore(&self, id: &str) -> AppResult<RuleRow> {
        let _guard = self.rules_lock.lock().await;
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let now = now_ms();
        if rule.deleted_at.is_none() {
            return Err(AppError::not_found("rule_not_found"));
        }
        if rule.deleted_at.is_some_and(|at| now - at > UNDO_WINDOW_MS) {
            return Err(AppError::gone("undo_expired"));
        }
        let name = {
            // Only live names count: the row being restored is soft-deleted, so
            // adding its own name here would suffix every restore with ` #2`.
            let taken = self.repo.rule_names_lowercase().await?;
            suffixed_name(&rule.name, &taken)
        };
        let rule_id = id.to_string();
        let new_name = name.clone();
        let restored = self
            .stmt(|connection| {
                let new_name = new_name.clone();
                let rule_id = rule_id.clone();
                let repo = self.repo.clone();
                Box::pin(async move {
                    // Rename first: the row is still soft-deleted, so the
                    // unique index does not see it yet and a name that a live
                    // rule claimed meanwhile cannot collide on restore.
                    repo.update_rule_name(connection, &rule_id, &new_name, now)
                        .await?;
                    repo.restore_rule(connection, &rule_id, now).await
                })
            })
            .await?;
        if restored == 0 {
            return Err(AppError::gone("undo_expired"));
        }
        let rebuild = self.rebuild_and_apply().await?;
        self.broadcast_rules(&rebuild).await;
        self.live_rule(id).await
    }

    /// "Stop all mocks": disable everything, then push an empty snapshot so
    /// in-flight delays and fulfills are released. Never waits for `rules_lock`.
    pub async fn stop_all(&self) -> AppResult<Value> {
        let ids = {
            let _gate = self.stop_gate.lock().await;
            self.stop_epoch.fetch_add(1, Ordering::SeqCst);
            let repo = self.repo.clone();
            self.stmt(|connection| {
                Box::pin(async move { repo.disable_all_rules(connection, now_ms()).await })
            })
            .await?
        };
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let report = match self.runtime.current() {
            Some(handle) => Some(
                handle
                    .apply_rules(Arc::new(RuntimeRuleSet {
                        generation,
                        rules: Vec::new(),
                    }))
                    .await,
            ),
            None => None,
        };
        let narrowed = report.is_none_or(|report| {
            !report.stale && !report.timed_out && report.acked == report.total
        });
        *self.snapshot_ids.write().await = Vec::new();
        for id in &ids {
            self.broadcast_rule(id).await;
        }
        Ok(json!({ "disabled": ids.len(), "narrowed": narrowed }))
    }

    /// Tabs closed: tab-scoped rules cannot be applied any more.
    pub async fn unbind_closed_tabs(&self, removed: &[i64]) -> AppResult<()> {
        if removed.is_empty() {
            return Ok(());
        }
        let _guard = self.rules_lock.lock().await;
        let rules = self.repo.list_rules(false).await?;
        let mut changed = Vec::new();
        for rule in rules {
            let scoped = rule.scope_kind == ScopeKind::Tab
                && rule.scope_tab.is_some_and(|tab| removed.contains(&tab));
            if !rule.enabled || !scoped {
                continue;
            }
            let rule_id = rule.id.clone();
            let rows = self
                .stmt(|connection| {
                    let rule_id = rule_id.clone();
                    let repo = self.repo.clone();
                    Box::pin(async move {
                        repo.update_rule_intent(
                            connection,
                            &rule_id,
                            true,
                            RuleState::Unbound,
                            Some(&StateReason::TabClosed),
                            now_ms(),
                        )
                        .await
                    })
                })
                .await?;
            if rows > 0 {
                changed.push(rule.id.clone());
            }
        }
        if !changed.is_empty() {
            let rebuild = self.rebuild_and_apply().await?;
            self.broadcast_rules(&rebuild).await;
        }
        Ok(())
    }

    // ---- runtime convergence -------------------------------------------

    /// The runtime confirmed a snapshot: `pending` rules inside it go `active`.
    /// Never takes `rules_lock`, so it cannot block rule operations.
    pub async fn on_applied(&self, report: ApplyReport) {
        if report.stale || report.acked == 0 {
            return;
        }
        if report.generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        let ids = self.snapshot_ids().await;
        if ids.is_empty() {
            return;
        }
        let promoted = self
            .stmt(|connection| {
                let ids = ids.clone();
                let repo = self.repo.clone();
                Box::pin(async move { repo.promote_pending(connection, &ids, now_ms()).await })
            })
            .await
            .unwrap_or_default();
        for id in promoted {
            self.broadcast_rule(&id).await;
        }
    }

    /// The browser went away: nothing is confirmed any more.
    pub async fn on_runtime_lost(&self) {
        let demoted = self
            .stmt(|connection| {
                let repo = self.repo.clone();
                Box::pin(async move { repo.demote_active_to_pending(connection, now_ms()).await })
            })
            .await
            .unwrap_or_default();
        for id in demoted {
            self.broadcast_rule(&id).await;
        }
    }

    /// Pushes the current snapshot to a freshly spawned runtime.
    pub async fn push_rules(&self) -> AppResult<Rebuild> {
        self.rebuild_and_apply().await
    }

    /// Periodic cleanup: expired handoffs plus directories the database no
    /// longer knows about (including bodies left behind by earlier versions).
    pub async fn maintenance(&self) {
        let now = now_ms();
        let expired = self
            .stmt(|connection| {
                let repo = self.repo.clone();
                Box::pin(async move { repo.list_expired_handoffs(connection, now).await })
            })
            .await
            .unwrap_or_default();
        for handoff in expired {
            let id = handoff.id.clone();
            let removed = self
                .stmt(|connection| {
                    let repo = self.repo.clone();
                    Box::pin(async move { repo.delete_handoff(connection, &id).await })
                })
                .await
                .unwrap_or(0);
            if removed > 0 {
                self.bodies.remove_handoff(&handoff.id).await.ok();
                let _ = self.sse.send(SseEvent {
                    event: "deleted",
                    data: json!({ "kind": "handoff", "id": handoff.id, "reason": "expired" }),
                });
            }
        }
        let rules = self.repo.list_rules(true).await.unwrap_or_default();
        let handoffs = self.repo.list_handoffs().await.unwrap_or_default();
        let rule_ids = rules.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>();
        let handoff_ids = handoffs
            .iter()
            .map(|handoff| handoff.id.clone())
            .collect::<Vec<_>>();
        self.bodies
            .sweep_orphans(&rule_ids, &handoff_ids)
            .await
            .ok();
    }

    /// The stored sample body for a revision, for previews and test runs.
    pub async fn sample_bytes_for(&self, revision: &RuleRevisionRow) -> AppResult<Option<Vec<u8>>> {
        if !revision.sample_available {
            return Ok(None);
        }
        self.bodies
            .read_rule_sample(&revision.rule_id, revision.sample_rev)
            .await
    }

    /// Runs one statement group on the shared connection. The pool holds a
    /// single connection, so this must never be nested inside itself.
    async fn stmt<F, T>(&self, work: F) -> AppResult<T>
    where
        F: for<'a> FnOnce(&'a DbConnection) -> BoxFuture<'a, AppResult<T>>,
    {
        self.repo.stmt(work).await
    }

    async fn live_rule(&self, id: &str) -> AppResult<RuleRow> {
        self.repo
            .get_rule(id)
            .await?
            .filter(|rule| rule.deleted_at.is_none())
            .ok_or_else(|| AppError::not_found("rule_not_found"))
    }

    /// Test seam: hold `rules_lock` so an apply captures the stop epoch and
    /// then waits, so an interleaved `stop_all` supersedes the pending apply.
    #[cfg(test)]
    pub(crate) async fn hold_rules_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.rules_lock.lock().await
    }

    pub async fn current_revision(&self, rule: &RuleRow) -> AppResult<RuleRevisionRow> {
        self.repo
            .get_revision(&rule.id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::Internal(format!("rule {} has no revision", rule.id)))
    }

    async fn tab_exists(&self, tab_id: i64) -> bool {
        self.tabs
            .read()
            .await
            .iter()
            .any(|tab| tab.tab_id == tab_id && tab.attached)
    }

    async fn resolve_target(
        &self,
        request: &ApplyRequest,
        handoff: Option<&crate::db::network::HandoffRow>,
    ) -> AppResult<Option<RuleRow>> {
        if let Some(handoff) = handoff
            && let Some(rule_id) = &handoff.rule_id
            && let Some(rule) = self.repo.get_rule(rule_id).await?
            && rule.deleted_at.is_none()
        {
            return Ok(Some(rule));
        }
        match &request.rule {
            Some(reference) => self.resolve_reference(reference).await.map(Some),
            None => Ok(None),
        }
    }

    /// A reference is a name, or a `rule_*` id.
    /// Lower-cased names of every live rule; tests assert de-duplication.
    pub async fn names_lowercase(&self) -> AppResult<Vec<String>> {
        self.repo.rule_names_lowercase().await
    }

    pub async fn resolve_reference(&self, reference: &str) -> AppResult<RuleRow> {
        if reference.starts_with("rule_") {
            if let Some(rule) = self.repo.get_rule(reference).await?
                && rule.deleted_at.is_none()
            {
                return Ok(rule);
            }
            return Err(self.not_found(reference).await);
        }
        if let Some(rule) = self.repo.find_rule_by_name(reference).await? {
            return Ok(rule);
        }
        Err(self.not_found(reference).await)
    }

    async fn not_found(&self, reference: &str) -> AppError {
        let rules = self.repo.list_rules(false).await.unwrap_or_default();
        let names = candidates(&rules, reference)
            .into_iter()
            .map(|rule| format!("「{}」", rule.name))
            .collect::<Vec<_>>()
            .join(", ");
        AppError::not_found(format!(
            "rule_not_found: no rule named 「{reference}」. Candidates: {names}"
        ))
    }

    fn build_matcher(
        &self,
        request: &ApplyRequest,
        handoff: Option<&crate::db::network::HandoffRow>,
        current: Option<&RuleRevisionRow>,
    ) -> AppResult<EndpointMatcher> {
        if let Some(handoff) = handoff {
            return matcher_from_seed(
                &handoff.matcher_seed,
                request.ignore_keys.clone(),
                request.strict_query.unwrap_or(false),
                request.body_fields.clone(),
                request.exact_url.clone(),
            );
        }
        let Some(current) = current else {
            return Err(AppError::bad_request(
                "invalid_request: creating a rule requires handoffId",
            ));
        };
        let mut matcher = current.matcher.clone();
        if let Some(keys) = &request.ignore_keys {
            matcher.query_policy.ignore_keys = keys.clone();
        }
        if let Some(strict) = request.strict_query {
            matcher.query_policy.mode = if strict {
                QueryMode::Strict
            } else {
                QueryMode::Lenient
            };
        }
        if let Some(fields) = &request.body_fields {
            matcher.body_fields = fields.clone();
        }
        if let Some(exact) = &request.exact_url {
            matcher.exact_url = if exact.is_empty() {
                None
            } else {
                Some(exact.clone())
            };
        }
        Ok(matcher)
    }

    async fn build_action(
        &self,
        request: &ApplyRequest,
        handoff: Option<&crate::db::network::HandoffRow>,
        current: Option<&RuleRevisionRow>,
    ) -> AppResult<(ResolvedAction, Option<Value>)> {
        if let Some(processor) = &request.processor {
            if request.response.is_some() {
                return Err(AppError::bad_request(
                    "invalid_request: response and processor are mutually exclusive",
                ));
            }
            let script = processor.script.trim();
            if script.is_empty() || script.len() > 64 * 1024 {
                return Err(AppError::bad_request(
                    "invalid_processor: script must be 1 byte - 64KiB",
                ));
            }
            let timeout_ms = processor.timeout_ms.unwrap_or(200);
            if !(50..=1000).contains(&timeout_ms) {
                return Err(AppError::bad_request(
                    "invalid_processor: timeoutMs must be 50-1000",
                ));
            }
            super::processor::validate_script(script).map_err(|message| {
                AppError::bad_request(format!("invalid_processor: {message}"))
            })?;
            let spec = ProcessorSpec {
                script: script.to_string(),
                timeout_ms,
            };
            let preview = self.preview_processor(&spec, handoff, current).await?;
            return Ok((ResolvedAction::Processor(spec), preview));
        }
        let sample = self.sample_bytes(handoff, current).await?;
        let current_body = match current {
            Some(revision) => match &revision.action {
                RuleAction::Static(_) => {
                    self.bodies
                        .read_rule(&revision.body_id.clone().unwrap_or_default())
                        .await?
                }
                RuleAction::Processor(_) => None,
            },
            None => None,
        };
        if let Some(input) = &request.response {
            let (response, body) =
                resolve_static_input(input, sample.as_deref(), current_body.as_deref())?;
            return Ok((ResolvedAction::Static { response, body }, None));
        }
        match current {
            Some(revision) => match &revision.action {
                RuleAction::Static(response) => {
                    let body = current_body.unwrap_or_default();
                    Ok((
                        ResolvedAction::Static {
                            response: response.clone(),
                            body,
                        },
                        None,
                    ))
                }
                RuleAction::Processor(spec) => {
                    let preview = self.preview_processor(spec, handoff, current).await?;
                    Ok((ResolvedAction::Processor(spec.clone()), preview))
                }
            },
            None => {
                let input = default_response(
                    handoff
                        .and_then(|handoff| handoff.source_meta.get("status"))
                        .and_then(serde_json::Value::as_i64),
                    sample.as_deref(),
                );
                let (response, body) = resolve_static_input(&input, sample.as_deref(), None)?;
                Ok((ResolvedAction::Static { response, body }, None))
            }
        }
    }

    /// Runs the rule's script on its sample, for the GUI's dry-run button.
    pub async fn processor_test(&self, id: &str) -> AppResult<Value> {
        let rule = self.resolve_reference(id).await?;
        let current = self.current_revision(&rule).await?;
        let RuleAction::Processor(spec) = &current.action else {
            return Err(AppError::conflict("not_a_processor"));
        };
        self.preview_processor(spec, None, Some(&current))
            .await?
            .ok_or_else(|| AppError::conflict("sample_body_unavailable"))
    }

    /// Runs the script on the rule's sample and describes what it produced.
    async fn preview_processor(
        &self,
        spec: &ProcessorSpec,
        handoff: Option<&crate::db::network::HandoffRow>,
        current: Option<&RuleRevisionRow>,
    ) -> AppResult<Option<Value>> {
        let Some(body) = self.sample_bytes(handoff, current).await? else {
            return Ok(None);
        };
        let (method, url, request_headers, request_body, status, status_text, response_headers) =
            match (handoff, current) {
                (Some(handoff), _) => (
                    handoff
                        .source_meta
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("GET")
                        .to_string(),
                    handoff
                        .source_meta
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    handoff.request_headers.clone(),
                    handoff.request_body.clone(),
                    handoff
                        .source_meta
                        .get("status")
                        .and_then(Value::as_i64)
                        .unwrap_or(200),
                    handoff
                        .source_meta
                        .get("statusText")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    handoff.response_headers.clone(),
                ),
                (None, Some(revision)) => (
                    "GET".to_string(),
                    format!("https://sample.invalid{}", revision.matcher.pathname),
                    Vec::new(),
                    None,
                    revision.sample_status.unwrap_or(200),
                    String::new(),
                    revision.sample_headers.clone(),
                ),
                _ => return Ok(None),
            };
        let input = ProcessorInput {
            method,
            url,
            request_headers,
            request_body,
            status,
            status_text,
            response_headers: response_headers.clone(),
            body: body.clone(),
            cookies_json: "[]".to_string(),
        };
        let hooks = ProcessorHooks {
            upstream_cookies: Box::new(|| Ok("[]".to_string())),
        };
        let run = self
            .processor
            .run(spec, input, hooks)
            .await
            .map_err(|error| {
                AppError::bad_request(format!("processor_failed_on_sample: {}", error.message))
            })?;
        let (outcome, plan) = match &run.result {
            ProcessorResult::Passthrough => (
                "passthrough",
                build_processed(
                    status,
                    &response_headers,
                    &body,
                    &ProcessorResult::Replace {
                        status: None,
                        status_text: None,
                        headers: None,
                        set_headers: HeaderPatch::new(),
                        body: None,
                    },
                ),
            ),
            replace => (
                "processed",
                build_processed(status, &response_headers, &body, replace),
            ),
        };
        let body_preview = std::str::from_utf8(&plan.body)
            .ok()
            .map(|text| text.chars().take(2048).collect::<String>());
        Ok(Some(json!({
            "outcome": outcome,
            "status": plan.response_code,
            "headers": plan
                .headers
                .iter()
                .map(|header| json!({"name": header.name, "value": header.value}))
                .collect::<Vec<_>>(),
            "bodyPreview": body_preview,
            "bytes": plan.body.len(),
            "logs": run.logs,
            "elapsedMs": run.elapsed_ms,
        })))
    }

    async fn sample_bytes(
        &self,
        handoff: Option<&crate::db::network::HandoffRow>,
        current: Option<&RuleRevisionRow>,
    ) -> AppResult<Option<Vec<u8>>> {
        if let Some(handoff) = handoff {
            return self.bodies.read_handoff(&handoff.id).await;
        }
        match current {
            Some(revision) if revision.sample_available => {
                self.bodies
                    .read_rule_sample(&revision.rule_id, revision.sample_rev)
                    .await
            }
            _ => Ok(None),
        }
    }

    #[allow(clippy::type_complexity)]
    fn sample_fields(
        &self,
        handoff: Option<&crate::db::network::HandoffRow>,
        current: Option<&RuleRevisionRow>,
        revision: u32,
    ) -> (
        u32,
        bool,
        Option<i64>,
        Vec<browseros_core::network::HeaderPair>,
        Value,
    ) {
        if let Some(handoff) = handoff {
            // Handoff bodies live at handoffs/{id}, so `body_state` is what
            // says whether a sample exists.
            let available = handoff.body_state == "available";
            return (
                revision,
                available,
                handoff.source_meta.get("status").and_then(Value::as_i64),
                handoff.response_headers.clone(),
                json!({
                    "method": handoff.source_meta.get("method"),
                    "url": handoff.source_meta.get("url"),
                    "headers": handoff.request_headers,
                    "body": handoff.request_body,
                }),
            );
        }
        match current {
            Some(current) => (
                current.sample_rev,
                current.sample_available,
                current.sample_status,
                current.sample_headers.clone(),
                current.sample_request.clone(),
            ),
            None => (revision, false, None, Vec::new(), Value::Null),
        }
    }

    async fn resolve_scope(
        &self,
        request: &ApplyRequest,
        existing: Option<&RuleRow>,
    ) -> AppResult<(ScopeKind, Option<i64>)> {
        let kind = match request.scope.as_deref() {
            Some("tab") => ScopeKind::Tab,
            Some("all_tabs") => ScopeKind::AllTabs,
            Some(other) => return Err(AppError::bad_request(format!("invalid_scope: {other}"))),
            None => existing.map_or(ScopeKind::AllTabs, |rule| rule.scope_kind),
        };
        match kind {
            ScopeKind::AllTabs => Ok((kind, None)),
            ScopeKind::Tab => {
                let tab = request
                    .tab_id
                    .or_else(|| existing.and_then(|rule| rule.scope_tab))
                    .ok_or_else(|| AppError::bad_request("tab_required"))?;
                if !self.tab_exists(tab).await {
                    return Err(AppError::bad_request(format!("tab_unbound: {tab}")));
                }
                Ok((kind, Some(tab)))
            }
        }
    }

    async fn resolve_name(
        &self,
        request: &ApplyRequest,
        existing: Option<&RuleRow>,
        matcher: &EndpointMatcher,
    ) -> AppResult<String> {
        if let Some(name) = &request.name {
            let name = validate_rule_name(name)?;
            // The unique index is the last line of defence; checking here turns
            // the conflict into the stable `rule_name_taken` error.
            let own = existing.is_some_and(|rule| rule.name.eq_ignore_ascii_case(&name));
            let taken = self.repo.rule_names_lowercase().await?;
            if !own && taken.iter().any(|taken| taken.eq_ignore_ascii_case(&name)) {
                return Err(AppError::conflict("rule_name_taken"));
            }
            return Ok(name);
        }
        if let Some(existing) = existing {
            return Ok(existing.name.clone());
        }
        let taken = self.repo.rule_names_lowercase().await?;
        Ok(default_rule_name(matcher, &taken))
    }

    async fn placeholder_name(&self, matcher: &EndpointMatcher) -> String {
        let taken = self.repo.rule_names_lowercase().await.unwrap_or_default();
        default_rule_name(matcher, &taken)
    }

    async fn replaced_by(
        &self,
        matcher: &EndpointMatcher,
        self_id: Option<&str>,
    ) -> AppResult<Vec<Replacement>> {
        let rules = self.repo.list_rules(false).await?;
        let revisions = self
            .repo
            .list_current_revisions(&rules.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>())
            .await?;
        let mut replaced = Vec::new();
        for revision in revisions {
            if Some(revision.rule_id.as_str()) == self_id {
                continue;
            }
            if !same_matcher(&revision.matcher, matcher) {
                continue;
            }
            let name = rules
                .iter()
                .find(|rule| rule.id == revision.rule_id)
                .map(|rule| rule.name.clone())
                .unwrap_or_default();
            replaced.push(Replacement {
                id: revision.rule_id,
                name,
            });
        }
        Ok(replaced)
    }

    async fn preview_matches(
        &self,
        matcher: &EndpointMatcher,
        scope: ScopeKind,
        tab: Option<i64>,
    ) -> AppResult<Vec<Value>> {
        let reader = self.records.read().await.clone();
        let records = match reader {
            Some(reader) => reader().await,
            None => Vec::new(),
        };
        let mut samples = Vec::new();
        for record in records {
            if let ScopeKind::Tab = scope
                && (tab.is_none() || record.tab_id != tab)
            {
                continue;
            }
            if matches(
                matcher,
                &record.method,
                &record.url,
                record.post_data.as_deref(),
                record.resource_type.as_deref(),
            )
            .is_match()
            {
                samples.push(json!({
                    "recordId": record.record_id,
                    "method": record.method,
                    "url": record.url,
                    "status": record.status,
                    "tabTitle": record.tab_title,
                    "at": record.started_at_ms,
                }));
            }
        }
        Ok(samples)
    }

    /// Rebuilds the snapshot from the database and pushes it to the runtime.
    /// Callers hold `rules_lock`, except `stop_all` which uses its own
    /// generation.
    pub async fn rebuild_and_apply(&self) -> AppResult<Rebuild> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let rules = self.repo.list_rules(false).await?;
        let revisions = self
            .repo
            .list_current_revisions(&rules.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>())
            .await?;
        let mut runtime_rules = Vec::new();
        let mut failed: Vec<(String, RuleState, StateReason)> = Vec::new();
        let mut included = Vec::new();

        for rule in &rules {
            if !rule.enabled || !matches!(rule.state, RuleState::Active | RuleState::Pending) {
                continue;
            }
            let Some(revision) = revisions.iter().find(|revision| {
                revision.rule_id == rule.id && revision.revision == rule.current_revision
            }) else {
                failed.push((rule.id.clone(), RuleState::Failed, StateReason::BodyMissing));
                continue;
            };
            match &revision.action {
                RuleAction::Static(response) => {
                    let body = self
                        .bodies
                        .read_rule(&revision.body_id.clone().unwrap_or_default())
                        .await?;
                    match body {
                        None => failed.push((
                            rule.id.clone(),
                            RuleState::Failed,
                            StateReason::BodyMissing,
                        )),
                        Some(body) if body.len() as u64 > RULE_BODY_LIMIT => failed.push((
                            rule.id.clone(),
                            RuleState::Failed,
                            StateReason::BodyTooLarge,
                        )),
                        Some(body) => {
                            included.push(rule.id.clone());
                            runtime_rules.push(RuntimeRule {
                                id: rule.id.clone(),
                                revision: revision.revision,
                                scope_tab: rule.scope_tab,
                                matcher: revision.matcher.clone(),
                                action: RuntimeAction::Static {
                                    response: response.clone(),
                                    body: Arc::from(body),
                                    sample_headers: Arc::from(
                                        revision.sample_headers.clone().into_boxed_slice(),
                                    ),
                                },
                                updated_at: rule.updated_at,
                            });
                        }
                    }
                }
                RuleAction::Processor(spec) => {
                    if super::processor::validate_script(&spec.script).is_err() {
                        // The stored script no longer compiles: keep the intent,
                        // surface the failure and leave it out of the snapshot.
                        let rule_id = rule.id.clone();
                        let repo = self.repo.clone();
                        let at = now_ms();
                        self.stmt(|connection| {
                            let repo = repo.clone();
                            let rule_id = rule_id.clone();
                            Box::pin(async move {
                                repo.update_rule_intent(
                                    connection,
                                    &rule_id,
                                    true,
                                    RuleState::Failed,
                                    Some(&StateReason::ProcessorInvalid),
                                    at,
                                )
                                .await
                            })
                        })
                        .await?;
                        continue;
                    }
                    included.push(rule.id.clone());
                    runtime_rules.push(RuntimeRule {
                        id: rule.id.clone(),
                        revision: revision.revision,
                        scope_tab: rule.scope_tab,
                        matcher: revision.matcher.clone(),
                        action: RuntimeAction::Processor {
                            spec: Arc::new(spec.clone()),
                        },
                        updated_at: rule.updated_at,
                    });
                }
            }
        }

        *self.snapshot_ids.write().await = included.clone();
        let set = Arc::new(RuntimeRuleSet {
            generation,
            rules: runtime_rules,
        });
        let report = match self.runtime.current() {
            Some(handle) => Some(handle.apply_rules(set).await),
            None => None,
        };

        let now = now_ms();
        let mut changed = Vec::new();
        if let Some((state, reason)) = derived_state(report.as_ref()) {
            let targets = included.clone();
            if !targets.is_empty() {
                let targets = targets.clone();
                changed = self
                    .stmt(|connection| {
                        let repo = self.repo.clone();
                        Box::pin(async move {
                            repo.set_state_for(connection, &targets, state, reason.as_ref(), now)
                                .await
                        })
                    })
                    .await?;
            }
        }
        for (id, failed_state, reason) in std::mem::take(&mut failed) {
            let failed_reason = reason.clone();
            let repo = self.repo.clone();
            let rule_id = id.clone();
            self.stmt(|connection| {
                Box::pin(async move {
                    repo.update_rule_intent(
                        connection,
                        &rule_id,
                        true,
                        failed_state,
                        Some(&failed_reason),
                        now,
                    )
                    .await
                })
            })
            .await?;
            changed.push(id.clone());
        }

        Ok(Rebuild {
            changed,
            narrowed: report.is_none_or(|report| {
                !report.stale && !report.timed_out && report.acked == report.total
            }),
        })
    }

    async fn broadcast_rules(&self, rebuild: &Rebuild) {
        for id in &rebuild.changed {
            self.broadcast_rule(id).await;
        }
    }

    async fn broadcast_rule(&self, id: &str) {
        if let Ok(Some(rule)) = self.repo.get_rule(id).await {
            self.broadcast(
                "rule",
                json!({
                    "id": rule.id,
                    "name": rule.name,
                    "scope": rule.scope_kind.as_str(),
                    "scopeTab": rule.scope_tab,
                    "enabled": rule.enabled,
                    "state": rule.state.as_str(),
                    "stateReason": rule.state_reason.as_ref().map(|reason| reason.as_str()),
                    "currentRevision": rule.current_revision,
                    "hitCount": rule.hit_count,
                    "lastHitAt": rule.last_hit_at,
                    "authorClient": rule.author_client,
                    "createdAt": rule.created_at,
                    "updatedAt": rule.updated_at,
                }),
            );
        }
    }
}

/// `None` means "leave every rule alone": a stale or empty snapshot must not
/// downgrade rules an operation did not touch.
fn derived_state(report: Option<&ApplyReport>) -> Option<(RuleState, Option<StateReason>)> {
    match report {
        None => Some((RuleState::Pending, Some(StateReason::NoBrowser))),
        Some(report) if report.stale || report.timed_out => {
            Some((RuleState::Pending, Some(StateReason::ApplyTimeout)))
        }
        Some(report) if report.total == 0 => Some((RuleState::Pending, Some(StateReason::NoTab))),
        Some(report) if report.acked == 0 => {
            Some((RuleState::Pending, Some(StateReason::ApplyTimeout)))
        }
        Some(_) => Some((RuleState::Active, None)),
    }
}

fn same_matcher(left: &EndpointMatcher, right: &EndpointMatcher) -> bool {
    left.method == right.method
        && left.origin == right.origin
        && left.pathname == right.pathname
        && left.exact_url == right.exact_url
        && left.query_policy.ignore_keys == right.query_policy.ignore_keys
        && left.body_fields == right.body_fields
        && left.resource_type == right.resource_type
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

enum ResolvedAction {
    Static {
        response: StaticResponse,
        body: Vec<u8>,
    },
    Processor(ProcessorSpec),
}

impl ResolvedAction {
    fn action(&self) -> RuleAction {
        match self {
            Self::Static { response, .. } => RuleAction::Static(response.clone()),
            Self::Processor(spec) => RuleAction::Processor(spec.clone()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Static { .. } => "static",
            Self::Processor(_) => "processor",
        }
    }

    fn body_id(&self, rule_id: &str, revision: u32) -> Option<String> {
        match self {
            Self::Static { .. } => Some(format!("{rule_id}/{revision}")),
            Self::Processor(_) => None,
        }
    }
}
