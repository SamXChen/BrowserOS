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
    BodyEncoding, BodyInput, BodyMeta, HeaderPatch, NoopProcessor, OneOrMany, ProcessorCookie,
    ProcessorError, ProcessorErrorKind, ProcessorHooks, ProcessorInput, ProcessorResult,
    ProcessorRun, ProcessorSpec, ResponseInput, ResponseProcessor, RuleAction, StaticResponse,
    apply_header_patch, base64_decode, base64_encode, build_fulfill, build_processed,
    escape_url_pattern, script_visible_headers, set_content_length,
};
pub use runtime::{
    ApplyReport, HitRecord, NetworkRuntime, NetworkRuntimeHandle, RuntimeAction, RuntimeCommand,
    RuntimeEvent, RuntimeRule, RuntimeRuleSet, RuntimeStats, SessionView, TabSummary,
};
pub use state::{BodyState, HeaderPair, MockMark, NetworkRecord, RecordState};
