//! Typed views for the network REST and MCP payloads.
//!
//! Every shape here mirrors `contracts/claw-api/schemas/network.yaml`. The
//! repository layer owns rows; this module owns what leaves the process, so the
//! HTTP handlers and the MCP host share one definition instead of two hand
//! built JSON objects.

use crate::db::network::{HandoffRow, HitRow, RuleRevisionRow, RuleRow};
use browseros_core::network::{HeaderPair, NetworkRecord, QueryMode, RuleAction};
use serde::{Serialize, Serializer};
use serde_json::Value;

fn is_false(value: &bool) -> bool {
    !*value
}

/// `[{name, value}]`, the header shape the GUI table renders.
#[derive(Debug, Clone, Serialize)]
pub struct HeaderView {
    pub name: String,
    pub value: String,
}

impl From<&HeaderPair> for HeaderView {
    fn from(header: &HeaderPair) -> Self {
        Self {
            name: header.name.clone(),
            value: header.value.clone(),
        }
    }
}

/// Serializes headers the way `header_lines` did, so no call site changes.
pub fn header_views(headers: &[HeaderPair]) -> Vec<HeaderView> {
    headers.iter().map(HeaderView::from).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHitView {
    pub id: String,
    #[serde(rename = "ruleId")]
    pub rule_id: String,
    pub revision: u32,
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tabTitle")]
    pub tab_title: Option<String>,
    pub url: String,
    pub method: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "networkId")]
    pub network_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkMatcherView {
    pub method: String,
    pub origin: String,
    pub pathname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "exactUrl")]
    pub exact_url: Option<String>,
    #[serde(rename = "ignoreKeys")]
    pub ignore_keys: Vec<String>,
    #[serde(rename = "strictQuery")]
    pub strict_query: bool,
    #[serde(rename = "bodyFieldCount")]
    pub body_field_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "resourceType")]
    pub resource_type: Option<String>,
}

/// The stored action, without the body: bytes live in the body store, never
/// inline.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetworkActionView {
    Static {
        status: i64,
        headers: Vec<HeaderView>,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        body: NetworkBodyMetaView,
    },
    Processor {
        script: String,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NetworkBodyMetaView {
    pub bytes: u64,
    pub encoding: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleView {
    pub id: String,
    pub name: String,
    pub scope: String,
    #[serde(rename = "scopeTab")]
    pub scope_tab: Option<i64>,
    pub enabled: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "stateReason")]
    pub state_reason: Option<String>,
    #[serde(rename = "currentRevision")]
    pub current_revision: u32,
    #[serde(rename = "hitCount")]
    pub hit_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "authorClient")]
    pub author_client: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    /// Only populated for the list view; the detail view carries the matcher
    /// and the action on `revision` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<NetworkActionView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<NetworkMatcherView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "responseStatus")]
    pub response_status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastHit")]
    pub last_hit: Option<NetworkHitView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRevisionSourceView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "handoffId")]
    pub handoff_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recordId")]
    pub record_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tabId")]
    pub tab_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRevisionView {
    pub revision: u32,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    pub matcher: serde_json::Value,
    pub action: NetworkActionView,
    pub source: NetworkRevisionSourceView,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkSampleView {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleDetailView {
    pub rule: NetworkRuleView,
    pub revision: NetworkRevisionView,
    pub hits: Vec<NetworkHitView>,
    pub sample: NetworkSampleView,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRevisionListView {
    pub revisions: Vec<NetworkRevisionView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleListView {
    pub rules: Vec<NetworkRuleView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleUpdated {
    pub rule: NetworkRuleView,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHandoffView {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ruleId")]
    pub rule_id: Option<String>,
    pub source: Value,
    #[serde(rename = "bodyState")]
    pub body_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "bodyReason")]
    pub body_reason: Option<String>,
    #[serde(rename = "bodyBytes")]
    pub body_bytes: u64,
    #[serde(rename = "requestHeaders")]
    pub request_headers: Vec<HeaderView>,
    #[serde(rename = "responseHeaders")]
    pub response_headers: Vec<HeaderView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestBody")]
    pub request_body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHandoffListView {
    pub handoffs: Vec<NetworkHandoffView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHandoffDetail {
    pub handoff: NetworkHandoffView,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHandoffCreated {
    #[serde(rename = "handoffId")]
    pub handoff_id: String,
    #[serde(rename = "clipboardText")]
    pub clipboard_text: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
    pub reused: bool,
}

/// `DELETE` results that carry no undo: handoffs and hits.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkDeleted {
    pub deleted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "undoToken")]
    pub undo_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "undoUntil")]
    pub undo_until: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRecordsCleared {
    pub cleared: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRecordDeleted {
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "undoToken")]
    pub undo_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "undoUntil")]
    pub undo_until: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRestoreResult {
    pub restored: bool,
    #[serde(rename = "recordId")]
    pub record_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRulesDeleted {
    pub deleted: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHitsDeleted {
    pub deleted: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkStopAllResult {
    pub disabled: u64,
    pub narrowed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleTestSampleView {
    #[serde(rename = "recordId")]
    pub record_id: String,
    pub method: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tabTitle")]
    pub tab_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRuleTestResult {
    pub matched: usize,
    pub considered: usize,
    pub samples: Vec<NetworkRuleTestSampleView>,
}

/// A window of a body: record bodies, rule samples and handoff samples all use
/// this shape. `length` is the size of the returned window.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkBodyPageView {
    pub encoding: String,
    pub offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    pub total: u64,
    pub truncated: bool,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

impl NetworkBodyPageView {
    #[must_use]
    pub fn new(encoding: &str, offset: u64, total: u64, truncated: bool, data: String) -> Self {
        Self {
            encoding: encoding.to_string(),
            offset,
            length: None,
            total,
            truncated,
            data,
            state: None,
            mime_type: None,
        }
    }

    #[must_use]
    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }

    #[must_use]
    pub fn with_record(mut self, state: &str, mime_type: Option<&str>) -> Self {
        self.state = Some(state.to_string());
        self.mime_type = mime_type.map(ToString::to_string);
        self
    }
}

/// Why a recent request did not match, for the MCP `rule_status` diagnosis.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkNearMissView {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recordId")]
    pub record_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tabTitle")]
    pub tab_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "blockedBy")]
    pub blocked_by: Option<String>,
}

impl NetworkNearMissView {
    #[must_use]
    pub fn reason_only(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
            record_id: None,
            method: None,
            url: None,
            tab_title: None,
            blocked_by: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRecordListView {
    pub records: Vec<NetworkRecord>,
}

/// Hand-serialized so `runtime` can be omitted entirely when the runtime is
/// not attached, without an `Option` wrapper on the wire.
#[derive(Debug, Clone)]
pub struct NetworkStatusView {
    pub browser: NetworkBrowserView,
    pub capture: Value,
    pub rules: NetworkRulesStateView,
    pub runtime: Option<NetworkRuntimeView>,
}

impl Serialize for NetworkStatusView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("browser", &self.browser)?;
        map.serialize_entry("capture", &self.capture)?;
        map.serialize_entry("rules", &self.rules)?;
        if let Some(runtime) = &self.runtime {
            map.serialize_entry("runtime", &runtime)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkBrowserView {
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRulesStateView {
    pub active: u64,
    pub total: u64,
}

/// Hand-serialized so a caller can omit the processor fields when the runtime
/// does not report them.
#[derive(Debug, Clone, Copy)]
pub struct NetworkRuntimeView {
    pub paused: usize,
    pub decision_p95_ms: u64,
    pub rules_generation: u64,
    pub processor_p95_ms: Option<u64>,
    pub processor_inflight: Option<usize>,
}

impl Serialize for NetworkRuntimeView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("paused", &self.paused)?;
        map.serialize_entry("decisionP95Ms", &self.decision_p95_ms)?;
        map.serialize_entry("rulesGeneration", &self.rules_generation)?;
        if let Some(p95) = self.processor_p95_ms {
            map.serialize_entry("processorP95Ms", &p95)?;
        }
        if let Some(inflight) = self.processor_inflight {
            map.serialize_entry("processorInflight", &inflight)?;
        }
        map.end()
    }
}

impl NetworkRuntimeView {
    #[must_use]
    pub fn new(
        paused: usize,
        decision_p95_ms: u64,
        rules_generation: u64,
        processor_p95_ms: Option<u64>,
        processor_inflight: Option<usize>,
    ) -> Self {
        Self {
            paused,
            decision_p95_ms,
            rules_generation,
            processor_p95_ms,
            processor_inflight,
        }
    }
}

// ---- constructors ------------------------------------------------------

impl NetworkHitView {
    #[must_use]
    pub fn from_row(hit: &HitRow) -> Self {
        Self {
            id: hit.id.clone(),
            rule_id: hit.rule_id.clone(),
            revision: hit.revision,
            at: hit.at_ms,
            tab_title: hit.tab_title.clone(),
            url: hit.url.clone(),
            method: hit.method.clone(),
            outcome: hit.outcome.clone(),
            reason: hit.reason.clone(),
            network_id: hit.network_id.clone(),
        }
    }
}

impl NetworkRuleView {
    /// List view: matcher and action are summarized inline.
    #[must_use]
    pub fn summary(rule: &RuleRow, revision: Option<&RuleRevisionRow>, hits: &[HitRow]) -> Self {
        let action = revision.map(|revision| action_view(&revision.action));
        let matcher = revision.map(|revision| NetworkMatcherView {
            method: revision.matcher.method.clone(),
            origin: revision.matcher.origin.clone(),
            pathname: revision.matcher.pathname.clone(),
            exact_url: revision.matcher.exact_url.clone(),
            ignore_keys: revision.matcher.query_policy.ignore_keys.clone(),
            strict_query: matches!(revision.matcher.query_policy.mode, QueryMode::Strict),
            body_field_count: revision.matcher.body_fields.len(),
            resource_type: revision.matcher.resource_type.clone(),
        });
        let response_status = match revision.map(|revision| &revision.action) {
            Some(RuleAction::Static(response)) => Some(response.status),
            _ => None,
        };
        Self {
            id: rule.id.clone(),
            name: rule.name.clone(),
            scope: rule.scope_kind.as_str().to_string(),
            scope_tab: rule.scope_tab,
            enabled: rule.enabled,
            state: rule.state.as_str().to_string(),
            state_reason: rule.state_reason.as_ref().map(|reason| reason.as_str()),
            current_revision: rule.current_revision,
            hit_count: rule.hit_count,
            author_client: rule.author_client.clone(),
            created_at: rule.created_at,
            updated_at: rule.updated_at,
            action,
            matcher,
            response_status,
            last_hit: hits
                .iter()
                .find(|hit| hit.rule_id == rule.id)
                .map(NetworkHitView::from_row),
        }
    }

    /// Detail view: matcher and action live on `revision`, so the rule itself
    /// carries only its own fields.
    #[must_use]
    pub fn detail(rule: &RuleRow, revision: &RuleRevisionRow, hits: &[HitRow]) -> Self {
        let mut view = Self::summary(rule, None, hits);
        view.response_status = match &revision.action {
            RuleAction::Static(response) => Some(response.status),
            RuleAction::Processor(_) => None,
        };
        view
    }
}

impl NetworkRevisionView {
    #[must_use]
    pub fn from_row(revision: &RuleRevisionRow) -> Self {
        Self {
            revision: revision.revision,
            created_at: revision.created_at,
            matcher: serde_json::to_value(&revision.matcher).unwrap_or(Value::Null),
            action: action_view(&revision.action),
            source: NetworkRevisionSourceView {
                handoff_id: revision.source_handoff_id.clone(),
                record_id: revision.source_record_id.clone(),
                tab_id: revision.source_tab_id,
            },
        }
    }
}

/// Static actions show their header patch; processor actions show the script.
#[must_use]
pub fn action_view(action: &RuleAction) -> NetworkActionView {
    match action {
        RuleAction::Static(response) => NetworkActionView::Static {
            status: response.status,
            headers: header_views_from_patch(&response.headers),
            delay_ms: response.delay_ms,
            body: NetworkBodyMetaView {
                bytes: response.body.bytes,
                encoding: match response.body.encoding {
                    browseros_core::network::BodyEncoding::Utf8 => "utf8",
                    browseros_core::network::BodyEncoding::Binary => "binary",
                },
            },
        },
        RuleAction::Processor(spec) => NetworkActionView::Processor {
            script: spec.script.clone(),
            timeout_ms: spec.timeout_ms,
        },
    }
}

fn header_views_from_patch(patch: &browseros_core::network::HeaderPatch) -> Vec<HeaderView> {
    patch
        .iter()
        .filter_map(|(name, value)| {
            let value = value.as_ref()?;
            match value {
                browseros_core::network::OneOrMany::One(value) => Some(HeaderView {
                    name: name.clone(),
                    value: value.clone(),
                }),
                browseros_core::network::OneOrMany::Many(values) => Some(HeaderView {
                    name: name.clone(),
                    value: values.join(", "),
                }),
            }
        })
        .collect()
}

impl NetworkHandoffView {
    #[must_use]
    pub fn from_row(handoff: &HandoffRow) -> Self {
        Self {
            id: handoff.id.clone(),
            created_at: handoff.created_at,
            expires_at: handoff.expires_at,
            rule_id: handoff.rule_id.clone(),
            source: handoff.source_meta.clone(),
            body_state: handoff.body_state.clone(),
            body_reason: handoff.body_reason.clone(),
            body_bytes: handoff.body_bytes,
            request_headers: header_views(&handoff.request_headers),
            response_headers: header_views(&handoff.response_headers),
            request_body: handoff.request_body.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::db::network::ScopeKind;

    fn rule() -> RuleRow {
        RuleRow {
            id: "rule_1".to_string(),
            name: "商品列表-空数据".to_string(),
            scope_kind: ScopeKind::Tab,
            scope_tab: Some(7),
            enabled: true,
            state: crate::db::network::RuleState::Active,
            state_reason: None,
            current_revision: 2,
            hit_count: 3,
            last_hit_at: None,
            author_client: Some("claude-code".to_string()),
            created_at: 1,
            updated_at: 2,
            deleted_at: None,
        }
    }

    #[test]
    fn rule_view_uses_the_contract_field_names() {
        let view = NetworkRuleView::summary(&rule(), None, &[]);
        let value = serde_json::to_value(&view).expect("serialize");
        let keys = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for required in [
            "id",
            "name",
            "scope",
            "scopeTab",
            "enabled",
            "state",
            "currentRevision",
            "hitCount",
            "createdAt",
            "updatedAt",
            "authorClient",
        ] {
            assert!(keys.contains(&required.to_string()), "missing {required}");
        }
        assert_eq!(value["scope"], "tab");
        assert_eq!(value["state"], "active");
        // Absent optional fields are omitted, not nulled.
        assert!(value.get("lastHit").is_none());
        assert!(value.get("stateReason").is_none());
    }

    #[test]
    fn runtime_view_omits_absent_processor_fields() {
        let value =
            serde_json::to_value(NetworkRuntimeView::new(0, 4, 9, None, None)).expect("serialize");
        assert!(value.get("processorP95Ms").is_none());
        let value = serde_json::to_value(NetworkRuntimeView::new(0, 4, 9, Some(12), Some(1)))
            .expect("serialize");
        assert_eq!(value["processorP95Ms"], 12);
        assert_eq!(value["processorInflight"], 1);
    }

    #[test]
    fn status_omits_runtime_when_the_browser_is_detached() {
        let status = NetworkStatusView {
            browser: NetworkBrowserView { connected: false },
            capture: Value::Null,
            rules: NetworkRulesStateView {
                active: 0,
                total: 0,
            },
            runtime: None,
        };
        let value = serde_json::to_value(&status).expect("serialize");
        assert!(value.get("runtime").is_none());
    }
}
