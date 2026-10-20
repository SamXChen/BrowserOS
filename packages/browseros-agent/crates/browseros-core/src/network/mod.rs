//! Network capture and Fetch-based mocking.
//!
//! This module is deliberately free of server concerns (no axum, no database, no
//! filesystem). [`NetworkRuntime`] owns the CDP interaction for one browser
//! connection; hosts consume [`RuntimeEvent`]s and persist them however they like.

mod events;
mod json_patch;
mod matcher;
mod response;
mod runtime;
mod state;

pub use events::{
    LoadingFailed, LoadingFinished, RequestWillBeSent, RequestWillBeSentExtraInfo,
    ResponseReceived, ResponseReceivedExtraInfo,
};
pub use json_patch::apply_json_patch;
pub use matcher::{
    BodyField, EndpointMatcher, MATCHER_VERSION, MatchOutcome, QueryMode, QueryPolicy, UrlParts,
    matches, parse_url_parts, query_mismatch_with_sample,
};
pub use response::{
    MockBody, MockResponse, base64_decode, base64_encode, build_fulfill, escape_url_pattern,
};
pub use runtime::{
    ApplyReport, HitRecord, NetworkRuntime, NetworkRuntimeHandle, RuntimeCommand, RuntimeEvent,
    RuntimeRule, RuntimeRuleSet, SessionView, TabSummary,
};
pub use state::{BodyState, HeaderPair, MockMark, NetworkRecord, RecordState};
