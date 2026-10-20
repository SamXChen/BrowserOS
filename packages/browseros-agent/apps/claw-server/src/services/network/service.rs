//! Session-scoped capture buffer, handoff snapshots and rule lifecycle.

use super::{
    apply::resolve_patch_response,
    apply::{default_response, matcher_from_seed, response_body, validate_response},
    body_store::BodyStore,
    handoff::{build_handoff, header_lines},
};
use crate::{
    db::{
        Database,
        network::{HandoffRow, HitRow, NetworkRepository, RuleRevisionRow, RuleRow, ScopeKind},
    },
    error::{AppError, AppResult},
};
use browseros_core::{
    BrowserSession,
    network::{
        ApplyReport, BodyField, MatchOutcome, MockBody, MockResponse, NetworkRecord,
        NetworkRuntime, NetworkRuntimeHandle, RuntimeEvent, RuntimeRule, RuntimeRuleSet,
        TabSummary, matches, parse_url_parts,
    },
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::warn;

const UNDO_WINDOW: Duration = Duration::from_secs(8);
const GLOBAL_RECORD_LIMIT: usize = 20_000;
const PER_TAB_RECORD_LIMIT: usize = 2_000;
const BODY_TOTAL_LIMIT: u64 = 256 * 1024 * 1024;
const SSE_CAPACITY: usize = 512;
const RULE_BODY_LIMIT: u64 = 64 * 1024 * 1024;
const HANDOFF_PIN_LIMIT: usize = 20;
const HANDOFF_BODY_LIMIT: u64 = 32 * 1024 * 1024;

/// Resolves the live browser session without making this service depend on
/// another service module. `app.rs` composes the two.
pub type SessionProvider =
    std::sync::Arc<dyn Fn() -> BoxFuture<'static, Option<Arc<BrowserSession>>> + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    pub event: &'static str,
    pub data: Value,
}

#[derive(Debug, Default)]
struct UndoEntry {
    record: NetworkRecord,
    expires_at_ms: i64,
}

#[derive(Default)]
struct CaptureState {
    running: bool,
    started_at_ms: Option<i64>,
    records: BTreeMap<u64, NetworkRecord>,
    index: HashMap<String, u64>,
    body_bytes: u64,
    body_sizes: HashMap<String, u64>,
    pinned: HashSet<String>,
    gap_events: u64,
    tabs: Vec<TabSummary>,
    deleted: HashMap<String, UndoEntry>,
}

fn enforce_buffer_limits(state: &mut CaptureState) -> Vec<NetworkRecord> {
    let mut victims: HashSet<u64> = HashSet::new();
    let mut per_tab: HashMap<i64, Vec<u64>> = HashMap::new();
    for (seq, record) in &state.records {
        if !state.pinned.contains(&record.record_id) {
            per_tab
                .entry(record.tab_id.unwrap_or(-1))
                .or_default()
                .push(*seq);
        }
    }
    for seqs in per_tab.values() {
        let excess = seqs.len().saturating_sub(PER_TAB_RECORD_LIMIT);
        victims.extend(seqs.iter().take(excess).copied());
    }
    let global_excess = state
        .records
        .len()
        .saturating_sub(victims.len())
        .saturating_sub(GLOBAL_RECORD_LIMIT);
    if global_excess > 0 {
        let mut marked = 0;
        for (seq, record) in &state.records {
            if marked >= global_excess {
                break;
            }
            if !state.pinned.contains(&record.record_id) && victims.insert(*seq) {
                marked += 1;
            }
        }
    }
    if state.body_bytes > BODY_TOTAL_LIMIT {
        let mut bytes = state.body_bytes;
        for (seq, record) in &state.records {
            if bytes <= BODY_TOTAL_LIMIT {
                break;
            }
            if state.pinned.contains(&record.record_id) {
                continue;
            }
            let size = state
                .body_sizes
                .get(&record.record_id)
                .copied()
                .unwrap_or(0);
            if size > 0 && victims.insert(*seq) {
                bytes = bytes.saturating_sub(size);
            }
        }
    }
    let mut removed = Vec::new();
    for seq in victims {
        if let Some(record) = state.records.remove(&seq) {
            state.index.remove(&record.record_id);
            if let Some(size) = state.body_sizes.remove(&record.record_id) {
                state.body_bytes = state.body_bytes.saturating_sub(size);
            }
            removed.push(record);
        }
    }
    removed
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyInput {
    pub handoff_id: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub scope_tab: Option<i64>,
    #[serde(default)]
    pub ignore_keys: Option<Vec<String>>,
    #[serde(default)]
    pub strict_query: bool,
    #[serde(default)]
    pub body_fields: Option<Vec<BodyField>>,
    #[serde(default)]
    pub exact_url: Option<String>,
    #[serde(default)]
    pub response: Option<MockResponse>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub author_client: Option<String>,
}

fn default_scope() -> String {
    "all_tabs".to_string()
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub rule_id: String,
    pub revision: u32,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub scope: String,
    pub ignore_keys: Vec<String>,
    pub strict_query: bool,
    pub replaced: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preview: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

pub struct NetworkService {
    repo: NetworkRepository,
    bodies: Arc<BodyStore>,
    session: SessionProvider,
    state: Mutex<CaptureState>,
    runtime_rules: RwLock<Arc<RuntimeRuleSet>>,
    runtime: Mutex<Option<NetworkRuntimeHandle>>,
    apply_lock: Mutex<()>,
    sse: broadcast::Sender<SseEvent>,
    generation: AtomicU64,
    cancel: CancellationToken,
}

impl NetworkService {
    pub async fn new(
        session: SessionProvider,
        database: &Database,
        browserclaw_dir: &std::path::Path,
    ) -> AppResult<Arc<Self>> {
        let repo = NetworkRepository::new(database.clone());
        let bodies = Arc::new(BodyStore::new(browserclaw_dir.join("network")).await?);
        // D3: the capture buffer is session-scoped. Metadata already lived in
        // memory, but body files from a previous process must not linger.
        bodies.clear_tmp().await?;
        let (sse, _) = broadcast::channel(SSE_CAPACITY);
        let service = Arc::new(Self {
            repo,
            bodies,
            session,
            state: Mutex::new(CaptureState::default()),
            runtime_rules: RwLock::new(Arc::new(RuntimeRuleSet::default())),
            runtime: Mutex::new(None),
            apply_lock: Mutex::new(()),
            sse,
            generation: AtomicU64::new(0),
            cancel: CancellationToken::new(),
        });
        let rules = service.build_runtime_rules(0).await?;
        *service.runtime_rules.write().await = rules;
        service.spawn_supervisor();
        service.spawn_maintenance();
        Ok(service)
    }

    // ---- capture -------------------------------------------------------

    pub async fn start_capture(&self) -> AppResult<Value> {
        {
            let mut state = self.state.lock().await;
            if !state.running {
                state.running = true;
                state.started_at_ms = Some(now_ms());
            }
        }
        if let Some(runtime) = self.runtime.lock().await.as_ref() {
            runtime.set_capture(true);
        }
        self.broadcast("capture", self.capture_json().await);
        Ok(self.capture_json().await)
    }

    /// Stops capture and discards the transient record buffer (D3).
    pub async fn stop_capture(&self) -> AppResult<Value> {
        {
            let mut state = self.state.lock().await;
            state.running = false;
            state.started_at_ms = None;
            state.records.clear();
            state.index.clear();
            state.body_bytes = 0;
            state.body_sizes.clear();
            state.pinned.clear();
            state.deleted.clear();
        }
        self.bodies.clear_tmp().await?;
        if let Some(runtime) = self.runtime.lock().await.as_ref() {
            runtime.set_capture(false);
        }
        let snapshot = self.capture_json().await;
        self.broadcast("capture", snapshot.clone());
        Ok(snapshot)
    }

    pub async fn capture_running(&self) -> bool {
        self.state.lock().await.running
    }

    async fn clear_records(&self) -> AppResult<u64> {
        let mut state = self.state.lock().await;
        let removed = state.records.len() as u64;
        state.records.clear();
        state.index.clear();
        state.body_bytes = 0;
        state.body_sizes.clear();
        state.pinned.clear();
        state.deleted.clear();
        drop(state);
        self.bodies.clear_tmp().await?;
        self.broadcast(
            "deleted",
            json!({ "kind": "record", "id": null, "all": true }),
        );
        Ok(removed)
    }

    // ---- records -------------------------------------------------------

    pub async fn list_records(&self, since: Option<u64>, limit: usize) -> Vec<NetworkRecord> {
        let state = self.state.lock().await;
        if since.is_none() {
            // The first screen wants the newest records, still in ascending order.
            let mut records = state
                .records
                .values()
                .rev()
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            records.reverse();
            return records;
        }
        state
            .records
            .range(since.map(|seq| (seq + 1)..).unwrap_or(u64::MIN..))
            .take(limit)
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub async fn oldest_seq(&self) -> Option<u64> {
        self.state.lock().await.records.keys().next().copied()
    }

    pub async fn get_record(&self, record_id: &str) -> Option<NetworkRecord> {
        let state = self.state.lock().await;
        state
            .index
            .get(record_id)
            .and_then(|seq| state.records.get(seq))
            .cloned()
    }

    pub async fn response_body_page(
        &self,
        record_id: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<Value>> {
        let Some((record, _)) = self.record_and_body(record_id).await else {
            return Ok(None);
        };
        let Some((bytes, total)) = self
            .bodies
            .read_record_range(record_id, offset, limit)
            .await?
        else {
            return Ok(Some(json!({
                "encoding": "utf8",
                "offset": offset,
                "total": 0,
                "truncated": false,
                "data": "",
                "state": record.body_state,
            })));
        };
        let (encoding, data) = text_or_base64(&bytes);
        let truncated = (offset + bytes.len() as u64) < total;
        Ok(Some(json!({
            "encoding": encoding,
            "offset": offset,
            "total": total,
            "truncated": truncated,
            "data": data,
            "state": record.body_state,
            "mimeType": record.mime_type,
        })))
    }

    pub async fn request_body_page(
        &self,
        record_id: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<Value>> {
        let Some((record, _)) = self.record_and_body(record_id).await else {
            return Ok(None);
        };
        let body = record.post_data.unwrap_or_default();
        let bytes = body.as_bytes();
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(limit as usize).min(bytes.len());
        Ok(Some(json!({
            "encoding": "utf8",
            "offset": start,
            "total": bytes.len(),
            "truncated": end < bytes.len(),
            "data": String::from_utf8_lossy(&bytes[start..end]),
        })))
    }

    /// Immediate removal with an eight-second undo window. The body file stays
    /// until the window closes so restore is exact.
    pub async fn delete_record(&self, record_id: &str) -> AppResult<Value> {
        let mut state = self.state.lock().await;
        let Some(seq) = state.index.remove(record_id) else {
            return Err(AppError::not_found("record_not_found"));
        };
        let Some(record) = state.records.remove(&seq) else {
            return Err(AppError::not_found("record_not_found"));
        };
        let token = format!("undo_{}", uuid::Uuid::new_v4().simple());
        let until = now_ms() + UNDO_WINDOW.as_millis() as i64;
        state.deleted.insert(
            token.clone(),
            UndoEntry {
                record,
                expires_at_ms: until,
            },
        );
        drop(state);
        self.broadcast(
            "deleted",
            json!({ "kind": "record", "id": record_id, "undoToken": token }),
        );
        Ok(json!({ "deleted": true, "undoToken": token, "undoUntil": until }))
    }

    pub async fn restore_record(&self, undo_token: &str) -> AppResult<Value> {
        let mut state = self.state.lock().await;
        let Some(entry) = state.deleted.remove(undo_token) else {
            return Err(AppError::gone("undo_expired"));
        };
        if entry.expires_at_ms < now_ms() {
            return Err(AppError::gone("undo_expired"));
        }
        let seq = entry.record.seq;
        let record_id = entry.record.record_id.clone();
        state.records.insert(seq, entry.record);
        state.index.insert(record_id.clone(), seq);
        drop(state);
        self.broadcast(
            "record",
            json!({ "record": self.get_record(&record_id).await }),
        );
        Ok(json!({ "restored": true, "recordId": record_id }))
    }

    pub async fn delete_all_records(&self) -> AppResult<Value> {
        let removed = self.clear_records().await?;
        Ok(json!({ "deleted": removed, "all": true }))
    }

    // ---- SSE -----------------------------------------------------------

    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.sse.subscribe()
    }

    fn broadcast(&self, event: &'static str, data: Value) {
        let _ = self.sse.send(SseEvent { event, data });
    }

    // ---- handoffs ------------------------------------------------------

    pub async fn create_handoff(&self, record_id: &str) -> AppResult<Value> {
        let Some(record) = self.get_record(record_id).await else {
            return Err(AppError::not_found("record_not_found"));
        };
        let response_body = self.bodies.read_record(record_id).await?;
        let now = now_ms();
        let active_handoffs = self
            .repo
            .list_handoffs()
            .await?
            .into_iter()
            .filter(|handoff| handoff.expires_at > now)
            .collect::<Vec<_>>();
        if active_handoffs.len() >= HANDOFF_PIN_LIMIT {
            return Err(AppError::conflict("handoff_pin_quota"));
        }
        let mut pinned_bytes = 0;
        for handoff in &active_handoffs {
            pinned_bytes += self.bodies.handoff_size(&handoff.id).await;
        }
        let new_bytes = response_body
            .as_ref()
            .map(|body| body.len() as u64)
            .unwrap_or(0);
        if pinned_bytes + new_bytes > HANDOFF_BODY_LIMIT {
            return Err(AppError::conflict("handoff_body_quota"));
        }
        let related = self.related_records(&record).await;
        let handoff_id = format!("netref_{}", uuid::Uuid::new_v4().simple());
        let draft = build_handoff(handoff_id.clone(), &record, response_body, &related, now);
        let mut handoff = draft.handoff;
        if let Some(body) = &draft.response_body {
            self.bodies.write_handoff(&handoff_id, body).await?;
            handoff.body_id = Some(handoff_id.clone());
        }
        self.repo.insert_handoff(&handoff).await?;
        self.state
            .lock()
            .await
            .pinned
            .insert(record.record_id.clone());
        Ok(json!({
            "handoffId": handoff_id,
            "clipboardText": draft.clipboard,
            "expiresAt": handoff.expires_at,
        }))
    }

    async fn related_records(&self, record: &NetworkRecord) -> Vec<NetworkRecord> {
        let parts = parse_url_parts(&record.url);
        let state = self.state.lock().await;
        state
            .records
            .values()
            .rev()
            .filter(|candidate| candidate.record_id != record.record_id)
            .filter(|candidate| {
                let other = parse_url_parts(&candidate.url);
                other.origin == parts.origin && other.pathname == parts.pathname
            })
            .take(20)
            .cloned()
            .collect()
    }

    pub async fn list_handoffs(&self) -> AppResult<Vec<Value>> {
        let rows = self.repo.list_handoffs().await?;
        Ok(rows.iter().map(handoff_summary).collect())
    }

    pub async fn get_handoff(&self, id: &str) -> AppResult<HandoffRow> {
        let handoff = self
            .repo
            .get_handoff(id)
            .await?
            .ok_or_else(|| AppError::not_found("handoff_not_found"))?;
        if handoff.expires_at < now_ms() {
            return Err(AppError::gone("handoff_expired"));
        }
        Ok(handoff)
    }

    pub async fn handoff_body(&self, id: &str) -> AppResult<Option<Vec<u8>>> {
        self.bodies.read_handoff(id).await
    }

    pub async fn delete_handoff(&self, id: &str) -> AppResult<Value> {
        let pinned_record = self.repo.get_handoff(id).await?.and_then(|handoff| {
            handoff
                .source_meta
                .get("recordId")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let deleted = self.repo.delete_handoff(id).await?;
        if let Some(record_id) = pinned_record {
            self.state.lock().await.pinned.remove(&record_id);
        }
        self.bodies.remove_handoff(id).await?;
        self.broadcast("deleted", json!({ "kind": "handoff", "id": id }));
        Ok(json!({ "deleted": deleted > 0 }))
    }

    // ---- rules ---------------------------------------------------------

    pub async fn list_rules(&self) -> AppResult<Vec<Value>> {
        let rules = self.repo.list_rules(false).await?;
        let mut out = Vec::with_capacity(rules.len());
        for rule in rules {
            let revision = self
                .repo
                .get_revision(&rule.id, rule.current_revision)
                .await?;
            let hits = self.repo.list_hits(Some(&rule.id), 1).await?;
            out.push(rule_json(&rule, revision.as_ref(), hits.first()));
        }
        Ok(out)
    }

    pub async fn get_rule_detail(&self, id: &str) -> AppResult<Value> {
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let revision = self.repo.get_revision(id, rule.current_revision).await?;
        let hits = self.repo.list_hits(Some(id), 200).await?;
        let body_preview = if let Some(revision) = &revision {
            match &revision.body_id {
                Some(body_id) => self
                    .bodies
                    .read_rule(body_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|body| {
                        let slice = &body[..body.len().min(4096)];
                        match std::str::from_utf8(slice) {
                            Ok(text) => json!({ "encoding": "utf8", "text": text }),
                            Err(_) => json!({
                                "encoding": "base64",
                                "text": browseros_core::network::base64_encode(slice),
                            }),
                        }
                    }),
                None => None,
            }
        } else {
            None
        };
        Ok(json!({
            "rule": rule_json(&rule, revision.as_ref(), hits.first()),
            "revision": revision.as_ref().map(|revision| json!({
                "revision": revision.revision,
                "matcher": revision.matcher,
                "response": revision.response,
                "createdAt": revision.created_at,
                "source": {
                    "handoffId": revision.source_handoff_id,
                    "recordId": revision.source_record_id,
                },
            })),
            "bodyPreview": body_preview,
            "hitCount": hits.len(),
            "hits": hits.iter().take(10).map(hit_json).collect::<Vec<_>>(),
        }))
    }

    /// Replay the rule matcher against the current capture buffer. This is the
    /// same matcher used by the runtime, so the UI preview cannot drift.
    pub async fn test_rule(&self, id: &str) -> AppResult<Value> {
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let revision = self
            .repo
            .get_revision(id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::not_found("revision_not_found"))?;
        let state = self.state.lock().await;
        let considered = state.records.len();
        let mut samples = Vec::new();
        for record in state.records.values().rev() {
            if matches(
                &revision.matcher,
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
                if samples.len() >= 20 {
                    break;
                }
            }
        }
        let matched = samples.len();
        Ok(json!({
            "matched": matched,
            "considered": considered,
            "samples": samples,
        }))
    }

    pub async fn list_revisions(&self, id: &str) -> AppResult<Vec<Value>> {
        Ok(self
            .repo
            .list_revisions(id)
            .await?
            .iter()
            .map(|revision| {
                json!({
                    "revision": revision.revision,
                    "matcher": revision.matcher,
                    "response": revision.response,
                    "createdAt": revision.created_at,
                })
            })
            .collect())
    }

    /// Explain why recent captured requests did not match this rule. The
    /// matcher itself stays the single source of truth; this only adds the
    /// query-key evidence the UI/MCP needs to offer a one-click ignore.
    pub async fn near_misses(&self, rule_id: &str) -> AppResult<Vec<Value>> {
        let rule = self
            .repo
            .get_rule(rule_id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let Some(revision) = self
            .repo
            .get_revision(rule_id, rule.current_revision)
            .await?
        else {
            return Ok(Vec::new());
        };
        let matcher = revision.matcher;
        let state = self.state.lock().await;
        let mut output = Vec::new();
        for record in state.records.values().rev().take(200) {
            let outcome = matches(
                &matcher,
                &record.method,
                &record.url,
                record.post_data.as_deref(),
                record.resource_type.as_deref(),
            );
            let MatchOutcome::NoMatch { reason } = outcome else {
                continue;
            };
            // Only explain requests for the same endpoint. An unrelated tab
            // is not a near miss and must not hide `no_request_seen`.
            let parts = parse_url_parts(&record.url);
            if !matcher.origin.eq_ignore_ascii_case(&parts.origin)
                || matcher.pathname != parts.pathname
            {
                continue;
            }
            let mut entry = json!({
                "recordId": record.record_id,
                "method": record.method,
                "url": record.url,
                "reason": reason,
                "tabTitle": record.tab_title,
            });
            if reason.starts_with("query_") {
                let sample = group_query(&matcher.sample_query);
                let actual = group_query(&parts.query);
                let mut diffs = Vec::new();
                for (key, values) in &sample {
                    if matcher
                        .query_policy
                        .ignore_keys
                        .iter()
                        .any(|ignored| ignored == key)
                    {
                        continue;
                    }
                    let observed = actual.get(key).cloned().unwrap_or_default();
                    if observed != *values {
                        diffs.push(json!({
                            "key": key,
                            "sample": values,
                            "actual": observed,
                        }));
                    }
                }
                if reason == "query_extra_key" {
                    for key in actual.keys() {
                        if !sample.contains_key(key) {
                            diffs.push(json!({
                                "key": key,
                                "sample": [],
                                "actual": actual.get(key),
                            }));
                        }
                    }
                }
                entry["queryDiffs"] = Value::Array(diffs.clone());
                let suggestions = diffs
                    .iter()
                    .filter_map(|diff| diff.get("key").and_then(Value::as_str))
                    .map(|key| json!(key))
                    .collect::<Vec<_>>();
                entry["suggestion"] = json!({ "ignoreKeys": suggestions });
            }
            if record
                .failure
                .as_deref()
                .is_some_and(|failure| failure.to_ascii_lowercase().contains("cors"))
            {
                entry["reason"] = json!("blocked");
                entry["blockedBy"] = json!("cors");
            }
            output.push(entry);
            if output.len() >= 8 {
                break;
            }
        }
        Ok(output)
    }

    pub async fn list_hits(&self, rule_id: Option<&str>, limit: u32) -> AppResult<Vec<Value>> {
        Ok(self
            .repo
            .list_hits(rule_id, limit)
            .await?
            .iter()
            .map(hit_json)
            .collect())
    }

    pub async fn apply_rule(&self, input: ApplyInput) -> AppResult<ApplyOutcome> {
        let _apply_guard = self.apply_lock.lock().await;
        let handoff = self.get_handoff(&input.handoff_id).await?;
        let matcher = matcher_from_seed(
            &handoff.matcher_seed,
            input.ignore_keys.clone(),
            input.strict_query,
            input.body_fields.clone(),
            input.exact_url.clone(),
        )?;
        let mut response = match input.response.clone() {
            Some(response) => response,
            None => {
                let body = self.handoff_body(&input.handoff_id).await?;
                default_response(
                    handoff.source_meta.get("status").and_then(Value::as_i64),
                    body.as_deref(),
                )
            }
        };
        let sample_body = if matches!(
            response.body,
            browseros_core::network::MockBody::PatchSample { .. }
        ) {
            self.handoff_body(&input.handoff_id).await?
        } else {
            None
        };
        resolve_patch_response(&mut response, sample_body.as_deref())?;
        validate_response(&response)?;
        let body = response_body(&response);

        let existing = self.repo.find_rule_by_handoff(&input.handoff_id).await?;
        let revision = existing
            .as_ref()
            .map(|rule| rule.current_revision + 1)
            .unwrap_or(1);
        let rule_id = existing
            .as_ref()
            .map(|rule| rule.id.clone())
            .unwrap_or_else(|| format!("rule_{}", uuid::Uuid::new_v4().simple()));

        // Disabling an existing rule must not replace its response with an empty
        // one. `rule_apply{enabled:false}` without a response is a pure switch.
        if !input.enabled
            && input.response.is_none()
            && let Some(rule) = &existing
        {
            let value = self.set_rule_enabled(&rule.id, false).await?;
            return Ok(ApplyOutcome {
                rule_id: rule.id.clone(),
                revision: rule.current_revision,
                state: value["state"].as_str().unwrap_or("disabled").to_string(),
                reason: None,
                scope: if rule.scope_kind == ScopeKind::AllTabs {
                    "all_tabs".to_string()
                } else {
                    "source_tab".to_string()
                },
                ignore_keys: matcher.query_policy.ignore_keys.clone(),
                strict_query: input.strict_query,
                replaced: Vec::new(),
                preview: Vec::new(),
                next_action: None,
            });
        }

        if input.dry_run {
            let sample = self
                .preview_matches(&rule_id, revision, &matcher, &response, &body, &input)
                .await;
            return Ok(ApplyOutcome {
                rule_id,
                revision,
                state: "dry_run".to_string(),
                reason: None,
                scope: input.scope.clone(),
                ignore_keys: matcher.query_policy.ignore_keys.clone(),
                strict_query: input.strict_query,
                replaced: sample.0,
                preview: sample.1,
                next_action: None,
            });
        }

        let scope_kind = if input.scope == "source_tab" || input.scope == "tab" {
            ScopeKind::Tab
        } else {
            ScopeKind::AllTabs
        };
        let scope_tab = if scope_kind == ScopeKind::Tab {
            input
                .scope_tab
                .or_else(|| handoff.source_meta.get("tabId").and_then(Value::as_i64))
        } else {
            None
        };
        if scope_kind == ScopeKind::Tab && scope_tab.is_none() {
            return Err(AppError::bad_request("tab_unbound"));
        }

        let now = now_ms();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let body_id = format!("{rule_id}/{revision}");
        self.bodies.write_rule(&rule_id, revision, &body).await?;
        self.repo
            .insert_revision(&RuleRevisionRow {
                rule_id: rule_id.clone(),
                revision,
                matcher: matcher.clone(),
                response: response.clone(),
                sample_headers: handoff.response_headers.clone(),
                body_id: Some(body_id),
                source_handoff_id: Some(input.handoff_id.clone()),
                source_record_id: handoff
                    .source_meta
                    .get("recordId")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                created_at: now,
            })
            .await?;

        let enabled = input.enabled;
        let state = if enabled { "applying" } else { "disabled" };
        match &existing {
            Some(_) => {
                self.repo
                    .update_rule_revision(
                        &rule_id,
                        revision,
                        &scope_kind,
                        scope_tab,
                        generation,
                        now,
                    )
                    .await?;
                self.repo
                    .update_rule_state(&rule_id, enabled, state, None, now)
                    .await?;
            }
            None => {
                self.repo
                    .insert_rule(&RuleRow {
                        id: rule_id.clone(),
                        scope_kind: scope_kind.clone(),
                        scope_tab,
                        enabled,
                        state: state.to_string(),
                        state_reason: None,
                        current_revision: revision,
                        generation,
                        created_at: now,
                        updated_at: now,
                        author_client: input.author_client.clone(),
                        deleted_at: None,
                    })
                    .await?;
            }
        }
        self.repo
            .set_handoff_rule(&input.handoff_id, &rule_id)
            .await?;

        let replaced = if enabled {
            self.replace_conflicting(&rule_id, &matcher).await?
        } else {
            Vec::new()
        };

        let applied = self.reload_runtime_rules().await?;
        // If a stop or another apply advanced the runtime while this request was
        // in flight, this apply must not revive the rule.
        let current_generation = self.runtime_rules.read().await.generation;
        if enabled && self.generation.load(Ordering::SeqCst) != current_generation {
            self.repo
                .update_rule_state(&rule_id, false, "disabled", Some("superseded"), now_ms())
                .await?;
            self.reload_runtime_rules().await.ok();
            self.broadcast_rule(&rule_id).await?;
            return Ok(ApplyOutcome {
                rule_id,
                revision,
                state: "superseded".to_string(),
                reason: Some("superseded".to_string()),
                scope: if scope_kind == ScopeKind::AllTabs {
                    "all_tabs".to_string()
                } else {
                    "source_tab".to_string()
                },
                ignore_keys: matcher.query_policy.ignore_keys.clone(),
                strict_query: input.strict_query,
                replaced,
                preview: Vec::new(),
                next_action: None,
            });
        }
        let final_state = if enabled && applied.sessions == 0 {
            self.repo
                .update_rule_state(
                    &rule_id,
                    false,
                    "failed",
                    Some("fetch_not_enabled"),
                    now_ms(),
                )
                .await?;
            "failed"
        } else if enabled {
            self.repo
                .update_rule_state(&rule_id, true, "active", None, now_ms())
                .await?;
            "active"
        } else {
            "disabled"
        };
        self.broadcast_rule(&rule_id).await?;

        Ok(ApplyOutcome {
            rule_id,
            revision,
            state: final_state.to_string(),
            reason: (final_state == "failed").then(|| "fetch_not_enabled".to_string()),
            scope: if scope_kind == ScopeKind::AllTabs {
                "all_tabs".to_string()
            } else {
                "source_tab".to_string()
            },
            ignore_keys: matcher.query_policy.ignore_keys.clone(),
            strict_query: input.strict_query,
            replaced,
            preview: Vec::new(),
            next_action: (final_state == "active").then(|| "refresh".to_string()),
        })
    }

    async fn preview_matches(
        &self,
        rule_id: &str,
        revision: u32,
        matcher: &browseros_core::network::EndpointMatcher,
        response: &MockResponse,
        body: &[u8],
        input: &ApplyInput,
    ) -> (Vec<String>, Vec<Value>) {
        let now = now_ms();
        let candidate = RuntimeRule {
            id: rule_id.to_string(),
            revision,
            scope_tab: input.scope_tab,
            matcher: matcher.clone(),
            response: response.clone(),
            body: body.to_vec(),
            sample_headers: Vec::new(),
            updated_at: now,
        };
        let matched_records = {
            let state = self.state.lock().await;
            state
                .records
                .values()
                .filter(|record| {
                    browseros_core::network::matches(
                        &candidate.matcher,
                        &record.method,
                        &record.url,
                        record.post_data.as_deref(),
                        record.resource_type.as_deref(),
                    )
                    .is_match()
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut replaced = Vec::new();
        let mut preview = Vec::new();
        let rules = self.repo.list_rules(false).await.unwrap_or_default();
        for record in &matched_records {
            let mut winner_id = candidate.id.clone();
            let mut winner_key = (
                candidate.matcher.body_fields.len(),
                -(candidate.matcher.query_policy.ignore_keys.len() as isize),
                candidate.updated_at,
                candidate.id.clone(),
            );
            for other in &rules {
                if other.id == rule_id || !other.enabled {
                    continue;
                }
                let Ok(Some(other_revision)) = self
                    .repo
                    .get_revision(&other.id, other.current_revision)
                    .await
                else {
                    continue;
                };
                if !browseros_core::network::matches(
                    &other_revision.matcher,
                    &record.method,
                    &record.url,
                    record.post_data.as_deref(),
                    record.resource_type.as_deref(),
                )
                .is_match()
                {
                    continue;
                }
                let key = (
                    other_revision.matcher.body_fields.len(),
                    -(other_revision.matcher.query_policy.ignore_keys.len() as isize),
                    other.updated_at,
                    other.id.clone(),
                );
                if key > winner_key {
                    winner_key = key;
                    winner_id = other.id.clone();
                }
                if other_revision.matcher == *matcher {
                    replaced.push(other.id.clone());
                }
            }
            preview.push(json!({
                "recordId": record.record_id,
                "method": record.method,
                "url": record.url,
                "winnerRuleId": winner_id,
                "candidateWon": winner_id == candidate.id,
            }));
        }
        replaced.sort();
        replaced.dedup();
        (replaced, preview)
    }

    async fn replace_conflicting(
        &self,
        rule_id: &str,
        matcher: &browseros_core::network::EndpointMatcher,
    ) -> AppResult<Vec<String>> {
        let mut replaced = Vec::new();
        for other in self.repo.list_rules(false).await? {
            if other.id == rule_id || !other.enabled {
                continue;
            }
            let Some(revision) = self
                .repo
                .get_revision(&other.id, other.current_revision)
                .await?
            else {
                continue;
            };
            if revision.matcher.same_semantics(matcher) {
                self.repo
                    .update_rule_state(
                        &other.id,
                        false,
                        "disabled",
                        Some(&format!("replaced_by:{rule_id}")),
                        now_ms(),
                    )
                    .await?;
                replaced.push(other.id.clone());
                self.broadcast_rule(&other.id).await?;
            }
        }
        Ok(replaced)
    }

    pub async fn set_rule_scope(
        &self,
        id: &str,
        scope: &str,
        tab_id: Option<i64>,
    ) -> AppResult<Value> {
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let (scope_kind, scope_tab) = match scope {
            "all_tabs" => (ScopeKind::AllTabs, None),
            "tab" => {
                let tab_id = tab_id.ok_or_else(|| AppError::bad_request("tab_unbound"))?;
                if !self
                    .state
                    .lock()
                    .await
                    .tabs
                    .iter()
                    .any(|tab| tab.tab_id == tab_id)
                {
                    return Err(AppError::bad_request("tab_unbound"));
                }
                (ScopeKind::Tab, Some(tab_id))
            }
            _ => return Err(AppError::bad_request("invalid_scope")),
        };
        self.repo
            .update_rule_scope(id, &scope_kind, scope_tab, now_ms())
            .await?;
        self.repo
            .update_rule_state(id, true, "applying", None, now_ms())
            .await?;
        let applied = self.reload_runtime_rules().await?;
        let state = if applied.sessions == 0 {
            self.repo
                .update_rule_state(id, false, "failed", Some("fetch_not_enabled"), now_ms())
                .await?;
            "failed"
        } else {
            self.repo
                .update_rule_state(id, true, "active", None, now_ms())
                .await?;
            "active"
        };
        self.broadcast_rule(id).await?;
        Ok(json!({
            "ruleId": id,
            "scope": scope_kind.as_str(),
            "scopeTab": scope_tab,
            "state": state,
            "revision": rule.current_revision,
        }))
    }

    pub async fn set_rule_enabled(&self, id: &str, enabled: bool) -> AppResult<Value> {
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        self.repo
            .update_rule_state(
                id,
                enabled,
                if enabled { "applying" } else { "disabled" },
                None,
                now_ms(),
            )
            .await?;
        if enabled
            && rule
                .state_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("replaced_by:"))
            && let Some(revision) = self.repo.get_revision(id, rule.current_revision).await?
        {
            // Restoring a replaced rule replaces the newer rule that displaced
            // it, so the switch means what the UI says.
            let _ = self.replace_conflicting(id, &revision.matcher).await?;
        }
        let applied = self.reload_runtime_rules().await?;
        if enabled {
            if applied.sessions == 0 {
                self.repo
                    .update_rule_state(id, false, "failed", Some("fetch_not_enabled"), now_ms())
                    .await?;
                self.broadcast_rule(id).await?;
                return Err(AppError::conflict("fetch_not_enabled"));
            }
            self.repo
                .update_rule_state(id, true, "active", None, now_ms())
                .await?;
        } else if applied.sessions == 0 && applied.fetch_enabled > 0 {
            // D16: the switch may only read "disabled" after Fetch actually
            // narrowed. `sessions > 0` also counts a successful re-enable of
            // the remaining patterns. The DB already says disabled, so restore
            // it to match the still-active runtime and report a visible failure.
            self.repo
                .update_rule_state(id, true, "active", None, now_ms())
                .await?;
            let _ = self.reload_runtime_rules().await;
            self.broadcast_rule(id).await?;
            return Err(AppError::conflict("fetch_not_disabled"));
        }
        self.broadcast_rule(id).await?;
        Ok(json!({
            "ruleId": id,
            "enabled": enabled,
            "state": if enabled { "active" } else { "disabled" },
            "revision": rule.current_revision,
        }))
    }

    /// One-click fix for a near miss: append ignored query keys and create a
    /// new immutable revision with the same response.
    pub async fn ignore_keys(&self, id: &str, keys: &[String]) -> AppResult<Value> {
        let _guard = self.apply_lock.lock().await;
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let source = self
            .repo
            .get_revision(id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::not_found("revision_not_found"))?;
        let mut matcher = source.matcher.clone();
        for key in keys {
            if !key.is_empty()
                && !matcher
                    .query_policy
                    .ignore_keys
                    .iter()
                    .any(|existing| existing == key)
            {
                matcher.query_policy.ignore_keys.push(key.clone());
            }
        }
        let next = rule.current_revision + 1;
        let body = if let Some(body_id) = &source.body_id {
            self.bodies.read_rule(body_id).await?.unwrap_or_default()
        } else {
            response_body(&source.response)
        };
        let body_id = format!("{id}/{next}");
        self.bodies.write_rule(id, next, &body).await?;
        self.repo
            .insert_revision(&RuleRevisionRow {
                rule_id: id.to_string(),
                revision: next,
                matcher: matcher.clone(),
                response: source.response.clone(),
                sample_headers: source.sample_headers.clone(),
                body_id: Some(body_id),
                source_handoff_id: source.source_handoff_id.clone(),
                source_record_id: source.source_record_id.clone(),
                created_at: now_ms(),
            })
            .await?;
        self.repo
            .update_rule_revision(
                id,
                next,
                &rule.scope_kind,
                rule.scope_tab,
                self.generation.fetch_add(1, Ordering::SeqCst) + 1,
                now_ms(),
            )
            .await?;
        self.repo
            .update_rule_state(id, true, "applying", None, now_ms())
            .await?;
        let applied = self.reload_runtime_rules().await?;
        let state = if applied.sessions > 0 {
            self.repo
                .update_rule_state(id, true, "active", None, now_ms())
                .await?;
            "active"
        } else {
            self.repo
                .update_rule_state(id, false, "failed", Some("fetch_not_enabled"), now_ms())
                .await?;
            "failed"
        };
        self.broadcast_rule(id).await?;
        Ok(json!({
            "ruleId": id,
            "revision": next,
            "ignoreKeys": matcher.query_policy.ignore_keys,
            "state": state,
        }))
    }

    pub async fn delete_rule(&self, id: &str) -> AppResult<Value> {
        let now = now_ms();
        let deleted = self.repo.soft_delete_rule(id, now).await?;
        if deleted == 0 {
            return Err(AppError::not_found("rule_not_found"));
        }
        let _ = self.reload_runtime_rules().await?;
        self.broadcast(
            "deleted",
            json!({
                "kind": "rule",
                "id": id,
                "undoToken": id,
                "undoUntil": now + UNDO_WINDOW.as_millis() as i64,
            }),
        );
        Ok(json!({
            "deleted": true,
            "undoToken": id,
            "undoUntil": now + UNDO_WINDOW.as_millis() as i64,
        }))
    }

    pub async fn restore_rule(&self, id: &str) -> AppResult<Value> {
        let restored = self.repo.restore_rule(id).await?;
        if restored == 0 {
            return Err(AppError::not_found("rule_not_found"));
        }
        self.repo
            .update_rule_state(id, true, "applying", None, now_ms())
            .await?;
        let applied = self.reload_runtime_rules().await?;
        let state = if applied.sessions > 0 {
            "active"
        } else {
            "failed"
        };
        self.repo
            .update_rule_state(id, applied.sessions > 0, state, None, now_ms())
            .await?;
        self.broadcast_rule(id).await?;
        Ok(json!({ "restored": true, "ruleId": id, "state": state }))
    }

    pub async fn restore_revision(&self, id: &str, revision: u32) -> AppResult<Value> {
        let rule = self
            .repo
            .get_rule(id)
            .await?
            .ok_or_else(|| AppError::not_found("rule_not_found"))?;
        let source = self
            .repo
            .get_revision(id, revision)
            .await?
            .ok_or_else(|| AppError::not_found("revision_not_found"))?;
        let next = rule.current_revision + 1;
        let body = if let Some(body_id) = &source.body_id {
            self.bodies.read_rule(body_id).await?.unwrap_or_default()
        } else {
            Vec::new()
        };
        let body_id = format!("{id}/{next}");
        self.bodies.write_rule(id, next, &body).await?;
        self.repo
            .insert_revision(&RuleRevisionRow {
                rule_id: id.to_string(),
                revision: next,
                matcher: source.matcher.clone(),
                response: source.response.clone(),
                sample_headers: source.sample_headers.clone(),
                body_id: Some(body_id),
                source_handoff_id: source.source_handoff_id.clone(),
                source_record_id: source.source_record_id.clone(),
                created_at: now_ms(),
            })
            .await?;
        self.repo
            .update_rule_revision(
                id,
                next,
                &rule.scope_kind,
                rule.scope_tab,
                self.generation.fetch_add(1, Ordering::SeqCst) + 1,
                now_ms(),
            )
            .await?;
        let applied = self.reload_runtime_rules().await?;
        let state = if applied.sessions > 0 && rule.enabled {
            "active"
        } else {
            "disabled"
        };
        self.repo
            .update_rule_state(
                id,
                applied.sessions > 0 && rule.enabled,
                state,
                None,
                now_ms(),
            )
            .await?;
        self.broadcast_rule(id).await?;
        Ok(json!({ "ruleId": id, "revision": next, "state": state }))
    }

    pub async fn stop_all(&self) -> AppResult<Value> {
        // Stop is a generation barrier: late applies must observe it. Push an
        // empty snapshot first so delays are cancelled before the DB loop.
        self.generation.fetch_add(1, Ordering::SeqCst);
        let empty = Arc::new(RuntimeRuleSet::default());
        *self.runtime_rules.write().await = empty.clone();
        if let Some(handle) = self.runtime.lock().await.as_ref() {
            handle.set_rules(empty);
        }
        let rules = self.repo.list_rules(false).await?;
        let mut disabled = 0;
        for rule in rules {
            if rule.enabled {
                self.repo
                    .update_rule_state(&rule.id, false, "disabled", Some("stopped_all"), now_ms())
                    .await?;
                disabled += 1;
                self.broadcast_rule(&rule.id).await?;
            }
        }
        // Narrow Fetch before the next request can be paused.
        let applied = self.reload_runtime_rules().await?;
        Ok(json!({
            "disabled": disabled,
            "narrowed": applied.fetch_enabled == 0,
        }))
    }

    pub async fn delete_all_rules(&self) -> AppResult<Value> {
        let rules = self.repo.list_rules(false).await?;
        for rule in &rules {
            self.repo.soft_delete_rule(&rule.id, now_ms()).await?;
        }
        let _ = self.reload_runtime_rules().await?;
        for rule in &rules {
            self.broadcast("deleted", json!({ "kind": "rule", "id": rule.id }));
        }
        Ok(json!({ "deleted": rules.len() }))
    }

    pub async fn delete_hits(&self, rule_id: &str) -> AppResult<Value> {
        let deleted = self.repo.delete_hits(rule_id).await?;
        Ok(json!({ "deleted": deleted }))
    }

    pub async fn rule_by_handoff(&self, handoff_id: &str) -> AppResult<Option<RuleRow>> {
        self.repo.find_rule_by_handoff(handoff_id).await
    }

    pub async fn status_json(&self) -> Value {
        let connected = (self.session)().await.is_some();
        let rules = self.repo.list_rules(false).await.unwrap_or_default();
        let capture = self.capture_json().await;
        let runtime = self
            .runtime
            .lock()
            .await
            .as_ref()
            .map(|handle| handle.stats());
        let mut status = json!({
            "browser": { "connected": connected },
            "capture": capture,
            "rules": {
                "active": rules.iter().filter(|rule| rule.enabled && rule.state == "active").count(),
                "total": rules.len(),
            },
        });
        if let Some(runtime) = runtime {
            status["runtime"] = json!(runtime);
        }
        status
    }

    async fn capture_json(&self) -> Value {
        let connected = (self.session)().await.is_some();
        let state = self.state.lock().await;
        let lifecycle = if state.running {
            if connected { "running" } else { "paused" }
        } else {
            "stopped"
        };
        json!({
            "running": state.running,
            "state": lifecycle,
            "startedAt": state.started_at_ms,
            "records": state.records.len(),
            "gapEvents": state.gap_events,
            "bodyBytes": state.body_bytes,
            "tabs": state.tabs,
        })
    }

    // ---- runtime -------------------------------------------------------

    fn spawn_supervisor(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut epoch: Option<u64> = None;
            let mut ever_connected = false;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                if service.cancel.is_cancelled() {
                    return;
                }
                let Some(session) = (service.session)().await else {
                    if let Some(handle) = service.runtime.lock().await.take() {
                        handle.shutdown();
                    }
                    epoch = None;
                    continue;
                };
                let current = session.connection().connection_epoch();
                if epoch == Some(current) {
                    continue;
                }
                if ever_connected {
                    let running = service.state.lock().await.running;
                    if running {
                        service.state.lock().await.gap_events += 1;
                        service.broadcast("gap", json!({ "reset": true }));
                    }
                }
                if let Some(handle) = service.runtime.lock().await.take() {
                    handle.shutdown();
                }
                let (tx, rx) = mpsc::unbounded_channel();
                let handle = NetworkRuntime::spawn(session, tx);
                handle.set_capture(service.state.lock().await.running);
                handle.set_rules(service.runtime_rules.read().await.clone());
                epoch = Some(current);
                ever_connected = true;
                *service.runtime.lock().await = Some(handle);
                let consumer = service.clone();
                tokio::spawn(async move {
                    consumer.consume(rx).await;
                });
            }
        });
    }

    fn spawn_maintenance(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                if service.cancel.is_cancelled() {
                    return;
                }
                let expired = {
                    let mut state = service.state.lock().await;
                    let now = now_ms();
                    let tokens: Vec<String> = state
                        .deleted
                        .iter()
                        .filter(|(_, entry)| entry.expires_at_ms < now)
                        .map(|(token, _)| token.clone())
                        .collect();
                    let mut ids = Vec::new();
                    for token in tokens {
                        if let Some(entry) = state.deleted.remove(&token) {
                            ids.push(entry.record.record_id);
                        }
                    }
                    ids
                };
                for record_id in expired {
                    {
                        let mut state = service.state.lock().await;
                        if let Some(size) = state.body_sizes.remove(&record_id) {
                            state.body_bytes = state.body_bytes.saturating_sub(size);
                        }
                    }
                    if let Err(error) = service.bodies.remove_record(&record_id).await {
                        warn!(record_id, error = %error, "failed to release deleted record body");
                    }
                }
                let before = now_ms() - UNDO_WINDOW.as_millis() as i64;
                match service.repo.purge_deleted_rules(before).await {
                    Ok(ids) => {
                        for id in ids {
                            if let Err(error) = service.bodies.remove_rule(&id).await {
                                warn!(rule_id = id, error = %error, "failed to release rule body");
                            }
                        }
                    }
                    Err(error) => warn!(error = %error, "failed to purge deleted rules"),
                }
            }
        });
    }

    async fn consume(self: Arc<Self>, mut receiver: mpsc::UnboundedReceiver<RuntimeEvent>) {
        while let Some(event) = receiver.recv().await {
            match event {
                RuntimeEvent::Record(record) => self.on_record(*record).await,
                RuntimeEvent::Body {
                    record_id,
                    bytes,
                    reason,
                } => self.on_body(record_id, bytes, reason).await,
                RuntimeEvent::Hit(hit) => self.on_hit(hit).await,
                RuntimeEvent::Tabs(tabs) => {
                    let previous = {
                        let mut state = self.state.lock().await;
                        let previous = state.tabs.clone();
                        state.tabs = tabs;
                        previous
                    };
                    let current = self.state.lock().await.tabs.clone();
                    let removed: Vec<i64> = previous
                        .iter()
                        .filter(|tab| !current.iter().any(|next| next.tab_id == tab.tab_id))
                        .map(|tab| tab.tab_id)
                        .collect();
                    if !removed.is_empty() {
                        self.unbind_closed_tabs(&removed).await;
                    }
                    self.broadcast("capture", self.capture_json().await);
                }
                RuntimeEvent::Gap { lost } => {
                    let mut state = self.state.lock().await;
                    state.gap_events += lost;
                    drop(state);
                    self.broadcast("gap", json!({ "lost": lost }));
                }
            }
        }
    }

    async fn on_record(&self, record: NetworkRecord) {
        let mut state = self.state.lock().await;
        if !state.running && !state.index.contains_key(&record.record_id) {
            return;
        }
        let seq = record.seq;
        if let Some(existing) = state.records.get(&seq)
            && existing.record_id == record.record_id
            && existing.body_state != record.body_state
            && record.body_state == browseros_core::network::BodyState::Pending
        {
            return;
        }
        state.index.insert(record.record_id.clone(), seq);
        state.records.insert(seq, record.clone());
        let evicted = enforce_buffer_limits(&mut state);
        drop(state);
        self.broadcast(
            "record",
            json!({
                "record": record,
            }),
        );
        for record in evicted {
            if let Err(error) = self.bodies.remove_record(&record.record_id).await {
                warn!(record_id = %record.record_id, error = %error, "failed to release evicted body");
            }
            self.broadcast(
                "evicted",
                json!({ "recordId": record.record_id, "reason": "buffer_limit" }),
            );
        }
    }

    async fn on_body(&self, record_id: String, bytes: Option<Vec<u8>>, reason: Option<String>) {
        // The buffer may have been cleared or the record evicted while the body
        // worker was in flight; writing then would leak a tmp file nobody owns.
        if !self.state.lock().await.index.contains_key(&record_id) {
            return;
        }
        let (body_state, body_reason) = match &bytes {
            Some(bytes) => {
                if let Err(error) = self.bodies.write_record(&record_id, bytes).await {
                    warn!(record_id = %record_id, error = %error, "failed to persist captured body");
                    (
                        browseros_core::network::BodyState::Unavailable,
                        Some("body_write_failed".to_string()),
                    )
                } else {
                    (browseros_core::network::BodyState::Available, None)
                }
            }
            None => (
                if reason.as_deref() == Some("too_large") {
                    browseros_core::network::BodyState::TooLarge
                } else {
                    browseros_core::network::BodyState::Unavailable
                },
                reason,
            ),
        };
        let mut state = self.state.lock().await;
        let Some(seq) = state.index.get(&record_id).copied() else {
            return;
        };
        let Some(record) = state.records.get_mut(&seq) else {
            return;
        };
        record.body_state = body_state;
        record.body_reason = body_reason.clone();
        let size = bytes.as_ref().map(|bytes| bytes.len() as u64).unwrap_or(0);
        if let Some(previous) = state.body_sizes.insert(record_id.clone(), size) {
            state.body_bytes = state.body_bytes.saturating_sub(previous);
        }
        state.body_bytes += size;
        let evicted = enforce_buffer_limits(&mut state);
        drop(state);
        for record in evicted {
            if let Err(error) = self.bodies.remove_record(&record.record_id).await {
                warn!(record_id = %record.record_id, error = %error, "failed to release evicted body");
            }
            self.broadcast(
                "evicted",
                json!({ "recordId": record.record_id, "reason": "buffer_limit" }),
            );
        }
        self.broadcast(
            "record_done",
            json!({
                "recordId": record_id,
                "bodyState": body_state,
                "bodyReason": body_reason,
                "bodyBytes": size,
            }),
        );
    }

    async fn unbind_closed_tabs(&self, removed: &[i64]) {
        let mut changed = Vec::new();
        let rules = match self.repo.list_rules(false).await {
            Ok(rules) => rules,
            Err(error) => {
                warn!(error = %error, "failed to list rules for tab unbind");
                return;
            }
        };
        for rule in rules {
            if rule.scope_kind == ScopeKind::Tab
                && rule.enabled
                && rule.scope_tab.is_some_and(|tab| removed.contains(&tab))
            {
                if let Err(error) = self
                    .repo
                    .update_rule_state(&rule.id, false, "disabled", Some("unbound"), now_ms())
                    .await
                {
                    warn!(rule = %rule.id, error = %error, "failed to unbind tab rule");
                } else {
                    changed.push(rule.id);
                }
            }
        }
        if !changed.is_empty() {
            let _ = self.reload_runtime_rules().await;
            for rule_id in changed {
                self.broadcast_rule(&rule_id).await.ok();
            }
        }
    }

    async fn on_hit(&self, hit: browseros_core::network::HitRecord) {
        let row = HitRow {
            id: format!("nethit_{}", uuid::Uuid::new_v4().simple()),
            rule_id: hit.rule_id.clone(),
            revision: hit.revision,
            at_ms: hit.at_ms,
            tab_title: hit.tab_title.clone(),
            url: hit.url.clone(),
            method: hit.method.clone(),
            outcome: hit.outcome.clone(),
            reason: hit.reason.clone(),
            network_id: hit.network_id.clone(),
        };
        if let Err(error) = self.repo.insert_hit(&row).await {
            warn!(error = %error, "failed to record network rule hit");
        }
        if let Some(network_id) = &hit.network_id {
            let mut state = self.state.lock().await;
            let target = state
                .records
                .values_mut()
                .rev()
                .find(|record| &record.request_id == network_id);
            if let Some(record) = target {
                record.mock = Some(browseros_core::network::MockMark {
                    rule_id: hit.rule_id.clone(),
                    revision: hit.revision,
                    outcome: hit.outcome.clone(),
                    reason: hit.reason.clone(),
                });
                let record = record.clone();
                drop(state);
                self.broadcast("record", json!({ "record": record }));
                self.broadcast("hit", hit_json(&row));
                return;
            }
        }
        self.broadcast("hit", hit_json(&row));
    }

    async fn reload_runtime_rules(&self) -> AppResult<ApplyReport> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let rules = self.build_runtime_rules(generation).await?;
        *self.runtime_rules.write().await = rules.clone();
        let applied = match self.runtime.lock().await.as_ref() {
            Some(runtime) => runtime.apply_rules(rules).await,
            None => ApplyReport::default(),
        };
        Ok(applied)
    }

    async fn build_runtime_rules(&self, generation: u64) -> AppResult<Arc<RuntimeRuleSet>> {
        let rows = self.repo.list_rules(false).await?;
        let mut rules = Vec::new();
        for row in rows {
            if !row.enabled || row.state == "failed" {
                continue;
            }
            let Some(revision) = self
                .repo
                .get_revision(&row.id, row.current_revision)
                .await?
            else {
                continue;
            };
            let body = match &revision.body_id {
                Some(body_id) => self.bodies.read_rule(body_id).await?.unwrap_or_default(),
                None => Vec::new(),
            };
            if body.len() as u64 > RULE_BODY_LIMIT {
                continue;
            }
            rules.push(RuntimeRule {
                id: row.id.clone(),
                revision: row.current_revision,
                scope_tab: if row.scope_kind == ScopeKind::Tab {
                    row.scope_tab
                } else {
                    None
                },
                matcher: revision.matcher,
                response: revision.response,
                body,
                sample_headers: revision.sample_headers,
                updated_at: row.updated_at,
            });
        }
        Ok(Arc::new(RuntimeRuleSet { generation, rules }))
    }

    async fn broadcast_rule(&self, id: &str) -> AppResult<()> {
        if let Some(rule) = self.repo.get_rule(id).await? {
            let revision = self.repo.get_revision(id, rule.current_revision).await?;
            let hits = self.repo.list_hits(Some(id), 1).await?;
            self.broadcast("rule", rule_json(&rule, revision.as_ref(), hits.first()));
        }
        Ok(())
    }

    async fn record_and_body(&self, record_id: &str) -> Option<(NetworkRecord, u64)> {
        let state = self.state.lock().await;
        let seq = *state.index.get(record_id)?;
        let record = state.records.get(&seq)?.clone();
        Some((record, seq))
    }
}

fn text_or_base64(bytes: &[u8]) -> (&'static str, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => ("utf8", text.to_string()),
        Err(_) => ("base64", browseros_core::network::base64_encode(bytes)),
    }
}

fn rule_json(
    rule: &RuleRow,
    revision: Option<&RuleRevisionRow>,
    last_hit: Option<&HitRow>,
) -> Value {
    json!({
        "id": rule.id,
        "scope": if rule.scope_kind == ScopeKind::AllTabs { "all_tabs" } else { "source_tab" },
        "scopeTab": rule.scope_tab,
        "enabled": rule.enabled,
        "state": rule.state,
        "stateReason": rule.state_reason,
        "authorClient": rule.author_client,
        "currentRevision": rule.current_revision,
        "createdAt": rule.created_at,
        "updatedAt": rule.updated_at,
        "method": revision.map(|revision| revision.matcher.method.clone()),
        "pathname": revision.map(|revision| revision.matcher.pathname.clone()),
        "origin": revision.map(|revision| revision.matcher.origin.clone()),
        "ignoreKeys": revision.map(|revision| revision.matcher.query_policy.ignore_keys.clone()),
        "strictQuery": revision.map(|revision| revision.matcher.query_policy.mode == browseros_core::network::QueryMode::Strict),
        "responseStatus": revision.map(|revision| revision.response.status),
        "lastHit": last_hit.map(hit_json),
    })
}

fn hit_json(hit: &HitRow) -> Value {
    json!({
        "id": hit.id,
        "ruleId": hit.rule_id,
        "revision": hit.revision,
        "at": hit.at_ms,
        "tabTitle": hit.tab_title,
        "url": hit.url,
        "method": hit.method,
        "outcome": hit.outcome,
        "reason": hit.reason,
        "networkId": hit.network_id,
    })
}

fn handoff_summary(handoff: &HandoffRow) -> Value {
    json!({
        "id": handoff.id,
        "createdAt": handoff.created_at,
        "expiresAt": handoff.expires_at,
        "state": handoff.state,
        "ruleId": handoff.rule_id,
        "source": handoff.source_meta,
        "requestHeaders": header_lines(&handoff.request_headers),
        "bodyState": handoff.body_state,
    })
}

/// Best-effort mapping of a stored mock body into the shape an AI reads back.
#[must_use]
pub fn response_for_display(response: &MockResponse) -> Value {
    json!({
        "status": response.status,
        "headers": response.headers,
        "delayMs": response.delay_ms,
        "bodyKind": match response.body {
            MockBody::Literal { .. } => "literal",
            MockBody::PatchSample { .. } => "patch_sample",
        },
    })
}

fn group_query(pairs: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::new();
    for (key, value) in pairs {
        grouped
            .entry(key.clone())
            .or_insert_with(Vec::new)
            .push(value.clone());
    }
    grouped
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod buffer_tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    fn record(seq: u64, tab: i64) -> NetworkRecord {
        let mut record = NetworkRecord::default();
        record.record_id = format!("netrec_{seq}");
        record.seq = seq;
        record.tab_id = Some(tab);
        record
    }

    #[test]
    fn per_tab_limit_evicts_oldest_unpinned_records() {
        let mut state = CaptureState::default();
        for seq in 0..=(PER_TAB_RECORD_LIMIT as u64 + 1) {
            let record = record(seq, 1);
            state.index.insert(record.record_id.clone(), seq);
            state.records.insert(seq, record);
        }
        state.pinned.insert("netrec_0".to_string());
        let removed = enforce_buffer_limits(&mut state);
        assert_eq!(removed.len(), 1);
        assert!(!state.records.contains_key(&1));
        assert!(state.records.contains_key(&0), "pinned records survive");
    }

    #[test]
    fn pins_survive_even_when_the_tab_is_over_budget() {
        let mut state = CaptureState::default();
        for seq in 0..=(PER_TAB_RECORD_LIMIT as u64) {
            state.pinned.insert(format!("netrec_{seq}"));
            let record = record(seq, 1);
            state.index.insert(record.record_id.clone(), seq);
            state.records.insert(seq, record);
        }
        let removed = enforce_buffer_limits(&mut state);
        assert!(removed.is_empty());
    }
}
