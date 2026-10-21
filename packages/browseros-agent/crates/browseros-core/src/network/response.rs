//! Build `Fetch.fulfillRequest` parameters from a stored mock response.
//!
//! Header inheritance, transport-header stripping and the CORS amendment are
//! pure functions so the GUI, `dryRun` and the runtime all explain the same
//! response.

use super::{matcher::parse_url_parts, state::HeaderPair};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// AI-supplied body. Resolved into bytes before anything is stored, so no
/// persisted shape ever carries a body inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BodyInput {
    #[serde(rename = "literal")]
    Literal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
    },
    /// RFC 6902 patch applied to the rule's sample body.
    #[serde(rename = "patch_sample")]
    PatchSample {
        #[serde(default, rename = "jsonPatch")]
        json_patch: Vec<serde_json::Value>,
    },
    /// RFC 6902 patch applied to the rule's current body; update-only.
    #[serde(rename = "patch_current")]
    PatchCurrent {
        #[serde(default, rename = "jsonPatch")]
        json_patch: Vec<serde_json::Value>,
    },
}

impl BodyInput {
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Literal {
            text: Some(value.into()),
            base64: None,
        }
    }

    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Literal { text, base64 } => {
                if let Some(base64) = base64 {
                    base64_decode(base64).unwrap_or_default()
                } else {
                    text.clone().unwrap_or_default().into_bytes()
                }
            }
            Self::PatchSample { .. } | Self::PatchCurrent { .. } => Vec::new(),
        }
    }
}

impl Default for BodyInput {
    fn default() -> Self {
        Self::Literal {
            text: Some(String::new()),
            base64: None,
        }
    }
}

/// One header value, or several for the same name (several `set-cookie` lines).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

/// Header overrides keyed by lower-cased name. `None` deletes every header
/// with that name.
pub type HeaderPatch = BTreeMap<String, Option<OneOrMany>>;

/// AI-supplied response. Resolved into a `RuleAction` before persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseInput {
    #[serde(default = "default_status")]
    pub status: i64,
    pub headers: HeaderPatch,
    pub body: BodyInput,
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_status() -> i64 {
    200
}

impl Default for ResponseInput {
    fn default() -> Self {
        Self {
            status: 200,
            headers: HeaderPatch::new(),
            body: BodyInput::default(),
            delay_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyEncoding {
    Utf8,
    Binary,
}

/// Describes the stored body file; the bytes themselves live in the body store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyMeta {
    pub bytes: u64,
    pub encoding: BodyEncoding,
}

/// A synchronous response-stage script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorSpec {
    pub script: String,
    pub timeout_ms: u32,
}

/// What a stored rule does. Persisted as `net_rule_revisions.action_json`;
/// bodies are never inlined (see `BodyMeta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleAction {
    Static(StaticResponse),
    Processor(ProcessorSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticResponse {
    pub status: i64,
    pub headers: HeaderPatch,
    pub delay_ms: u64,
    pub body: BodyMeta,
}

impl StaticResponse {
    #[must_use]
    pub fn new(status: i64, headers: HeaderPatch, delay_ms: u64, bytes: u64) -> Self {
        Self {
            status,
            headers,
            delay_ms,
            body: BodyMeta {
                bytes,
                encoding: BodyEncoding::Utf8,
            },
        }
    }
}

/// A ready-to-send fulfill decision.
#[derive(Debug, Clone, PartialEq)]
pub struct FulfillPlan {
    pub response_code: i64,
    /// `Fetch.fulfillRequest.responsePhrase`; `None` omits the field.
    pub response_phrase: Option<String>,
    pub headers: Vec<HeaderPair>,
    pub body: Vec<u8>,
    pub delay_ms: u64,
}

impl FulfillPlan {
    #[must_use]
    pub fn to_params(&self, request_id: &str) -> serde_json::Value {
        let body = if self.body.is_empty() {
            None
        } else {
            Some(base64_encode(&self.body))
        };
        let mut params = serde_json::json!({
            "requestId": request_id,
            "responseCode": self.response_code,
            "responseHeaders": self
                .headers
                .iter()
                .map(|header| serde_json::json!({"name": header.name, "value": header.value}))
                .collect::<Vec<_>>(),
            "body": body,
        });
        if let Some(phrase) = &self.response_phrase {
            params["responsePhrase"] = serde_json::json!(phrase);
        }
        params
    }
}

/// Header names NEOB always owns, because they describe framing rather than
/// content. A wrong value truncates or hangs the response.
const TRANSPORT_HEADERS: [&str; 3] = ["content-encoding", "content-length", "transfer-encoding"];

fn is_transport_header(name: &str) -> bool {
    TRANSPORT_HEADERS
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// Applies a header patch in place: `None` deletes every header with that
/// name, otherwise every existing value is replaced by the supplied one(s).
/// Other headers keep their order.
pub fn apply_header_patch(headers: &mut Vec<HeaderPair>, patch: &HeaderPatch) {
    for (name, value) in patch {
        headers.retain(|header| !header.name.eq_ignore_ascii_case(name));
        if let Some(values) = value {
            for value in values.to_vec() {
                headers.push(HeaderPair::new(name.clone(), value));
            }
        }
    }
}

/// Drops every `content-length` and `transfer-encoding`, then sets
/// `content-length` from the real body unless the status forbids one.
pub fn set_content_length(headers: &mut Vec<HeaderPair>, status: i64, body_len: usize) {
    headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
    });
    if !matches!(status, 204 | 304) {
        headers.push(HeaderPair::new("content-length", body_len.to_string()));
    }
}

/// Upstream headers as a script sees them: lower-cased names, order and
/// duplicates kept, framing headers removed because the body is decoded.
#[must_use]
pub fn script_visible_headers(upstream: &[HeaderPair]) -> Vec<HeaderPair> {
    upstream
        .iter()
        .filter(|header| {
            !is_transport_header(&header.name)
                && !header.name.contains(['\r', '\n'])
                && !header.value.contains(['\r', '\n'])
        })
        .map(|header| HeaderPair::new(header.name.to_ascii_lowercase(), header.value.clone()))
        .collect()
}

/// What the script asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorResult {
    Passthrough,
    Replace {
        status: Option<i64>,
        status_text: Option<String>,
        headers: Option<Vec<HeaderPair>>,
        set_headers: HeaderPatch,
        body: Option<Vec<u8>>,
    },
}

/// Upstream response handed to the script.
#[derive(Debug, Clone)]
pub struct ProcessorInput {
    pub method: String,
    pub url: String,
    pub request_headers: Vec<HeaderPair>,
    pub request_body: Option<String>,
    pub status: i64,
    pub status_text: String,
    pub response_headers: Vec<HeaderPair>,
    pub body: Vec<u8>,
    /// Serialized jar view of the upstream cookies for this request, or `"[]"`.
    pub cookies_json: String,
}

/// One upstream cookie, as the browser reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: String,
}

/// Synchronous hooks the sandbox may call while the script runs.
pub struct ProcessorHooks {
    /// Invoked when the script calls `response.cookies()`. Performs
    /// `Fetch.getCookies{requestId}` on the caller thread.
    pub upstream_cookies: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
}

impl std::fmt::Debug for ProcessorHooks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProcessorHooks { .. }")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRun {
    pub result: ProcessorResult,
    pub logs: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessorErrorKind {
    Timeout,
    Exception,
    InvalidReturn,
    OutputTooLarge,
    Busy,
    Engine,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorError {
    pub kind: ProcessorErrorKind,
    pub message: String,
    pub logs: Vec<String>,
}

/// Rewrites an upstream response. Implemented by the server's sandbox; core
/// only carries the trait and a fake for tests.
pub trait ResponseProcessor: Send + Sync {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessorSpec,
        input: ProcessorInput,
        hooks: ProcessorHooks,
    ) -> BoxFuture<'a, Result<ProcessorRun, ProcessorError>>;
}

/// Processor used when no sandbox engine is attached: every response passes
/// through unchanged. The server swaps in the real engine.
pub struct NoopProcessor;

impl ResponseProcessor for NoopProcessor {
    fn run<'a>(
        &'a self,
        _spec: &'a ProcessorSpec,
        _input: ProcessorInput,
        _hooks: ProcessorHooks,
    ) -> BoxFuture<'a, Result<ProcessorRun, ProcessorError>> {
        Box::pin(async {
            Ok(ProcessorRun {
                result: ProcessorResult::Passthrough,
                logs: Vec::new(),
                elapsed_ms: 0,
            })
        })
    }
}

/// Builds a fulfilled response from a processed outcome.
///
/// Only `content-length` and `transfer-encoding` are forced; every other
/// header, `set-cookie` included, is exactly what the script produced.
#[must_use]
pub fn build_processed(
    upstream_status: i64,
    upstream_headers: &[HeaderPair],
    upstream_body: &[u8],
    result: &ProcessorResult,
) -> FulfillPlan {
    let ProcessorResult::Replace {
        status,
        status_text,
        headers,
        set_headers,
        body,
    } = result
    else {
        unreachable!("build_processed is only called for ProcessorResult::Replace");
    };

    let response_code = status.unwrap_or(upstream_status);
    let mut resolved = headers
        .clone()
        .unwrap_or_else(|| script_visible_headers(upstream_headers));
    apply_header_patch(&mut resolved, set_headers);
    let payload = body.clone().unwrap_or_else(|| upstream_body.to_vec());
    set_content_length(&mut resolved, response_code, payload.len());

    FulfillPlan {
        response_code,
        response_phrase: status_text.clone(),
        headers: resolved,
        body: payload,
        delay_ms: 0,
    }
}

/// Build the fulfill response.
///
/// - Transport headers are dropped because the body is already decoded.
/// - Sample CORS headers are inherited.
/// - A cross-origin request without sample CORS gets the minimum headers the
///   browser accepts.
#[must_use]
pub fn build_fulfill(
    response: &StaticResponse,
    body: &[u8],
    sample_headers: &[HeaderPair],
    request_url: &str,
    request_headers: &[HeaderPair],
) -> FulfillPlan {
    let mut headers: Vec<HeaderPair> = sample_headers
        .iter()
        // CDP rejects header values containing CR/LF. Multi-value sample
        // headers (for example Google's `accept-ch`) are not worth failing a
        // mock over, so drop only the invalid ones.
        .filter(|header| {
            !is_transport_header(&header.name)
                && !header.name.contains(['\r', '\n'])
                && !header.value.contains(['\r', '\n'])
        })
        .cloned()
        .collect();

    apply_header_patch(&mut headers, &response.headers);

    // Content-Length always describes the bytes we are about to fulfill, even
    // when the sample or the AI supplied one.
    set_content_length(&mut headers, response.status, body.len());

    apply_cors(&mut headers, request_url, request_headers);

    FulfillPlan {
        response_code: response.status,
        response_phrase: None,
        headers,
        body: body.to_vec(),
        delay_ms: response.delay_ms.min(5000),
    }
}

fn apply_cors(headers: &mut Vec<HeaderPair>, request_url: &str, request_headers: &[HeaderPair]) {
    let Some(origin) = request_headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("origin"))
        .map(|header| header.value.clone())
    else {
        return;
    };
    let request_origin = parse_url_parts(&origin).origin;
    let target_origin = parse_url_parts(request_url).origin;
    if request_origin.is_empty() || request_origin == target_origin {
        return;
    }
    // Inherited ACAO/ACAC may describe the original server (including `*`),
    // which is wrong for a mock and fatal with credentials. Values are always
    // rebuilt from the paused request's Origin.
    headers.retain(|header| {
        !header
            .name
            .eq_ignore_ascii_case("access-control-allow-origin")
            && !header
                .name
                .eq_ignore_ascii_case("access-control-allow-credentials")
    });
    headers.push(HeaderPair::new(
        "access-control-allow-origin",
        origin.clone(),
    ));
    let has_credentials = request_headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("cookie"));
    if has_credentials {
        headers.push(HeaderPair::new("access-control-allow-credentials", "true"));
    }
    match headers
        .iter_mut()
        .find(|header| header.name.eq_ignore_ascii_case("vary"))
    {
        Some(vary)
            if !vary
                .value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("origin")) =>
        {
            vary.value.push_str(", Origin");
        }
        Some(_) => {}
        None => headers.push(HeaderPair::new("vary", "Origin")),
    }
}

/// CDP's `Fetch` URL patterns treat `*` and `?` as wildcards, so both the
/// literal characters in the origin/path and the appended wildcard must be
/// escaped deliberately.
#[must_use]
pub fn escape_url_pattern(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len() + 2);
    for character in literal.chars() {
        match character {
            '*' | '?' | '\\' => {
                escaped.push('\\');
                escaped.push(character);
            }
            other => escaped.push(other),
        }
    }
    escaped.push('*');
    escaped
}

pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decodes standard base64. Returns `None` for anything malformed, including
/// wrong padding or characters outside the alphabet.
#[must_use]
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let cleaned: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn invalid_inherited_header_values_are_dropped() {
        let response = StaticResponse::new(200, HeaderPatch::new(), 0, 2);
        let plan = build_fulfill(
            &response,
            b"{}",
            &[
                HeaderPair::new("accept-ch", "Sec-CH-UA\nDownlink"),
                HeaderPair::new("content-type", "text/html"),
            ],
            "https://example.test/",
            &[],
        );
        assert!(plan.headers.iter().all(|header| header.name != "accept-ch"));
        assert!(
            plan.headers
                .iter()
                .any(|header| header.name == "content-type")
        );
    }

    #[test]
    fn cors_headers_are_rebuilt_from_the_request_origin() {
        let response = StaticResponse::new(200, HeaderPatch::new(), 0, 2);
        let request_headers = vec![
            HeaderPair::new("origin", "https://app.test"),
            HeaderPair::new("cookie", "session=1"),
        ];
        let plan = build_fulfill(
            &response,
            b"{}",
            &[
                HeaderPair::new("access-control-allow-origin", "*"),
                HeaderPair::new("access-control-allow-credentials", "false"),
            ],
            "https://api.test/items",
            &request_headers,
        );
        let acao = plan
            .headers
            .iter()
            .filter(|header| {
                header
                    .name
                    .eq_ignore_ascii_case("access-control-allow-origin")
            })
            .collect::<Vec<_>>();
        assert_eq!(acao.len(), 1);
        assert_eq!(acao[0].value, "https://app.test");
        let acac = plan
            .headers
            .iter()
            .filter(|header| {
                header
                    .name
                    .eq_ignore_ascii_case("access-control-allow-credentials")
            })
            .collect::<Vec<_>>();
        assert_eq!(acac.len(), 1);
        assert_eq!(acac[0].value, "true");
    }

    #[test]
    fn pattern_escaping_protects_query_markers_and_wildcards() {
        assert_eq!(
            escape_url_pattern("https://example.test/api/items?x=1"),
            "https://example.test/api/items\\?x=1*"
        );
        assert_eq!(escape_url_pattern("/a*b"), "/a\\*b*");
    }

    #[test]
    fn fulfill_drops_transport_headers_and_recomputes_length() {
        let response = StaticResponse::new(
            200,
            HeaderPatch::from([(
                "content-type".to_string(),
                Some(OneOrMany::One("application/json".to_string())),
            )]),
            0,
            2,
        );
        let plan = build_fulfill(
            &response,
            b"{}",
            &[
                HeaderPair::new("content-encoding", "gzip"),
                HeaderPair::new("transfer-encoding", "chunked"),
                HeaderPair::new("content-length", "999"),
                HeaderPair::new("x-sample", "kept"),
            ],
            "https://example.test/api",
            &[],
        );
        assert!(plan.headers.iter().all(|header| {
            !header.name.eq_ignore_ascii_case("content-encoding")
                && !header.name.eq_ignore_ascii_case("transfer-encoding")
        }));
        assert_eq!(
            plan.headers
                .iter()
                .find(|header| header.name == "content-length")
                .map(|header| header.value.as_str()),
            Some("2")
        );
        assert!(plan.headers.iter().any(|header| header.name == "x-sample"));
    }

    #[test]
    fn cross_origin_fulfill_adds_the_sample_origin_not_a_wildcard() {
        let response = StaticResponse::new(200, HeaderPatch::new(), 0, 2);
        let plan = build_fulfill(
            &response,
            b"",
            &[],
            "https://api.example.test/x",
            &[
                HeaderPair::new("origin", "https://app.example.test"),
                HeaderPair::new("cookie", "sid=1"),
            ],
        );
        let find = |name: &str| {
            plan.headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case(name))
                .map(|header| header.value.as_str())
        };
        assert_eq!(
            find("access-control-allow-origin"),
            Some("https://app.example.test")
        );
        assert_eq!(find("access-control-allow-credentials"), Some("true"));
        assert_eq!(find("vary"), Some("Origin"));
    }

    #[test]
    fn inherited_cors_headers_are_not_duplicated() {
        let response = StaticResponse::new(200, HeaderPatch::new(), 0, 2);
        let plan = build_fulfill(
            &response,
            b"",
            &[HeaderPair::new("access-control-allow-origin", "*")],
            "https://api.example.test/x",
            &[HeaderPair::new("origin", "https://app.example.test")],
        );
        assert_eq!(
            plan.headers
                .iter()
                .filter(|header| header
                    .name
                    .eq_ignore_ascii_case("access-control-allow-origin"))
                .count(),
            1
        );
    }

    #[test]
    fn base64_round_trips_arbitrary_bytes() {
        let bytes = vec![0u8, 1, 2, 250, 255, 128];
        assert_eq!(base64_decode(&base64_encode(&bytes)), Some(bytes));
    }

    #[test]
    fn base64_decode_rejects_garbage() {
        assert_eq!(base64_decode("@@@"), None);
        assert_eq!(base64_decode("QQ"), None);
    }

    #[test]
    fn action_json_round_trips_static_with_multi_value_headers() {
        let action = RuleAction::Static(StaticResponse::new(
            200,
            HeaderPatch::from([
                (
                    "set-cookie".to_string(),
                    Some(OneOrMany::Many(vec![
                        "a=1; Path=/".to_string(),
                        "b=2; Path=/".to_string(),
                    ])),
                ),
                ("x-debug".to_string(), None),
            ]),
            0,
            12,
        ));
        let encoded = serde_json::to_string(&action).expect("serialize");
        assert!(encoded.contains("set-cookie"), "{encoded}");
        let decoded: RuleAction = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, action);
    }

    #[test]
    fn apply_header_patch_semantics() {
        let mut headers = vec![
            HeaderPair::new("X-A", "1"),
            HeaderPair::new("x-a", "2"),
            HeaderPair::new("keep", "yes"),
            HeaderPair::new("set-cookie", "old=1"),
        ];
        let patch = HeaderPatch::from([
            (
                "x-a".to_string(),
                Some(OneOrMany::One("replaced".to_string())),
            ),
            (
                "set-cookie".to_string(),
                Some(OneOrMany::Many(vec![
                    "new=1; Path=/".to_string(),
                    "new=2; Path=/".to_string(),
                ])),
            ),
            ("absent".to_string(), None),
        ]);
        apply_header_patch(&mut headers, &patch);

        let values = |name: &str| {
            headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case(name))
                .map(|header| header.value.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(values("x-a"), vec!["replaced"]);
        assert_eq!(values("set-cookie"), vec!["new=1; Path=/", "new=2; Path=/"]);
        assert_eq!(values("keep"), vec!["yes"]);
        // Untouched headers keep their position; patched ones are re-appended.
        assert_eq!(headers.first().map(|h| h.value.as_str()), Some("yes"));
        assert_eq!(
            headers
                .iter()
                .map(|header| header.value.clone())
                .collect::<Vec<_>>(),
            vec!["yes", "new=1; Path=/", "new=2; Path=/", "replaced"],
            "patch order follows the lower-cased key order"
        );
        assert!(values("absent").is_empty());
    }

    #[test]
    fn script_visible_headers_drop_transport_and_keep_duplicates() {
        let upstream = vec![
            HeaderPair::new("Content-Encoding", "gzip"),
            HeaderPair::new("Content-Length", "10"),
            HeaderPair::new("Transfer-Encoding", "chunked"),
            HeaderPair::new("Set-Cookie", "a=1"),
            HeaderPair::new("set-cookie", "b=2"),
            HeaderPair::new("X-Keep", "yes"),
        ];
        let visible = script_visible_headers(&upstream);
        let names = visible
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["set-cookie", "set-cookie", "x-keep"],
            "framing headers are gone, duplicates and order survive"
        );
        assert_eq!(visible[0].value, "a=1");
    }

    #[test]
    fn build_processed_overrides_content_length_and_keeps_script_headers() {
        let result = ProcessorResult::Replace {
            status: Some(201),
            status_text: Some("Created".to_string()),
            headers: Some(vec![
                HeaderPair::new("content-length", "1"),
                HeaderPair::new("set-cookie", "a=1; Path=/"),
            ]),
            set_headers: HeaderPatch::new(),
            body: Some(b"payload".to_vec()),
        };
        let plan = build_processed(200, &[], b"upstream", &result);
        assert_eq!(plan.response_code, 201);
        assert_eq!(plan.response_phrase.as_deref(), Some("Created"));
        assert_eq!(plan.body, b"payload".to_vec());
        let length = plan
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("content-length"))
            .map(|header| header.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(length, vec!["7"]);
        assert!(
            plan.headers
                .iter()
                .any(|header| header.name == "set-cookie" && header.value == "a=1; Path=/")
        );
    }

    fn replace(
        headers: Option<Vec<HeaderPair>>,
        set_headers: HeaderPatch,
        body: Option<Vec<u8>>,
    ) -> ProcessorResult {
        ProcessorResult::Replace {
            status: None,
            status_text: None,
            headers,
            set_headers,
            body,
        }
    }

    #[test]
    fn build_processed_defaults_to_visible_headers_and_recomputes_length() {
        let upstream = vec![
            HeaderPair::new("content-type", "application/json"),
            HeaderPair::new("content-length", "99"),
            HeaderPair::new("set-cookie", "a=1"),
            HeaderPair::new("set-cookie", "b=2"),
        ];
        let plan = build_processed(
            200,
            &upstream,
            b"{}",
            &replace(None, HeaderPatch::new(), None),
        );
        let names = plan
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["content-type", "set-cookie", "set-cookie", "content-length"]
        );
        assert_eq!(plan.headers[3].value, "2");
    }

    #[test]
    fn build_processed_full_replace_headers() {
        let headers = Some(vec![HeaderPair::new("a", "1"), HeaderPair::new("a", "2")]);
        let plan = build_processed(200, &[], b"x", &replace(headers, HeaderPatch::new(), None));
        let names = plan
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "a", "content-length"]);
    }

    #[test]
    fn build_processed_set_headers_can_rewrite_set_cookie() {
        let patch = HeaderPatch::from([(
            "set-cookie".to_string(),
            Some(OneOrMany::Many(vec!["x=1".to_string(), "y=2".to_string()])),
        )]);
        let upstream = vec![HeaderPair::new("set-cookie", "up=1")];
        let plan = build_processed(200, &upstream, b"", &replace(None, patch, None));
        let cookies = plan
            .headers
            .iter()
            .filter(|header| header.name == "set-cookie")
            .map(|header| header.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(cookies, vec!["x=1", "y=2"]);
    }

    #[test]
    fn build_processed_explicit_content_encoding_is_kept() {
        let patch = HeaderPatch::from([(
            "content-encoding".to_string(),
            Some(OneOrMany::One("gzip".to_string())),
        )]);
        let plan = build_processed(200, &[], b"", &replace(None, patch, None));
        assert!(
            plan.headers
                .iter()
                .any(|header| header.name == "content-encoding" && header.value == "gzip")
        );
    }

    #[test]
    fn build_processed_status_text_maps_to_response_phrase() {
        let mut result = replace(None, HeaderPatch::new(), None);
        if let ProcessorResult::Replace { status_text, .. } = &mut result {
            *status_text = Some("Teapot".to_string());
        }
        let plan = build_processed(200, &[], b"", &result);
        assert_eq!(plan.response_phrase.as_deref(), Some("Teapot"));
    }

    #[test]
    fn static_fulfill_emits_two_set_cookie_entries() {
        let response = StaticResponse::new(
            200,
            HeaderPatch::from([(
                "set-cookie".to_string(),
                Some(OneOrMany::Many(vec![
                    "a=1; Path=/".to_string(),
                    "b=2; Path=/".to_string(),
                ])),
            )]),
            0,
            2,
        );
        let plan = build_fulfill(&response, b"{}", &[], "https://example.test/", &[]);
        assert_eq!(
            plan.headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("set-cookie"))
                .count(),
            2
        );
    }
}
