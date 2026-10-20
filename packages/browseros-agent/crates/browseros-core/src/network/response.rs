//! Build `Fetch.fulfillRequest` parameters from a stored mock response.
//!
//! Header inheritance, transport-header stripping and the CORS amendment are
//! pure functions so the GUI, `dryRun` and the runtime all explain the same
//! response.

use super::{matcher::parse_url_parts, state::HeaderPair};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum MockBody {
    #[serde(rename = "literal")]
    Literal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
    },
    /// Resolved before persistence; never reaches the runtime.
    #[serde(rename = "patch_sample")]
    PatchSample {
        #[serde(default, rename = "jsonPatch")]
        json_patch: Vec<serde_json::Value>,
    },
}

impl MockBody {
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
            Self::PatchSample { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockResponse {
    #[serde(default = "default_status")]
    pub status: i64,
    /// Explicit overrides. A `null` value deletes an inherited header.
    #[serde(default)]
    pub headers: BTreeMap<String, Option<String>>,
    pub body: MockBody,
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_status() -> i64 {
    200
}

impl Default for MockResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            body: MockBody::default(),
            delay_ms: 0,
        }
    }
}

impl Default for MockBody {
    fn default() -> Self {
        Self::Literal {
            text: Some(String::new()),
            base64: None,
        }
    }
}

/// A ready-to-send fulfill decision.
#[derive(Debug, Clone, PartialEq)]
pub struct FulfillPlan {
    pub response_code: i64,
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
        serde_json::json!({
            "requestId": request_id,
            "responseCode": self.response_code,
            "responseHeaders": self
                .headers
                .iter()
                .map(|header| serde_json::json!({"name": header.name, "value": header.value}))
                .collect::<Vec<_>>(),
            "body": body,
        })
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
    response: &MockResponse,
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

    for (name, value) in &response.headers {
        let existing = headers
            .iter()
            .position(|header| header.name.eq_ignore_ascii_case(name));
        match (existing, value) {
            (Some(index), Some(_)) => {
                headers[index].value = value.clone().unwrap_or_default();
            }
            (Some(index), None) => {
                headers.remove(index);
            }
            (None, Some(value)) => headers.push(HeaderPair::new(name.clone(), value.clone())),
            (None, None) => {}
        }
    }

    // Content-Length always describes the bytes we are about to fulfill, even
    // when the sample or the AI supplied one.
    headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.name.eq_ignore_ascii_case("content-encoding")
    });
    if !matches!(response.status, 204 | 304) {
        headers.push(HeaderPair::new("content-length", body.len().to_string()));
    }

    apply_cors(&mut headers, request_url, request_headers);

    FulfillPlan {
        response_code: response.status,
        headers,
        body: body.to_vec(),
        delay_ms: response.delay_ms.min(5000),
    }
}

fn is_transport_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-encoding")
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("transfer-encoding")
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
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[must_use]
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !cleaned.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
    for chunk in cleaned.chunks(4) {
        let mut padding = 0;
        let mut triple = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte == b'=' {
                padding += 1;
                triple <<= 6;
            } else {
                if padding > 0 {
                    return None;
                }
                triple = (triple << 6) | value(*byte)?;
            }
            let _ = index;
        }
        out.push((triple >> 16) as u8);
        if padding < 2 {
            out.push((triple >> 8) as u8);
        }
        if padding < 1 {
            out.push(triple as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_inherited_header_values_are_dropped() {
        let response = MockResponse::default();
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
        let response = MockResponse::default();
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
        let response = MockResponse {
            status: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                Some("application/json".to_string()),
            )]),
            body: MockBody::text("{}"),
            delay_ms: 0,
        };
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
        let response = MockResponse::default();
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
        let response = MockResponse::default();
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
}
