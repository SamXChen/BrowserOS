use crate::framework::{ToolCtx, ToolError, ToolExecResult, ToolResult, parse_args, text_result};
use futures_util::future::BoxFuture;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

const DESCRIPTION: &str = "\
Capture HTTP traffic from the user's browser and install mock responses.

Typical loop: the user clicks Copy reference on a request in the Network panel and pastes the \
netref_* id here. Call `handoff_get` to read the full request/response, then `rule_apply` to \
install a mock. `rule_apply` answers `active` only after the browser's Fetch interception has \
actually been enabled; then tell the user the rule is live and to refresh or repeat the action.

Rules apply to every tab by default (`scope: \"all_tabs\"`), so say so explicitly. Use \
`rule_status` to check whether a rule is active and what it has matched. Use `cleanup` only when \
the user asks to delete something; disabling is `rule_apply` with `enabled: false` or the switch \
in the Network panel. Batch cleanup (`target: \"rules\" | \"records\" | \"handoffs\"`) must first \
report the count and wait for the user's confirmation.";

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Status,
    HandoffGet,
    RuleApply,
    RuleStatus,
    Cleanup,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BodyFieldArg {
    /// RFC 6901 JSON Pointer into the request body.
    pointer: String,
    /// Expected JSON value at that pointer.
    equals: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetworkArgs {
    action: Action,
    /// `netref_*` copied from the Network panel.
    handoff_id: Option<String>,
    /// `rule_*` id for status or cleanup.
    rule_id: Option<String>,
    /// `netrec_*` id for cleanup of one captured record.
    record_id: Option<String>,
    /// `all_tabs` (default) or `source_tab`.
    scope: Option<String>,
    /// Query keys the AI decided are dynamic. Values of other keys must match.
    ignore_keys: Option<Vec<String>>,
    /// Reject query keys the sample did not contain. Defaults to lenient.
    strict_query: Option<bool>,
    /// Extra request-body equality conditions.
    body_fields: Option<Vec<BodyFieldArg>>,
    /// Match this exact URL (including query order) instead of the endpoint.
    exact_url: Option<String>,
    /// Mock response: `{status, headers, body, delayMs}`. `body` is
    /// `{kind:"literal", text}` or `{kind:"literal", base64}`.
    response: Option<Value>,
    /// `false` installs the rule disabled; defaults to `true`.
    enabled: Option<bool>,
    /// Return the match preview without persisting.
    dry_run: Option<bool>,
    /// Byte offset into the sample response body.
    body_offset: Option<u64>,
    /// Byte window for the sample response body (default 64 KiB).
    body_limit: Option<u64>,
    /// `rule` | `rules` | `handoff` | `handoffs` | `record` | `records` | `hits`.
    target: Option<String>,
    /// Cleanup only: delete everything of that target.
    all: Option<bool>,
    /// Cleanup only: clear hits for this rule.
    hits_rule_id: Option<String>,
}

pub fn definition() -> crate::framework::ToolDef {
    super::def::<NetworkArgs>(
        "network",
        DESCRIPTION,
        Some(super::open_world_annotations()),
        handler,
    )
}

fn handler<'a>(
    raw: Value,
    ctx: &'a ToolCtx,
    _response: &'a mut crate::response::ToolResponse,
) -> BoxFuture<'a, ToolExecResult<Option<ToolResult>>> {
    Box::pin(async move {
        let args: NetworkArgs = parse_args(raw)?;
        let Some(host) = ctx.network.clone() else {
            return Ok(Some(ToolResult::error("network_unavailable")));
        };
        let result = match args.action {
            Action::Status => host.status().await,
            Action::HandoffGet => {
                let Some(handoff_id) = args.handoff_id else {
                    return Err(ToolError::Message(
                        "handoff_get requires handoffId".to_string(),
                    ));
                };
                host.handoff_get(
                    &handoff_id,
                    args.body_offset.unwrap_or(0),
                    args.body_limit.unwrap_or(64 * 1024),
                )
                .await
            }
            Action::RuleApply => {
                let payload = apply_payload(&args)?;
                host.rule_apply(payload).await
            }
            Action::RuleStatus => {
                host.rule_status(args.rule_id.as_deref(), args.handoff_id.as_deref())
                    .await
            }
            Action::Cleanup => {
                let payload = cleanup_payload(&args)?;
                host.cleanup(payload).await
            }
        };
        match result {
            Ok(value) => {
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string());
                Ok(Some(text_result(text, Some(value))))
            }
            Err(message) => Ok(Some(ToolResult::error(message))),
        }
    })
}

fn apply_payload(args: &NetworkArgs) -> ToolExecResult<Value> {
    let Some(handoff_id) = &args.handoff_id else {
        return Err(ToolError::Message(
            "rule_apply requires handoffId".to_string(),
        ));
    };
    let body_fields = args.body_fields.as_ref().map(|fields| {
        fields
            .iter()
            .map(|field| json!({"pointer": field.pointer, "equals": field.equals}))
            .collect::<Vec<_>>()
    });
    Ok(json!({
        "handoffId": handoff_id,
        "scope": args.scope.clone().unwrap_or_else(|| "all_tabs".to_string()),
        "ignoreKeys": args.ignore_keys,
        "strictQuery": args.strict_query.unwrap_or(false),
        "bodyFields": body_fields,
        "exactUrl": args.exact_url,
        "response": args.response,
        "enabled": args.enabled.unwrap_or(true),
        "dryRun": args.dry_run.unwrap_or(false),
    }))
}

fn cleanup_payload(args: &NetworkArgs) -> ToolExecResult<Value> {
    let Some(target) = &args.target else {
        return Err(ToolError::Message("cleanup requires target".to_string()));
    };
    Ok(json!({
        "target": target,
        "ruleId": args.rule_id,
        "recordId": args.record_id,
        "handoffId": args.handoff_id,
        "hitsRuleId": args.hits_rule_id,
        "all": args.all.unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn apply_args() -> NetworkArgs {
        NetworkArgs {
            action: Action::RuleApply,
            handoff_id: Some("netref_1".to_string()),
            rule_id: None,
            record_id: None,
            scope: None,
            ignore_keys: Some(vec!["nonce".to_string()]),
            strict_query: None,
            body_fields: None,
            exact_url: None,
            response: Some(json!({"status": 200, "body": {"kind": "literal", "text": "{}"}})),
            enabled: None,
            dry_run: None,
            body_offset: None,
            body_limit: None,
            target: None,
            all: None,
            hits_rule_id: None,
        }
    }

    #[test]
    fn rule_apply_payload_defaults_to_all_tabs_and_enabled() {
        let payload = apply_payload(&apply_args()).expect("payload");
        assert_eq!(payload["scope"], "all_tabs");
        assert_eq!(payload["enabled"], true);
        assert_eq!(payload["strictQuery"], false);
    }

    #[test]
    fn cleanup_requires_a_target() {
        let mut args = apply_args();
        args.action = Action::Cleanup;
        args.target = None;
        assert!(cleanup_payload(&args).is_err());
        args.target = Some("rule".to_string());
        args.rule_id = Some("rule_1".to_string());
        let payload = cleanup_payload(&args).expect("payload");
        assert_eq!(payload["target"], "rule");
        assert_eq!(payload["ruleId"], "rule_1");
    }
}
