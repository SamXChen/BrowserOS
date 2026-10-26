//! Keeps the `network` tool guide honest about the states the server can report
//!.
//!
//! The tags are listed by exhaustive `match`, so adding a variant without deciding
//! whether the guide must mention it fails to compile, so the guide cannot drift.
//! outcomes are still plain strings, so those stay a hand-written list and are recorded
//! as a known limitation.

#![cfg(test)]

use crate::db::network::{RuleState, StateReason};

use browseros_mcp::tools::network::GUIDE;

const GUIDE_BUDGET: usize = 3 * 1024;

/// Reasons an agent cannot act on, so the guide does not have to name them.
const REASON_ALLOWLIST: [&str; 2] = ["user", "replaced_by"];

/// `RuleState` / `StateReason` serialised forms, derived by exhaustive match.
fn state_tags() -> Vec<String> {
    let states = [
        RuleState::Active,
        RuleState::Pending,
        RuleState::Unbound,
        RuleState::Failed,
        RuleState::Disabled,
    ]
    .into_iter()
    .map(|state| state.as_str().to_string());

    let reasons = [
        StateReason::NoBrowser,
        StateReason::NoTab,
        StateReason::ApplyTimeout,
        StateReason::TabClosed,
        StateReason::BodyMissing,
        StateReason::BodyTooLarge,
        StateReason::ProcessorInvalid,
        StateReason::User,
        StateReason::StoppedAll,
        StateReason::ReplacedBy("id".to_string()),
        StateReason::Superseded,
    ]
    .into_iter()
    .map(|reason| {
        // `replaced_by:<id>` is a prefix match; the allowlist covers it.
        reason.as_str().split(':').next().unwrap_or_default().to_string()
    });

    states.chain(reasons).collect()
}

/// Hit outcomes are `&'static str` values produced by the network service. Kept as a
/// list on purpose: they are not an enum yet (see the module docs).
const HIT_OUTCOMES: [&str; 11] = [
    "fulfilled",
    "processed",
    "processor_error",
    "processor_timeout",
    "blocked",
    "passthrough",
    "failed",
    "decision_timeout",
    "delay_cancelled",
    "unknown",
    "superseded",
];

#[test]
fn the_guide_covers_every_state_tag() {
    let mut missing = Vec::new();
    for tag in state_tags() {
        let tag = tag.as_str();
        if REASON_ALLOWLIST.contains(&tag) {
            continue;
        }
        if !GUIDE.contains(tag) {
            missing.push(tag.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "the network guide never names these states: {missing:?}"
    );
}

#[test]
fn the_guide_covers_every_hit_outcome() {
    let missing = HIT_OUTCOMES
        .into_iter()
        .filter(|outcome| !GUIDE.contains(outcome))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "the network guide never names these outcomes: {missing:?}"
    );
}

#[test]
fn the_guide_stays_within_its_budget_and_drops_dead_wording() {
    assert!(
        GUIDE.len() <= GUIDE_BUDGET,
        "the network guide is {} bytes, over the {GUIDE_BUDGET} byte budget",
        GUIDE.len()
    );
    for dead in ["apply_failed", "one-click ignore", "ignore-keys", "near-misses"] {
        assert!(!GUIDE.contains(dead), "the guide still says `{dead}`");
    }
}
