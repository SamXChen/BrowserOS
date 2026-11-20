//! Session-scoped capture buffer, handoff snapshots and the rule surface.
//!
//! Rule changes all live in [`RuleEngine`]; this module owns capture, records,
//! handoffs and the SSE fan-out, and delegates every rule write.

use super::{
    body_store::BodyStore,
    dto::{
        NetworkBodyPageView, NetworkBrowserView, NetworkDeleted, NetworkHandoffView,
        NetworkHitView, NetworkHitsDeleted, NetworkNearMissView, NetworkRecordDeleted,
        NetworkRecordsCleared, NetworkRestoreResult, NetworkRevisionListView, NetworkRevisionView,
        NetworkRuleDetailView, NetworkRuleListView, NetworkRuleTestResult,
        NetworkRuleTestSampleView, NetworkRuleView, NetworkRulesStateView, NetworkRuntimeView,
        NetworkSampleView, NetworkStatusView,
    },
    handoff::build_handoff,
    processor::ScriptProcessor,
    rules::{ApplyOutcome, ApplyRequest, Rebuild, RecordReader, RuleEngine},
    runtime_port::{RuntimePort, RuntimeSlot},
};
use crate::db::DbConnection;
use crate::{
    db::{
        Database,
        network::{
            HandoffRow, HitRow, NetworkRepository, RuleRevisionRow, RuleRow, RuleState, ScopeKind,
        },
    },
    error::{AppError, AppResult},
};
use browseros_core::{
    BrowserSession,
    network::{
        HitRecord, MockMark, NetworkRecord, NetworkRuntime, ResponseProcessor, RuntimeEvent,
        TabSummary, matches,
    },
};
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub use super::rules::UNDO_WINDOW_MS;

const GLOBAL_RECORD_LIMIT: usize = 20_000;
const PER_TAB_RECORD_LIMIT: usize = 2_000;
const BODY_TOTAL_LIMIT: u64 = 256 * 1024 * 1024;
const SSE_CAPACITY: usize = 512;
const HANDOFF_PIN_LIMIT: usize = 20;
/// Bound on hits that are still waiting for their record.
const PENDING_MARKS_LIMIT: usize = 512;
const HANDOFF_BODY_LIMIT: u64 = 32 * 1024 * 1024;
const MAINTENANCE_EVERY: u64 = 60;

/// Resolves the live browser session without making this service depend on
/// another service module. `app.rs` composes the two.
pub type SessionProvider =
    Arc<dyn Fn() -> BoxFuture<'static, Option<Arc<BrowserSession>>> + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    pub event: &'static str,
    pub data: Value,
}

/// Counts how many of the three PATCH groups a body touches: 1 is valid,
/// anything else is rejected by [`NetworkService::patch_rule`].
fn patch_groups(patch: &Value) -> usize {
    usize::from(patch.get("enabled").is_some())
        + usize::from(patch.get("scope").is_some() || patch.get("tabId").is_some())
        + usize::from(patch.get("name").is_some())
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
    /// Latest seq per `request_id`, so a hit resolves without scanning.
    by_request: HashMap<String, u64>,
    /// Hits that arrived before their record, keyed by `request_id`.
    pending_marks: HashMap<String, MockMark>,
    body_bytes: u64,
    body_sizes: HashMap<String, u64>,
    pinned: HashSet<String>,
    gap_events: u64,
    tabs: Vec<TabSummary>,
    deleted: HashMap<String, UndoEntry>,
}

impl CaptureState {
    fn forget(&mut self, record: &NetworkRecord) {
        self.index.remove(&record.record_id);
        self.by_request.remove(&record.request_id);
        self.pending_marks.remove(&record.request_id);
        if let Some(size) = self.body_sizes.remove(&record.record_id) {
            self.body_bytes = self.body_bytes.saturating_sub(size);
        }
    }
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
            state.forget(&record);
            removed.push(record);
        }
    }
    removed
}

pub struct NetworkService {
    repo: NetworkRepository,
    pub(crate) bodies: Arc<BodyStore>,
    session: SessionProvider,
    state: Mutex<CaptureState>,
    sse: broadcast::Sender<SseEvent>,
    runtime: Arc<RuntimeSlot>,
    pub(crate) rules: Arc<RuleEngine>,
    processor: Arc<dyn ResponseProcessor>,
    cancel: CancellationToken,
}

impl NetworkService {
    pub async fn new(
        session: SessionProvider,
        database: &Database,
        browserclaw_dir: &std::path::Path,
        runtime_port: Option<Arc<dyn RuntimePort>>,
    ) -> AppResult<Arc<Self>> {
        let repo = NetworkRepository::new(database.clone());
        let bodies = Arc::new(BodyStore::new(browserclaw_dir.join("network")).await?);
        // D3: the capture buffer is session-scoped. Metadata already lived in
        // memory, but body files from a previous process must not linger.
        bodies.clear_tmp().await?;
        let (sse, _) = broadcast::channel(SSE_CAPACITY);
        let processor: Arc<dyn ResponseProcessor> = Arc::new(ScriptProcessor::new());
        let runtime = Arc::new(RuntimeSlot::empty());
        if let Some(port) = runtime_port {
            runtime.replace(Some(port));
        }
        let rules = Arc::new(RuleEngine::new(
            repo.clone(),
            Arc::clone(&bodies),
            Arc::clone(&runtime),
            sse.clone(),
            Arc::clone(&processor),
        ));
        let service = Arc::new(Self {
            repo,
            bodies,
            session,
            state: Mutex::new(CaptureState::default()),
            sse,
            runtime,
            rules,
            processor,
            cancel: CancellationToken::new(),
        });
        let reader: RecordReader = {
            let weak = Arc::downgrade(&service);
            Arc::new(move || {
                let weak = weak.clone();
                Box::pin(async move {
                    match weak.upgrade() {
                        Some(service) => service.list_records(None, GLOBAL_RECORD_LIMIT).await,
                        None => Vec::new(),
                    }
                })
            })
        };
        service.rules.set_record_reader(reader).await;
        service.rules.rebuild_and_apply().await?;
        service.spawn_supervisor();
        service.spawn_maintenance();
        Ok(service)
    }

    /// Inserts a captured record straight into the buffer, so tests can seed a
    /// handoff without running capture.
    #[cfg(test)]
    pub async fn insert_record_for_test(
        &self,
        record_id: &str,
        body: &[u8],
        tab_id: i64,
        method: &str,
        url: &str,
        post_data: Option<&str>,
    ) -> AppResult<()> {
        use browseros_core::network::RecordState;
        let seq = {
            let mut state = self.state.lock().await;
            let seq = state.records.keys().next_back().map_or(1, |last| last + 1);
            let record = NetworkRecord {
                seq,
                record_id: record_id.to_string(),
                request_id: format!("request-{seq}"),
                tab_id: Some(tab_id),
                method: method.to_string(),
                url: url.to_string(),
                post_data: post_data.map(ToString::to_string),
                state: RecordState::Done,
                status: Some(200),
                body_state: browseros_core::network::BodyState::Available,
                ..NetworkRecord::default()
            };
            state.index.insert(record_id.to_string(), seq);
            state.by_request.insert(record.request_id.clone(), seq);
            state.records.insert(seq, record);
            seq
        };
        self.bodies.write_record(record_id, body).await?;
        let _ = seq;
        Ok(())
    }

    /// Test-only constructor: no supervisor, no maintenance loop, and an
    /// injected runtime port, so rule lifecycles can be asserted without a
    /// browser.
    #[cfg(test)]
    pub async fn for_test(
        database: &Database,
        browserclaw_dir: &std::path::Path,
        runtime_port: Option<Arc<dyn RuntimePort>>,
    ) -> AppResult<Arc<Self>> {
        let repo = NetworkRepository::new(database.clone());
        let bodies = Arc::new(BodyStore::new(browserclaw_dir.join("network")).await?);
        let (sse, _) = broadcast::channel(SSE_CAPACITY);
        let runtime = Arc::new(RuntimeSlot::empty());
        if let Some(port) = runtime_port {
            runtime.replace(Some(port));
        }
        let rules = Arc::new(RuleEngine::new(
            repo.clone(),
            Arc::clone(&bodies),
            Arc::clone(&runtime),
            sse.clone(),
            Arc::new(ScriptProcessor::new()),
        ));
        let service = Arc::new(Self {
            repo,
            bodies,
            session: Arc::new(|| Box::pin(async { None })),
            state: Mutex::new(CaptureState::default()),
            sse,
            runtime,
            rules,
            processor: Arc::new(ScriptProcessor::new()),
            cancel: CancellationToken::new(),
        });
        let reader: RecordReader = Arc::new(|| Box::pin(async { Vec::new() }));
        service.rules.set_record_reader(reader).await;
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
        if let Some(handle) = self.runtime.current() {
            handle.set_capture(true);
        }
        let snapshot = self.capture_json().await;
        self.broadcast("capture", snapshot.clone());
        Ok(snapshot)
    }

    /// Stops capture. The buffer is kept so the user can still inspect what was
    /// collected; only "clear" or a server restart discards it.
    pub async fn stop_capture(&self) -> AppResult<Value> {
        {
            let mut state = self.state.lock().await;
            state.running = false;
            state.started_at_ms = None;
        }
        if let Some(handle) = self.runtime.current() {
            handle.set_capture(false);
        }
        let snapshot = self.capture_json().await;
        self.broadcast("capture", snapshot.clone());
        Ok(snapshot)
    }

    async fn clear_records(&self) -> AppResult<u64> {
        let mut state = self.state.lock().await;
        let removed = state.records.len() as u64;
        state.records.clear();
        state.index.clear();
        state.by_request.clear();
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
    ) -> AppResult<Option<NetworkBodyPageView>> {
        let Some((record, _)) = self.record_and_body(record_id).await else {
            return Ok(None);
        };
        let Some((bytes, total)) = self
            .bodies
            .read_record_range(record_id, offset, limit)
            .await?
        else {
            return Ok(Some(
                NetworkBodyPageView::new("utf8", offset, 0, false, String::new()).with_record(
                    body_state_str(&record.body_state),
                    record.mime_type.as_deref(),
                ),
            ));
        };
        let (encoding, data) = text_or_base64(&bytes);
        let truncated = (offset + bytes.len() as u64) < total;
        Ok(Some(
            NetworkBodyPageView::new(encoding, offset, total, truncated, data).with_record(
                body_state_str(&record.body_state),
                record.mime_type.as_deref(),
            ),
        ))
    }

    pub async fn request_body_page(
        &self,
        record_id: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<NetworkBodyPageView>> {
        let Some((record, _)) = self.record_and_body(record_id).await else {
            return Ok(None);
        };
        let body = record.post_data.unwrap_or_default();
        let bytes = body.as_bytes();
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(limit as usize).min(bytes.len());
        Ok(Some(NetworkBodyPageView::new(
            "utf8",
            start as u64,
            bytes.len() as u64,
            end < bytes.len(),
            String::from_utf8_lossy(&bytes[start..end]).into_owned(),
        )))
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
        state.by_request.remove(&record.request_id);
        let token = format!("undo_{}", uuid::Uuid::new_v4().simple());
        let until = now_ms() + UNDO_WINDOW_MS;
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
        Ok(json!(NetworkRecordDeleted {
            deleted: true,
            id: Some(record_id.to_string()),
            undo_token: Some(token),
            undo_until: Some(until),
        }))
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
        state
            .by_request
            .insert(entry.record.request_id.clone(), seq);
        state.records.insert(seq, entry.record);
        state.index.insert(record_id.clone(), seq);
        drop(state);
        self.broadcast(
            "record",
            json!({ "record": self.get_record(&record_id).await }),
        );
        Ok(json!(NetworkRestoreResult {
            restored: true,
            record_id: Some(record_id.to_string()),
        }))
    }

    pub async fn delete_all_records(&self) -> AppResult<Value> {
        let removed = self.clear_records().await?;
        Ok(json!(NetworkRecordsCleared {
            cleared: removed > 0
        }))
    }

    // ---- SSE -----------------------------------------------------------

    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.sse.subscribe()
    }

    fn broadcast(&self, event: &'static str, data: Value) {
        let _ = self.sse.send(SseEvent { event, data });
    }

    // ---- handoffs ------------------------------------------------------

    /// Copying a reference twice returns the same one; the quotas recycle the
    /// oldest instead of failing.
    pub async fn create_handoff(&self, record_id: &str) -> AppResult<Value> {
        let Some(record) = self.get_record(record_id).await else {
            return Err(AppError::not_found("record_not_found"));
        };
        let now = now_ms();
        if let Some(existing) = self.repo.find_handoff_by_record(record_id, now).await?
            && existing.body_state == body_state_str(&record.body_state)
        {
            return self.handoff_payload(&existing, true).await;
        }

        let related = self.related_records(&record).await;
        let mut body = self.get_record_body(record_id).await;
        if let Some(bytes) = &body
            && bytes.len() as u64 > HANDOFF_BODY_LIMIT
        {
            body = None;
        }
        let id = format!("netref_{}", uuid::Uuid::new_v4().simple());
        let draft = build_handoff(id, &record, body.clone(), &related, now);
        let mut row = draft.handoff;
        if body.is_none() && row.body_state == "available" {
            row.body_state = "too_large".to_string();
            row.body_reason = Some("handoff_body_quota".to_string());
        }

        // Recycle: never fail because the user copied a lot of references.
        let repo = self.repo.clone();
        let live = self
            .repo
            .stmt(|connection| {
                let repo = repo.clone();
                Box::pin(async move { repo.list_handoffs_by_age(connection, now).await })
            })
            .await?;
        let mut recycle: Vec<HandoffRow> = Vec::new();
        let mut count = live.len();
        let mut total = self.repo.total_handoff_body_bytes().await?;
        let incoming = row.body_bytes;
        for candidate in live {
            if count < HANDOFF_PIN_LIMIT && total + incoming <= HANDOFF_BODY_LIMIT {
                break;
            }
            count = count.saturating_sub(1);
            total = total.saturating_sub(candidate.body_bytes);
            recycle.push(candidate);
        }

        let transaction = self.repo.begin().await?;
        self.repo.insert_handoff(&transaction, &row).await?;
        for victim in &recycle {
            self.repo.delete_handoff(&transaction, &victim.id).await?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Db(Box::new(error)))?;

        if let Some(bytes) = &body {
            self.bodies.write_handoff(&row.id, bytes).await?;
        }
        for victim in &recycle {
            self.bodies.remove_handoff(&victim.id).await?;
            self.broadcast(
                "deleted",
                json!({ "kind": "handoff", "id": victim.id, "reason": "recycled" }),
            );
        }
        self.handoff_payload(&row, false).await
    }

    async fn handoff_payload(&self, handoff: &HandoffRow, reused: bool) -> AppResult<Value> {
        Ok(json!({
            "handoffId": handoff.id,
            "clipboardText": handoff_clipboard(handoff),
            "expiresAt": handoff.expires_at,
            "reused": reused,
        }))
    }

    async fn get_record_body(&self, record_id: &str) -> Option<Vec<u8>> {
        self.bodies.read_record(record_id).await.unwrap_or(None)
    }

    async fn related_records(&self, record: &NetworkRecord) -> Vec<NetworkRecord> {
        let records = self.list_records(None, GLOBAL_RECORD_LIMIT).await;
        records
            .into_iter()
            .filter(|candidate| {
                candidate.record_id != record.record_id
                    && candidate.url == record.url
                    && candidate.method == record.method
            })
            .take(5)
            .collect()
    }

    pub async fn list_handoffs(&self) -> AppResult<Vec<Value>> {
        let handoffs = self.repo.list_handoffs().await?;
        Ok(handoffs.iter().map(handoff_summary).collect())
    }

    pub async fn get_handoff(&self, id: &str) -> AppResult<HandoffRow> {
        self.repo
            .get_handoff(id)
            .await?
            .ok_or_else(|| AppError::not_found("handoff_not_found"))
    }

    pub async fn delete_handoff(&self, id: &str) -> AppResult<Value> {
        let repo = self.repo.clone();
        let handoff_id = id.to_string();
        let removed = self
            .stmt(|connection| {
                let repo = repo.clone();
                let handoff_id = handoff_id.clone();
                Box::pin(async move { repo.delete_handoff(connection, &handoff_id).await })
            })
            .await?;
        if removed == 0 {
            return Err(AppError::not_found("handoff_not_found"));
        }
        self.bodies.remove_handoff(id).await?;
        self.broadcast("deleted", json!({ "kind": "handoff", "id": id }));
        Ok(json!(NetworkDeleted {
            deleted: true,
            all: false,
            id: Some(id.to_string()),
            kind: None,
            undo_token: None,
            undo_until: None,
        }))
    }

    pub async fn rule_by_handoff(&self, handoff_id: &str) -> AppResult<Option<RuleRow>> {
        let Some(handoff) = self.repo.get_handoff(handoff_id).await? else {
            return Ok(None);
        };
        match handoff.rule_id {
            Some(rule_id) => self.repo.get_rule(&rule_id).await,
            None => Ok(None),
        }
    }

    /// A window of a static rule's stored body, for MCP `rule_get`.
    pub async fn rule_body_window(
        &self,
        reference: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<NetworkBodyPageView>> {
        let rule = self.rules.resolve_reference(reference).await?;
        let revision = self
            .repo
            .get_revision(&rule.id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::Internal("current revision missing".to_string()))?;
        let Some(body_id) = revision.body_id else {
            return Ok(None);
        };
        let Some((bytes, total)) = self.bodies.read_rule_range(&body_id, offset, limit).await?
        else {
            return Ok(None);
        };
        let end = utf8_boundary(&bytes);
        let (encoding, data) = text_or_base64(&bytes[..end]);
        Ok(Some(
            NetworkBodyPageView::new(encoding, offset, total, (offset + end as u64) < total, data)
                .with_length(end as u64),
        ))
    }

    /// A window of the handoff body, for MCP `handoff_get`. Always honours
    /// `offset`/`limit`, even past 512KiB.
    pub async fn handoff_body_window(
        &self,
        id: &str,
        offset: u64,
        limit: u64,
    ) -> AppResult<Option<NetworkBodyPageView>> {
        let Some((bytes, total)) = self.bodies.read_handoff_range(id, offset, limit).await? else {
            return Ok(None);
        };
        // Never cut a multi-byte character in half.
        let end = utf8_boundary(&bytes);
        let (encoding, data) = text_or_base64(&bytes[..end]);
        Ok(Some(
            NetworkBodyPageView::new(encoding, offset, total, (offset + end as u64) < total, data)
                .with_length(end as u64),
        ))
    }

    // ---- rules ---------------------------------------------------------

    pub async fn apply_rule(&self, request: ApplyRequest) -> AppResult<ApplyOutcome> {
        self.rules.apply(request).await
    }

    /// Typed rule views; MCP filters on these instead of on raw JSON.
    pub async fn rule_views(&self) -> AppResult<Vec<NetworkRuleView>> {
        let rules = self.repo.list_rules(false).await?;
        let revisions = self
            .repo
            .list_current_revisions(&rules.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>())
            .await?;
        let repo = self.repo.clone();
        let hits = self
            .stmt(|connection| {
                let repo = repo.clone();
                Box::pin(async move { repo.list_hits(connection, None, 500).await })
            })
            .await?;
        Ok(rules
            .iter()
            .map(|rule| {
                let revision = revisions.iter().find(|revision| {
                    revision.rule_id == rule.id && revision.revision == rule.current_revision
                });
                NetworkRuleView::summary(rule, revision, &hits)
            })
            .collect())
    }

    pub async fn list_rules(&self) -> AppResult<Value> {
        Ok(json!(NetworkRuleListView {
            rules: self.rule_views().await?
        }))
    }

    /// Typed detail, so MCP `rule_get` reads fields instead of parsing JSON.
    pub async fn rule_detail_view(&self, id: &str) -> AppResult<NetworkRuleDetailView> {
        let rule = self.rules.resolve_reference(id).await?;
        let revisions = self.repo.list_revisions(&rule.id).await?;
        let current = revisions
            .iter()
            .find(|revision| revision.revision == rule.current_revision)
            .cloned()
            .ok_or_else(|| AppError::Internal("current revision missing".to_string()))?;
        let repo = self.repo.clone();
        let rule_id = rule.id.clone();
        let hits = self
            .stmt(|connection| {
                let repo = repo.clone();
                let rule_id = rule_id.clone();
                Box::pin(async move { repo.list_hits(connection, Some(&rule_id), 10).await })
            })
            .await?;
        let sample_body = self.rules.sample_bytes_for(&current).await?;
        Ok(NetworkRuleDetailView {
            rule: NetworkRuleView::detail(&rule, &current, &hits),
            revision: NetworkRevisionView::from_row(&current),
            hits: hits.iter().map(NetworkHitView::from_row).collect(),
            sample: NetworkSampleView {
                available: current.sample_available,
                status: current.sample_status,
                bytes: sample_body.map(|bytes| bytes.len()),
            },
        })
    }

    pub async fn get_rule_detail(&self, id: &str) -> AppResult<Value> {
        serde_json::to_value(self.rule_detail_view(id).await?)
            .map_err(|error| AppError::Internal(format!("serialize rule detail: {error}")))
    }

    /// Samples are counted inside the rule's scope; the old version ignored it.
    pub async fn test_rule(&self, id: &str) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        let current = self
            .repo
            .get_revision(&rule.id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::Internal("current revision missing".to_string()))?;
        let records = self.list_records(None, GLOBAL_RECORD_LIMIT).await;
        let mut matched = Vec::new();
        for record in records {
            if let ScopeKind::Tab = rule.scope_kind
                && (rule.scope_tab.is_none() || record.tab_id != rule.scope_tab)
            {
                continue;
            }
            let outcome = matches(
                &current.matcher,
                &record.method,
                &record.url,
                record.post_data.as_deref(),
                record.resource_type.as_deref(),
            );
            if outcome.is_match() {
                matched.push(NetworkRuleTestSampleView {
                    record_id: record.record_id.clone(),
                    method: record.method.clone(),
                    url: record.url.clone(),
                    status: record.status,
                    tab_title: record.tab_title.clone(),
                    at: Some(record.started_at_ms),
                });
            }
        }
        Ok(json!(NetworkRuleTestResult {
            matched: matched.len(),
            considered: match rule.scope_kind {
                ScopeKind::AllTabs => self.list_records(None, GLOBAL_RECORD_LIMIT).await.len(),
                ScopeKind::Tab => self
                    .list_records(None, GLOBAL_RECORD_LIMIT)
                    .await
                    .into_iter()
                    .filter(|record| record.tab_id == rule.scope_tab)
                    .count(),
            },
            samples: matched,
        }))
    }

    pub async fn list_revisions(&self, id: &str) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        let revisions = self.repo.list_revisions(&rule.id).await?;
        Ok(json!(NetworkRevisionListView {
            revisions: revisions
                .iter()
                .map(NetworkRevisionView::from_row)
                .collect(),
        }))
    }

    /// Explains why recent requests missed this rule, for AI diagnosis only;
    /// reached through MCP `rule_status`.
    pub async fn near_misses(&self, rule_id: &str) -> AppResult<Vec<NetworkNearMissView>> {
        let rule = self.rules.resolve_reference(rule_id).await?;
        let current = self
            .repo
            .get_revision(&rule.id, rule.current_revision)
            .await?
            .ok_or_else(|| AppError::Internal("current revision missing".to_string()))?;
        let records = self.list_records(None, GLOBAL_RECORD_LIMIT).await;
        let mut misses = Vec::new();
        for record in records.iter().rev().take(200) {
            let Some(url) = parse_endpoint(&record.url) else {
                continue;
            };
            if url.origin != current.matcher.origin || url.pathname != current.matcher.pathname {
                continue;
            }
            if !matches(
                &current.matcher,
                &record.method,
                &record.url,
                record.post_data.as_deref(),
                record.resource_type.as_deref(),
            )
            .is_match()
            {
                continue;
            }
            let blocked = record
                .mock
                .as_ref()
                .is_some_and(|mark| mark.outcome == "blocked");
            misses.push(NetworkNearMissView {
                reason: if blocked {
                    "blocked"
                } else {
                    "query_extra_key"
                }
                .to_string(),
                record_id: Some(record.record_id.clone()),
                method: Some(record.method.clone()),
                url: Some(record.url.clone()),
                tab_title: record.tab_title.clone(),
                blocked_by: blocked.then(|| "cors".to_string()),
            });
        }
        Ok(misses)
    }

    pub async fn hit_views(
        &self,
        rule_id: Option<&str>,
        limit: u32,
    ) -> AppResult<Vec<NetworkHitView>> {
        let repo = self.repo.clone();
        let rule_id = rule_id.map(ToString::to_string);
        let hits = self
            .stmt(|connection| {
                let repo = repo.clone();
                let rule_id = rule_id.clone();
                Box::pin(async move { repo.list_hits(connection, rule_id.as_deref(), limit).await })
            })
            .await?;
        Ok(hits.iter().map(NetworkHitView::from_row).collect())
    }

    /// The HTTP handler adds the `{ hits: [...] }` envelope once; returning a
    /// view here made the payload `{ hits: { hits: [...] } }` and broke the
    /// contract.
    pub async fn list_hits(
        &self,
        rule_id: Option<&str>,
        limit: u32,
    ) -> AppResult<Vec<NetworkHitView>> {
        self.hit_views(rule_id, limit).await
    }

    pub async fn delete_hits(&self, rule_id: Option<&str>) -> AppResult<Value> {
        let repo = self.repo.clone();
        let rule_id = rule_id.map(ToString::to_string);
        let removed = self
            .repo
            .stmt(|connection| {
                let repo = repo.clone();
                let rule_id = rule_id.clone();
                Box::pin(async move { repo.delete_hits(connection, rule_id.as_deref()).await })
            })
            .await?;
        Ok(json!(NetworkHitsDeleted { deleted: removed }))
    }

    pub async fn set_rule_enabled(&self, id: &str, enabled: bool) -> AppResult<Value> {
        let rule = self
            .rules
            .set_enabled(&self.rules.resolve_reference(id).await?.id, enabled)
            .await?;
        Ok(rule_summary(&rule, None, &[]))
    }

    /// PATCH is exactly one of `{enabled}`, `{scope[, tabId]}` or `{name}`
    /// anything else is a 400.
    pub async fn patch_rule(&self, id: &str, patch: &Value) -> AppResult<Value> {
        if patch_groups(patch) != 1 {
            return Err(AppError::bad_request(
                "invalid_request: patch exactly one of enabled / scope / name",
            ));
        }
        if let Some(enabled) = patch.get("enabled") {
            let enabled = enabled.as_bool().ok_or_else(|| {
                AppError::bad_request("invalid_request: enabled must be a boolean")
            })?;
            return self.set_rule_enabled(id, enabled).await;
        }
        if let Some(name) = patch.get("name") {
            let name = name
                .as_str()
                .ok_or_else(|| AppError::bad_request("invalid_request: name must be a string"))?;
            return self.rename_rule(id, name).await;
        }
        self.set_rule_scope_json(id, patch).await
    }

    pub async fn set_rule_scope_json(&self, id: &str, patch: &Value) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        let tab_id = patch.get("tabId").and_then(Value::as_i64);
        let scope = match patch.get("scope").and_then(Value::as_str) {
            Some("tab") => ScopeKind::Tab,
            Some("all_tabs") => ScopeKind::AllTabs,
            other => {
                return Err(AppError::bad_request(format!(
                    "invalid_scope: {}",
                    other.unwrap_or("<missing>")
                )));
            }
        };
        let rule = self.rules.set_scope(&rule.id, scope, tab_id).await?;
        Ok(rule_summary(&rule, None, &[]))
    }

    pub async fn rename_rule(&self, id: &str, name: &str) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        let rule = self.rules.rename(&rule.id, name).await?;
        Ok(rule_summary(&rule, None, &[]))
    }

    pub async fn delete_rule(&self, id: &str) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        self.rules.delete(&rule.id).await
    }

    pub async fn delete_all_rules(&self) -> AppResult<Value> {
        self.rules.delete_all().await
    }

    /// Runs the rule's processor script on its sample. The processor engine is
    /// wired in separately, so every rule is static for now.
    pub async fn processor_test(&self, id: &str) -> AppResult<Value> {
        self.rules.processor_test(id).await
    }

    pub async fn restore_rule(&self, id: &str) -> AppResult<Value> {
        let rule = self.rules.restore(id).await?;
        Ok(rule_summary(&rule, None, &[]))
    }

    pub async fn restore_revision(&self, id: &str, revision: u32) -> AppResult<Value> {
        let rule = self.rules.resolve_reference(id).await?;
        let rule = self.rules.restore_revision(&rule.id, revision).await?;
        Ok(rule_summary(&rule, None, &[]))
    }

    pub async fn stop_all(&self) -> AppResult<Value> {
        self.rules.stop_all().await
    }

    pub async fn tab_ids(&self) -> Vec<i64> {
        self.state
            .lock()
            .await
            .tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect()
    }

    /// Reloads the tab a rule targets, so the user sees the mock immediately.
    pub async fn reload_tab(&self, tab_id: i64, ignore_cache: bool) -> AppResult<()> {
        let Some(session) = (self.session)().await else {
            return Err(AppError::unavailable("browser_unavailable"));
        };
        let pages = session.pages.list().await.unwrap_or_default();
        let Some(page) = pages
            .iter()
            .find(|page| page.tab_id == browseros_core::TabId::from(tab_id))
        else {
            return Err(AppError::not_found("tab_not_found"));
        };
        let page_session = session
            .pages
            .get_session(page.page_id.clone())
            .await
            .map_err(|error| AppError::Internal(format!("page session: {error}")))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            page_session
                .session
                .send_value("Page.reload", json!({ "ignoreCache": ignore_cache })),
        )
        .await;
        match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(AppError::unavailable(format!("reload failed: {error}"))),
            Err(_) => Err(AppError::unavailable("reload timed out")),
        }
    }

    // ---- status --------------------------------------------------------

    pub async fn status_json(&self) -> Value {
        let connected = (self.session)().await.is_some();
        let rules = self.repo.list_rules(false).await.unwrap_or_default();
        let capture = self.capture_json().await;
        let runtime = self.runtime.current().map(|handle| {
            let stats = handle.stats();
            NetworkRuntimeView::new(
                stats.paused,
                stats.decision_p95_ms,
                stats.rules_generation,
                Some(stats.processor_p95_ms),
                Some(stats.processor_inflight),
            )
        });
        let status = NetworkStatusView {
            browser: NetworkBrowserView { connected },
            capture,
            rules: NetworkRulesStateView {
                active: rules
                    .iter()
                    .filter(|rule| rule.enabled && rule.state == RuleState::Active)
                    .count() as u64,
                total: rules.len() as u64,
            },
            runtime,
        };
        serde_json::to_value(&status).unwrap_or(Value::Null)
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

    // ---- runtime plumbing ----------------------------------------------

    /// One `NetworkRuntime` per browser session. A new session pointer after a
    /// disconnect means the old runtime is dead and must be replaced.
    fn spawn_supervisor(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut attached: Option<usize> = None;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                if service.cancel.is_cancelled() {
                    return;
                }
                let current = (service.session)().await;
                let pointer = current
                    .as_ref()
                    .map(|session| Arc::as_ptr(session) as *const () as usize);
                if pointer.is_none() {
                    if attached.take().is_some() {
                        // The browser went away: nothing is confirmed any more.
                        service.rules.on_runtime_lost().await;
                        if let Some(previous) = service.runtime.replace(None) {
                            previous.shutdown();
                        }
                    }
                    continue;
                }
                let Some(session) = current else {
                    continue;
                };
                if attached == pointer {
                    continue;
                }
                attached = pointer;
                let (tx, rx) = mpsc::unbounded_channel();
                let handle =
                    NetworkRuntime::spawn(Arc::clone(&session), tx, Arc::clone(&service.processor));
                handle.set_capture(service.state.lock().await.running);
                // Install the new runtime first: `push_rules` pushes through
                // `runtime.current()`, so doing it in the other order sends the
                // snapshot to the closed runtime and the rules stay pending.
                if let Some(previous) = service
                    .runtime
                    .replace(Some(Arc::new(handle) as Arc<dyn RuntimePort>))
                {
                    previous.shutdown();
                }
                match service.rules.push_rules().await {
                    Ok(rebuild) => service.broadcast_rules(&rebuild).await,
                    Err(error) => {
                        warn!(error = %error, "failed to build the runtime rule set");
                        // Retry on the next tick instead of pinning this session.
                        attached = None;
                        continue;
                    }
                }
                let consumer = Arc::clone(&service);
                tokio::spawn(async move {
                    consumer.consume(rx).await;
                });
            }
        });
    }

    fn spawn_maintenance(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticks: u64 = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                if service.cancel.is_cancelled() {
                    return;
                }
                ticks += 1;
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
                let before = now_ms() - UNDO_WINDOW_MS;
                let repo = service.repo.clone();
                let purged = service
                    .repo
                    .stmt(|connection| {
                        let repo = repo.clone();
                        Box::pin(async move { repo.purge_deleted_rules(connection, before).await })
                    })
                    .await;
                match purged {
                    Ok(ids) => {
                        for id in &ids {
                            if let Err(error) = service.bodies.remove_rule(id).await {
                                warn!(rule_id = id, error = %error, "failed to release rule body");
                            }
                        }
                    }
                    Err(error) => warn!(error = %error, "failed to purge deleted rules"),
                }
                if ticks.is_multiple_of(MAINTENANCE_EVERY) {
                    service.rules.maintenance().await;
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
                RuntimeEvent::Hit(hit) => {
                    if let Err(error) = self.on_hit(hit).await {
                        warn!(error = %error, "failed to record a rule hit");
                    }
                }
                RuntimeEvent::Applied(report) => self.rules.on_applied(report).await,
                RuntimeEvent::Tabs(tabs) => {
                    let (previous, current) = {
                        let mut state = self.state.lock().await;
                        let previous = std::mem::take(&mut state.tabs);
                        state.tabs = tabs.clone();
                        (previous, tabs)
                    };
                    self.rules.set_tabs(current.clone()).await;
                    let removed: Vec<i64> = previous
                        .iter()
                        .filter(|tab| !current.iter().any(|next| next.tab_id == tab.tab_id))
                        .map(|tab| tab.tab_id)
                        .collect();
                    if !removed.is_empty()
                        && let Err(error) = self.rules.unbind_closed_tabs(&removed).await
                    {
                        warn!(error = %error, "failed to unbind closed tabs");
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
        // The runtime's `record_done` snapshot has no host marks; a processor
        // hit lands before it, so keep the mark instead of overwriting it.
        let mut record = keep_host_marks(state.records.get(&seq), record);
        take_pending_mark(&mut state, &mut record);
        state.index.insert(record.record_id.clone(), seq);
        state.by_request.insert(record.request_id.clone(), seq);
        state.records.insert(seq, record.clone());
        let evicted = enforce_buffer_limits(&mut state);
        drop(state);
        self.broadcast("record", json!({ "record": record }));
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

    /// A follow-up outcome (blocked, failed) rewrites the recorded hit instead
    /// of appending one; only `fulfilled`/`processed` advance `hit_count`.
    pub(crate) async fn on_hit(&self, hit: HitRecord) -> AppResult<()> {
        let counted = matches!(hit.outcome.as_str(), "fulfilled" | "processed");
        let at = hit.at_ms;
        let row = hit_from(&hit);

        let repo = self.repo.clone();
        let network_id = hit.network_id.clone();
        let rule_id = hit.rule_id.clone();
        let outcome = hit.outcome.clone();
        let reason = hit.reason.clone();
        let updated = self
            .stmt(|connection| {
                let repo = repo.clone();
                let network_id = network_id.clone();
                let rule_id = rule_id.clone();
                let outcome = outcome.clone();
                let reason = reason.clone();
                Box::pin(async move {
                    let Some(network_id) = &network_id else {
                        return Ok(0);
                    };
                    repo.update_hit_outcome(
                        connection,
                        &rule_id,
                        network_id,
                        &outcome,
                        reason.as_deref(),
                    )
                    .await
                })
            })
            .await?;

        let inserted = if updated > 0 {
            self.stmt(|connection| {
                let repo = repo.clone();
                let rule_id = rule_id.clone();
                Box::pin(async move { repo.increment_hit(connection, &rule_id, at, false).await })
            })
            .await?;
            false
        } else {
            self.stmt(|connection| {
                let repo = repo.clone();
                let row = row.clone();
                Box::pin(async move {
                    repo.insert_hit(connection, &row).await?;
                    repo.increment_hit(connection, &row.rule_id, at, counted)
                        .await
                })
            })
            .await?;
            true
        };

        // The record carries the mark the list renders; the hit row alone is
        // not enough, because the row is built from the capture buffer.
        if let Some(network_id) = hit.network_id.as_deref() {
            let mark = MockMark {
                rule_id: hit.rule_id.clone(),
                revision: hit.revision,
                outcome: hit.outcome.clone(),
                reason: hit.reason.clone(),
            };
            let marked = {
                let mut state = self.state.lock().await;
                let seq = state.by_request.get(network_id).copied();
                match seq.and_then(|seq| state.records.get_mut(&seq)) {
                    Some(record) => {
                        record.mock = Some(mark);
                        Some(record.clone())
                    }
                    None => {
                        // The hit beat the record here; the mark is applied in
                        // `on_record`, which runs on the next runtime event.
                        if state.pending_marks.len() >= PENDING_MARKS_LIMIT {
                            state.pending_marks.clear();
                        }
                        state.pending_marks.insert(network_id.to_string(), mark);
                        None
                    }
                }
            };
            if let Some(record) = marked {
                self.broadcast("record", json!({ "record": record }));
            }
        }

        if inserted {
            self.broadcast("hit", json!(hit_view(&row)));
        } else {
            self.broadcast("hit", json!(hit_view(&hit_from(&hit))));
        }
        Ok(())
    }

    async fn broadcast_rules(&self, rebuild: &Rebuild) {
        for id in &rebuild.changed {
            if let Ok(Some(rule)) = self.repo.get_rule(id).await {
                self.broadcast("rule", rule_summary(&rule, None, &[]));
            }
        }
    }

    /// The pooled connection, for single statements that do not need a
    /// transaction. Never nest this inside another statement group.
    async fn stmt<F, T>(&self, work: F) -> AppResult<T>
    where
        F: for<'a> FnOnce(&'a DbConnection) -> BoxFuture<'a, AppResult<T>>,
    {
        self.repo.stmt(work).await
    }

    async fn record_and_body(&self, record_id: &str) -> Option<(NetworkRecord, u64)> {
        let record = self.get_record(record_id).await?;
        let size = self
            .state
            .lock()
            .await
            .body_sizes
            .get(record_id)
            .copied()
            .unwrap_or(0);
        Some((record, size))
    }
}

// ---- helpers -----------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Mirrors the contract's `bodyState` values.
fn body_state_str(state: &browseros_core::network::BodyState) -> &'static str {
    match state {
        browseros_core::network::BodyState::Pending => "pending",
        browseros_core::network::BodyState::Available => "available",
        browseros_core::network::BodyState::TooLarge => "too_large",
        browseros_core::network::BodyState::Unavailable => "unavailable",
        browseros_core::network::BodyState::Evicted => "unavailable",
    }
}

fn text_or_base64(bytes: &[u8]) -> (&'static str, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => ("utf8", text.to_string()),
        Err(_) => ("base64", browseros_core::network::base64_encode(bytes)),
    }
}

/// Walks back up to three bytes so a multi-byte character is never split.
fn utf8_boundary(bytes: &[u8]) -> usize {
    if std::str::from_utf8(bytes).is_ok() {
        return bytes.len();
    }
    for back in 1..=3 {
        let end = bytes.len().saturating_sub(back);
        if std::str::from_utf8(&bytes[..end]).is_ok() {
            return end;
        }
    }
    0
}

fn parse_endpoint(url: &str) -> Option<browseros_core::network::UrlParts> {
    let parts = browseros_core::network::parse_url_parts(url);
    if parts.origin.is_empty() {
        return None;
    }
    Some(parts)
}

fn hit_from(hit: &HitRecord) -> HitRow {
    HitRow {
        id: format!("hit_{}", uuid::Uuid::new_v4().simple()),
        rule_id: hit.rule_id.clone(),
        revision: hit.revision,
        at_ms: hit.at_ms,
        tab_title: hit.tab_title.clone(),
        url: hit.url.clone(),
        method: hit.method.clone(),
        outcome: hit.outcome.clone(),
        reason: hit.reason.clone(),
        network_id: hit.network_id.clone(),
    }
}

fn hit_view(hit: &HitRow) -> Value {
    serde_json::to_value(NetworkHitView::from_row(hit)).unwrap_or(Value::Null)
}

fn rule_summary(rule: &RuleRow, revision: Option<&RuleRevisionRow>, hits: &[HitRow]) -> Value {
    serde_json::to_value(NetworkRuleView::summary(rule, revision, hits)).unwrap_or(Value::Null)
}

/// The text the GUI puts on the clipboard and the user pastes to the agent.
fn handoff_clipboard(handoff: &HandoffRow) -> String {
    let method = handoff
        .source_meta
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let url = handoff
        .source_meta
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!(
        "NEOB network reference {}\n{} {}\nPaste this to the agent and describe what to mock.",
        handoff.id, method, url
    )
}

fn handoff_summary(handoff: &HandoffRow) -> Value {
    serde_json::to_value(NetworkHandoffView::from_row(handoff)).unwrap_or(Value::Null)
}

/// Carries host-only marks (the mock badge) across a later runtime snapshot.
fn keep_host_marks(previous: Option<&NetworkRecord>, mut record: NetworkRecord) -> NetworkRecord {
    if record.mock.is_none()
        && let Some(previous) = previous
        && previous.record_id == record.record_id
    {
        record.mock = previous.mock.clone();
    }
    record
}

/// A hit can arrive before its record on a freshly attached page; apply the
/// stored mark as soon as the record shows up.
fn take_pending_mark(state: &mut CaptureState, record: &mut NetworkRecord) {
    if record.mock.is_some() {
        return;
    }
    if let Some(mark) = state.pending_marks.remove(&record.request_id) {
        record.mock = Some(mark);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn record(seq: u64, record_id: &str) -> NetworkRecord {
        NetworkRecord {
            seq,
            record_id: record_id.to_string(),
            request_id: format!("request-{seq}"),
            ..NetworkRecord::default()
        }
    }

    #[test]
    fn buffer_limits_evict_oldest_unpinned_records_first() {
        let mut state = CaptureState::default();
        for seq in 0..=PER_TAB_RECORD_LIMIT as u64 {
            let mut entry = record(seq, &format!("netrec_{seq}"));
            entry.tab_id = Some(1);
            state.records.insert(seq, entry);
            state.index.insert(format!("netrec_{seq}"), seq);
            state.by_request.insert(format!("request-{seq}"), seq);
        }
        let extra = PER_TAB_RECORD_LIMIT as u64 + 1;
        let mut pinned = record(extra, "netrec_pinned");
        pinned.tab_id = Some(7);
        state.pinned.insert("netrec_pinned".to_string());
        state.records.insert(extra, pinned);
        state.index.insert("netrec_pinned".to_string(), extra);

        let evicted = enforce_buffer_limits(&mut state);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].record_id, "netrec_0");
        assert!(
            state.records.contains_key(&extra),
            "pinned records survive eviction"
        );
        assert!(!state.by_request.contains_key("request-0"));
    }

    #[test]
    fn a_later_record_snapshot_keeps_the_mock_mark() {
        let mut previous = record(7, "netrec_7");
        previous.mock = Some(MockMark {
            rule_id: "rule_1".to_string(),
            revision: 1,
            outcome: "processed".to_string(),
            reason: None,
        });

        let merged = keep_host_marks(Some(&previous), record(7, "netrec_7"));
        assert_eq!(merged.mock.expect("mark kept").outcome, "processed");

        let other = keep_host_marks(Some(&previous), record(8, "netrec_8"));
        assert!(other.mock.is_none(), "a different record must not inherit");
    }

    #[test]
    fn a_hit_that_beats_its_record_still_marks_it() {
        let mut state = CaptureState::default();
        let mut record = record(9, "netrec_9");
        state.pending_marks.insert(
            record.request_id.clone(),
            MockMark {
                rule_id: "rule_9".to_string(),
                revision: 3,
                outcome: "processed".to_string(),
                reason: None,
            },
        );

        take_pending_mark(&mut state, &mut record);
        let mark = record.mock.expect("pending mark applied");
        assert_eq!(mark.rule_id, "rule_9");
        assert!(state.pending_marks.is_empty());
    }

    #[test]
    fn utf8_boundary_never_splits_a_character() {
        let bytes = "aあb".as_bytes();
        for cut in 1..bytes.len() {
            let end = utf8_boundary(&bytes[..cut]);
            assert!(std::str::from_utf8(&bytes[..end]).is_ok(), "cut at {cut}");
        }
        assert_eq!(utf8_boundary(bytes), bytes.len());
    }

    #[test]
    fn rule_patch_requires_exactly_one_group() {
        let mut patch = json!({ "enabled": true });
        assert_eq!(patch_groups(&patch), 1);
        patch["name"] = json!("x");
        assert_eq!(patch_groups(&patch), 2);
    }
}
