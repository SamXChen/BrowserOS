//! Rule names: validation, default generation and lookup.

use crate::{
    db::network::RuleRow,
    error::{AppError, AppResult},
};
use browseros_core::network::{EndpointMatcher, parse_url_parts};

pub const NAME_MAX_CHARS: usize = 60;

/// Reserved prefixes would make a name indistinguishable from an id.
const RESERVED_PREFIXES: [&str; 2] = ["rule_", "netref_"];

/// Callers must supply the live-name set so both the check and the default
/// generator use the same case-insensitive comparison.
pub fn validate_rule_name(raw: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if !(1..=NAME_MAX_CHARS).contains(&length) {
        return Err(AppError::bad_request(format!(
            "invalid_rule_name: name must be 1-{NAME_MAX_CHARS} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "invalid_rule_name: name must not contain control characters",
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    for prefix in RESERVED_PREFIXES {
        if lower.starts_with(prefix) {
            return Err(AppError::bad_request(format!(
                "invalid_rule_name: name must not start with {prefix}"
            )));
        }
    }
    Ok(trimmed.to_string())
}

/// `METHOD /path`, suffixed with ` #2`, ` #3`... until it is free.
pub fn default_rule_name(matcher: &EndpointMatcher, taken: &[String]) -> String {
    let parts = match &matcher.exact_url {
        Some(exact) => parse_url_parts(exact),
        None => parse_url_parts(&format!("{}{}", matcher.origin, matcher.pathname)),
    };
    let base = truncate(
        &format!("{} {}", matcher.method, parts.pathname),
        NAME_MAX_CHARS,
    );
    if !taken.iter().any(|name| name == &base.to_ascii_lowercase()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let marker = format!(" #{suffix}");
        let candidate = format!(
            "{} #{suffix}",
            truncate(&base, NAME_MAX_CHARS - marker.chars().count())
        );
        if !taken
            .iter()
            .any(|name| name == &candidate.to_ascii_lowercase())
        {
            return candidate;
        }
        suffix += 1;
    }
}

/// Like [`default_rule_name`] but starting from an existing name, used when a
/// restored rule's name was taken meanwhile.
pub fn suffixed_name(base: &str, taken: &[String]) -> String {
    let base = truncate(base, NAME_MAX_CHARS);
    if !taken.iter().any(|name| name == &base.to_ascii_lowercase()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let marker = format!(" #{suffix}");
        let candidate = format!(
            "{} #{suffix}",
            truncate(&base, NAME_MAX_CHARS - marker.chars().count())
        );
        if !taken
            .iter()
            .any(|name| name == &candidate.to_ascii_lowercase())
        {
            return candidate;
        }
        suffix += 1;
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// Two names collide when they are equal after ASCII lower-casing; SQLite's
/// NOCASE collation folds the same set of characters.
pub fn names_collide(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Suggests rules to show when a reference does not resolve, newest first.
/// Matching is on the name and the id; the endpoint lives in the revision, so
/// callers that need it filter on the revision separately.
pub fn candidates<'a>(rules: &'a [RuleRow], reference: &str) -> Vec<&'a RuleRow> {
    let needle = reference.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return newest(rules);
    }
    let mut matched = rules
        .iter()
        .filter(|rule| {
            rule.name.to_ascii_lowercase().contains(&needle)
                || rule.id.to_ascii_lowercase().contains(&needle)
        })
        .take(5)
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return newest(rules);
    }
    matched.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    matched
}

fn newest(rules: &[RuleRow]) -> Vec<&RuleRow> {
    let mut ordered = rules.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    ordered.truncate(5);
    ordered
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use browseros_core::network::{EndpointMatcher, QueryMode, QueryPolicy};

    fn matcher(method: &str, origin: &str, pathname: &str) -> EndpointMatcher {
        EndpointMatcher {
            matcher_version: 1,
            method: method.to_string(),
            origin: origin.to_string(),
            pathname: pathname.to_string(),
            sample_query: Vec::new(),
            query_policy: QueryPolicy {
                mode: QueryMode::Lenient,
                ignore_keys: Vec::new(),
            },
            body_fields: Vec::new(),
            resource_type: None,
            exact_url: None,
        }
    }

    #[test]
    fn validation_covers_length_control_and_prefixes() {
        assert!(validate_rule_name("   ").is_err());
        assert!(validate_rule_name("a".repeat(61).as_str()).is_err());
        assert!(validate_rule_name("has\nnewline").is_err());
        assert!(validate_rule_name("rule_x").is_err());
        assert!(validate_rule_name("netref_x").is_err());
        assert_eq!(validate_rule_name("  商品  ").expect("ok"), "商品");
    }

    #[test]
    fn default_name_is_method_and_path_then_suffixed() {
        let matcher = matcher("GET", "https://shop.test", "/api/items");
        assert_eq!(
            default_rule_name(&matcher, &[]),
            "GET /api/items".to_string()
        );
        assert_eq!(
            default_rule_name(&matcher, &["get /api/items".to_string()]),
            "GET /api/items #2".to_string()
        );
        assert_eq!(
            default_rule_name(
                &matcher,
                &[
                    "get /api/items".to_string(),
                    "get /api/items #2".to_string()
                ]
            ),
            "GET /api/items #3".to_string()
        );
    }

    #[test]
    fn default_name_stays_within_the_limit() {
        let matcher = matcher("GET", "https://shop.test", &format!("/{}", "p".repeat(200)));
        let name = default_rule_name(
            &matcher,
            &[default_rule_name(&matcher, &[]).to_ascii_lowercase()],
        );
        assert!(name.chars().count() <= NAME_MAX_CHARS, "{name}");
    }

    #[test]
    fn collision_is_ascii_case_insensitive_only() {
        assert!(names_collide("Foo", "foo"));
        assert!(names_collide("商品", "商品"), "same name always collides");
        assert!(!names_collide("商品", "订单"), "different names are free");
        assert!(!names_collide("İ", "i"), "ASCII-only folding");
    }

    #[test]
    fn suffixed_name_keeps_the_original_base() {
        assert_eq!(
            suffixed_name("order-empty", &["order-empty".to_string()]),
            "order-empty #2".to_string()
        );
        assert_eq!(suffixed_name("order-empty", &[]), "order-empty".to_string());
    }
}
