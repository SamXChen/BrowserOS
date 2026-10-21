//! The network DTOs must stay in step with contracts/claw-api.
//!
//! `schemas/network.yaml` is parsed directly, so adding a required field to the
//! contract without adding it to `services/network/dto.rs` fails here.
#![allow(clippy::expect_used)]

use browseros_core::network::NetworkRecord;
use claw_server::services::network::dto::{
    NetworkBodyPageView, NetworkHitView, NetworkNearMissView, NetworkRecordListView,
    NetworkRevisionListView, NetworkRuleTestResult, NetworkRuleTestSampleView, NetworkRuleView,
    NetworkStatusView,
};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn schema(name: &str) -> Value {
    // CARGO_MANIFEST_DIR is <workspace>/apps/claw-server, and the contracts
    // live at the workspace root.
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/claw-api/schemas/network.yaml"
    ));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("network schema at {}: {error}", path.display()));
    let schemas: Value = serde_yaml::from_str(&text).expect("yaml");
    schemas.get(name).cloned().unwrap_or(Value::Null)
}

fn required(name: &str) -> Vec<String> {
    schema(name)
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(|key| key.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn keys(name: &str) -> Vec<String> {
    schema(name)
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn rule_view_covers_every_contract_field() {
    let required = required("NetworkRule");
    let view = NetworkRuleView {
        id: "rule_1".to_string(),
        name: "商品列表-空数据".to_string(),
        scope: "tab".to_string(),
        scope_tab: Some(1),
        enabled: true,
        state: "active".to_string(),
        state_reason: Some("no_tab".to_string()),
        current_revision: 1,
        hit_count: 0,
        author_client: Some("claude-code".to_string()),
        created_at: 1,
        updated_at: 2,
        action: None,
        matcher: None,
        response_status: Some(200),
        last_hit: None,
    };
    let value = serde_json::to_value(&view).expect("serialize");
    let present = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for field in &required {
        assert!(present.contains(field), "NetworkRule is missing {field}");
    }
    // The view must not invent fields the contract does not describe.
    for field in present {
        assert!(
            keys("NetworkRule").contains(&field),
            "unexpected field {field}"
        );
    }
}

#[test]
fn hit_view_covers_every_contract_field() {
    let required = required("NetworkHit");
    let view = NetworkHitView {
        id: "hit_1".to_string(),
        rule_id: "rule_1".to_string(),
        revision: 1,
        at: 1,
        tab_title: None,
        url: "https://example.test/api".to_string(),
        method: "GET".to_string(),
        outcome: "fulfilled".to_string(),
        reason: None,
        network_id: Some("1.2".to_string()),
    };
    let value = serde_json::to_value(&view).expect("serialize");
    let present = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for field in required {
        assert!(present.contains(&field), "NetworkHit is missing {field}");
    }
}

#[test]
fn body_page_view_covers_every_contract_field() {
    let view = NetworkBodyPageView::new("utf8", 0, 12, false, "{}".to_string())
        .with_length(2)
        .with_record("available", Some("application/json"));
    let value = serde_json::to_value(&view).expect("serialize");
    let present = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for field in required("NetworkBodyPage") {
        assert!(
            present.contains(&field),
            "NetworkBodyPage is missing {field}"
        );
    }
    for field in present {
        assert!(
            keys("NetworkBodyPage").contains(&field),
            "unexpected field {field}"
        );
    }
}

#[test]
fn near_miss_view_covers_every_contract_field() {
    let view = NetworkNearMissView {
        reason: "query_extra_key".to_string(),
        record_id: Some("netrec_1".to_string()),
        method: Some("GET".to_string()),
        url: Some("https://example.test/api".to_string()),
        tab_title: Some("tab".to_string()),
        blocked_by: Some("cors".to_string()),
    };
    let value = serde_json::to_value(&view).expect("serialize");
    let present = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for field in required("NetworkNearMiss") {
        assert!(
            present.contains(&field),
            "NetworkNearMiss is missing {field}"
        );
    }
    for field in present {
        assert!(
            keys("NetworkNearMiss").contains(&field),
            "unexpected field {field}"
        );
    }
}

#[test]
fn rule_test_result_covers_every_contract_field() {
    let result = NetworkRuleTestResult {
        matched: 1,
        considered: 3,
        samples: vec![NetworkRuleTestSampleView {
            record_id: "netrec_1".to_string(),
            method: "GET".to_string(),
            url: "https://example.test/api".to_string(),
            status: Some(200),
            tab_title: Some("tab".to_string()),
            at: Some(1),
        }],
    };
    let value = serde_json::to_value(&result).expect("serialize");
    for field in required("NetworkRuleTestResult") {
        assert!(
            value.get(&field).is_some(),
            "NetworkRuleTestResult is missing {field}"
        );
    }
    let sample = value["samples"][0].as_object().expect("sample object");
    for field in required("NetworkTestSample") {
        assert!(
            sample.contains_key(&field),
            "NetworkTestSample is missing {field}"
        );
    }
    for field in sample.keys() {
        assert!(
            keys("NetworkTestSample").contains(field),
            "unexpected sample field {field}"
        );
    }
}

#[test]
fn list_wrappers_match_their_contract_shapes() {
    let revisions = serde_json::to_value(NetworkRevisionListView {
        revisions: Vec::new(),
    })
    .expect("serialize");
    assert!(revisions.get("revisions").is_some());

    let records = serde_json::to_value(NetworkRecordListView {
        records: vec![NetworkRecord::default()],
    })
    .expect("serialize");
    let record = records["records"][0].as_object().expect("record object");
    for field in required("NetworkRecord") {
        assert!(
            record.contains_key(&field),
            "NetworkRecord is missing {field}"
        );
    }
}

#[test]
fn status_view_uses_the_nested_contract_shapes() {
    assert!(!required("NetworkBrowserState").is_empty());
    assert!(!required("NetworkRulesState").is_empty());
    let status = NetworkStatusView {
        browser: claw_server::services::network::dto::NetworkBrowserView { connected: true },
        capture: Value::Null,
        rules: claw_server::services::network::dto::NetworkRulesStateView {
            active: 1,
            total: 2,
        },
        runtime: None,
    };
    let value = serde_json::to_value(&status).expect("serialize");
    for field in required("NetworkStatus") {
        assert!(
            value.get(&field).is_some(),
            "NetworkStatus is missing {field}"
        );
    }
}
