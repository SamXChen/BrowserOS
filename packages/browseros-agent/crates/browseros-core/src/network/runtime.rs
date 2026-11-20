//! CDP ownership for capture and Fetch interception.
//!
//! One [`NetworkRuntime`] exists per browser connection. It owns the targeted
//! event subscriptions (a method can only have one subscriber, see
//! `CdpClient::events_targeted`) and the in-memory rule snapshot. It never
//! touches the filesystem, the database or the GUI: hosts receive
//! [`RuntimeEvent`]s and own persistence.

use super::{
    events::{
        LoadingFailed, LoadingFinished, RequestWillBeSent, RequestWillBeSentExtraInfo,
        ResponseReceived, ResponseReceivedExtraInfo, header_pairs_from_value, headers_from_value,
        merge_headers,
    },
    matcher::{EndpointMatcher, matches, parse_url_parts},
    response::{
        FulfillPlan, NoopProcessor, ProcessorErrorKind, ProcessorHooks, ProcessorInput,
        ProcessorResult, ProcessorSpec, ResponseProcessor, StaticResponse, build_fulfill,
        build_processed, escape_url_pattern,
    },
    state::{BodyState, HeaderPair, NetworkRecord, RecordState},
};
use crate::{BrowserSession, PageId, ProtocolSession, SessionId};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex as StdMutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_POST_DATA_BYTES: u64 = 1024 * 1024;
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const BODY_CONCURRENCY: usize = 4;

/// One attached page, resolved for tab-scoped rules and for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabSummary {
    pub tab_id: i64,
    pub page_id: u32,
    pub title: String,
    pub url: String,
    pub attached: bool,
}

#[derive(Clone)]
pub struct SessionView {
    pub session_id: SessionId,
    pub page_id: PageId,
    pub tab_id: i64,
    pub title: String,
    pub url: String,
    pub protocol: ProtocolSession,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRecord {
    pub rule_id: String,
    pub revision: u32,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    pub url: String,
    pub method: String,
    /// `fulfilled` | `blocked` | `decision_timeout` | `delay_cancelled`
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `Network.requestWillBeSent.requestId`, so the host can associate this hit
    /// with a captured record without runtime bookkeeping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Record(Box<NetworkRecord>),
    Body {
        record_id: String,
        bytes: Option<Vec<u8>>,
        reason: Option<String>,
    },
    Hit(HitRecord),
    /// The reconcile loop confirmed a rule snapshot; lets the host move rules
    /// from `pending` to `active` without polling.
    Applied(ApplyReport),
    Tabs(Vec<TabSummary>),
    Gap {
        lost: u64,
    },
}

/// What the runtime does when a rule matches.
#[derive(Debug, Clone)]
pub enum RuntimeAction {
    /// Fulfill from the stored response; the request never reaches upstream.
    Static {
        response: StaticResponse,
        body: Arc<[u8]>,
        sample_headers: Arc<[HeaderPair]>,
    },
    /// Let the request through, then rewrite the upstream response.
    Processor { spec: Arc<ProcessorSpec> },
}

#[derive(Debug, Clone)]
pub struct RuntimeRule {
    pub id: String,
    pub revision: u32,
    /// `None` means all tabs.
    pub scope_tab: Option<i64>,
    pub matcher: EndpointMatcher,
    pub action: RuntimeAction,
    /// Rule update time; the winner tie-break prefers the newest rule.
    pub updated_at: i64,
}

impl RuntimeRule {
    /// Fewer body fields and more ignored query keys are less strict, so the
    /// winner sort prefers the strictest rule.
    fn strictness(&self) -> (usize, isize, i64) {
        (
            self.matcher.body_fields.len(),
            -(self.matcher.query_policy.ignore_keys.len() as isize),
            self.updated_at,
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeRuleSet {
    pub generation: u64,
    pub rules: Vec<RuntimeRule>,
}

impl RuntimeRuleSet {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    #[must_use]
    pub fn patterns(&self) -> Vec<Value> {
        self.rules
            .iter()
            .map(|rule| {
                let literal = if let Some(exact) = &rule.matcher.exact_url {
                    let parts = parse_url_parts(exact);
                    format!("{}{}", parts.origin, parts.pathname)
                } else {
                    format!("{}{}", rule.matcher.origin, rule.matcher.pathname)
                };
                let stage = match &rule.action {
                    RuntimeAction::Static { .. } => "Request",
                    RuntimeAction::Processor { .. } => "Response",
                };
                let mut pattern = json!({
                    "urlPattern": escape_url_pattern(&literal),
                    "requestStage": stage,
                });
                if let Some(resource_type) = &rule.matcher.resource_type {
                    pattern["resourceType"] = Value::String(resource_type.clone());
                }
                pattern
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStats {
    pub paused: usize,
    pub decision_p95_ms: u64,
    pub rules_generation: u64,
    pub processor_p95_ms: u64,
    pub processor_inflight: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// The snapshot generation this report describes.
    pub generation: u64,
    /// Live sessions seen by the reconcile tick that produced the report.
    pub total: usize,
    /// Sessions that acknowledged this generation and have Fetch on.
    pub acked: usize,
    /// Sessions that still have the Fetch domain enabled (any generation).
    pub fetch_enabled: usize,
    /// A newer generation already replaced this snapshot, so the runtime
    /// ignored it without touching anything.
    pub stale: bool,
    /// `apply_rules` gave up before any session acknowledged.
    pub timed_out: bool,
}

#[derive(Debug)]
pub enum RuntimeCommand {
    SetCapture(bool),
    SetRules {
        rules: Arc<RuntimeRuleSet>,
        ack: oneshot::Sender<ApplyReport>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct NetworkRuntimeHandle {
    commands: mpsc::UnboundedSender<RuntimeCommand>,
    cancel: CancellationToken,
    shared: Arc<Shared>,
}

impl NetworkRuntimeHandle {
    pub fn set_capture(&self, running: bool) {
        let _ = self.commands.send(RuntimeCommand::SetCapture(running));
    }

    /// Replace the rule snapshot and wait until every attached page session has
    /// the new Fetch patterns enabled. Returns the number of sessions that
    /// acknowledged; `0` means the caller must not claim `active`.
    pub async fn apply_rules(&self, rules: Arc<RuntimeRuleSet>) -> ApplyReport {
        let (ack, receiver) = oneshot::channel();
        if self
            .commands
            .send(RuntimeCommand::SetRules { rules, ack })
            .is_err()
        {
            return ApplyReport::default();
        }
        tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    pub fn shutdown(&self) {
        self.cancel.cancel();
        if let Ok(release) = self.shared.release.write() {
            release.cancel();
        }
        let _ = self.commands.send(RuntimeCommand::Shutdown);
    }

    #[must_use]
    pub fn stats(&self) -> RuntimeStats {
        self.shared.stats()
    }
}

struct Shared {
    capture: AtomicBool,
    rules: RwLock<Arc<RuntimeRuleSet>>,
    release: RwLock<CancellationToken>,
    sequence: AtomicU64,
    apply_waiters: std::sync::Mutex<Vec<oneshot::Sender<ApplyReport>>>,
    /// OOPIF child session → parent session. Kept in sync from
    /// `Target.attachedToTarget` / `detachedFromTarget` broadcast events.
    children: std::sync::Mutex<HashMap<SessionId, SessionId>>,
    /// Top-level target id → the session obtained by waiting for the new tab
    /// before its first request. Falls back to PageManager sessions when absent.
    auto_pages: std::sync::Mutex<HashMap<String, SessionId>>,
    paused_inflight: AtomicUsize,
    decision_times: StdMutex<VecDeque<u64>>,
    processor_times: StdMutex<VecDeque<u64>>,
    processor_inflight: StdMutex<HashMap<SessionId, usize>>,
    pending_hits: StdMutex<HashMap<String, HitRecord>>,
    /// The sandbox that rewrites upstream responses.
    processor: Arc<dyn ResponseProcessor>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            capture: AtomicBool::new(false),
            rules: RwLock::new(Arc::new(RuntimeRuleSet::default())),
            release: RwLock::new(CancellationToken::new()),
            sequence: AtomicU64::new(0),
            apply_waiters: std::sync::Mutex::new(Vec::new()),
            children: std::sync::Mutex::new(HashMap::new()),
            auto_pages: std::sync::Mutex::new(HashMap::new()),
            paused_inflight: AtomicUsize::new(0),
            decision_times: StdMutex::new(VecDeque::new()),
            processor_times: StdMutex::new(VecDeque::new()),
            processor_inflight: StdMutex::new(HashMap::new()),
            pending_hits: StdMutex::new(HashMap::new()),
            processor: Arc::new(NoopProcessor),
        }
    }
}

impl Shared {
    /// Generations are strictly monotonic, so an older snapshot
    /// must never replace a newer one. Returns false when the snapshot was
    /// rejected; the caller's ack already carries `stale`.
    fn apply_command_rules(
        &self,
        rules: Arc<RuntimeRuleSet>,
        ack: oneshot::Sender<ApplyReport>,
    ) -> bool {
        let generation = rules.generation;
        if generation < self.rules().generation {
            let _ = ack.send(ApplyReport {
                generation,
                stale: true,
                ..ApplyReport::default()
            });
            return false;
        }
        if let Ok(mut slot) = self.rules.write() {
            *slot = rules;
        }
        // Any in-flight delay or fulfill decision belongs to the previous rule
        // generation: release it now.
        if let Ok(mut slot) = self.release.write() {
            slot.cancel();
            *slot = CancellationToken::new();
        }
        if let Ok(mut waiters) = self.apply_waiters.lock() {
            waiters.push(ack);
        }
        true
    }

    fn rules(&self) -> Arc<RuntimeRuleSet> {
        self.rules
            .read()
            .map(|rules| rules.clone())
            .unwrap_or_default()
    }

    fn release(&self) -> CancellationToken {
        self.release
            .read()
            .map(|token| token.clone())
            .unwrap_or_else(|_| CancellationToken::new())
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn enter_paused(self: &Arc<Self>) -> InflightGuard {
        let now = self.paused_inflight.fetch_add(1, Ordering::Relaxed) + 1;
        if now > 100 {
            warn!(paused = now, "network paused queue is backing up");
        }
        InflightGuard(self.clone())
    }

    fn record_decision(&self, elapsed_ms: u64) {
        if let Ok(mut times) = self.decision_times.lock() {
            times.push_back(elapsed_ms);
            while times.len() > 200 {
                times.pop_front();
            }
        }
    }

    fn record_processor(&self, elapsed_ms: u64) {
        if let Ok(mut times) = self.processor_times.lock() {
            times.push_back(elapsed_ms);
            while times.len() > 200 {
                times.pop_front();
            }
        }
    }

    fn enter_processor(self: &Arc<Self>, session: &SessionId) -> ProcessorGuard {
        if let Ok(mut map) = self.processor_inflight.lock() {
            *map.entry(session.clone()).or_insert(0) += 1;
        }
        ProcessorGuard {
            shared: Arc::clone(self),
            session: session.clone(),
        }
    }

    fn leave_processor(&self, session: &SessionId) {
        if let Ok(mut map) = self.processor_inflight.lock()
            && let Some(count) = map.get_mut(session)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(session);
            }
        }
    }

    /// Waits up to 1500ms for body reads on this session to finish, so a
    /// `Fetch.disable` cannot drop an in-flight `getResponseBody`.
    async fn wait_processor_idle(&self, session: &SessionId) {
        for _ in 0..30 {
            let busy = self
                .processor_inflight
                .lock()
                .map(|map| map.get(session).copied().unwrap_or(0))
                .unwrap_or(0);
            if busy == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        warn!(session = %session, "fetch disabled with processor body read in flight");
    }

    fn stats(&self) -> RuntimeStats {
        let mut times = self
            .decision_times
            .lock()
            .map(|times| times.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        times.sort_unstable();
        let processor_p95_ms = percentile_lock(&self.processor_times);
        let processor_inflight = self
            .processor_inflight
            .lock()
            .map(|map| map.values().sum())
            .unwrap_or(0);
        let p95 = if times.is_empty() {
            0
        } else {
            times[(times.len() - 1) * 95 / 100]
        };
        RuntimeStats {
            paused: self.paused_inflight.load(Ordering::Relaxed),
            decision_p95_ms: p95,
            rules_generation: self.rules().generation,
            processor_p95_ms,
            processor_inflight,
        }
    }
}

struct InflightGuard(Arc<Shared>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.paused_inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

struct ActiveRecord {
    record: NetworkRecord,
    redirect_ordinal: u32,
}

pub struct NetworkRuntime;

impl NetworkRuntime {
    /// Start the runtime for one browser connection. Dropping the handle (or a
    /// reconnect) must call [`NetworkRuntimeHandle::shutdown`] so the targeted
    /// subscriptions are released for the next connection.
    pub fn spawn(
        session: Arc<BrowserSession>,
        events: mpsc::UnboundedSender<RuntimeEvent>,
        processor: Arc<dyn ResponseProcessor>,
    ) -> NetworkRuntimeHandle {
        let shared = Arc::new(Shared {
            capture: AtomicBool::new(false),
            rules: RwLock::new(Arc::new(RuntimeRuleSet::default())),
            release: RwLock::new(CancellationToken::new()),
            sequence: AtomicU64::new(0),
            apply_waiters: std::sync::Mutex::new(Vec::new()),
            children: std::sync::Mutex::new(HashMap::new()),
            auto_pages: std::sync::Mutex::new(HashMap::new()),
            paused_inflight: AtomicUsize::new(0),
            decision_times: StdMutex::new(VecDeque::new()),
            pending_hits: StdMutex::new(HashMap::new()),
            processor_times: StdMutex::new(VecDeque::new()),
            processor_inflight: StdMutex::new(HashMap::new()),
            processor,
        });
        let cancel = CancellationToken::new();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (config_tx, config_rx) = watch::channel(0u64);
        let (session_tx, session_rx) = mpsc::unbounded_channel::<SessionView>();

        tokio::spawn(reconcile_loop(
            session.pages.clone(),
            session.connection(),
            shared.clone(),
            events.clone(),
            config_rx,
            session_tx,
            cancel.clone(),
        ));
        tokio::spawn(run_loop(
            session.clone(),
            shared.clone(),
            events,
            command_rx,
            config_tx,
            session_rx,
            cancel.clone(),
        ));

        NetworkRuntimeHandle {
            commands: command_tx,
            cancel,
            shared,
        }
    }
}

/// `Network.enable` parameters; identical for page and OOPIF sessions.
fn network_enable_params() -> Value {
    json!({
        "maxTotalBufferSize": 64 * 1024 * 1024,
        "maxResourceBufferSize": 8 * 1024 * 1024,
        "maxPostDataSize": MAX_POST_DATA_BYTES,
    })
}

/// `Fetch.enable` parameters for the current snapshot.
fn fetch_enable_params(patterns: Vec<Value>) -> Value {
    json!({ "patterns": patterns, "handleAuthRequests": false })
}

async fn reconcile_loop(
    pages: Arc<crate::pages::PageManager>,
    connection: Arc<dyn crate::connection::CdpConnection>,
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<RuntimeEvent>,
    mut config_rx: watch::Receiver<u64>,
    session_tx: mpsc::UnboundedSender<SessionView>,
    cancel: CancellationToken,
) {
    // Per-session configuration this loop last applied.
    struct Applied {
        capture: bool,
        capture_ok: bool,
        generation: u64,
        fetch_ok: bool,
        patterns: Vec<Value>,
    }

    /// Waits for in-flight processor work before `Fetch.disable`. Processors are
    /// not wired in yet, so it returns immediately.
    async fn wait_processor_idle(_protocol: &ProtocolSession) {}

    async fn change_fetch(
        protocol: &ProtocolSession,
        wants_fetch: bool,
        patterns: Vec<Value>,
        previous: Option<&Applied>,
    ) -> (bool, bool) {
        let previous_fetch = previous.is_some_and(|entry| entry.fetch_ok);
        let previous_patterns = previous
            .map(|entry| entry.patterns.clone())
            .unwrap_or_default();
        if !wants_fetch {
            // In-flight processor work must finish before interception is turned
            // off; with no processors wired in this is a no-op.
            wait_processor_idle(protocol).await;
            return match protocol.send_value("Fetch.disable", json!({})).await {
                Ok(_) => (false, true),
                Err(error) => {
                    warn!(error = %error, "network fetch disable failed");
                    (previous_fetch, false)
                }
            };
        }
        // A second `Fetch.enable` replaces the patterns of an
        // already-enabled session without a disable in between, and leaves an
        // in-flight paused request continuable. Disabling first would open an
        // interception gap and, once processors read response bodies, an
        // undefined-behaviour window.
        match protocol
            .send_value("Fetch.enable", fetch_enable_params(patterns))
            .await
        {
            Ok(_) => (true, true),
            Err(error) => {
                warn!(error = %error, "network fetch enable failed");
                let rollback_ok = if previous_fetch && !previous_patterns.is_empty() {
                    protocol
                        .send_value("Fetch.enable", fetch_enable_params(previous_patterns))
                        .await
                        .is_ok()
                } else {
                    false
                };
                (rollback_ok, false)
            }
        }
    }
    let mut applied: HashMap<SessionId, Applied> = HashMap::new();
    // Last reported confirmation set and tab list; the host is only told about
    // changes so the GUI is not woken every tick.
    let mut last_report: Option<ApplyReport> = None;
    let mut last_tabs: Option<Vec<TabSummary>> = None;
    // PageManager session per target, used to hand over to an auto-attached
    // session without leaving two Network domains capturing the same request.
    let mut page_sessions: HashMap<String, SessionId> = HashMap::new();

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = config_rx.changed() => {}
            () = tokio::time::sleep(RECONCILE_INTERVAL) => {}
        }

        let capture = shared.capture.load(Ordering::Relaxed);
        let rules = shared.rules();
        // A newer snapshot arrived after this tick read `shared.rules()`; report
        // this one as ignored and let the next tick apply the newer generation.
        let stale_snapshot = rules.generation != shared.rules().generation;
        if stale_snapshot {
            let report = ApplyReport {
                generation: rules.generation,
                stale: true,
                ..ApplyReport::default()
            };
            if last_report.as_ref() != Some(&report) {
                last_report = Some(report);
                let _ = events.send(RuntimeEvent::Applied(report));
            }
            continue;
        }
        let needs_capture = capture;
        let needs_rules = !rules.is_empty();
        // A session that still has Fetch enabled must be reconciled even after
        // the last rule disappears and capture is off, otherwise stale patterns
        // keep pausing requests.
        let has_applied_domains = applied
            .values()
            .any(|entry| entry.capture || entry.fetch_ok);
        if !needs_capture && !needs_rules && !has_applied_domains {
            continue;
        }

        let pages_list = match pages.list().await {
            Ok(list) => list,
            Err(error) => {
                debug!(error = %error, "network reconcile could not list pages");
                continue;
            }
        };

        let mut tabs = Vec::with_capacity(pages_list.len());
        let mut live_sessions = Vec::with_capacity(pages_list.len());
        for page in pages_list {
            let page_id = page.page_id.clone();
            let target_key = page.target_id.as_str().to_string();
            let auto_session = shared
                .auto_pages
                .lock()
                .ok()
                .and_then(|pages| pages.get(&target_key).cloned());
            let (session_id, protocol, page_url) = if let Some(auto_session) = auto_session {
                // The auto-attached session already owns Network.enable from
                // before the first request; do not attach a second session.
                if let Some(previous) = page_sessions.remove(&target_key)
                    && previous != auto_session
                    && applied.remove(&previous).is_some()
                {
                    let old = ProtocolSession::for_session(connection.clone(), previous);
                    let _ = old.send_value("Network.disable", json!({})).await;
                    let _ = old.send_value("Fetch.disable", json!({})).await;
                }
                (
                    auto_session.clone(),
                    ProtocolSession::for_session(connection.clone(), auto_session),
                    page.url.clone(),
                )
            } else {
                let page_session = match pages.get_session(page_id.clone()).await {
                    Ok(session) => session,
                    Err(error) => {
                        debug!(page = page_id.0, error = %error, "network reconcile could not attach page");
                        continue;
                    }
                };
                let session_id = page_session.session_id.clone();
                page_sessions.insert(target_key.clone(), session_id.clone());
                (
                    session_id,
                    page_session.session.clone(),
                    page_session.url.clone(),
                )
            };
            let view = SessionView {
                session_id: session_id.clone(),
                page_id: page_id.clone(),
                tab_id: page.tab_id.0,
                title: page.title.clone(),
                url: page_url,
                protocol,
            };
            tabs.push(TabSummary {
                tab_id: page.tab_id.0,
                page_id: page_id.0,
                title: page.title.clone(),
                url: page.url.clone(),
                attached: true,
            });

            let previous = applied.get(&session_id);
            let wants_fetch = !rules.is_empty();
            // A failed domain change must be retried on the next tick: otherwise a
            // transient CDP error would pin the session to the old state forever.
            let same_capture =
                previous.is_some_and(|entry| entry.capture == capture && entry.capture_ok);
            let same_rules = previous.is_some_and(|entry| {
                entry.generation == rules.generation && entry.fetch_ok == wants_fetch
            });
            if same_capture && same_rules {
                let _ = session_tx.send(view);
                live_sessions.push(session_id);
                continue;
            }

            let mut capture_ok = previous.is_some_and(|entry| entry.capture_ok);
            if !same_capture {
                let params = if capture {
                    network_enable_params()
                } else {
                    json!({})
                };
                let method = if capture {
                    "Network.enable"
                } else {
                    "Network.disable"
                };
                match view.protocol.send_value(method, params).await {
                    Ok(_) => capture_ok = true,
                    Err(error) => {
                        capture_ok = false;
                        warn!(session = %session_id, error = %error, "network capture domain change failed");
                    }
                }
            }
            let (fetch_ok, rules_ok) = if !same_rules || !same_capture {
                if !wants_fetch {
                    shared.wait_processor_idle(&session_id).await;
                }
                change_fetch(&view.protocol, wants_fetch, rules.patterns(), previous).await
            } else {
                (previous.is_some_and(|entry| entry.fetch_ok), same_rules)
            };

            applied.insert(
                session_id.clone(),
                Applied {
                    capture,
                    capture_ok,
                    generation: if rules_ok {
                        rules.generation
                    } else {
                        previous.map(|entry| entry.generation).unwrap_or(0)
                    },
                    fetch_ok,
                    patterns: if fetch_ok {
                        rules.patterns()
                    } else {
                        Vec::new()
                    },
                },
            );
            live_sessions.push(session_id.clone());
            let _ = session_tx.send(view);
        }

        applied.retain(|session_id, _| live_sessions.contains(session_id));
        let children = shared
            .children
            .lock()
            .map(|children| children.clone())
            .unwrap_or_default();
        for (child, parent) in children {
            if !live_sessions.iter().any(|session| session == &parent) {
                continue;
            }
            let protocol = ProtocolSession::for_session(connection.clone(), child.clone());
            let previous = applied.get(&child);
            let wants_fetch = !rules.is_empty();
            let same_capture =
                previous.is_some_and(|entry| entry.capture == capture && entry.capture_ok);
            let same_rules = previous.is_some_and(|entry| {
                entry.generation == rules.generation && entry.fetch_ok == wants_fetch
            });
            if same_capture && same_rules {
                live_sessions.push(child);
                continue;
            }
            let mut capture_ok = previous.is_some_and(|entry| entry.capture_ok);
            if !same_capture {
                let method = if capture {
                    "Network.enable"
                } else {
                    "Network.disable"
                };
                let params = if capture {
                    network_enable_params()
                } else {
                    json!({})
                };
                match protocol.send_value(method, params).await {
                    Ok(_) => capture_ok = true,
                    Err(error) => {
                        capture_ok = false;
                        warn!(session = %child, error = %error, "network capture domain change failed on OOPIF");
                    }
                }
            }
            let (fetch_ok, rules_ok) = if !same_rules || !same_capture {
                if !wants_fetch {
                    shared.wait_processor_idle(&child).await;
                }
                change_fetch(&protocol, wants_fetch, rules.patterns(), previous).await
            } else {
                (previous.is_some_and(|entry| entry.fetch_ok), same_rules)
            };
            applied.insert(
                child.clone(),
                Applied {
                    capture,
                    capture_ok,
                    generation: if rules_ok {
                        rules.generation
                    } else {
                        previous.map(|entry| entry.generation).unwrap_or(0)
                    },
                    fetch_ok,
                    patterns: if fetch_ok {
                        rules.patterns()
                    } else {
                        Vec::new()
                    },
                },
            );
            live_sessions.push(child);
        }

        let report = ApplyReport {
            generation: rules.generation,
            total: live_sessions.len(),
            // An ack means "this session applied this generation". `fetch_ok`
            // is reported separately: disabling the last rule turns Fetch off,
            // and that must still confirm the narrowing.
            acked: applied
                .values()
                .filter(|entry| entry.generation == rules.generation)
                .count(),
            fetch_enabled: applied.values().filter(|entry| entry.fetch_ok).count(),
            stale: stale_snapshot,
            timed_out: false,
        };
        // A reconcile that started before a newer SetRules must not answer that
        // command's waiter with its stale snapshot; the next tick will.
        if rules.generation == shared.rules().generation {
            let waiters = shared
                .apply_waiters
                .lock()
                .map(|mut waiters| std::mem::take(&mut *waiters))
                .unwrap_or_default();
            for waiter in waiters {
                let _ = waiter.send(report);
            }
        }
        // Only a changed confirmation set is worth a host round trip: the host
        // uses it to move `pending` rules to `active`.
        if last_report.as_ref() != Some(&report) {
            last_report = Some(report);
            let _ = events.send(RuntimeEvent::Applied(report));
        }
        if last_tabs.as_ref() != Some(&tabs) {
            last_tabs = Some(tabs.clone());
            let _ = events.send(RuntimeEvent::Tabs(tabs));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    session: Arc<BrowserSession>,
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<RuntimeEvent>,
    mut commands: mpsc::UnboundedReceiver<RuntimeCommand>,
    config_tx: watch::Sender<u64>,
    mut session_rx: mpsc::UnboundedReceiver<SessionView>,
    cancel: CancellationToken,
) {
    let mut will_be_sent = session.cdp_events_targeted("Network.requestWillBeSent");
    let mut response_received = session.cdp_events_targeted("Network.responseReceived");
    let mut loading_finished = session.cdp_events_targeted("Network.loadingFinished");
    let mut loading_failed = session.cdp_events_targeted("Network.loadingFailed");
    let mut request_extra = session.cdp_events_targeted("Network.requestWillBeSentExtraInfo");
    let mut response_extra = session.cdp_events_targeted("Network.responseReceivedExtraInfo");
    let mut paused = session.cdp_events_targeted("Fetch.requestPaused");
    // Target events are shared with FrameRegistry, so use the broadcast ring
    // rather than a targeted subscription (which would steal them).
    let mut target_events = session.connection().events();

    let mut tabs: HashMap<SessionId, SessionView> = HashMap::new();
    let mut active: HashMap<(SessionId, String), ActiveRecord> = HashMap::new();
    let body_permits = Arc::new(Semaphore::new(BODY_CONCURRENCY));
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::SetCapture(running)) => {
                        shared.capture.store(running, Ordering::Relaxed);
                        let _ = config_tx.send(shared.sequence.fetch_add(1, Ordering::Relaxed) + 1);
                    }
                    Some(RuntimeCommand::SetRules { rules, ack }) => {
                        // An older generation is acknowledged as stale and does
                        // not touch the snapshot.
                        if shared.apply_command_rules(rules, ack) {
                            let _ = config_tx
                                .send(shared.sequence.fetch_add(1, Ordering::Relaxed) + 1);
                        }
                    }
                    Some(RuntimeCommand::Shutdown) | None => break,
                }
            }
            Some(view) = session_rx.recv() => {
                tabs.insert(view.session_id.clone(), view);
            }
            event = target_events.recv() => {
                let Ok(event) = event else { continue };
                match event.method.as_str() {
                    "Target.targetCreated" => {
                        if let Ok(params) = serde_json::from_value::<
                            browseros_cdp::target::TargetCreatedEvent,
                        >(event.params)
                            && params.target_info.r#type == "page"
                            && (shared.capture.load(Ordering::Relaxed)
                                || !shared.rules().is_empty())
                        {
                            // Wake reconcile; it owns attachment so a direct
                            // attach here cannot race the native PageManager.
                            let _ = config_tx
                                .send(shared.sequence.fetch_add(1, Ordering::Relaxed) + 1);
                        }
                    }
                    "Target.attachedToTarget" => {
                        if let Ok(params) = serde_json::from_value::<
                            browseros_cdp::target::AttachedToTargetEvent,
                        >(event.params.clone())
                            && params.target_info.r#type == "page"
                            && event.session_id.is_none()
                        {
                            let child = SessionId::from(params.session_id);
                            if let Ok(mut pages) = shared.auto_pages.lock() {
                                pages.insert(
                                    params.target_info.target_id.as_str().to_string(),
                                    child.clone(),
                                );
                            }
                            let protocol = ProtocolSession::for_session(
                                session.connection(),
                                child.clone(),
                            );
                            if shared.capture.load(Ordering::Relaxed) {
                                let _ = protocol
                                    .send_value(
                                        "Network.enable",
                                        network_enable_params(),
                                    )
                                    .await;
                            }
                            let rules = shared.rules();
                            if !rules.is_empty() {
                                let _ = protocol
                                    .send_value(
                                        "Fetch.enable",
                                        fetch_enable_params(rules.patterns()),
                                    )
                                    .await;
                            }
                            tabs.insert(
                                child.clone(),
                                SessionView {
                                    session_id: child.clone(),
                                    page_id: PageId(0),
                                    tab_id: 0,
                                    title: params.target_info.title.clone(),
                                    url: params.target_info.url.clone(),
                                    protocol: protocol.clone(),
                                },
                            );
                            if params.waiting_for_debugger {
                                let _ = protocol
                                    .send_value("Runtime.runIfWaitingForDebugger", json!({}))
                                    .await;
                            }
                        }
                        if let Ok(params) = serde_json::from_value::<
                            browseros_cdp::target::AttachedToTargetEvent,
                        >(event.params)
                            && params.target_info.r#type == "iframe"
                            && let Some(parent) = event.session_id.clone()
                        {
                            let child = SessionId::from(params.session_id);
                            if let Ok(mut children) = shared.children.lock() {
                                children.insert(child.clone(), parent);
                            }
                            let protocol = ProtocolSession::for_session(
                                session.connection(),
                                child.clone(),
                            );
                            if shared.capture.load(Ordering::Relaxed) {
                                let _ = protocol
                                    .send_value(
                                        "Network.enable",
                                        network_enable_params(),
                                    )
                                    .await;
                            }
                            let rules = shared.rules();
                            if !rules.is_empty() {
                                let _ = protocol
                                    .send_value(
                                        "Fetch.enable",
                                        fetch_enable_params(rules.patterns()),
                                    )
                                    .await;
                            }
                        }
                    }
                    "Target.detachedFromTarget" => {
                        if let Ok(params) = serde_json::from_value::<
                            browseros_cdp::target::DetachedFromTargetEvent,
                        >(event.params)
                        {
                            let session = SessionId::from(params.session_id);
                            tabs.remove(&session);
                            if let Ok(mut children) = shared.children.lock() {
                                children.remove(&session);
                            }
                            if let Ok(mut pages) = shared.auto_pages.lock() {
                                pages.retain(|_, value| value != &session);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(event) = will_be_sent.recv() => {
                handle_request_will_be_sent(&shared, &events, &tabs, &mut active, event);
            }
            Some(event) = response_received.recv() => {
                handle_response_received(&events, &mut active, event);
            }
            Some(event) = loading_finished.recv() => {
                if let Some(request_id) = event.params.get("requestId").and_then(Value::as_str)
                    && let Ok(mut pending) = shared.pending_hits.lock()
                {
                    pending.remove(request_id);
                }
                handle_loading_finished(
                    &shared,
                    &events,
                    &tabs,
                    &mut active,
                    &body_permits,
                    event,
                );
            }
            Some(event) = loading_failed.recv() => {
                if let Some(request_id) = event.params.get("requestId").and_then(Value::as_str)
                    && let Ok(mut pending) = shared.pending_hits.lock()
                    && let Some(mut hit) = pending.remove(request_id)
                {
                    let cors = event.params.get("corsErrorStatus").is_some()
                        || event.params.get("blockedReason").is_some();
                    hit.outcome = if cors { "blocked" } else { "failed" }.to_string();
                    hit.reason = event
                        .params
                        .get("errorText")
                        .and_then(Value::as_str)
                        .map(short_reason);
                    let _ = events.send(RuntimeEvent::Hit(hit));
                }
                handle_loading_failed(&events, &mut active, event);
            }
            Some(event) = request_extra.recv() => {
                handle_request_extra(&events, &mut active, event);
            }
            Some(event) = response_extra.recv() => {
                handle_response_extra(&events, &mut active, event);
            }
            Some(event) = paused.recv() => {
                let Some(view) = event
                    .session_id
                    .as_ref()
                    .and_then(|session_id| view_for_session(&shared, &tabs, session_id))
                else {
                    // Fail open: a paused request whose session we did not
                    // register (for example an OOPIF child) must never hang.
                    if let Some(session_id) = event.session_id.clone() {
                        let request_id = event
                            .params
                            .get("requestId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let protocol =
                            ProtocolSession::for_session(session.connection(), session_id);
                        tokio::spawn(async move {
                            let _ = protocol
                                .send_value(
                                    "Fetch.continueRequest",
                                    json!({ "requestId": request_id }),
                                )
                                .await;
                        });
                    }
                    continue;
                };
                handle_paused(&shared, &events, &view, event);
            }
        }
    }

    // Releasing the targeted receivers restores broadcast delivery for the next
    // runtime and lets other components subscribe again.
    drop((
        will_be_sent,
        response_received,
        loading_finished,
        loading_failed,
        request_extra,
        response_extra,
        paused,
    ));
    debug!("network runtime stopped");
}

fn view_for_session(
    shared: &Arc<Shared>,
    tabs: &HashMap<SessionId, SessionView>,
    session_id: &SessionId,
) -> Option<SessionView> {
    let mut current = session_id.clone();
    for _ in 0..8 {
        if let Some(view) = tabs.get(&current) {
            return Some(view.clone());
        }
        current = shared.children.lock().ok()?.get(&current).cloned()?;
    }
    None
}

fn handle_request_will_be_sent(
    shared: &Arc<Shared>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    tabs: &HashMap<SessionId, SessionView>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    event: browseros_cdp::CdpEvent,
) {
    let Ok(params) = serde_json::from_value::<RequestWillBeSent>(event.params) else {
        return;
    };
    let Some(session_id) = event.session_id else {
        return;
    };
    let key = (session_id.clone(), params.request_id.clone());

    let mut ordinal = 0;
    if let Some(redirect) = &params.redirect_response
        && let Some(previous) = active.remove(&key)
    {
        let mut record = previous.record;
        record.status = Some(redirect.status);
        record.status_text = Some(redirect.status_text.clone());
        record.response_headers = headers_from_value(&redirect.headers);
        record.state = RecordState::Done;
        record.finished_at_ms = Some(now_ms());
        record.body_state = BodyState::Unavailable;
        record.body_reason = Some("redirect".to_string());
        let _ = events.send(RuntimeEvent::Record(Box::new(record)));
        ordinal = previous.redirect_ordinal;
    }

    let tab = view_for_session(shared, tabs, &session_id);
    let seq = shared.next_sequence();
    let record_id = format!("netrec_{}", uuid::Uuid::new_v4().simple());
    let record = NetworkRecord {
        record_id: record_id.clone(),
        seq,
        request_id: params.request_id.clone(),
        redirect_ordinal: ordinal,
        tab_id: tab.as_ref().map(|tab| tab.tab_id),
        tab_title: tab.as_ref().map(|tab| tab.title.clone()),
        page_id: tab.as_ref().map(|tab| tab.page_id.0),
        frame_id: params.frame_id.clone(),
        method: params.request.method.to_ascii_uppercase(),
        url: params.request.url.clone(),
        resource_type: params.resource_type.clone(),
        request_headers: headers_from_value(&params.request.headers),
        post_data: params.request.post_data.clone(),
        started_at_ms: now_ms(),
        state: RecordState::Created,
        ..NetworkRecord::default()
    };

    active.insert(
        key,
        ActiveRecord {
            record: record.clone(),
            redirect_ordinal: ordinal + 1,
        },
    );
    let _ = events.send(RuntimeEvent::Record(Box::new(record)));
}

fn update_active(
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    session_id: Option<&SessionId>,
    request_id: &str,
    mutate: impl FnOnce(&mut NetworkRecord),
    events: &mpsc::UnboundedSender<RuntimeEvent>,
) {
    let Some(session_id) = session_id.cloned() else {
        return;
    };
    let Some(entry) = active.get_mut(&(session_id, request_id.to_string())) else {
        return;
    };
    mutate(&mut entry.record);
    let _ = events.send(RuntimeEvent::Record(Box::new(entry.record.clone())));
}

fn handle_response_received(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    event: browseros_cdp::CdpEvent,
) {
    let session_id = event.session_id.clone();
    let Ok(params) = serde_json::from_value::<ResponseReceived>(event.params) else {
        return;
    };
    update_active(
        active,
        session_id.as_ref(),
        &params.request_id,
        |record| {
            record.status = Some(params.response.status);
            record.status_text = Some(params.response.status_text.clone());
            record.mime_type = Some(params.response.mime_type.clone());
            record.response_headers = headers_from_value(&params.response.headers);
            record.encoded_data_length = Some(params.response.encoded_data_length as u64);
            record.from_cache = params.response.from_disk_cache.unwrap_or(false)
                || params.response.from_prefetch_cache.unwrap_or(false);
            record.from_service_worker = params.response.from_service_worker.unwrap_or(false);
            if record.resource_type.is_none() {
                record.resource_type = params.resource_type.clone();
            }
            record.state = RecordState::ResponseReady;
        },
        events,
    );
}

fn handle_request_extra(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    event: browseros_cdp::CdpEvent,
) {
    let session_id = event.session_id.clone();
    let Ok(params) = serde_json::from_value::<RequestWillBeSentExtraInfo>(event.params) else {
        return;
    };
    update_active(
        active,
        session_id.as_ref(),
        &params.request_id,
        |record| {
            merge_headers(&mut record.request_headers, &params.headers);
        },
        events,
    );
}

fn handle_response_extra(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    event: browseros_cdp::CdpEvent,
) {
    let session_id = event.session_id.clone();
    let Ok(params) = serde_json::from_value::<ResponseReceivedExtraInfo>(event.params) else {
        return;
    };
    update_active(
        active,
        session_id.as_ref(),
        &params.request_id,
        |record| {
            merge_headers(&mut record.response_headers, &params.headers);
            if let Some(status) = params.status_code {
                record.status = Some(status);
            }
        },
        events,
    );
}

fn handle_loading_finished(
    shared: &Arc<Shared>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    tabs: &HashMap<SessionId, SessionView>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    body_permits: &Arc<Semaphore>,
    event: browseros_cdp::CdpEvent,
) {
    let Ok(params) = serde_json::from_value::<LoadingFinished>(event.params) else {
        return;
    };
    let Some(session_id) = event.session_id.clone() else {
        return;
    };
    let Some(entry) = active.remove(&(session_id.clone(), params.request_id.clone())) else {
        return;
    };
    let mut record = entry.record;
    record.state = RecordState::Done;
    record.finished_at_ms = Some(now_ms());
    record.encoded_data_length = Some(params.encoded_data_length as u64);
    if !shared.capture.load(Ordering::Relaxed) {
        let _ = events.send(RuntimeEvent::Record(Box::new(record)));
        return;
    }
    if params.encoded_data_length as u64 > MAX_BODY_BYTES {
        record.body_state = BodyState::TooLarge;
        record.body_reason = Some("too_large".to_string());
        let _ = events.send(RuntimeEvent::Record(Box::new(record)));
        return;
    }
    let _ = events.send(RuntimeEvent::Record(Box::new(record.clone())));

    let Some(view) = view_for_session(shared, tabs, &session_id) else {
        return;
    };
    let events = events.clone();
    let request_id = params.request_id;
    let record_id = record.record_id;
    let body_permits = body_permits.clone();
    tokio::spawn(async move {
        // A bounded worker pool, not a drop: a burst of responses must queue
        // rather than silently lose bodies.
        let Ok(_permit) = body_permits.acquire_owned().await else {
            return;
        };
        let result = view
            .protocol
            .send_value(
                "Network.getResponseBody",
                json!({ "requestId": request_id }),
            )
            .await;
        match result {
            Ok(value) => {
                let base64 = value
                    .get("base64Encoded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let body = value
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let bytes = if base64 {
                    super::response::base64_decode(body)
                } else {
                    Some(body.as_bytes().to_vec())
                };
                match bytes {
                    Some(bytes) if bytes.len() as u64 <= MAX_BODY_BYTES => {
                        let _ = events.send(RuntimeEvent::Body {
                            record_id,
                            bytes: Some(bytes),
                            reason: None,
                        });
                    }
                    _ => {
                        let _ = events.send(RuntimeEvent::Body {
                            record_id,
                            bytes: None,
                            reason: Some("too_large".to_string()),
                        });
                    }
                }
            }
            Err(error) => {
                let _ = events.send(RuntimeEvent::Body {
                    record_id,
                    bytes: None,
                    reason: Some(short_reason(&error.to_string())),
                });
            }
        }
    });
}

fn handle_loading_failed(
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    active: &mut HashMap<(SessionId, String), ActiveRecord>,
    event: browseros_cdp::CdpEvent,
) {
    let session_id = event.session_id.clone();
    let Ok(params) = serde_json::from_value::<LoadingFailed>(event.params) else {
        return;
    };
    let cors = params.cors_error_status.as_ref().map(|value| {
        value
            .get("corsError")
            .and_then(Value::as_str)
            .unwrap_or("cors_error")
            .to_string()
    });
    let failure = cors.clone().unwrap_or_else(|| params.error_text.clone());
    update_active(
        active,
        session_id.as_ref(),
        &params.request_id,
        |record| {
            record.state = RecordState::Failed;
            record.finished_at_ms = Some(now_ms());
            record.failure = Some(failure);
            record.body_state = BodyState::Unavailable;
            record.body_reason = Some("request_failed".to_string());
            if cors.is_some()
                && let Some(mock) = &mut record.mock
            {
                mock.outcome = "blocked".to_string();
                mock.reason = cors.clone();
            }
        },
        events,
    );
}

fn handle_paused(
    shared: &Arc<Shared>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    view: &SessionView,
    event: browseros_cdp::CdpEvent,
) {
    if is_response_stage(&event.params) {
        handle_response_stage(shared, events, view, event);
    } else {
        handle_request_stage(shared, events, view, event);
    }
}

fn is_response_stage(params: &serde_json::Value) -> bool {
    params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some()
}

fn handle_request_stage(
    shared: &Arc<Shared>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    view: &SessionView,
    event: browseros_cdp::CdpEvent,
) {
    let request_id = event
        .params
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request = event.params.get("request").cloned().unwrap_or(Value::Null);
    let url = request
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_string();
    let post_data = request
        .get("postData")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let request_headers = headers_from_value(request.get("headers").unwrap_or(&Value::Null));
    let resource_type = event
        .params
        .get("resourceType")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let network_id = event
        .params
        .get("networkId")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    // Read the release token first: if the snapshot below is replaced by a
    // newer SetRules, this token is cancelled and the check before fulfill
    // stops us from acting on a deleted rule.
    let release = shared.release();
    let rules = shared.rules();
    let protocol = view.protocol.clone();
    let events = events.clone();
    let tab_id = view.tab_id;
    let tab_title = Some(view.title.clone());
    let started = Instant::now();
    let shared = Arc::clone(shared);
    let inflight = shared.enter_paused();

    tokio::spawn(async move {
        let _inflight = inflight;
        let terminal = Arc::new(AtomicBool::new(false));
        let winner = pick_rule(
            &rules,
            tab_id,
            &method,
            &url,
            post_data.as_deref(),
            resource_type.as_deref(),
        );
        let Some(rule) = winner else {
            let _ = protocol
                .send_value("Fetch.continueRequest", json!({ "requestId": request_id }))
                .await;
            return;
        };
        let RuntimeAction::Static {
            response,
            body,
            sample_headers,
        } = &rule.action
        else {
            // Processor rules let the request through; the response stage
            // rewrites the upstream response.
            let _ = protocol
                .send_value("Fetch.continueRequest", json!({ "requestId": request_id }))
                .await;
            return;
        };
        let plan: FulfillPlan =
            build_fulfill(response, body, sample_headers, &url, &request_headers);
        let decision_ms = started.elapsed().as_millis() as u64;
        shared.record_decision(decision_ms);
        if decision_ms > 100 {
            if !terminal.swap(true, Ordering::SeqCst) {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    protocol
                        .send_value("Fetch.continueRequest", json!({ "requestId": request_id })),
                )
                .await;
            }
            let _ = events.send(RuntimeEvent::Hit(HitRecord {
                rule_id: rule.id.clone(),
                revision: rule.revision,
                at_ms: now_ms(),
                tab_title,
                url,
                method,
                outcome: "decision_timeout".to_string(),
                reason: None,
                network_id,
            }));
            return;
        }
        if plan.delay_ms > 0 {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(plan.delay_ms)) => {}
                () = release.cancelled() => {
                    if terminal.swap(true, Ordering::SeqCst) {
                        return;
                    }
                    let _ = tokio::time::timeout(
                        Duration::from_secs(1),
                        protocol.send_value(
                            "Fetch.continueRequest",
                            json!({ "requestId": request_id }),
                        ),
                    )
                    .await;
                    let _ = events.send(RuntimeEvent::Hit(HitRecord {
                        rule_id: rule.id.clone(),
                        revision: rule.revision,
                        at_ms: now_ms(),
                        tab_title,
                        url,
                        method,
                        outcome: "delay_cancelled".to_string(),
                        reason: None,
                        network_id,
                    }));
                    return;
                }
            }
        }
        if release.is_cancelled() {
            if !terminal.swap(true, Ordering::SeqCst) {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    protocol
                        .send_value("Fetch.continueRequest", json!({ "requestId": request_id })),
                )
                .await;
            }
            return;
        }
        if terminal.swap(true, Ordering::SeqCst) {
            return;
        }
        let params = plan.to_params(&request_id);
        let fulfilled_hit = HitRecord {
            rule_id: rule.id.clone(),
            revision: rule.revision,
            at_ms: now_ms(),
            tab_title: tab_title.clone(),
            url: url.clone(),
            method: method.clone(),
            outcome: "fulfilled".to_string(),
            reason: None,
            network_id: network_id.clone(),
        };
        if let Some(network_id) = &network_id
            && let Ok(mut pending) = shared.pending_hits.lock()
        {
            if pending.len() >= 1_000 {
                pending.clear();
            }
            pending.insert(network_id.clone(), fulfilled_hit.clone());
        }
        match tokio::time::timeout(
            Duration::from_secs(1),
            protocol.send_value("Fetch.fulfillRequest", params),
        )
        .await
        {
            Ok(Ok(_)) => {
                let _ = events.send(RuntimeEvent::Hit(fulfilled_hit));
            }
            Ok(Err(error)) => {
                // Never leave the page hanging on a fulfill error: release the
                // paused request so it can reach the origin.
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    protocol
                        .send_value("Fetch.continueRequest", json!({ "requestId": request_id })),
                )
                .await;
                let _ = events.send(RuntimeEvent::Hit(HitRecord {
                    rule_id: rule.id.clone(),
                    revision: rule.revision,
                    at_ms: now_ms(),
                    tab_title,
                    url,
                    method,
                    outcome: "failed".to_string(),
                    reason: Some(short_reason(&error.to_string())),
                    network_id,
                }));
            }
            Err(_) => {
                // The command timeout is not safe to retry. Best effort to
                // release the request and record an unknown outcome.
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    protocol
                        .send_value("Fetch.continueRequest", json!({ "requestId": request_id })),
                )
                .await;
                let _ = events.send(RuntimeEvent::Hit(HitRecord {
                    rule_id: rule.id.clone(),
                    revision: rule.revision,
                    at_ms: now_ms(),
                    tab_title,
                    url,
                    method,
                    outcome: "unknown".to_string(),
                    reason: Some("send_timeout".to_string()),
                    network_id,
                }));
            }
        }
    });
}

fn handle_response_stage(
    shared: &Arc<Shared>,
    events: &mpsc::UnboundedSender<RuntimeEvent>,
    view: &SessionView,
    event: browseros_cdp::CdpEvent,
) {
    let params = event.params;
    let request_id = params
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request = params.get("request").cloned().unwrap_or(Value::Null);
    let url = request
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_string();
    let request_headers = headers_from_value(request.get("headers").unwrap_or(&Value::Null));
    let post_data = request
        .get("postData")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let resource_type = params
        .get("resourceType")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let status = params
        .get("responseStatusCode")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let status_text = params
        .get("responseStatusText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let response_headers =
        header_pairs_from_value(params.get("responseHeaders").unwrap_or(&Value::Null));
    let network_id = params
        .get("networkId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let failed = params.get("responseErrorReason").is_some();

    let protocol = view.protocol.clone();
    let events = events.clone();
    let shared = Arc::clone(shared);
    let session = view.session_id.clone();
    let tab_id = view.tab_id;
    let tab_title = Some(view.title.clone());

    tokio::spawn(async move {
        if failed {
            continue_request(&protocol, &request_id).await;
            return;
        }
        let rules = shared.rules();
        let Some(rule) = pick_processor_rule(
            &rules,
            tab_id,
            &method,
            &url,
            post_data.as_deref(),
            resource_type.as_deref(),
        ) else {
            continue_request(&protocol, &request_id).await;
            return;
        };
        if let Some(reason) = response_skip_reason(status, &method, &response_headers) {
            continue_request(&protocol, &request_id).await;
            let _ = events.send(processor_hit(
                rule,
                "processor_skipped",
                reason,
                &url,
                &method,
                tab_title,
                network_id,
            ));
            return;
        }

        let _guard = shared.enter_processor(&session);
        let body = match tokio::time::timeout(
            Duration::from_secs(10),
            protocol.send_value(
                "Fetch.getResponseBody",
                json!({ "requestId": request_id.clone() }),
            ),
        )
        .await
        {
            Ok(Ok(value)) => decode_get_response_body(&value),
            Ok(Err(error)) => {
                continue_request(&protocol, &request_id).await;
                let _ = events.send(processor_hit(
                    rule,
                    "processor_skipped",
                    "body_unavailable",
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
                debug!(error = %error, "processor body read failed");
                return;
            }
            Err(_) => {
                continue_request(&protocol, &request_id).await;
                let _ = events.send(processor_hit(
                    rule,
                    "processor_skipped",
                    "body_timeout",
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
                return;
            }
        };
        if body.len() as u64 > MAX_BODY_BYTES {
            continue_request(&protocol, &request_id).await;
            let _ = events.send(processor_hit(
                rule,
                "processor_skipped",
                "body_too_large",
                &url,
                &method,
                tab_title,
                network_id,
            ));
            return;
        }

        let RuntimeAction::Processor { spec } = &rule.action else {
            continue_request(&protocol, &request_id).await;
            return;
        };
        let hook_protocol = protocol.clone();
        let hook_request = request_id.clone();
        let hooks = ProcessorHooks {
            upstream_cookies: Box::new(move || upstream_cookies(&hook_protocol, &hook_request)),
        };
        let input = ProcessorInput {
            method: method.clone(),
            url: url.clone(),
            request_headers,
            request_body: post_data,
            status,
            status_text,
            response_headers: response_headers.clone(),
            body: body.clone(),
            cookies_json: "[]".to_string(),
        };
        let started = Instant::now();
        let outcome = shared.processor.run(spec, input, hooks).await;
        shared.record_processor(started.elapsed().as_millis() as u64);

        let release = shared.release();
        if release.is_cancelled() {
            continue_request(&protocol, &request_id).await;
            let _ = events.send(processor_hit(
                rule,
                "processor_skipped",
                "rule_changed",
                &url,
                &method,
                tab_title,
                network_id,
            ));
            return;
        }

        let plan = match outcome {
            Ok(run) => match run.result {
                ProcessorResult::Passthrough => {
                    continue_request(&protocol, &request_id).await;
                    let _ = events.send(processor_hit(
                        rule,
                        "passthrough",
                        "",
                        &url,
                        &method,
                        tab_title,
                        network_id,
                    ));
                    return;
                }
                ProcessorResult::Replace { .. } => {
                    build_processed(status, &response_headers, &body, &run.result)
                }
            },
            Err(error) => {
                continue_request(&protocol, &request_id).await;
                let (outcome_label, reason) = match error.kind {
                    ProcessorErrorKind::Timeout => ("processor_timeout", error.message),
                    ProcessorErrorKind::Busy => ("processor_skipped", "busy".to_string()),
                    _ => ("processor_error", error.message),
                };
                let _ = events.send(processor_hit(
                    rule,
                    outcome_label,
                    &short_reason(&reason),
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
                return;
            }
        };

        let params = plan.to_params(&request_id);
        match tokio::time::timeout(
            Duration::from_secs(1),
            protocol.send_value("Fetch.fulfillRequest", params),
        )
        .await
        {
            Ok(Ok(_)) => {
                let _ = events.send(processor_hit(
                    rule,
                    "processed",
                    "",
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
            }
            Ok(Err(error)) => {
                continue_request(&protocol, &request_id).await;
                let _ = events.send(processor_hit(
                    rule,
                    "failed",
                    &short_reason(&error.to_string()),
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
            }
            Err(_) => {
                continue_request(&protocol, &request_id).await;
                let _ = events.send(processor_hit(
                    rule,
                    "unknown",
                    "send_timeout",
                    &url,
                    &method,
                    tab_title,
                    network_id,
                ));
            }
        }
    });
}

async fn continue_request(protocol: &ProtocolSession, request_id: &str) {
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        protocol.send_value("Fetch.continueRequest", json!({ "requestId": request_id })),
    )
    .await;
}

/// Every failure path releases the paused response exactly once.
fn processor_hit(
    rule: &RuntimeRule,
    outcome: &str,
    reason: &str,
    url: &str,
    method: &str,
    tab_title: Option<String>,
    network_id: Option<String>,
) -> RuntimeEvent {
    RuntimeEvent::Hit(HitRecord {
        rule_id: rule.id.clone(),
        revision: rule.revision,
        at_ms: now_ms(),
        tab_title,
        url: url.to_string(),
        method: method.to_string(),
        outcome: outcome.to_string(),
        reason: (!reason.is_empty()).then(|| reason.to_string()),
        network_id,
    })
}

struct ProcessorGuard {
    shared: Arc<Shared>,
    session: SessionId,
}

impl Drop for ProcessorGuard {
    fn drop(&mut self) {
        self.shared.leave_processor(&self.session);
    }
}

fn percentile_lock(times: &StdMutex<VecDeque<u64>>) -> u64 {
    let mut values = times
        .lock()
        .map(|times| times.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = ((values.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[index]
}

fn decode_get_response_body(value: &Value) -> Vec<u8> {
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value
        .get("base64Encoded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        super::response::base64_decode(body).unwrap_or_default()
    } else {
        body.as_bytes().to_vec()
    }
}

/// Reads the upstream cookies for this request. The sandbox calls this through
/// `response.cookies()`, so it stays lazy.
fn upstream_cookies(protocol: &ProtocolSession, request_id: &str) -> Result<String, String> {
    let handle = tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?;
    let response = handle
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(2),
                protocol.send_value("Fetch.getCookies", json!({ "requestId": request_id })),
            )
            .await
        })
        .map_err(|_| "cookie read timed out".to_string())?
        .map_err(|error| error.to_string())?;
    let cookies = response
        .get("cookies")
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::to_string(&cookies).map_err(|error| error.to_string())
}

/// The responses a processor never sees (design §4.4).
fn response_skip_reason(status: i64, method: &str, headers: &[HeaderPair]) -> Option<&'static str> {
    let header = |name: &str| {
        headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    };
    if matches!(status, 301 | 302 | 303 | 307 | 308) && header("location").is_some() {
        return Some("redirect");
    }
    if method.eq_ignore_ascii_case("HEAD") || matches!(status, 204 | 304) {
        return Some("no_body");
    }
    if let Some(content_type) = header("content-type") {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.starts_with("text/event-stream")
            || content_type.starts_with("multipart/x-mixed-replace")
        {
            return Some("streaming");
        }
    }
    if let Some(length) = header("content-length").and_then(|value| value.parse::<u64>().ok())
        && length > MAX_BODY_BYTES
    {
        return Some("body_too_large");
    }
    None
}

fn pick_processor_rule<'a>(
    rules: &'a RuntimeRuleSet,
    tab_id: i64,
    method: &str,
    url: &str,
    post_data: Option<&str>,
    resource_type: Option<&str>,
) -> Option<&'a RuntimeRule> {
    pick_rule(rules, tab_id, method, url, post_data, resource_type)
        .filter(|rule| matches!(rule.action, RuntimeAction::Processor { .. }))
}

fn pick_rule<'a>(
    rules: &'a RuntimeRuleSet,
    tab_id: i64,
    method: &str,
    url: &str,
    post_data: Option<&str>,
    resource_type: Option<&str>,
) -> Option<&'a RuntimeRule> {
    let mut candidates: Vec<&RuntimeRule> = rules
        .rules
        .iter()
        .filter(|rule| rule.scope_tab.is_none() || rule.scope_tab == Some(tab_id))
        .filter(|rule| matches(&rule.matcher, method, url, post_data, resource_type).is_match())
        .collect();
    candidates.sort_by(|left, right| {
        right
            .strictness()
            .cmp(&left.strictness())
            .then_with(|| right.id.cmp(&left.id))
    });
    candidates.into_iter().next()
}

fn short_reason(message: &str) -> String {
    message.chars().take(200).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::network::HeaderPatch;
    use crate::network::matcher::QueryMode;

    fn rule(id: &str, matcher: EndpointMatcher) -> RuntimeRule {
        RuntimeRule {
            id: id.to_string(),
            revision: 1,
            scope_tab: None,
            matcher,
            action: RuntimeAction::Static {
                response: StaticResponse::new(200, HeaderPatch::new(), 0, 0),
                body: Arc::from(Vec::new()),
                sample_headers: Arc::from(Vec::new()),
            },
            updated_at: 0,
        }
    }

    #[test]
    fn patterns_escape_the_query_marker_and_append_a_wildcard() {
        let mut matcher = EndpointMatcher::from_url("get", "https://example.test/api/items?x=1");
        matcher.origin = "https://example.test".to_string();
        matcher.pathname = "/api/items".to_string();
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![rule("r1", matcher)],
        };
        assert_eq!(
            set.patterns()[0]["urlPattern"],
            Value::String("https://example.test/api/items*".to_string())
        );
    }

    #[test]
    fn patterns_use_response_stage_for_processor_rules() {
        let matcher = EndpointMatcher::from_url("GET", "https://example.test/api/items");
        let mut processor = rule("r2", matcher.clone());
        processor.action = RuntimeAction::Processor {
            spec: Arc::new(ProcessorSpec {
                script: "function process() {}".to_string(),
                timeout_ms: 200,
            }),
        };
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![rule("r1", matcher), processor],
        };
        let patterns = set.patterns();
        assert_eq!(patterns[0]["requestStage"], "Request");
        assert_eq!(patterns[1]["requestStage"], "Response");
    }

    #[test]
    fn response_skip_rules_table() {
        let header = |name: &str, value: &str| HeaderPair::new(name.to_string(), value.to_string());
        assert_eq!(
            response_skip_reason(302, "GET", &[header("location", "/next")]),
            Some("redirect")
        );
        assert_eq!(response_skip_reason(200, "HEAD", &[]), Some("no_body"));
        assert_eq!(response_skip_reason(304, "GET", &[]), Some("no_body"));
        assert_eq!(
            response_skip_reason(
                200,
                "GET",
                &[header("content-type", "text/event-stream; charset=utf-8")]
            ),
            Some("streaming")
        );
        assert_eq!(
            response_skip_reason(200, "GET", &[header("content-length", "3000000")]),
            Some("body_too_large")
        );
        assert_eq!(
            response_skip_reason(200, "GET", &[header("content-type", "application/json")]),
            None
        );
    }

    #[test]
    fn extra_sample_keys_are_not_needed_for_a_lenient_match() {
        let matcher = EndpointMatcher::from_url("GET", "https://example.test/items?category=books");
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![rule("r1", matcher)],
        };
        assert!(
            pick_rule(
                &set,
                1,
                "GET",
                "https://example.test/items?category=books&nonce=9",
                None,
                Some("XHR")
            )
            .is_some()
        );
        assert!(
            pick_rule(
                &set,
                1,
                "GET",
                "https://example.test/items?category=music",
                None,
                Some("XHR")
            )
            .is_none()
        );
    }

    #[test]
    fn newest_rule_wins_an_exact_strictness_tie() {
        let matcher = EndpointMatcher::from_url("GET", "https://example.test/items");
        let mut older = rule("a", matcher.clone());
        older.updated_at = 10;
        let mut newer = rule("b", matcher);
        newer.updated_at = 20;
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![older, newer],
        };
        let winner =
            pick_rule(&set, 1, "GET", "https://example.test/items", None, None).expect("winner");
        assert_eq!(winner.id, "b");
    }

    #[test]
    fn tab_scoped_rules_only_match_their_tab() {
        let matcher = EndpointMatcher::from_url("GET", "https://example.test/items");
        let mut scoped = rule("r1", matcher);
        scoped.scope_tab = Some(7);
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![scoped],
        };
        assert!(pick_rule(&set, 7, "GET", "https://example.test/items", None, None).is_some());
        assert!(pick_rule(&set, 8, "GET", "https://example.test/items", None, None).is_none());
    }

    #[test]
    fn stricter_rules_win_when_both_match() {
        let mut lenient = EndpointMatcher::from_url("GET", "https://example.test/items");
        lenient.query_policy.mode = QueryMode::Lenient;
        let mut strict = lenient.clone();
        strict.body_fields.push(crate::network::BodyField {
            pointer: "/a".to_string(),
            equals: Value::String("b".to_string()),
        });
        let set = RuntimeRuleSet {
            generation: 1,
            rules: vec![rule("lenient", lenient), rule("strict", strict)],
        };
        let winner = pick_rule(
            &set,
            1,
            "GET",
            "https://example.test/items",
            Some(r#"{"a":"b"}"#),
            None,
        )
        .expect("a winner");
        assert_eq!(winner.id, "strict");
    }

    #[test]
    fn runtime_rejects_older_generation() {
        let shared = Shared::default();
        let (accepted_ack, _accepted_receiver) = oneshot::channel();
        assert!(shared.apply_command_rules(
            Arc::new(RuntimeRuleSet {
                generation: 5,
                rules: Vec::new(),
            }),
            accepted_ack,
        ));

        let (ack, mut receiver) = oneshot::channel();
        let accepted = shared.apply_command_rules(
            Arc::new(RuntimeRuleSet {
                generation: 4,
                rules: Vec::new(),
            }),
            ack,
        );
        assert!(!accepted, "an older generation must not replace g5");
        let report = receiver.try_recv().expect("the stale ack is immediate");
        assert!(report.stale);
        assert_eq!(report.generation, 4);
        assert_eq!(
            shared.rules().generation,
            5,
            "the snapshot stays at the newest generation"
        );
        assert!(
            !shared.release().is_cancelled(),
            "a rejected snapshot must not release in-flight decisions"
        );
    }
}
