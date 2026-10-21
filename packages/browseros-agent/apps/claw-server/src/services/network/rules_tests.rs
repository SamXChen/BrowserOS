//! Rule lifecycle tests.
//!
//! The real `RuleEngine` runs against a temporary database and an injected
//! `RuntimePort`, so no browser is involved: the fake returns exactly the
//! `ApplyReport` the runtime would have produced.

#![allow(clippy::expect_used)]

use super::{apply::BodyInput, runtime_port::RuntimePort};
use crate::db::{
    Database,
    network::{HandoffRow, NetworkRepository, ScopeKind, StateReason},
};
use browseros_core::network::{
    ApplyReport, HitRecord, QueryMode, RuntimeRuleSet, RuntimeStats, TabSummary,
};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

/// Records every snapshot and answers with the report the test sets.
struct FakeRuntime {
    report: Mutex<ApplyReport>,
    applied: Mutex<Vec<Arc<RuntimeRuleSet>>>,
}

impl FakeRuntime {
    fn new(total: usize, acked: usize) -> Arc<Self> {
        Arc::new(Self {
            report: Mutex::new(report(total, acked)),
            applied: Mutex::new(Vec::new()),
        })
    }

    fn last(&self) -> Option<Arc<RuntimeRuleSet>> {
        self.applied.lock().expect("applied lock").last().cloned()
    }
}

impl RuntimePort for FakeRuntime {
    fn set_capture(&self, _running: bool) {}

    fn apply_rules(&self, rules: Arc<RuntimeRuleSet>) -> BoxFuture<'_, ApplyReport> {
        self.applied.lock().expect("applied lock").push(rules);
        let report = *self.report.lock().expect("report lock");
        Box::pin(async move { report })
    }

    fn stats(&self) -> RuntimeStats {
        RuntimeStats::default()
    }

    fn shutdown(&self) {}
}

fn report(total: usize, acked: usize) -> ApplyReport {
    ApplyReport {
        generation: 0,
        total,
        acked,
        fetch_enabled: acked,
        stale: false,
        timed_out: false,
    }
}

/// Blocks inside `apply_rules` until released, so the stop-all race can be
/// reproduced deterministically.
struct BlockingRuntime {
    gate: Arc<tokio::sync::Semaphore>,
    applied: Mutex<Vec<Arc<RuntimeRuleSet>>>,
}

impl BlockingRuntime {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            applied: Mutex::new(Vec::new()),
        })
    }

    /// Lets one in-flight `apply_rules` finish.
    /// Lets every parked `apply_rules` finish. Release *before* calling
    /// `stop_all`, which takes the same `rules_lock` and applies too.
    fn release(&self) {
        self.gate.add_permits(64);
    }

    fn last(&self) -> Option<Arc<RuntimeRuleSet>> {
        self.applied.lock().expect("applied lock").last().cloned()
    }
}

impl RuntimePort for BlockingRuntime {
    fn set_capture(&self, _running: bool) {}

    fn apply_rules(&self, rules: Arc<RuntimeRuleSet>) -> BoxFuture<'_, ApplyReport> {
        let gate = Arc::clone(&self.gate);
        self.applied.lock().expect("applied lock").push(rules);
        Box::pin(async move {
            let _permit = gate.acquire().await;
            report(1, 1)
        })
    }

    fn stats(&self) -> RuntimeStats {
        RuntimeStats::default()
    }

    fn shutdown(&self) {}
}

struct Harness {
    _directory: tempfile::TempDir,
    database: Database,
    service: Arc<super::service::NetworkService>,
    runtime: Arc<FakeRuntime>,
}

async fn harness(total: usize, acked: usize) -> anyhow::Result<Harness> {
    let directory = tempfile::tempdir()?;
    let database = Database::open(directory.path().join("browserclaw.sqlite")).await?;
    let runtime = FakeRuntime::new(total, acked);
    let service = super::service::NetworkService::for_test(
        &database,
        directory.path(),
        Some(Arc::clone(&runtime) as Arc<dyn RuntimePort>),
    )
    .await?;
    Ok(Harness {
        _directory: directory,
        database,
        service,
        runtime,
    })
}

/// Seeds a captured record plus a handoff, which is how every rule is created.
async fn seed(harness: &Harness, body: &str) -> anyhow::Result<String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let record_id = format!(
        "netrec_{}",
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    harness
        .service
        .insert_record_for_test(
            &record_id,
            body.as_bytes(),
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let created = harness.service.create_handoff(&record_id).await?;
    Ok(created["handoffId"]
        .as_str()
        .expect("handoff id")
        .to_string())
}

fn apply_request(handoff_id: &str, name: &str) -> super::ApplyRequest {
    super::ApplyRequest {
        handoff_id: Some(handoff_id.to_string()),
        name: Some(name.to_string()),
        ..serde_json::from_value(json!({})).expect("empty request")
    }
}

#[tokio::test]
async fn concurrent_disable_and_apply_converge() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let first = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&first, "规则A"))
        .await?;
    let second = seed(&harness, "{\"price\":20}").await?;

    let rules = Arc::clone(&harness.service.rules);
    let rule_a = applied.rule_id.clone();
    let disable = tokio::spawn({
        let rules = Arc::clone(&rules);
        let rule_a = rule_a.clone();
        async move { rules.set_enabled(&rule_a, false).await }
    });
    let created = harness
        .service
        .rules
        .apply(apply_request(&second, "规则B"))
        .await?;
    disable.await??;

    let stored = harness.service.rules.resolve_reference(&rule_a).await?;
    assert!(!stored.enabled, "the disable must not be lost");
    assert_eq!(stored.state_reason, Some(StateReason::User));
    let stored_b = harness
        .service
        .rules
        .resolve_reference(&created.rule_id)
        .await?;
    assert_eq!(stored_b.state.as_str(), "active", "the new rule is live");

    let snapshot = harness.runtime.last().expect("snapshot");
    let ids = snapshot
        .rules
        .iter()
        .map(|rule| rule.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![created.rule_id.clone()],
        "only the live rule ships"
    );
    Ok(())
}

#[tokio::test]
async fn test_rule_respects_tab_scope() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let mut tabs = Vec::new();
    for tab_id in [11_i64, 22_i64] {
        tabs.push(TabSummary {
            tab_id,
            page_id: tab_id as u32,
            title: format!("tab {tab_id}"),
            url: "https://example.test/".to_string(),
            attached: true,
        });
        harness
            .service
            .insert_record_for_test(
                &format!("netrec_tab_{tab_id}"),
                br#"{"price":1}"#,
                tab_id,
                "GET",
                "https://example.test/api/items?page=1",
                None,
            )
            .await?;
    }
    harness.service.rules.set_tabs(tabs).await;

    let handoff = seed(&harness, r#"{"name":"x","price":10}"#).await?;
    let mut request = apply_request(&handoff, "改价格");
    request.response = Some(super::apply::ResponseInput {
        status: 200,
        headers: Default::default(),
        body: BodyInput::text(r#"{"name":"x","price":10}"#),
        delay_ms: 0,
    });
    let applied = harness.service.rules.apply(request).await?;
    let all = harness.service.test_rule(&applied.rule_id).await?;
    let considered_all = all["considered"].as_u64().unwrap_or(0);

    harness
        .service
        .rules
        .set_scope(&applied.rule_id, ScopeKind::Tab, Some(22))
        .await?;
    let scoped = harness.service.test_rule(&applied.rule_id).await?;
    let considered_scoped = scoped["considered"].as_u64().unwrap_or(0);

    assert!(
        considered_scoped < considered_all,
        "the tab scope narrows the sample set: {considered_scoped} vs {considered_all}"
    );
    Ok(())
}

#[tokio::test]
async fn failed_body_missing_excluded_from_runtime() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "会丢body"))
        .await?;
    harness.service.bodies.remove_rule(&applied.rule_id).await?;

    harness.service.rules.rebuild_and_apply().await?;
    let stored = harness
        .service
        .rules
        .resolve_reference(&applied.rule_id)
        .await?;
    assert_eq!(stored.state.as_str(), "failed");
    assert_eq!(stored.state_reason, Some(StateReason::BodyMissing));
    let snapshot = harness.runtime.last().expect("snapshot");
    assert!(
        snapshot.rules.is_empty(),
        "a rule with a missing body must not be applied"
    );
    Ok(())
}

#[tokio::test]
async fn orphan_dirs_are_swept() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let rule_dir = harness
        .service
        .bodies
        .root()
        .join("rules")
        .join("rule_gone");
    let handoff_dir = harness
        .service
        .bodies
        .root()
        .join("handoffs")
        .join("netref_gone");
    tokio::fs::create_dir_all(&rule_dir).await?;
    tokio::fs::create_dir_all(&handoff_dir).await?;
    tokio::fs::write(rule_dir.join("1.bin"), b"{}").await?;
    tokio::fs::write(handoff_dir.join("body.bin"), b"{}").await?;
    backdate(&rule_dir.join("1.bin")).await?;
    backdate(&handoff_dir.join("body.bin")).await?;

    harness.service.rules.maintenance().await;
    assert!(!rule_dir.exists(), "the orphan rule dir is gone");
    assert!(!handoff_dir.exists(), "the orphan handoff dir is gone");
    Ok(())
}

/// Orphans younger than a minute are left alone on purpose, so the test ages
/// them with `touch` before asserting the sweep.
async fn backdate(path: &std::path::Path) -> anyhow::Result<()> {
    let stamp = "202001010000";
    for target in [path, path.parent().unwrap_or(path)] {
        let status = std::process::Command::new("touch")
            .args(["-t", stamp])
            .arg(target)
            .status()?;
        assert!(status.success(), "touch failed for {}", target.display());
    }
    Ok(())
}

#[tokio::test]
async fn handoff_copy_is_idempotent_per_record() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let record_id = "netrec_same";
    harness
        .service
        .insert_record_for_test(
            record_id,
            br#"{"price":10}"#,
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let first = harness.service.create_handoff(record_id).await?;
    let second = harness.service.create_handoff(record_id).await?;
    assert_eq!(first["handoffId"], second["handoffId"], "same reference");
    assert_eq!(second["reused"], json!(true));
    Ok(())
}

#[tokio::test]
async fn resolve_reference_not_found_lists_candidates() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    harness
        .service
        .rules
        .apply(apply_request(&handoff, "订单列表"))
        .await?;

    let error = harness
        .service
        .rules
        .resolve_reference("不存在的规则")
        .await
        .expect_err("unknown names are an error");
    let message = error.to_string();
    assert!(message.contains("rule_not_found"), "{message}");
    assert!(message.contains("订单列表"), "the message names candidates");
    Ok(())
}

#[tokio::test]
async fn default_name_suffixes_on_collision() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let first = {
        let handoff = seed(&harness, "{\"price\":10}").await?;
        harness
            .service
            .rules
            .apply(apply_request(&handoff, "GET /api/items"))
            .await?
    };
    // A second record on the same endpoint: same generated default, so it must
    // be de-duplicated rather than clash.
    let handoff = seed(&harness, "{\"price\":20}").await?;
    let second = harness
        .service
        .rules
        .apply(super::ApplyRequest {
            handoff_id: Some(handoff),
            ..serde_json::from_value(json!({})).expect("empty request")
        })
        .await?;

    let stored = harness
        .service
        .rules
        .resolve_reference(&second.rule_id)
        .await?;
    assert_eq!(
        stored.name, "GET /api/items #2",
        "the default name is de-duplicated"
    );
    assert_ne!(stored.id, first.rule_id);
    Ok(())
}

#[tokio::test]
async fn reserved_name_prefix_is_rejected() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let error = harness
        .service
        .rules
        .apply(super::ApplyRequest {
            handoff_id: Some(handoff),
            name: Some("rule_x".to_string()),
            ..serde_json::from_value(json!({})).expect("empty request")
        })
        .await
        .expect_err("rule_ is reserved for ids");
    assert!(error.to_string().contains("invalid_rule_name"), "{error}");
    Ok(())
}

#[tokio::test]
async fn apply_with_zero_sessions_is_pending_no_tab() -> anyhow::Result<()> {
    let harness = harness(0, 0).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "商品价格-改为0"))
        .await?;
    assert_eq!(outcome.state, "pending");
    assert_eq!(outcome.reason.as_deref(), Some("no_tab"));

    let stored = harness
        .service
        .rules
        .resolve_reference("商品价格-改为0")
        .await?;
    assert!(stored.enabled, "intent survives a missing tab");
    assert_eq!(stored.state.as_str(), "pending");
    Ok(())
}

#[tokio::test]
async fn apply_ack_marks_active() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "商品价格-改为0"))
        .await?;
    assert_eq!(outcome.state, "active");

    let snapshot = harness.runtime.last().expect("applied snapshot");
    assert_eq!(snapshot.rules.len(), 1);
    Ok(())
}

#[tokio::test]
async fn applied_event_promotes_pending_to_active() -> anyhow::Result<()> {
    let harness = harness(1, 0).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "商品价格-改为0"))
        .await?;
    assert_eq!(outcome.state, "pending");

    let generation = harness.runtime.last().expect("snapshot").generation;
    harness
        .service
        .rules
        .on_applied(ApplyReport {
            generation,
            ..report(1, 1)
        })
        .await;
    let stored = harness
        .service
        .rules
        .resolve_reference("商品价格-改为0")
        .await?;
    assert_eq!(stored.state.as_str(), "active");
    Ok(())
}

#[tokio::test]
async fn disable_not_narrowed_rolls_back_with_409() -> anyhow::Result<()> {
    let harness = harness(1, 0).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "商品价格-改为0"))
        .await?;
    let rule_id = applied.rule_id;

    let error = harness
        .service
        .rules
        .set_enabled(&rule_id, false)
        .await
        .expect_err("the browser never confirmed the narrowing");
    assert!(error.to_string().contains("fetch_not_disabled"), "{error}");

    let stored = harness.service.rules.resolve_reference(&rule_id).await?;
    assert!(stored.enabled, "intent is restored, not flipped");
    Ok(())
}

#[tokio::test]
async fn scope_change_keeps_disabled_rule_disabled() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "商品价格-改为0"))
        .await?;
    let disabled = harness
        .service
        .rules
        .set_enabled(&applied.rule_id, false)
        .await?;
    assert!(!disabled.enabled);

    let rescoped = harness
        .service
        .rules
        .set_scope(
            &applied.rule_id,
            crate::db::network::ScopeKind::AllTabs,
            None,
        )
        .await?;
    assert!(!rescoped.enabled, "changing scope must not switch it on");
    assert_eq!(rescoped.scope_tab, None);
    Ok(())
}

#[tokio::test]
async fn stop_all_race_leaves_the_rule_disabled() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let database = Database::open(directory.path().join("browserclaw.sqlite")).await?;
    let runtime = BlockingRuntime::new();
    let service = super::service::NetworkService::for_test(
        &database,
        directory.path(),
        Some(Arc::clone(&runtime) as Arc<dyn RuntimePort>),
    )
    .await?;

    let record_id = "netrec_blocking_pre";
    service
        .insert_record_for_test(
            record_id,
            br#"{"price":10}"#,
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let handoff = service.create_handoff(record_id).await?;
    let handoff_id = handoff["handoffId"]
        .as_str()
        .expect("handoff id")
        .to_string();

    let rules = Arc::clone(&service.rules);
    let apply_task = tokio::spawn({
        let rules = Arc::clone(&rules);
        async move { rules.apply(apply_request(&handoff_id, "阻塞规则")).await }
    });
    // Wait until the apply is parked inside the runtime's apply_rules.
    while runtime.last().is_none() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // stop_all pushes an empty snapshot of its own and takes the same
    // `rules_lock`, so the parked apply must be able to proceed first.
    runtime.release();
    service.rules.stop_all().await?;

    // Two legal endings, depending on which side reached step 6 first: the
    // apply saw the new stop epoch and persisted `superseded`, or stop-all
    // landed after the commit and disabled the fresh rule. Either way the rule
    // must not come back enabled.
    let outcome = apply_task.await??;
    assert!(
        outcome.state == "superseded" || outcome.state == "disabled",
        "expected superseded or disabled, got {}",
        outcome.state
    );
    let stored = service.rules.resolve_reference(&outcome.rule_id).await?;
    assert!(!stored.enabled);
    assert!(
        matches!(
            stored.state_reason,
            Some(StateReason::Superseded) | Some(StateReason::StoppedAll)
        ),
        "the rule came back disabled: {:?}",
        stored.state_reason
    );

    let last = runtime.last().expect("a snapshot was applied");
    assert!(
        last.rules.is_empty(),
        "the newest generation must be the empty one: {:?}",
        last.rules.len()
    );
    Ok(())
}

#[tokio::test]
async fn apply_after_stop_all_is_active() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    harness.service.rules.stop_all().await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "停止之后新建"))
        .await?;
    assert_eq!(
        outcome.state, "active",
        "a rule created after stop-all must go live"
    );
    Ok(())
}

#[tokio::test]
async fn update_by_rule_keeps_unspecified_fields() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let mut request = apply_request(&handoff, "商品价格-改为0");
    request.ignore_keys = Some(vec!["nonce".to_string()]);
    request.strict_query = Some(true);
    let applied = harness.service.rules.apply(request).await?;

    let mut update = super::ApplyRequest {
        rule: Some(applied.rule_id.clone()),
        ..serde_json::from_value(json!({})).expect("empty request")
    };
    update.response = Some(super::apply::ResponseInput {
        status: 200,
        headers: Default::default(),
        body: BodyInput::text("{\"price\":0}"),
        delay_ms: 0,
    });
    let updated = harness.service.rules.apply(update).await?;

    let stored = harness
        .service
        .rules
        .resolve_reference(&updated.rule_id)
        .await?;
    assert_eq!(stored.current_revision, 2);
    let revision = harness
        .service
        .rules
        .current_revision(&stored)
        .await
        .expect("revision");
    assert_eq!(
        revision.matcher.query_policy.ignore_keys,
        vec!["nonce".to_string()],
        "ignore_keys survive an update that did not mention them"
    );
    assert_eq!(revision.matcher.query_policy.mode, QueryMode::Strict);
    Ok(())
}

#[tokio::test]
async fn rename_rejects_reserved_prefix_and_conflicts() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "规则一"))
        .await?;
    let second = {
        let handoff = seed(&harness, "{\"price\":20}").await?;
        harness
            .service
            .rules
            .apply(apply_request(&handoff, "规则二"))
            .await?
    };

    let reserved = harness
        .service
        .rules
        .rename(&applied.rule_id, "rule_x")
        .await;
    assert!(reserved.is_err(), "rule_ is reserved for ids");

    let conflict = harness
        .service
        .rules
        .rename(&applied.rule_id, "规则二")
        .await;
    assert!(conflict.is_err(), "names are unique among live rules");

    // A same-name lookup is case-insensitive for ASCII.
    let lowered = harness.service.rules.resolve_reference("规则二").await?;
    assert_eq!(lowered.id, second.rule_id);

    let renamed = harness
        .service
        .rules
        .rename(&applied.rule_id, "规则三")
        .await?;
    assert_eq!(renamed.name, "规则三");
    Ok(())
}

#[tokio::test]
async fn apply_stores_a_processor_script() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let mut request = apply_request(&handoff, "改价");
    request.processor = Some(super::rules::ProcessorArg {
        script: "function process({ response }) { return { body: response.json() } }".to_string(),
        timeout_ms: Some(300),
    });

    let outcome = harness.service.rules.apply(request).await?;
    assert_eq!(outcome.action, "processor");
    assert!(
        outcome.sample_preview.is_some(),
        "the sample runs before the rule is stored"
    );
    Ok(())
}

#[tokio::test]
async fn apply_processor_runs_on_sample_and_returns_preview() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let mut request = apply_request(&handoff, "改价");
    request.processor = Some(super::rules::ProcessorArg {
        script: "function process({ response }) { const data = response.json(); data.price = 0; return { body: data } }".to_string(),
        timeout_ms: None,
    });

    let outcome = harness.service.rules.apply(request).await?;
    assert_eq!(outcome.action, "processor");
    let preview = outcome
        .sample_preview
        .expect("a stored processor rule reports its sample run");
    assert_eq!(preview["outcome"], "processed");
    assert_eq!(preview["status"], 200);
    assert_eq!(preview["bodyPreview"], "{\"price\":0}");
    Ok(())
}

#[tokio::test]
async fn a_hit_marks_the_captured_record() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    harness
        .service
        .insert_record_for_test(
            "netrec_marked",
            b"{}",
            11,
            "GET",
            "https://example.test/api/items",
            None,
        )
        .await?;
    let record = harness
        .service
        .list_records(None, 10)
        .await
        .into_iter()
        .find(|record| record.record_id == "netrec_marked")
        .expect("record");

    harness
        .service
        .on_hit(HitRecord {
            rule_id: "rule_marked".to_string(),
            revision: 2,
            at_ms: 1,
            tab_title: None,
            url: record.url.clone(),
            method: "GET".to_string(),
            outcome: "processed".to_string(),
            reason: None,
            network_id: Some(record.request_id.clone()),
        })
        .await?;

    let marked = harness
        .service
        .list_records(None, 10)
        .await
        .into_iter()
        .find(|record| record.record_id == "netrec_marked")
        .expect("record");
    let mock = marked.mock.expect("the hit marks its record");
    assert_eq!(mock.rule_id, "rule_marked");
    assert_eq!(mock.revision, 2);
    assert_eq!(mock.outcome, "processed");
    Ok(())
}

#[tokio::test]
async fn processor_test_endpoint_requires_processor_rule() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "静态规则"))
        .await?;

    let error = harness
        .service
        .rules
        .processor_test(&outcome.rule_id)
        .await
        .expect_err("static rule");
    assert!(error.to_string().contains("not_a_processor"), "{error}");
    Ok(())
}

#[tokio::test]
async fn apply_processor_failing_on_sample_is_rejected() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let mut request = apply_request(&handoff, "坏样本脚本");
    request.processor = Some(super::rules::ProcessorArg {
        script: "function process() { throw new Error('nope') }".to_string(),
        timeout_ms: None,
    });

    let error = harness
        .service
        .rules
        .apply(request)
        .await
        .expect_err("sample failure");
    assert!(
        error.to_string().contains("processor_failed_on_sample"),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn apply_rejects_response_and_processor_together() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let mut request = apply_request(&handoff, "冲突");
    request.response = Some(serde_json::from_value(json!({
        "status": 200,
        "headers": {},
        "body": { "kind": "literal", "text": "{}" }
    }))?);
    request.processor = Some(super::rules::ProcessorArg {
        script: "function process() {}".to_string(),
        timeout_ms: None,
    });

    let error = harness
        .service
        .rules
        .apply(request)
        .await
        .expect_err("mutually exclusive");
    assert!(error.to_string().contains("mutually exclusive"), "{error}");
    Ok(())
}

#[tokio::test]
async fn apply_rejects_a_bad_processor_script() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{}").await?;
    let mut request = apply_request(&handoff, "坏脚本");
    request.processor = Some(super::rules::ProcessorArg {
        script: "const a = 1".to_string(),
        timeout_ms: Some(30),
    });

    let error = harness
        .service
        .rules
        .apply(request)
        .await
        .expect_err("invalid processor");
    assert!(error.to_string().contains("invalid_processor"), "{error}");
    Ok(())
}

#[tokio::test]
async fn apply_rejects_a_taken_name() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let first = seed(&harness, "{\"price\":10}").await?;
    harness
        .service
        .rules
        .apply(apply_request(&first, "重名"))
        .await?;
    let second = seed(&harness, "{\"price\":20}").await?;

    let error = harness
        .service
        .rules
        .apply(apply_request(&second, "重名"))
        .await
        .expect_err("live names are unique");
    assert!(error.to_string().contains("rule_name_taken"), "{error}");
    Ok(())
}

#[tokio::test]
async fn restore_keeps_the_name_when_it_is_free() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let rule = harness
        .service
        .rules
        .apply(apply_request(&handoff, "原名"))
        .await?
        .rule_id;
    harness.service.rules.delete(&rule).await?;

    let restored = harness.service.rules.restore(&rule).await?;
    assert_eq!(restored.name, "原名");
    Ok(())
}

#[tokio::test]
async fn delete_all_then_restore_each_keeps_intent() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "会被停用"))
        .await?;
    harness
        .service
        .rules
        .set_enabled(&applied.rule_id, false)
        .await?;

    let deleted = harness.service.rules.delete_all().await?;
    assert_eq!(deleted["deleted"], json!(1));
    assert!(
        harness
            .service
            .rules
            .resolve_reference(&applied.rule_id)
            .await
            .is_err(),
        "deleted rules are gone"
    );

    let restored = harness.service.rules.restore(&applied.rule_id).await?;
    assert!(!restored.enabled, "restoring must not switch a rule on");
    Ok(())
}

#[tokio::test]
async fn restore_with_taken_name_appends_suffix() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "重名"))
        .await?;
    let second = {
        let handoff = seed(&harness, "{\"price\":20}").await?;
        harness
            .service
            .rules
            .apply(apply_request(&handoff, "另一个"))
            .await?
    };
    // Take the name while the first rule is deleted.
    harness.service.rules.delete(&applied.rule_id).await?;
    harness
        .service
        .rules
        .rename(&second.rule_id, "重名")
        .await?;

    let restored = harness.service.rules.restore(&applied.rule_id).await?;
    assert_eq!(restored.name, "重名 #2", "the name is de-duplicated");
    Ok(())
}

#[tokio::test]
async fn patch_current_applies_to_current_body() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, r#"{"name":"x","price":10}"#).await?;
    let mut request = apply_request(&handoff, "改价格");
    request.response = Some(super::apply::ResponseInput {
        status: 200,
        headers: Default::default(),
        body: BodyInput::text(r#"{"name":"x","price":10}"#),
        delay_ms: 0,
    });
    let applied = harness.service.rules.apply(request).await?;

    let mut update = super::ApplyRequest {
        rule: Some(applied.rule_id.clone()),
        ..serde_json::from_value(json!({})).expect("empty request")
    };
    update.response = Some(super::apply::ResponseInput {
        status: 200,
        headers: Default::default(),
        body: BodyInput::PatchCurrent {
            json_patch: vec![json!({ "op": "replace", "path": "/price", "value": 0 })],
        },
        delay_ms: 0,
    });
    let updated = harness.service.rules.apply(update).await?;
    assert_eq!(updated.revision, 2);

    let stored = harness
        .service
        .rules
        .resolve_reference(&updated.rule_id)
        .await?;
    let revision = harness
        .service
        .rules
        .current_revision(&stored)
        .await
        .expect("revision");
    let body_id = revision.body_id.as_deref().unwrap_or_default();
    let body = harness
        .service
        .bodies
        .read_rule(body_id)
        .await?
        .expect("stored body");
    assert_eq!(
        String::from_utf8(body)?,
        r#"{"name":"x","price":0}"#,
        "patch_current edits the body the rule already has"
    );
    Ok(())
}

#[tokio::test]
async fn rule_sample_survives_handoff_deletion() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "有样本"))
        .await?;
    harness.service.delete_handoff(&handoff).await?;

    let rule = harness
        .service
        .rules
        .resolve_reference(&applied.rule_id)
        .await?;
    let revision = harness
        .service
        .rules
        .current_revision(&rule)
        .await
        .expect("revision");
    let sample = harness.service.rules.sample_bytes_for(&revision).await?;
    assert_eq!(sample.as_deref(), Some(br#"{"price":10}"#.as_slice()));
    Ok(())
}

#[tokio::test]
async fn sample_unavailable_when_handoff_had_no_body() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let record_id = "netrec_nobody";
    harness
        .service
        .insert_record_for_test(
            record_id,
            br#"{"price":10}"#,
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    // The record keeps its metadata but loses the body file, so the handoff is
    // created with `body_state = too_large`.
    harness.service.bodies.remove_record(record_id).await?;
    let created = harness.service.create_handoff(record_id).await?;
    let handoff_id = created["handoffId"]
        .as_str()
        .expect("handoff id")
        .to_string();

    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff_id, "没有样本"))
        .await?;
    let detail = harness.service.rule_detail_view(&applied.rule_id).await?;
    assert!(!detail.sample.available, "there is no body to sample");
    assert!(detail.sample.bytes.is_none());

    let rule = harness
        .service
        .rules
        .resolve_reference(&applied.rule_id)
        .await?;
    let revision = harness
        .service
        .rules
        .current_revision(&rule)
        .await
        .expect("revision");
    assert!(
        harness
            .service
            .rules
            .sample_bytes_for(&revision)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn stop_capture_keeps_records() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    seed(&harness, "{\"price\":10}").await?;
    harness.service.start_capture().await?;
    let before = harness.service.list_records(None, 100).await.len();
    harness.service.stop_capture().await?;
    let after = harness.service.list_records(None, 100).await.len();
    assert_eq!(before, after, "stopping must keep the buffer");
    assert_eq!(after, 1);
    Ok(())
}

#[tokio::test]
async fn handoff_quota_recycles_oldest() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let mut ids = Vec::new();
    for index in 0..21 {
        let record_id = format!("netrec_quota_{index}");
        harness
            .service
            .insert_record_for_test(
                &record_id,
                b"{}",
                11,
                "GET",
                &format!("https://example.test/api/items?page={index}"),
                None,
            )
            .await?;
        let created = harness.service.create_handoff(&record_id).await?;
        ids.push(
            created["handoffId"]
                .as_str()
                .expect("handoff id")
                .to_string(),
        );
    }

    assert!(
        harness.service.get_handoff(&ids[0]).await.is_err(),
        "the oldest reference is recycled, not rejected"
    );
    assert_eq!(
        harness.service.list_handoffs().await?.len(),
        20,
        "the pin limit holds"
    );
    Ok(())
}

#[tokio::test]
async fn expired_handoffs_are_purged() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let record_id = "netrec_expiring";
    harness
        .service
        .insert_record_for_test(
            record_id,
            b"{}",
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let row = HandoffRow {
        id: "netref_expired".to_string(),
        created_at: now - 10_000,
        expires_at: now - 1_000,
        record_id: record_id.to_string(),
        source_meta: json!({ "method": "GET", "url": "https://example.test/api/items?page=1" }),
        request_headers: Vec::new(),
        response_headers: Vec::new(),
        request_body: None,
        body_id: None,
        body_bytes: 2,
        body_state: "available".to_string(),
        body_reason: None,
        matcher_seed: json!({}),
        suggested_ignore: json!([]),
        related: json!([]),
        rule_id: None,
    };
    let repo = NetworkRepository::new(harness.database.clone());
    let inserted = row.clone();
    repo.stmt(|connection| {
        let repo = repo.clone();
        let row = row.clone();
        Box::pin(async move { repo.insert_handoff(connection, &row).await })
    })
    .await?;
    harness
        .service
        .bodies
        .write_handoff(&inserted.id, b"{}")
        .await?;

    harness.service.rules.maintenance().await;
    assert!(repo.get_handoff(&inserted.id).await?.is_none());
    assert!(
        !harness
            .service
            .bodies
            .root()
            .join("handoffs")
            .join(&inserted.id)
            .exists(),
        "the body directory goes with the row"
    );
    Ok(())
}

#[tokio::test]
async fn hit_count_counts_only_fulfilled_and_processed() -> anyhow::Result<()> {
    use browseros_core::network::HitRecord;
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "计数"))
        .await?;
    let hit = |outcome: &str, network_id: &str| HitRecord {
        rule_id: applied.rule_id.clone(),
        revision: 1,
        at_ms: 1,
        tab_title: None,
        url: "https://example.test/api/items?page=1".to_string(),
        method: "GET".to_string(),
        outcome: outcome.to_string(),
        reason: None,
        network_id: Some(network_id.to_string()),
    };
    harness.service.on_hit(hit("fulfilled", "1.1")).await?;
    // The follow-up rewrites the same row instead of adding one.
    harness.service.on_hit(hit("blocked", "1.1")).await?;
    harness
        .service
        .on_hit(hit("decision_timeout", "2.2"))
        .await?;

    let stored = harness
        .service
        .rules
        .resolve_reference(&applied.rule_id)
        .await?;
    assert_eq!(stored.hit_count, 1, "only fulfilled/processed count");
    let hits = harness
        .service
        .hit_views(Some(&applied.rule_id), 100)
        .await?;
    assert_eq!(hits.len(), 2, "the rewrite updates the existing row");
    let rewritten = hits
        .iter()
        .find(|hit| hit.network_id.as_deref() == Some("1.1"))
        .expect("the rewritten hit");
    assert_eq!(rewritten.outcome, "blocked");
    Ok(())
}

#[tokio::test]
async fn replaced_rule_restore_reverses_replacement() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let first = {
        let handoff = seed(&harness, "{\"price\":10}").await?;
        harness
            .service
            .rules
            .apply(apply_request(&handoff, "规则A"))
            .await?
    };
    let second = {
        let handoff = seed(&harness, "{\"price\":20}").await?;
        harness
            .service
            .rules
            .apply(apply_request(&handoff, "规则B"))
            .await?
    };

    let replaced = harness
        .service
        .rules
        .resolve_reference(&first.rule_id)
        .await?;
    assert_eq!(
        replaced.state_reason,
        Some(StateReason::ReplacedBy(second.rule_id.clone())),
        "the later rule wins the same endpoint"
    );

    let restored = harness
        .service
        .rules
        .set_enabled(&first.rule_id, true)
        .await?;
    assert!(restored.enabled);
    let victim = harness
        .service
        .rules
        .resolve_reference(&second.rule_id)
        .await?;
    assert_eq!(
        victim.state_reason,
        Some(StateReason::ReplacedBy(first.rule_id.clone())),
        "turning the replaced rule back on replaces the winner"
    );
    Ok(())
}

#[tokio::test]
async fn restore_keeps_previous_intent() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "停用后删除"))
        .await?;
    harness
        .service
        .rules
        .set_enabled(&applied.rule_id, false)
        .await?;
    harness.service.rules.delete(&applied.rule_id).await?;

    let restored = harness.service.rules.restore(&applied.rule_id).await?;
    assert!(
        !restored.enabled,
        "a restore keeps the intent it had before the delete"
    );
    assert_eq!(restored.state_reason, Some(StateReason::User));
    Ok(())
}

#[tokio::test]
async fn restore_revision_without_tabs_keeps_intent() -> anyhow::Result<()> {
    let harness = harness(0, 0).await?;
    let handoff = seed(&harness, r#"{"name":"x","price":10}"#).await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "回退版本"))
        .await?;
    assert_eq!(applied.state, "pending");

    let mut update = super::ApplyRequest {
        rule: Some(applied.rule_id.clone()),
        ..serde_json::from_value(json!({})).expect("empty request")
    };
    update.response = Some(super::apply::ResponseInput {
        status: 200,
        headers: Default::default(),
        body: BodyInput::text(r#"{"name":"x","price":10}"#),
        delay_ms: 0,
    });
    let updated = harness.service.rules.apply(update).await?;
    assert_eq!(updated.revision, 2);

    let restored = harness
        .service
        .rules
        .restore_revision(&applied.rule_id, 1)
        .await?;
    assert!(restored.enabled, "restoring a revision keeps the intent");
    assert_eq!(restored.state.as_str(), "pending");
    assert_eq!(restored.state_reason, Some(StateReason::NoTab));
    Ok(())
}

#[tokio::test]
async fn stale_applied_event_is_ignored() -> anyhow::Result<()> {
    let harness = harness(1, 0).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let outcome = harness
        .service
        .rules
        .apply(apply_request(&handoff, "旧事件"))
        .await?;
    assert_eq!(outcome.state, "pending");

    let generation = harness.runtime.last().expect("snapshot").generation;
    harness
        .service
        .rules
        .on_applied(ApplyReport {
            generation: generation.saturating_sub(1),
            ..report(1, 1)
        })
        .await;
    let stored = harness
        .service
        .rules
        .resolve_reference(&outcome.rule_id)
        .await?;
    assert_eq!(
        stored.state.as_str(),
        "pending",
        "an ack for an older generation must not promote the rule"
    );
    Ok(())
}

#[tokio::test]
async fn runtime_lost_demotes_active_to_pending() -> anyhow::Result<()> {
    let harness = harness(1, 1).await?;
    let handoff = seed(&harness, "{\"price\":10}").await?;
    let applied = harness
        .service
        .rules
        .apply(apply_request(&handoff, "浏览器断开"))
        .await?;
    assert_eq!(applied.state, "active");

    harness.service.rules.on_runtime_lost().await;
    let stored = harness
        .service
        .rules
        .resolve_reference(&applied.rule_id)
        .await?;
    assert!(stored.enabled, "the intent is untouched");
    assert_eq!(stored.state.as_str(), "pending");
    assert_eq!(stored.state_reason, Some(StateReason::NoBrowser));
    Ok(())
}

#[tokio::test]
async fn apply_without_runtime_saves_pending_no_browser() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let database = Database::open(directory.path().join("browserclaw.sqlite")).await?;
    let service =
        super::service::NetworkService::for_test(&database, directory.path(), None).await?;
    service
        .insert_record_for_test(
            "netrec_nobrowser",
            br#"{"price":10}"#,
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let created = service.create_handoff("netrec_nobrowser").await?;
    let handoff_id = created["handoffId"]
        .as_str()
        .expect("handoff id")
        .to_string();

    let outcome = service
        .rules
        .apply(apply_request(&handoff_id, "没有运行时"))
        .await?;
    assert_eq!(outcome.state, "pending");
    assert_eq!(outcome.reason.as_deref(), Some("no_browser"));

    let stored = service.rules.resolve_reference("没有运行时").await?;
    assert!(stored.enabled);
    assert_eq!(stored.state.as_str(), "pending");
    assert_eq!(stored.state_reason, Some(StateReason::NoBrowser));
    Ok(())
}

#[tokio::test]
async fn stop_all_supersedes_inflight_apply() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let database = Database::open(directory.path().join("browserclaw.sqlite")).await?;
    let runtime = FakeRuntime::new(1, 1);
    let service = super::service::NetworkService::for_test(
        &database,
        directory.path(),
        Some(Arc::clone(&runtime) as Arc<dyn RuntimePort>),
    )
    .await?;
    service
        .insert_record_for_test(
            "netrec_superseded",
            br#"{"price":10}"#,
            11,
            "GET",
            "https://example.test/api/items?page=1",
            None,
        )
        .await?;
    let created = service.create_handoff("netrec_superseded").await?;
    let handoff_id = created["handoffId"]
        .as_str()
        .expect("handoff id")
        .to_string();

    // Holding `rules_lock` parks the apply right after it captured the stop
    // epoch, which is the window `stop_all` must win.
    let guard = service.rules.hold_rules_lock().await;
    let apply_task = tokio::spawn({
        let rules = Arc::clone(&service.rules);
        async move { rules.apply(apply_request(&handoff_id, "在途规则")).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !apply_task.is_finished(),
        "the apply must be queued on the lock"
    );

    // `stop_all` never waits for `rules_lock`.
    service.rules.stop_all().await?;
    drop(guard);

    let outcome = apply_task.await??;
    assert_eq!(outcome.state, "superseded");
    assert_eq!(outcome.reason.as_deref(), Some("superseded"));

    let stored = service.rules.resolve_reference(&outcome.rule_id).await?;
    assert!(!stored.enabled);
    assert_eq!(stored.state_reason, Some(StateReason::Superseded));
    let snapshot = runtime.last().expect("snapshot");
    assert!(
        snapshot.rules.is_empty(),
        "the last snapshot is the empty one"
    );
    Ok(())
}
