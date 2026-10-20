//! Tolerant typed views over the `Network.*` events this feature consumes.
//!
//! Field names follow the protocol. Unknown fields are ignored and every optional
//! field defaults, so a protocol addition cannot take capture down.

use super::state::HeaderPair;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestWillBeSent {
    pub request_id: String,
    pub loader_id: Option<String>,
    pub document_url: Option<String>,
    pub request: RequestPayload,
    pub timestamp: f64,
    pub wall_time: Option<f64>,
    pub initiator: Option<Value>,
    pub redirect_response: Option<ResponsePayload>,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    pub frame_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestPayload {
    pub url: String,
    pub method: String,
    pub headers: Value,
    pub post_data: Option<String>,
    pub has_post_data: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponsePayload {
    pub url: String,
    pub status: i64,
    pub status_text: String,
    pub headers: Value,
    pub headers_text: Option<String>,
    pub mime_type: String,
    pub charset: String,
    pub encoded_data_length: f64,
    pub from_disk_cache: Option<bool>,
    pub from_service_worker: Option<bool>,
    pub from_prefetch_cache: Option<bool>,
    pub timing: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponseReceived {
    pub request_id: String,
    pub timestamp: f64,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    pub response: ResponsePayload,
    pub has_extra_info: Option<bool>,
    pub frame_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoadingFinished {
    pub request_id: String,
    pub timestamp: f64,
    pub encoded_data_length: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoadingFailed {
    pub request_id: String,
    pub timestamp: f64,
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    pub error_text: String,
    pub canceled: Option<bool>,
    pub blocked_reason: Option<String>,
    pub cors_error_status: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestWillBeSentExtraInfo {
    pub request_id: String,
    pub headers: Value,
    pub associated_cookies: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponseReceivedExtraInfo {
    pub request_id: String,
    pub headers: Value,
    pub status_code: Option<i64>,
    pub blocked_cookies: Option<Value>,
}

/// `Network.requestWillBeSent` and friends deliver headers as an object. Merge
/// into the record's ordered pair list without duplicating names.
#[must_use]
pub fn headers_from_value(value: &Value) -> Vec<HeaderPair> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .map(|(name, value)| {
            let value = value
                .as_str()
                .map(ToString::to_string)
                .unwrap_or_else(|| value.to_string());
            HeaderPair::new(name.clone(), value)
        })
        .collect()
}

/// Extra info arrives after the first event and is authoritative for headers
/// such as `Cookie`. Replace the whole list when it carries any entries.
pub fn merge_headers(target: &mut Vec<HeaderPair>, extra: &Value) {
    let headers = headers_from_value(extra);
    if !headers.is_empty() {
        *target = headers;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn a_protocol_addition_does_not_break_parsing() {
        let event = serde_json::from_value::<RequestWillBeSent>(json!({
            "requestId": "r1",
            "request": {
                "url": "https://example.test/api/items?x=1",
                "method": "GET",
                "headers": {"accept": "application/json"},
                "futureField": {"nested": true}
            },
            "timestamp": 12.5,
            "type": "XHR",
            "wallTime": 1700000000.0,
            "unknownTopLevel": "ignored"
        }))
        .expect("tolerant parse");
        assert_eq!(event.request_id, "r1");
        assert_eq!(event.request.method, "GET");
        assert_eq!(event.resource_type.as_deref(), Some("XHR"));
        assert_eq!(headers_from_value(&event.request.headers).len(), 1);
    }

    #[test]
    fn extra_headers_replace_instead_of_append() {
        let mut current = vec![HeaderPair::new("accept", "*/*")];
        merge_headers(&mut current, &json!({"cookie": "a=1"}));
        assert_eq!(current, vec![HeaderPair::new("cookie", "a=1")]);
    }
}
