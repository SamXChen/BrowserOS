use serde::{Deserialize, Serialize};

/// One redirect hop is one record. `created` is the very first event, then the
/// record gains response data; `failed` is terminal without a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    #[default]
    Created,
    HeadersReady,
    ResponseReady,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyState {
    #[default]
    Pending,
    Available,
    Unavailable,
    TooLarge,
    Evicted,
}

/// Mark left on a captured record when the runtime fulfilled it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockMark {
    pub rule_id: String,
    pub revision: u32,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Header names keep their original case and duplicates are preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

impl HeaderPair {
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// The wire shape the GUI and the MCP tool both read. Field names are camelCase
/// because this is serialized straight into REST/SSE payloads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRecord {
    pub record_id: String,
    pub seq: u64,
    pub request_id: String,
    pub redirect_ordinal: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    pub method: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers: Vec<HeaderPair>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<HeaderPair>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoded_data_length: Option<u64>,
    pub started_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_cache: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_service_worker: bool,
    pub state: RecordState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub body_state: BodyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockMark>,
}

impl NetworkRecord {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, RecordState::Done | RecordState::Failed)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn records_serialize_to_camel_case_and_omit_absent_fields() {
        let record = NetworkRecord {
            record_id: "netrec_1".to_string(),
            seq: 7,
            method: "GET".to_string(),
            url: "https://example.test/api".to_string(),
            state: RecordState::HeadersReady,
            ..NetworkRecord::default()
        };
        let value = serde_json::to_value(&record).expect("serialize");
        assert_eq!(value["recordId"], "netrec_1");
        assert_eq!(value["bodyState"], "pending");
        assert!(value.get("tabId").is_none());
        assert!(value.get("mock").is_none());
    }
}
