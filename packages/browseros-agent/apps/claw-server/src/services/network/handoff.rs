//! Freeze one captured request into a durable handoff snapshot.
//!
//! The sample is copied out of the capture buffer so eviction, `clear` or a
//! server restart cannot change what an AI was pointed at.

use crate::db::network::HandoffRow;
use browseros_core::network::{
    BodyState, EndpointMatcher, HeaderPair, NetworkRecord, parse_url_parts,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const HANDOFF_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// Keys that look like per-request noise. Matching is case-insensitive and by
/// whole name.
const NOISE_NAMES: &[&str] = &[
    "_",
    "_t",
    "t",
    "ts",
    "timestamp",
    "time",
    "nonce",
    "rand",
    "random",
    "r",
    "_r",
    "cb",
    "callback",
    "reqid",
    "requestid",
    "traceid",
    "sessionid",
    "_dc",
    "nocache",
    "v",
];

pub struct HandoffDraft {
    pub handoff: HandoffRow,
    pub response_body: Option<Vec<u8>>,
    pub clipboard: String,
}

/// Build the snapshot. `related` are other captured samples for the same
/// origin + pathname, newest first.
#[must_use]
pub fn build_handoff(
    handoff_id: String,
    record: &NetworkRecord,
    response_body: Option<Vec<u8>>,
    related: &[NetworkRecord],
    now_ms: i64,
) -> HandoffDraft {
    let matcher = EndpointMatcher::from_url(&record.method, &record.url);
    let parts = parse_url_parts(&record.url);
    let matcher_seed = json!({
        "method": matcher.method,
        "origin": matcher.origin,
        "pathname": matcher.pathname,
        "query": parts.query,
    });

    let suggested = suggested_ignore_keys(&parts.query, related);
    let related_json = related
        .iter()
        .take(20)
        .map(|sample| {
            let sample_parts = parse_url_parts(&sample.url);
            json!({
                "recordId": sample.record_id,
                "method": sample.method,
                "url": sample.url,
                "status": sample.status,
                "resourceType": sample.resource_type,
                "query": sample_parts.query,
                "queryDiff": query_diff(&parts.query, &sample_parts.query),
                "bodySummary": {
                    "state": sample.body_state,
                    "reason": sample.body_reason,
                    "mimeType": sample.mime_type,
                    "encodedDataLength": sample.encoded_data_length,
                },
                "fromCache": sample.from_cache,
                "fromServiceWorker": sample.from_service_worker,
            })
        })
        .collect::<Vec<_>>();

    let source_meta = json!({
        "recordId": record.record_id,
        "requestId": record.request_id,
        "tabTitle": record.tab_title,
        "tabId": record.tab_id,
        "method": record.method,
        "url": record.url,
        "status": record.status,
        "statusText": record.status_text,
        "mimeType": record.mime_type,
        "resourceType": record.resource_type,
        "startedAt": record.started_at_ms,
        "fromCache": record.from_cache,
        "fromServiceWorker": record.from_service_worker,
    });

    let (body_state, body_reason) =
        if response_body.is_some() {
            ("available", None)
        } else if record.body_state == BodyState::TooLarge {
            ("too_large", Some("too_large".to_string()))
        } else {
            (
                "unavailable",
                Some(record.body_reason.clone().unwrap_or_else(|| {
                    "response body is not available for this request".to_string()
                })),
            )
        };

    let host = parts
        .origin
        .split_once("://")
        .map(|(_, host)| host)
        .unwrap_or(&parts.origin);
    let clipboard = format!(
        "NEOB 网络请求 `{handoff_id}` · {} {}{} · {} · 用 NEOB MCP 的 network 工具读取这个请求，按我的要求做 mock",
        record.method,
        host,
        parts.pathname,
        record
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "-".to_string()),
    );

    HandoffDraft {
        handoff: HandoffRow {
            id: handoff_id,
            record_id: record.record_id.clone(),
            created_at: now_ms,
            expires_at: now_ms + HANDOFF_TTL_MS,
            source_meta,
            request_headers: record.request_headers.clone(),
            response_headers: record.response_headers.clone(),
            request_body: record.post_data.clone(),
            body_id: None,
            body_bytes: response_body
                .as_ref()
                .map(|bytes| bytes.len() as u64)
                .unwrap_or(0),
            body_state: body_state.to_string(),
            body_reason,
            matcher_seed,
            suggested_ignore: suggested,
            related: Value::Array(related_json),
            rule_id: None,
        },
        response_body,
        clipboard,
    }
}

/// Names come from a heuristic plus variation across samples.
#[must_use]
pub fn suggested_ignore_keys(
    sample_query: &[(String, String)],
    related: &[NetworkRecord],
) -> Value {
    let mut sample_values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in sample_query {
        sample_values
            .entry(key.clone())
            .or_default()
            .push(value.clone());
    }

    let mut varying: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in related.iter().take(20) {
        let parts = parse_url_parts(&record.url);
        let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, value) in &parts.query {
            seen.entry(key.clone()).or_default().push(value.clone());
        }
        for (key, values) in seen {
            varying.entry(key).or_default().extend(values);
        }
    }

    let mut output = Vec::new();
    for (key, values) in &sample_values {
        let by_name = is_noise_name(key);
        let first = values.first().cloned().unwrap_or_default();
        let varies = varying
            .get(key)
            .is_some_and(|values| values.iter().any(|value| value != &first));
        let suggested = by_name || varies;
        let reason = if by_name {
            "name"
        } else if varies {
            "varying"
        } else {
            "stable"
        };
        let confidence = if related.is_empty() {
            "low"
        } else if varies || by_name {
            "high"
        } else {
            "medium"
        };
        output.push(json!({
            "key": key,
            "reason": reason,
            "suggested": suggested,
            "confidence": confidence,
        }));
    }

    // A key present only in other samples is still useful evidence.
    for (key, values) in varying {
        if sample_values.contains_key(&key) {
            continue;
        }
        output.push(json!({
            "key": key,
            "reason": "varying",
            "suggested": true,
            "confidence": if related.is_empty() { "low" } else { "high" },
            "sampleValues": values,
        }));
    }

    Value::Array(output)
}

fn query_diff(sample: &[(String, String)], other: &[(String, String)]) -> Value {
    let sample_map: BTreeMap<&str, Vec<&str>> = {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, value) in sample {
            map.entry(key.as_str()).or_default().push(value.as_str());
        }
        map
    };
    let other_map: BTreeMap<&str, Vec<&str>> = {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, value) in other {
            map.entry(key.as_str()).or_default().push(value.as_str());
        }
        map
    };
    let mut diff = serde_json::Map::new();
    for key in sample_map.keys().chain(other_map.keys()) {
        let before = sample_map.get(key).cloned().unwrap_or_default();
        let after = other_map.get(key).cloned().unwrap_or_default();
        if before != after {
            diff.insert(
                (*key).to_string(),
                json!({ "sample": before, "related": after }),
            );
        }
    }
    Value::Object(diff)
}

fn is_noise_name(key: &str) -> bool {
    NOISE_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(key))
}

/// Headers as the AI sees them: original order, duplicates preserved.
#[must_use]
pub fn header_lines(headers: &[HeaderPair]) -> String {
    headers
        .iter()
        .map(|header| format!("{}: {}", header.name, header.value))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use browseros_core::network::{BodyState, RecordState};

    fn record(id: &str, url: &str, status: Option<i64>) -> NetworkRecord {
        NetworkRecord {
            record_id: id.to_string(),
            request_id: id.to_string(),
            method: "GET".to_string(),
            url: url.to_string(),
            status,
            state: RecordState::Done,
            body_state: BodyState::Available,
            ..NetworkRecord::default()
        }
    }

    #[test]
    fn noise_names_are_suggested_and_stable_keys_are_listed_as_stable() {
        let sample = record(
            "r1",
            "https://example.test/items?category=books&nonce=1&_t=2",
            Some(200),
        );
        let related = vec![record(
            "r2",
            "https://example.test/items?category=books&nonce=9&_t=3",
            Some(200),
        )];
        let draft = build_handoff(
            "netref_1".to_string(),
            &sample,
            Some(b"{}".to_vec()),
            &related,
            0,
        );
        let suggested = draft.handoff.suggested_ignore.as_array().expect("array");
        let by_key = |key: &str| {
            suggested
                .iter()
                .find(|entry| entry["key"] == key)
                .expect("entry")
        };
        assert_eq!(by_key("nonce")["suggested"], true);
        assert_eq!(by_key("_t")["suggested"], true);
        assert_eq!(by_key("category")["suggested"], false);
        assert_eq!(by_key("category")["reason"], "stable");
    }

    #[test]
    fn matcher_seed_keeps_query_pairs_and_absolute_origin() {
        let sample = record(
            "r1",
            "https://Example.test:443/items?category=books&category=music",
            Some(200),
        );
        let draft = build_handoff("netref_1".to_string(), &sample, None, &[], 1000);
        assert_eq!(draft.handoff.matcher_seed["origin"], "https://example.test");
        assert_eq!(draft.handoff.matcher_seed["pathname"], "/items");
        assert_eq!(draft.handoff.matcher_seed["query"][0][0], "category");
        assert_eq!(draft.handoff.body_state, "unavailable");
        assert_eq!(draft.handoff.expires_at, 1000 + HANDOFF_TTL_MS);
        assert!(
            draft
                .clipboard
                .starts_with("NEOB 网络请求 `netref_1` · GET example.test/items")
        );
    }
}
