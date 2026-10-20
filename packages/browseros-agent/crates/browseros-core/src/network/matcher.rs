//! The single matcher implementation shared by runtime interception, `dryRun`
//! and the rule-detail test button. Pure functions only.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bump when matching semantics change; rules record the version they were
/// built with so a future migration can explain a behavior change.
pub const MATCHER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QueryMode {
    #[default]
    Lenient,
    Strict,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPolicy {
    #[serde(default)]
    pub mode: QueryMode,
    #[serde(default)]
    pub ignore_keys: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyField {
    /// RFC 6901 JSON Pointer, for example `/operationName`.
    pub pointer: String,
    #[serde(default)]
    pub equals: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointMatcher {
    /// Matching semantics version recorded with every rule revision.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub matcher_version: u32,
    pub method: String,
    pub origin: String,
    pub pathname: String,
    /// The sample's query pairs. The policy above decides which of them may
    /// vary; the rest are compared against these values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_query: Vec<(String, String)>,
    #[serde(default)]
    pub query_policy: QueryPolicy,
    #[serde(default)]
    pub body_fields: Vec<BodyField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    /// Optional exact-URL alternative to the endpoint fields above. When set,
    /// the URL must match byte-for-byte, including query order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_url: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl EndpointMatcher {
    /// Semantic equality for conflict replacement: the matcher version explains
    /// behavior, it does not make two otherwise identical rules different.
    #[must_use]
    pub fn same_semantics(&self, other: &Self) -> bool {
        self.method == other.method
            && self.origin == other.origin
            && self.pathname == other.pathname
            && self.sample_query == other.sample_query
            && self.query_policy == other.query_policy
            && self.body_fields == other.body_fields
            && self.resource_type == other.resource_type
            && self.exact_url == other.exact_url
    }

    /// Build the default matcher for a sample request. Query keys are the
    /// sample's own keys, all initially preserved.
    #[must_use]
    pub fn from_url(method: &str, url: &str) -> Self {
        let parts = parse_url_parts(url);
        Self {
            matcher_version: MATCHER_VERSION,
            method: method.to_ascii_uppercase(),
            origin: parts.origin,
            pathname: parts.pathname,
            sample_query: parts.query,
            query_policy: QueryPolicy {
                mode: QueryMode::Lenient,
                ignore_keys: Vec::new(),
            },
            body_fields: Vec::new(),
            resource_type: None,
            exact_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    Matched,
    NoMatch { reason: &'static str },
}

impl MatchOutcome {
    #[must_use]
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Matched)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlParts {
    pub origin: String,
    pub pathname: String,
    pub query: Vec<(String, String)>,
}

/// Split a URL into the three fields the default matcher compares. This is a
/// deliberately small parser: absolute http(s) URLs only, no normalization
/// beyond host case and default ports.
#[must_use]
pub fn parse_url_parts(url: &str) -> UrlParts {
    let Some(scheme_end) = url.find("://") else {
        return UrlParts {
            origin: String::new(),
            pathname: url.to_string(),
            query: Vec::new(),
        };
    };
    let scheme = url[..scheme_end].to_ascii_lowercase();
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let after_authority = &rest[authority_end..];

    let mut host = authority;
    let mut port = None;
    if let Some((before, after)) = authority.rsplit_once(':')
        && !before.ends_with(']')
        && after.chars().all(|c| c.is_ascii_digit())
    {
        host = before;
        port = Some(after);
    }
    let host = host.to_ascii_lowercase();
    let default_port = matches!(
        (scheme.as_str(), port),
        ("http", Some("80")) | ("https", Some("443"))
    );
    let origin = if default_port || port.is_none() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{}", port.unwrap_or_default())
    };

    let without_fragment = after_authority.split('#').next().unwrap_or_default();
    let (path, query) = match without_fragment.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (without_fragment, None),
    };
    let pathname = if path.is_empty() { "/" } else { path };

    let mut pairs = Vec::new();
    if let Some(query) = query {
        for part in query.split('&') {
            if part.is_empty() {
                continue;
            }
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            pairs.push((key.to_string(), value.to_string()));
        }
    }

    UrlParts {
        origin,
        pathname: pathname.to_string(),
        query: pairs,
    }
}

/// Match one paused request against the rule's matcher.
///
/// `post_data` is the raw request body when the protocol supplied it. Missing
/// body with `body_fields` configured is a non-match with `body_unavailable`,
/// never a silent loose match.
#[must_use]
pub fn matches(
    matcher: &EndpointMatcher,
    method: &str,
    url: &str,
    post_data: Option<&str>,
    resource_type: Option<&str>,
) -> MatchOutcome {
    if !matcher.method.eq_ignore_ascii_case(method) {
        return MatchOutcome::NoMatch {
            reason: "method_differs",
        };
    }
    if let Some(expected) = &matcher.resource_type {
        let Some(actual) = resource_type else {
            return MatchOutcome::NoMatch {
                reason: "resource_type_differs",
            };
        };
        if !expected.eq_ignore_ascii_case(actual) {
            return MatchOutcome::NoMatch {
                reason: "resource_type_differs",
            };
        }
    }
    if let Some(exact_url) = &matcher.exact_url {
        return if url == exact_url {
            MatchOutcome::Matched
        } else {
            MatchOutcome::NoMatch {
                reason: "url_differs",
            }
        };
    }
    let parts = parse_url_parts(url);
    if !matcher.origin.eq_ignore_ascii_case(&parts.origin) {
        return MatchOutcome::NoMatch {
            reason: "origin_differs",
        };
    }
    if matcher.pathname != parts.pathname {
        return MatchOutcome::NoMatch {
            reason: "path_differs",
        };
    }
    if let Some(reason) =
        query_mismatch_with_sample(&matcher.query_policy, &matcher.sample_query, &parts.query)
    {
        return MatchOutcome::NoMatch { reason };
    }
    if !matcher.body_fields.is_empty() {
        let Some(post_data) = post_data else {
            return MatchOutcome::NoMatch {
                reason: "body_unavailable",
            };
        };
        let Ok(json) = serde_json::from_str::<Value>(post_data) else {
            return MatchOutcome::NoMatch {
                reason: "body_unavailable",
            };
        };
        for field in &matcher.body_fields {
            let Some(actual) = json_pointer(&json, &field.pointer) else {
                return MatchOutcome::NoMatch {
                    reason: "body_field_missing",
                };
            };
            if actual != &field.equals {
                return MatchOutcome::NoMatch {
                    reason: "body_field_differs",
                };
            }
        }
    }
    MatchOutcome::Matched
}

/// Compare a request's query against the sample's query under the configured
/// policy. Used by the runtime (the rule carries the seed at apply time).
#[must_use]
pub fn query_mismatch_with_sample(
    policy: &QueryPolicy,
    sample: &[(String, String)],
    actual: &[(String, String)],
) -> Option<&'static str> {
    let sample_map = group(sample);
    let actual_map = group(actual);

    for (key, values) in &sample_map {
        if policy.ignore_keys.iter().any(|ignored| ignored == key) {
            continue;
        }
        if let Some(seen) = actual_map.get(key)
            && seen != values
        {
            return Some("query_differs");
        }
    }

    if policy.mode == QueryMode::Strict {
        for key in actual_map.keys() {
            if policy.ignore_keys.iter().any(|ignored| ignored == key) {
                continue;
            }
            if !sample_map.contains_key(key) {
                return Some("query_extra_key");
            }
        }
    }
    None
}

fn group(pairs: &[(String, String)]) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (key, value) in pairs {
        grouped.entry(key.clone()).or_default().push(value.clone());
    }
    grouped
}

/// RFC 6901 lookup, including `~0` and `~1` escapes.
#[must_use]
fn json_pointer<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for token in pointer.trim_start_matches('/').split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(&token)?,
            Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: QueryMode, ignore: &[&str]) -> QueryPolicy {
        QueryPolicy {
            mode,
            ignore_keys: ignore.iter().map(|key| (*key).to_string()).collect(),
        }
    }

    #[test]
    fn parses_origin_path_and_query_without_decoding() {
        let parts = parse_url_parts("https://Example.TEST:443/api/items?a=1&a=2&b=&c#frag");
        assert_eq!(parts.origin, "https://example.test");
        assert_eq!(parts.pathname, "/api/items");
        assert_eq!(
            parts.query,
            vec![
                ("a".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
                ("b".to_string(), String::new()),
                ("c".to_string(), String::new())
            ]
        );
    }

    #[test]
    fn keeps_non_default_ports_in_the_origin() {
        assert_eq!(
            parse_url_parts("http://localhost:9200/x").origin,
            "http://localhost:9200"
        );
        assert_eq!(
            parse_url_parts("http://localhost:80/x").origin,
            "http://localhost"
        );
    }

    #[test]
    fn lenient_mode_ignores_extra_and_missing_keys_but_compares_known_values() {
        let sample = vec![("category".to_string(), "books".to_string())];
        assert_eq!(
            query_mismatch_with_sample(
                &policy(QueryMode::Lenient, &[]),
                &sample,
                &[
                    ("category".to_string(), "books".to_string()),
                    ("nonce".to_string(), "9".to_string())
                ]
            ),
            None
        );
        assert_eq!(
            query_mismatch_with_sample(&policy(QueryMode::Lenient, &[]), &sample, &[]),
            None
        );
        assert_eq!(
            query_mismatch_with_sample(
                &policy(QueryMode::Lenient, &[]),
                &sample,
                &[("category".to_string(), "music".to_string())]
            ),
            Some("query_differs")
        );
    }

    #[test]
    fn strict_mode_rejects_extra_keys_and_ignore_keys_win() {
        let sample = vec![("category".to_string(), "books".to_string())];
        assert_eq!(
            query_mismatch_with_sample(
                &policy(QueryMode::Strict, &[]),
                &sample,
                &[
                    ("category".to_string(), "books".to_string()),
                    ("nonce".to_string(), "9".to_string())
                ]
            ),
            Some("query_extra_key")
        );
        assert_eq!(
            query_mismatch_with_sample(
                &policy(QueryMode::Strict, &["nonce"]),
                &sample,
                &[
                    ("category".to_string(), "books".to_string()),
                    ("nonce".to_string(), "9".to_string())
                ]
            ),
            None
        );
        assert_eq!(
            query_mismatch_with_sample(
                &policy(QueryMode::Lenient, &["nonce"]),
                &[
                    ("category".to_string(), "books".to_string()),
                    ("nonce".to_string(), "1".to_string())
                ],
                &[
                    ("category".to_string(), "books".to_string()),
                    ("nonce".to_string(), "2".to_string())
                ]
            ),
            None
        );
    }

    #[test]
    fn semantic_equality_ignores_the_matcher_version() {
        let mut left = EndpointMatcher::from_url("GET", "https://example.test/a?x=1");
        let mut right = left.clone();
        left.matcher_version = 1;
        right.matcher_version = 99;
        assert!(left.same_semantics(&right));
        right.pathname = "/b".to_string();
        assert!(!left.same_semantics(&right));
    }

    #[test]
    fn exact_url_requires_the_same_query_order() {
        let matcher = EndpointMatcher {
            method: "GET".to_string(),
            exact_url: Some("https://example.test/items?a=1&b=2".to_string()),
            ..EndpointMatcher::default()
        };
        assert_eq!(
            matches(
                &matcher,
                "GET",
                "https://example.test/items?a=1&b=2",
                None,
                None
            ),
            MatchOutcome::Matched
        );
        assert_eq!(
            matches(
                &matcher,
                "GET",
                "https://example.test/items?b=2&a=1",
                None,
                None
            ),
            MatchOutcome::NoMatch {
                reason: "url_differs"
            }
        );
    }

    #[test]
    fn body_fields_use_json_pointer_equality() {
        let matcher = EndpointMatcher {
            method: "POST".to_string(),
            origin: "https://example.test".to_string(),
            pathname: "/graphql".to_string(),
            body_fields: vec![BodyField {
                pointer: "/operationName".to_string(),
                equals: Value::String("ProductList".to_string()),
            }],
            ..EndpointMatcher::default()
        };
        assert_eq!(
            matches(
                &matcher,
                "POST",
                "https://example.test/graphql",
                Some(r#"{"operationName":"ProductList"}"#),
                Some("XHR")
            ),
            MatchOutcome::Matched
        );
        assert_eq!(
            matches(
                &matcher,
                "POST",
                "https://example.test/graphql",
                None,
                Some("XHR")
            ),
            MatchOutcome::NoMatch {
                reason: "body_unavailable"
            }
        );
        assert_eq!(
            matches(
                &matcher,
                "POST",
                "https://example.test/graphql",
                Some(r#"{"operationName":"Other"}"#),
                Some("XHR")
            ),
            MatchOutcome::NoMatch {
                reason: "body_field_differs"
            }
        );
    }

    #[test]
    fn json_pointer_handles_escapes_and_arrays() {
        let value = serde_json::json!({"a/b": {"~x": [1, 2]}});
        assert_eq!(
            json_pointer(&value, "/a~1b/~0x/1"),
            Some(&Value::Number(2.into()))
        );
        assert_eq!(json_pointer(&value, "/missing"), None);
    }
}
