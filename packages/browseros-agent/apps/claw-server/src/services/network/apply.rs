//! Pure validation and construction for `rule_apply`.

use crate::error::{AppError, AppResult};
use browseros_core::network::{
    BodyField, EndpointMatcher, MATCHER_VERSION, MockBody, MockResponse, QueryMode, QueryPolicy,
    base64_decode,
};
use serde_json::Value;

/// Build the stored matcher from the handoff seed. `ignoreKeys` and the query
/// mode are the only parts AI may change; origin/path/method come from the
/// sample the user pointed at.
pub fn matcher_from_seed(
    seed: &Value,
    ignore_keys: Option<Vec<String>>,
    strict_query: bool,
    body_fields: Option<Vec<BodyField>>,
    exact_url: Option<String>,
) -> AppResult<EndpointMatcher> {
    let method = seed
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::bad_request("handoff matcher seed is missing method"))?;
    let mut origin = seed
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::bad_request("handoff matcher seed is missing origin"))?
        .to_string();
    let mut pathname = seed
        .get("pathname")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::bad_request("handoff matcher seed is missing pathname"))?
        .to_string();
    if let Some(exact) = &exact_url {
        let parts = browseros_core::network::parse_url_parts(exact);
        origin = parts.origin;
        pathname = parts.pathname;
    }
    let sample_query: Vec<(String, String)> = seed
        .get("query")
        .and_then(Value::as_array)
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    for field in body_fields.as_deref().unwrap_or_default() {
        if !field.pointer.is_empty() && !field.pointer.starts_with('/') {
            return Err(AppError::bad_request(
                "unsupported_match_condition: bodyFields.pointer must be an RFC 6901 JSON Pointer",
            ));
        }
    }

    Ok(EndpointMatcher {
        matcher_version: MATCHER_VERSION,
        method: method.to_ascii_uppercase(),
        origin,
        pathname,
        sample_query,
        query_policy: QueryPolicy {
            mode: if strict_query {
                QueryMode::Strict
            } else {
                QueryMode::Lenient
            },
            ignore_keys: ignore_keys.unwrap_or_default(),
        },
        body_fields: body_fields.unwrap_or_default(),
        resource_type: seed
            .get("resourceType")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        exact_url,
    })
}

/// A rule with no explicit response repeats the sample status and body.
#[must_use]
pub fn default_response(sample_status: Option<i64>, sample_body: Option<&[u8]>) -> MockResponse {
    let body = match sample_body {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => MockBody::text(text),
            Err(_) => MockBody::Literal {
                text: None,
                base64: Some(browseros_core::network::base64_encode(bytes)),
            },
        },
        None => MockBody::text(String::new()),
    };
    MockResponse {
        status: sample_status.unwrap_or(200),
        headers: std::collections::BTreeMap::new(),
        body,
        delay_ms: 0,
    }
}

/// Enforce the response rules from design §5.2 before anything is persisted or
/// sent to Fetch.
pub fn validate_response(response: &MockResponse) -> AppResult<()> {
    if !(200..=599).contains(&response.status) {
        return Err(AppError::bad_request(
            "response status must be 200-599 and must not be 1xx",
        ));
    }
    for (name, value) in &response.headers {
        if name.is_empty() || name.contains('\r') || name.contains('\n') || name.contains(':') {
            return Err(AppError::bad_request(format!(
                "invalid response header name: {name:?}"
            )));
        }
        if let Some(value) = value
            && (value.contains('\r') || value.contains('\n'))
        {
            return Err(AppError::bad_request(format!(
                "invalid response header value for {name}"
            )));
        }
    }
    if let MockBody::Literal {
        base64: Some(value),
        ..
    } = &response.body
        && base64_decode(value).is_none()
    {
        return Err(AppError::bad_request(
            "invalid_response: body.base64 is not valid base64",
        ));
    }
    if matches!(response.status, 204 | 304) && !response.body.bytes().is_empty() {
        return Err(AppError::bad_request(
            "204 and 304 responses must not carry a body",
        ));
    }
    if response.delay_ms > 5000 {
        return Err(AppError::bad_request("delayMs must be between 0 and 5000"));
    }
    Ok(())
}

/// Resolve `patch_sample` into a literal body. The patch runs once, at revision
/// creation; the runtime only ever sees the frozen result.
pub fn resolve_patch_response(
    response: &mut MockResponse,
    sample_body: Option<&[u8]>,
) -> AppResult<()> {
    let MockBody::PatchSample { json_patch } = &response.body else {
        return Ok(());
    };
    let Some(sample) = sample_body else {
        return Err(AppError::bad_request("sample_body_unavailable"));
    };
    let patched = browseros_core::network::apply_json_patch(sample, json_patch)
        .map_err(|reason| AppError::bad_request(format!("invalid_response: {reason}")))?;
    let text = String::from_utf8(patched)
        .map_err(|_| AppError::bad_request("invalid_response: patch output is not utf8"))?;
    response.body = MockBody::text(text);
    Ok(())
}

pub fn response_body(response: &MockResponse) -> Vec<u8> {
    if matches!(response.status, 204 | 304) {
        Vec::new()
    } else {
        response.body.bytes()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn seed() -> Value {
        json!({
            "method": "GET",
            "origin": "https://example.test",
            "pathname": "/api/items",
            "query": [["category", "books"], ["nonce", "1"]],
            "resourceType": "XHR",
        })
    }

    #[test]
    fn matcher_defaults_to_lenient_and_keeps_the_sample_query() {
        let matcher =
            matcher_from_seed(&seed(), Some(vec!["nonce".to_string()]), false, None, None)
                .expect("matcher");
        assert_eq!(matcher.method, "GET");
        assert_eq!(matcher.query_policy.mode, QueryMode::Lenient);
        assert_eq!(matcher.query_policy.ignore_keys, vec!["nonce".to_string()]);
        assert_eq!(matcher.sample_query.len(), 2);
        assert_eq!(matcher.resource_type.as_deref(), Some("XHR"));
    }

    #[test]
    fn strict_query_is_explicit() {
        let matcher = matcher_from_seed(&seed(), None, true, None, None).expect("matcher");
        assert_eq!(matcher.query_policy.mode, QueryMode::Strict);
    }

    #[test]
    fn exact_url_is_kept_and_overrides_the_endpoint_fields() {
        let matcher = matcher_from_seed(
            &seed(),
            None,
            false,
            None,
            Some("https://other.test/exact?a=1&b=2".to_string()),
        )
        .expect("matcher");
        assert_eq!(
            matcher.exact_url.as_deref(),
            Some("https://other.test/exact?a=1&b=2")
        );
        assert_eq!(matcher.origin, "https://other.test");
        assert_eq!(matcher.pathname, "/exact");
        assert_eq!(
            matcher.matcher_version,
            browseros_core::network::MATCHER_VERSION
        );
    }

    #[test]
    fn response_validation_rejects_unsafe_headers_and_bodies() {
        let mut response = MockResponse::default();
        assert!(validate_response(&response).is_ok());

        response.status = 150;
        assert!(validate_response(&response).is_err());

        response.status = 200;
        response.headers =
            std::collections::BTreeMap::from([("bad\r\nname".to_string(), Some("x".to_string()))]);
        assert!(validate_response(&response).is_err());

        response.headers = std::collections::BTreeMap::new();
        response.status = 204;
        response.body = MockBody::text("not allowed");
        assert!(validate_response(&response).is_err());

        response.status = 200;
        response.delay_ms = 5001;
        assert!(validate_response(&response).is_err());

        response.delay_ms = 0;
        response.body = MockBody::Literal {
            text: None,
            base64: Some("not base64!!".to_string()),
        };
        assert!(validate_response(&response).is_err());
    }

    #[test]
    fn patch_sample_is_frozen_into_a_literal_body() {
        let mut response = MockResponse {
            body: MockBody::PatchSample {
                json_patch: vec![serde_json::json!({
                    "op": "replace",
                    "path": "/price",
                    "value": 0
                })],
            },
            ..MockResponse::default()
        };
        resolve_patch_response(&mut response, Some(br#"{"price":10,"name":"x"}"#)).expect("patch");
        assert!(matches!(response.body, MockBody::Literal { .. }));
        assert_eq!(response_body(&response), br#"{"name":"x","price":0}"#);
    }

    #[test]
    fn patch_sample_requires_a_sample_body() {
        let mut response = MockResponse {
            body: MockBody::PatchSample {
                json_patch: vec![serde_json::json!({"op": "remove", "path": "/name"})],
            },
            ..MockResponse::default()
        };
        assert!(resolve_patch_response(&mut response, None).is_err());
    }

    #[test]
    fn body_comes_from_the_response_for_literal_rules() {
        let response = MockResponse {
            body: MockBody::text("{\"ok\":true}"),
            ..MockResponse::default()
        };
        assert_eq!(response_body(&response), b"{\"ok\":true}");
    }
}
