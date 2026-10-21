use crate::framework::{ToolCtx, ToolError, ToolExecResult, ToolResult, parse_args, text_result};
use futures_util::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DESCRIPTION: &str = "\
Capture HTTP traffic from the user's browser and install mock rules.

Typical loop: the user clicks Copy reference on a request in the Network panel and pastes the \
netref_* id here. Call `handoff_get` to read the full request/response, then `rule_apply` with a \
short `name` to install a mock. Rules have names: later, pass the name (or the rule_* id) as \
`rule` to `rule_get`, `rule_apply`, `rule_status` or `cleanup`; `rule_list` lists them. \
`rule_apply` answers `active` only after the browser's Fetch interception is enabled; `pending` \
means it is saved and waiting for the browser and goes live automatically. Only then tell the \
user the rule is live and to refresh or repeat the action.

For edits that depend on live upstream data, pass `processor: {script}` instead of `response`: \
the script defines a synchronous `function process({request, response})` and returns \
`{status?, statusText?, headers?, setHeaders?, body?}` or `undefined`. It runs after the real \
response arrives and may rewrite any response header, including `set-cookie`; failures pass \
the original response through.

Rules apply to every tab by default (`scope: \"all_tabs\"`), so say so explicitly. Use \
`rule_status` to check whether a rule is active and what it has matched. Use `cleanup` only when \
the user asks to delete something; disabling is `rule_apply` with `rule` and `enabled: false`, \
or the switch in the Network panel. Batch cleanup (`target: \"rules\" | \"records\" | \
\"handoffs\"`) must first report the count and wait for the user's confirmation.";

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Status,
    HandoffGet,
    RuleList,
    RuleGet,
    RuleApply,
    RuleStatus,
    Cleanup,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ScopeArg {
    AllTabs,
    Tab,
}

impl ScopeArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::AllTabs => "all_tabs",
            Self::Tab => "tab",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BodyFieldArg {
    /// RFC 6901 JSON Pointer into the request body.
    pointer: String,
    /// Expected JSON value at that pointer.
    equals: Value,
}

/// One header value, or several for the same name (several `set-cookie` lines).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum OneOrManyArg {
    One(String),
    Many(Vec<String>),
}

/// Mirrors the stored action's body kinds, so the MCP schema and the host
/// agree without a translation layer.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BodyArg {
    Literal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
    },
    /// RFC 6902 patch applied to the rule's sample body.
    PatchSample {
        #[serde(rename = "jsonPatch")]
        json_patch: Vec<Value>,
    },
    /// RFC 6902 patch applied to the rule's current body; update-only.
    PatchCurrent {
        #[serde(rename = "jsonPatch")]
        json_patch: Vec<Value>,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseArg {
    status: Option<i64>,
    /// Header overrides. A `null` value deletes the header; an array writes
    /// several lines of the same name.
    headers: Option<std::collections::BTreeMap<String, Option<OneOrManyArg>>>,
    body: Option<BodyArg>,
    delay_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetworkArgs {
    action: Action,
    /// `netref_*` copied from the Network panel.
    handoff_id: Option<String>,
    /// A rule name, or its `rule_*` id.
    rule: Option<String>,
    /// Short scenario name for a new rule, or the new name when updating one.
    name: Option<String>,
    /// Substring filter for `rule_list` (matches the name or the id).
    query: Option<String>,
    /// `netrec_*` id for cleanup of one captured record.
    record_id: Option<String>,
    /// `all_tabs` (default) or `tab`.
    scope: Option<ScopeArg>,
    /// Tab id when `scope` is `tab`.
    tab_id: Option<i64>,
    /// Query keys the AI decided are dynamic. Values of other keys must match.
    ignore_keys: Option<Vec<String>>,
    /// Reject query keys the sample did not contain. Defaults to lenient.
    strict_query: Option<bool>,
    /// Extra request-body equality conditions.
    body_fields: Option<Vec<BodyFieldArg>>,
    /// Match this exact URL (including query order) instead of the endpoint.
    exact_url: Option<String>,
    /// Mock response; mutually exclusive with `processor`.
    response: Option<ResponseArg>,
    /// Response-stage script. Must define `function process(ctx)`.
    processor_script: Option<String>,
    /// Script budget in milliseconds (50-1000, default 200).
    processor_timeout_ms: Option<u32>,
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
                let Some(handoff_id) = args.handoff_id.as_deref() else {
                    return Err(ToolError::Message(
                        "handoff_get requires handoffId".to_string(),
                    ));
                };
                host.handoff_get(
                    handoff_id,
                    args.body_offset.unwrap_or(0),
                    args.body_limit.unwrap_or(64 * 1024),
                )
                .await
            }
            Action::RuleList => host.rule_list(args.query.as_deref()).await,
            Action::RuleGet => {
                let Some(rule) = args.rule.as_deref() else {
                    return Err(ToolError::Message("rule_get requires rule".to_string()));
                };
                host.rule_get(
                    rule,
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
                if args.rule.is_none() && args.handoff_id.is_none() {
                    return Err(ToolError::Message(
                        "rule_status requires rule or handoffId".to_string(),
                    ));
                }
                host.rule_status(args.rule.as_deref(), args.handoff_id.as_deref())
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
    if args.handoff_id.is_none() && args.rule.is_none() {
        return Err(ToolError::Message(
            "rule_apply requires handoffId or rule".to_string(),
        ));
    }
    if args.response.is_some() && args.processor_script.is_some() {
        return Err(ToolError::Message(
            "response and processor are mutually exclusive".to_string(),
        ));
    }
    let body_fields = args.body_fields.as_ref().map(|fields| {
        fields
            .iter()
            .map(|field| json!({"pointer": field.pointer, "equals": field.equals}))
            .collect::<Vec<_>>()
    });
    let response = args.response.as_ref().map(|response| {
        let body = response.body.as_ref().map(|body| json!(body));
        let mut payload = json!({});
        if let Some(status) = response.status {
            payload["status"] = json!(status);
        }
        if let Some(headers) = &response.headers {
            payload["headers"] = json!(headers);
        }
        if let Some(body) = body {
            payload["body"] = body;
        }
        if let Some(delay_ms) = response.delay_ms {
            payload["delayMs"] = json!(delay_ms);
        }
        payload
    });
    // The request type denies unknown fields, so the processor block is only
    // sent when a script was provided.
    let mut payload = json!({
        "handoffId": args.handoff_id,
        "rule": args.rule,
        "name": args.name,
        "scope": args.scope.map(|scope| scope.as_str()),
        "tabId": args.tab_id,
        "ignoreKeys": args.ignore_keys,
        "strictQuery": args.strict_query,
        "bodyFields": body_fields,
        "exactUrl": args.exact_url,
        "response": response,
        "enabled": args.enabled,
        "dryRun": args.dry_run.unwrap_or(false),
    });
    if let Some(script) = args.processor_script.as_ref() {
        payload["processor"] = json!({
            "script": script,
            "timeoutMs": args.processor_timeout_ms,
        });
    }
    Ok(payload)
}

fn cleanup_payload(args: &NetworkArgs) -> ToolExecResult<Value> {
    let Some(target) = &args.target else {
        return Err(ToolError::Message("cleanup requires target".to_string()));
    };
    Ok(json!({
        "target": target,
        "rule": args.rule,
        "recordId": args.record_id,
        "handoffId": args.handoff_id,
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
            rule: None,
            name: None,
            query: None,
            record_id: None,
            scope: None,
            tab_id: None,
            ignore_keys: Some(vec!["nonce".to_string()]),
            strict_query: None,
            body_fields: None,
            exact_url: None,
            response: None,
            processor_script: None,
            processor_timeout_ms: None,
            enabled: None,
            dry_run: None,
            body_offset: None,
            body_limit: None,
            target: None,
            all: None,
        }
    }

    #[test]
    fn rule_apply_accepts_a_rule_without_a_handoff() {
        let mut args = apply_args();
        args.handoff_id = None;
        args.rule = Some("商品列表-空数据".to_string());
        args.response = Some(ResponseArg {
            status: Some(200),
            headers: None,
            body: Some(BodyArg::Literal {
                text: Some("{}".to_string()),
                base64: None,
            }),
            delay_ms: None,
        });
        let payload = apply_payload(&args).expect("payload");
        assert_eq!(payload["rule"], "商品列表-空数据");
        assert_eq!(payload["response"]["body"]["kind"], "literal");
        assert!(
            payload["scope"].is_null(),
            "no scope defaults at this layer"
        );
    }

    #[test]
    fn rule_apply_omits_the_processor_block_without_a_script() {
        let payload = apply_payload(&apply_args()).expect("payload");
        assert!(
            payload.get("processor").is_none(),
            "ApplyRequest denies unknown fields, so a null block breaks every apply"
        );
    }

    #[test]
    fn rule_apply_requires_a_target() {
        let mut args = apply_args();
        args.handoff_id = None;
        assert!(apply_payload(&args).is_err());
    }

    #[test]
    fn header_patch_arguments_keep_multiple_values() {
        let mut args = apply_args();
        args.response = Some(ResponseArg {
            status: Some(200),
            headers: Some(std::collections::BTreeMap::from([
                (
                    "set-cookie".to_string(),
                    Some(OneOrManyArg::Many(vec![
                        "a=1; Path=/".to_string(),
                        "b=2; Path=/".to_string(),
                    ])),
                ),
                ("x-debug".to_string(), None),
            ])),
            body: None,
            delay_ms: None,
        });
        let payload = apply_payload(&args).expect("payload");
        let headers = &payload["response"]["headers"];
        assert_eq!(headers["set-cookie"][0], "a=1; Path=/");
        assert!(headers["x-debug"].is_null());
    }

    #[test]
    fn scope_is_an_enum_not_a_free_string() {
        let parsed: Result<NetworkArgs, _> = serde_json::from_value(json!({
            "action": "rule_apply",
            "rule": "rule_1",
            "scope": "source_tab",
        }));
        assert!(parsed.is_err(), "unknown scope values are rejected");
    }

    #[test]
    fn cleanup_requires_a_target() {
        let mut args = apply_args();
        args.action = Action::Cleanup;
        args.target = None;
        assert!(cleanup_payload(&args).is_err());
        args.target = Some("rule".to_string());
        args.rule = Some("rule_1".to_string());
        let payload = cleanup_payload(&args).expect("payload");
        assert_eq!(payload["target"], "rule");
        assert_eq!(payload["rule"], "rule_1");
    }
}
