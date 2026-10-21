//! Pure validation and construction for `rule_apply`.

use crate::error::{AppError, AppResult};
pub use browseros_core::network::{
    BodyEncoding, BodyField, BodyInput, BodyMeta, EndpointMatcher, HeaderPatch, MATCHER_VERSION,
    QueryMode, QueryPolicy, ResponseInput, StaticResponse, base64_decode,
};
use serde_json::Value;

/// Stored response bodies are rejected above this size, so the runtime never
/// holds one it cannot fulfill.
pub const RULE_BODY_LIMIT: u64 = 64 * 1024 * 1024;

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

/// Builds the default response input: repeat the sample status and body.
#[must_use]
pub fn default_response(sample_status: Option<i64>, sample_body: Option<&[u8]>) -> ResponseInput {
    let body = match sample_body {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => BodyInput::text(text),
            Err(_) => BodyInput::Literal {
                text: None,
                base64: Some(browseros_core::network::base64_encode(bytes)),
            },
        },
        None => BodyInput::text(String::new()),
    };
    ResponseInput {
        status: sample_status.unwrap_or(200),
        headers: HeaderPatch::new(),
        body,
        delay_ms: 0,
    }
}

/// Validates a response *input* before it is resolved: status, header names and
/// values, base64 bodies and the 204/304 rule.
pub fn validate_response(response: &ResponseInput) -> AppResult<()> {
    if !(200..=599).contains(&response.status) {
        return Err(AppError::bad_request(
            "response status must be 200-599 and must not be 1xx",
        ));
    }
    for (name, value) in &response.headers {
        validate_header_name(name)?;
        let Some(values) = value else { continue };
        let values = values.to_vec();
        if values.is_empty() {
            return Err(AppError::bad_request(format!(
                "invalid_response: {name} must not be an empty array"
            )));
        }
        for value in values {
            if value.contains('\r') || value.contains('\n') {
                return Err(AppError::bad_request(format!(
                    "invalid response header value for {name}"
                )));
            }
        }
    }
    if let BodyInput::Literal {
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

fn validate_header_name(name: &str) -> AppResult<()> {
    if name.is_empty() || name.contains('\r') || name.contains('\n') || name.contains(':') {
        return Err(AppError::bad_request(format!(
            "invalid response header name: {name:?}"
        )));
    }
    Ok(())
}

/// Lower-cases the patch keys and rejects anything a header patch cannot
/// express, so the stored action and the runtime agree.
pub fn normalize_header_patch(patch: &HeaderPatch) -> AppResult<HeaderPatch> {
    let mut normalized = HeaderPatch::new();
    for (name, value) in patch {
        validate_header_name(name)?;
        let key = name.to_ascii_lowercase();
        if normalized.contains_key(&key) {
            return Err(AppError::bad_request(format!(
                "invalid_response: duplicate header {key}"
            )));
        }
        normalized.insert(key, value.clone());
    }
    Ok(normalized)
}

/// Resolves a response input into the stored action plus its body bytes.
///
/// `patch_sample` patches the rule's sample, `patch_current` the rule's current
/// body; both fail when the corresponding body is unavailable.
pub fn resolve_static_input(
    input: &ResponseInput,
    sample: Option<&[u8]>,
    current: Option<&[u8]>,
) -> AppResult<(StaticResponse, Vec<u8>)> {
    validate_response(input)?;
    let body = match &input.body {
        BodyInput::Literal { .. } => input.body.bytes(),
        BodyInput::PatchSample { json_patch } => {
            let Some(sample) = sample else {
                return Err(AppError::bad_request("sample_body_unavailable"));
            };
            apply_patch(sample, json_patch)?
        }
        BodyInput::PatchCurrent { json_patch } => {
            let Some(current) = current else {
                return Err(AppError::bad_request(
                    "invalid_response: patch_current requires an existing rule body",
                ));
            };
            apply_patch(current, json_patch)?
        }
    };
    if body.len() as u64 > RULE_BODY_LIMIT {
        return Err(AppError::bad_request(format!(
            "invalid_response: body exceeds {}MiB",
            RULE_BODY_LIMIT / (1024 * 1024)
        )));
    }
    let encoding = if std::str::from_utf8(&body).is_ok() {
        BodyEncoding::Utf8
    } else {
        BodyEncoding::Binary
    };
    let action = StaticResponse {
        status: input.status,
        headers: normalize_header_patch(&input.headers)?,
        delay_ms: input.delay_ms,
        body: BodyMeta {
            bytes: body.len() as u64,
            encoding,
        },
    };
    Ok((action, body))
}

fn apply_patch(base: &[u8], patch: &[Value]) -> AppResult<Vec<u8>> {
    let patched = browseros_core::network::apply_json_patch(base, patch)
        .map_err(|reason| AppError::bad_request(format!("invalid_response: {reason}")))?;
    String::from_utf8(patched)
        .map(String::into_bytes)
        .map_err(|_| AppError::bad_request("invalid_response: patch output is not utf8"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use browseros_core::network::OneOrMany;
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

    fn input(body: BodyInput) -> ResponseInput {
        ResponseInput {
            status: 200,
            headers: HeaderPatch::new(),
            body,
            delay_ms: 0,
        }
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
        let mut response = input(BodyInput::text("{}"));
        assert!(validate_response(&response).is_ok());

        response.status = 150;
        assert!(validate_response(&response).is_err());

        response.status = 200;
        response.headers = HeaderPatch::from([(
            "bad\r\nname".to_string(),
            Some(OneOrMany::One("x".to_string())),
        )]);
        assert!(validate_response(&response).is_err());

        response.headers = HeaderPatch::new();
        response.status = 204;
        response.body = BodyInput::text("not allowed");
        assert!(validate_response(&response).is_err());

        response.status = 200;
        response.delay_ms = 5001;
        assert!(validate_response(&response).is_err());

        response.delay_ms = 0;
        response.body = BodyInput::Literal {
            text: None,
            base64: Some("not base64!!".to_string()),
        };
        assert!(validate_response(&response).is_err());
    }

    #[test]
    fn validate_static_rejects_empty_header_array() {
        let mut response = input(BodyInput::text("{}"));
        response.headers = HeaderPatch::from([("x".to_string(), Some(OneOrMany::Many(vec![])))]);
        assert!(validate_response(&response).is_err());
    }

    #[test]
    fn resolve_static_input_normalizes_keys_and_records_encoding() {
        let mut response = input(BodyInput::text("{\"a\":1}"));
        response.headers = HeaderPatch::from([(
            "Set-Cookie".to_string(),
            Some(OneOrMany::Many(vec![
                "a=1; Path=/".to_string(),
                "b=2; Path=/".to_string(),
            ])),
        )]);
        let (action, body) = resolve_static_input(&response, None, None).expect("resolve");
        assert_eq!(action.status, 200);
        assert_eq!(action.body.bytes, body.len() as u64);
        assert_eq!(action.body.encoding, BodyEncoding::Utf8);
        assert_eq!(
            action.headers.keys().next().expect("one header"),
            "set-cookie"
        );
    }

    #[test]
    fn resolve_static_input_patch_sample_is_frozen() {
        let response = input(BodyInput::PatchSample {
            json_patch: vec![json!({ "op": "replace", "path": "/price", "value": 0 })],
        });
        let (_, body) = resolve_static_input(&response, Some(br#"{"price":10,"name":"x"}"#), None)
            .expect("patch");
        assert_eq!(
            String::from_utf8(body).expect("utf8"),
            "{\"name\":\"x\",\"price\":0}"
        );
    }

    #[test]
    fn resolve_static_input_patch_current_requires_current() {
        let response = input(BodyInput::PatchCurrent {
            json_patch: vec![json!({ "op": "remove", "path": "/name" })],
        });
        assert!(
            resolve_static_input(&response, None, None).is_err(),
            "no current body"
        );
        let (_, body) = resolve_static_input(&response, None, Some(br#"{"name":"x","price":1}"#))
            .expect("patch");
        assert_eq!(String::from_utf8(body).expect("utf8"), "{\"price\":1}");
    }

    #[test]
    fn resolve_static_input_patch_sample_requires_sample() {
        let response = input(BodyInput::PatchSample {
            json_patch: vec![json!({ "op": "remove", "path": "/name" })],
        });
        assert!(resolve_static_input(&response, None, None).is_err());
    }
}
