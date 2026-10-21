//! Host implementation of the `network` MCP tool.

use crate::{
    AppState,
    error::AppError,
    services::network::{
        ApplyRequest, NetworkActionView, NetworkBodyPageView, NetworkHandoffView,
        NetworkNearMissView, NetworkRuleView, handoff::header_lines,
    },
};
use browseros_mcp::NetworkHost;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

pub struct ClawNetworkHost {
    state: AppState,
    /// The agent that last wrote a rule, recorded on every revision.
    author: Option<String>,
}

impl ClawNetworkHost {
    pub fn new(state: AppState, author: Option<String>) -> Self {
        Self { state, author }
    }
}

/// Internal failures never reach the agent with their details; every other
/// error keeps its stable code and message.
fn host_error(operation: &str, error: AppError) -> String {
    match &error {
        AppError::Internal(_) | AppError::Db(_) | AppError::Io { .. } | AppError::Join(_) => {
            tracing::warn!(operation, error = %error, "network tool operation failed");
            format!("{operation} failed: internal_error")
        }
        other => format!("{operation} failed: {other}"),
    }
}

/// Maps `pending` reasons onto the GUI's wording, so the agent and the panel
/// say the same thing.
fn pending_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("no_tab") => "打开目标页面后自动生效",
        Some("apply_timeout") => "浏览器尚未确认",
        _ => "浏览器未连接",
    }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "active" => "生效中",
        "pending" => "等待生效",
        "unbound" => "标签页已关闭",
        "failed" => "无法生效",
        _ => "已停用",
    }
}

/// One line per rule, built from the shared view so MCP and the panel agree.
fn rule_line(rule: &NetworkRuleView) -> String {
    format!(
        "「{}」（{}）{} · {} {} · {} · v{} · 命中 {}",
        rule.name,
        rule.id,
        state_label(&rule.state),
        rule.matcher
            .as_ref()
            .map_or("?", |matcher| matcher.method.as_str()),
        rule.matcher
            .as_ref()
            .map_or("?", |matcher| matcher.pathname.as_str()),
        kind_label(&rule.action),
        rule.current_revision,
        rule.hit_count,
    )
}

/// For call sites that still hold the serialized view.
fn value_kind_label(action: &Value) -> String {
    match action.get("kind").and_then(Value::as_str) {
        Some("processor") => format!(
            "处理器 · 超时 {}ms",
            action["timeoutMs"].as_u64().unwrap_or(200)
        ),
        _ => format!(
            "固定响应 {} · 延迟 {}ms",
            action["status"].as_i64().unwrap_or(200),
            action["delayMs"].as_u64().unwrap_or(0)
        ),
    }
}

/// One-line description of the stored action.
fn view_kind_label(action: &NetworkActionView) -> String {
    match action {
        NetworkActionView::Static {
            status, delay_ms, ..
        } => format!("固定响应 {status} · 延迟 {delay_ms}ms"),
        NetworkActionView::Processor { timeout_ms, .. } => format!("处理器 · 超时 {timeout_ms}ms"),
    }
}

fn kind_label(action: &Option<NetworkActionView>) -> String {
    action
        .as_ref()
        .map_or_else(|| "固定响应".to_string(), view_kind_label)
}

impl NetworkHost for ClawNetworkHost {
    fn status<'a>(&'a self) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let status = self.state.network.status_json().await;
            let handoffs = self
                .state
                .network
                .list_handoffs()
                .await
                .map_err(|error| host_error("status", error))?
                .len();
            let capture = &status["capture"];
            let text = format!(
                "浏览器{} · 采集{} · {} 条记录 · {} 个引用 · 规则 {} 生效 / {} 总\n限制：关闭采集后仍可 mock，但看不到新的命中记录；删除与关停仅影响后续请求。",
                if status["browser"]["connected"] == true {
                    "已连接"
                } else {
                    "未连接"
                },
                if capture["running"] == true {
                    "中"
                } else {
                    "已停止"
                },
                capture["records"].as_u64().unwrap_or(0),
                handoffs,
                status["rules"]["active"].as_u64().unwrap_or(0),
                status["rules"]["total"].as_u64().unwrap_or(0),
            );
            Ok(json!({ "text": text, "status": status }))
        })
    }

    fn handoff_get<'a>(
        &'a self,
        handoff_id: &'a str,
        body_offset: u64,
        body_limit: u64,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let handoff = self
                .state
                .network
                .get_handoff(handoff_id)
                .await
                .map_err(|error| host_error("handoff_get", error))?;
            let window = self
                .state
                .network
                .handoff_body_window(handoff_id, body_offset, body_limit.min(512 * 1024))
                .await
                .map_err(|error| host_error("handoff_get", error))?;
            let (start, end, total, encoding, body_text) = match &window {
                Some(window) => (
                    window.offset as usize,
                    window.length.unwrap_or(0) as usize,
                    window.total,
                    window.encoding.clone(),
                    window.data.clone(),
                ),
                None => (0, 0, 0, "utf8".to_string(), String::new()),
            };
            let hint = if total > 512 * 1024 {
                "\nbody 较大，修改少量字段请用 patch_sample"
            } else {
                ""
            };

            let source = &handoff.source_meta;
            let mut text = String::new();
            text.push_str(&format!(
                "{} {} · {} · {} 条同 endpoint 样本\n",
                source["method"].as_str().unwrap_or("?"),
                source["url"].as_str().unwrap_or("?"),
                source["status"]
                    .as_i64()
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                handoff.related.as_array().map(Vec::len).unwrap_or(0) + 1,
            ));
            text.push_str(&format!(
                "\nmatcherSeed: {}\n",
                serde_json::to_string_pretty(&handoff.matcher_seed).unwrap_or_default()
            ));
            text.push_str("\nrequest headers:\n");
            text.push_str(&header_lines(&handoff.request_headers));
            if let Some(request_body) = &handoff.request_body {
                text.push_str("\n\nrequest body:\n");
                text.push_str(request_body);
            }
            text.push_str("\n\nresponse headers:\n");
            text.push_str(&header_lines(&handoff.response_headers));
            text.push_str(&format!(
                "\n\nresponse body (offset {start} / total {total}, {encoding}):\n{body_text}{hint}"
            ));
            if total > 512 * 1024 {
                text.push_str(
                    "\n\n[body larger than 512 KiB] Prefer `response.body.kind=patch_sample` with a JSON Patch; it is applied server-side without reading the whole sample here.",
                );
            }
            text.push_str(&format!(
                "\n\nsuggestedIgnoreKeys:\n{}\n",
                serde_json::to_string_pretty(&handoff.suggested_ignore).unwrap_or_default()
            ));
            if let Some(rule_id) = &handoff.rule_id {
                text.push_str(&format!("\n已关联规则: {rule_id}\n"));
            } else {
                text.push_str("\n尚未创建规则。\n");
            }

            Ok(json!({
                "text": text,
                // The shared view: HTTP and MCP describe a
                // handoff the same way.
                "handoff": serde_json::to_value(NetworkHandoffView::from_row(&handoff))
                    .unwrap_or(Value::Null),
                "responseBody": serde_json::to_value(NetworkBodyPageView::new(
                    &encoding,
                    start as u64,
                    total,
                    (end as u64) < total,
                    body_text,
                ))
                .unwrap_or(Value::Null),
                "suggestedIgnoreKeys": handoff.suggested_ignore,
                "relatedRecords": handoff.related,
                "ruleId": handoff.rule_id,
            }))
        })
    }

    fn rule_apply<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let mut input: ApplyRequest = serde_json::from_value(args)
                .map_err(|error| format!("invalid_request: {error}"))?;
            if input.author_client.is_none() {
                input.author_client = self.author.clone();
            }
            let outcome = self
                .state
                .network
                .apply_rule(input)
                .await
                .map_err(|error| host_error("rule_apply", error))?;
            let replaced = outcome
                .replaced
                .iter()
                .map(|victim| format!("「{}」", victim.name))
                .collect::<Vec<_>>()
                .join("、");
            let suffix = if replaced.is_empty() {
                String::new()
            } else {
                format!("已替换规则：{replaced}")
            };
            let ignore = if outcome.ignore_keys.is_empty() {
                "无".to_string()
            } else {
                outcome.ignore_keys.join(", ")
            };
            let preview_line = outcome
                .sample_preview
                .as_ref()
                .map(|preview| {
                    format!(
                        "\n样本试跑：{} · {} · {}ms",
                        preview["outcome"].as_str().unwrap_or("?"),
                        preview["status"].as_i64().unwrap_or(0),
                        preview["elapsedMs"].as_u64().unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            let text = match outcome.state {
                "active" => format!(
                    "已生效：规则「{name}」（{endpoint}），作用于{scope}。请刷新页面或重新操作以命中。revision {revision} · 忽略 query: [{ignore}]{suffix}",
                    name = outcome.name,
                    endpoint = outcome.endpoint,
                    scope = if outcome.scope == "all_tabs" {
                        "所有标签页"
                    } else {
                        "标签页"
                    },
                    revision = outcome.revision,
                ),
                "pending" => format!(
                    "已保存，等待生效：规则「{name}」——{reason}。浏览器确认后会自动生效，无需重试；生效前不要让用户刷新验证。{suffix}",
                    name = outcome.name,
                    reason = pending_reason(outcome.reason.as_deref()),
                ),
                "disabled" => format!("规则「{}」已保存但未启用。{suffix}", outcome.name),
                "superseded" => format!(
                    "已丢弃：应用期间用户执行了「停止全部 Mock」，规则「{}」保持停用。",
                    outcome.name
                ),
                "unbound" => format!(
                    "规则「{}」已保存，但绑定的标签页已关闭；请让用户在 Network 面板重新绑定。",
                    outcome.name
                ),
                _ => format!(
                    "预演完成：{} 个历史样本命中；将被替换的规则：[{}]。未写入。",
                    outcome.preview.len(),
                    replaced
                ),
            };
            let text = format!("{text}{preview_line}");
            Ok(json!({
                "text": text,
                "ruleId": outcome.rule_id,
                "name": outcome.name,
                "revision": outcome.revision,
                "state": outcome.state,
                "endpoint": outcome.endpoint,
                "replaced": outcome.replaced,
                "preview": outcome.preview,
            }))
        })
    }

    fn rule_list<'a>(&'a self, query: Option<&'a str>) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let rules = self
                .state
                .network
                .rule_views()
                .await
                .map_err(|error| host_error("rule_list", error))?;
            let needle = query.unwrap_or_default().trim().to_ascii_lowercase();
            let matched = rules
                .iter()
                .filter(|rule| {
                    needle.is_empty()
                        || rule.name.to_ascii_lowercase().contains(&needle)
                        || rule.id.to_ascii_lowercase().contains(&needle)
                        || rule.matcher.as_ref().is_some_and(|matcher| {
                            matcher.pathname.to_ascii_lowercase().contains(&needle)
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            let mut text = format!("共 {} 条规则：\n", matched.len());
            for rule in &matched {
                text.push_str(&rule_line(rule));
                text.push('\n');
            }
            if matched.is_empty() {
                text.push_str("没有规则。让用户复制一个请求引用，或先在 Network 面板创建规则。");
            }
            Ok(json!({ "text": text, "rules": matched }))
        })
    }

    fn rule_get<'a>(
        &'a self,
        rule: &'a str,
        body_offset: u64,
        body_limit: u64,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let detail = self
                .state
                .network
                .get_rule_detail(rule)
                .await
                .map_err(|error| host_error("rule_get", error))?;
            let rule_view = &detail["rule"];
            let action = &detail["revision"]["action"];
            // The detail view keeps the matcher on the revision,
            // in the contract's `NetworkMatcher` shape.
            let matcher = detail["revision"].get("matcher");
            let query_policy = matcher.and_then(|matcher| matcher.get("queryPolicy"));
            let strict = query_policy
                .and_then(|policy| policy["mode"].as_str())
                .is_some_and(|mode| mode == "strict");
            let ignore_keys = query_policy
                .and_then(|policy| policy["ignoreKeys"].as_array())
                .map(|keys| {
                    keys.iter()
                        .filter_map(|key| key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let body_fields = matcher
                .and_then(|matcher| matcher["bodyFields"].as_array())
                .map_or(0, Vec::len);
            let mut text = format!(
                "「{}」（{}）· {} · {} · v{}\nmatcher: {} {} · query {} · 忽略 [{}] · bodyFields {}\n响应: {}\n来源: handoff {} · record {} · 样本 {}",
                rule_view["name"].as_str().unwrap_or("?"),
                rule_view["id"].as_str().unwrap_or("?"),
                state_label(rule_view["state"].as_str().unwrap_or("disabled")),
                rule_view["scope"].as_str().unwrap_or("all_tabs"),
                rule_view["currentRevision"].as_u64().unwrap_or(0),
                matcher
                    .and_then(|matcher| matcher["method"].as_str())
                    .unwrap_or("?"),
                matcher
                    .and_then(|matcher| matcher["pathname"].as_str())
                    .unwrap_or("?"),
                if strict { "严格" } else { "宽松" },
                ignore_keys,
                body_fields,
                value_kind_label(action),
                detail["revision"]["source"]["handoffId"]
                    .as_str()
                    .unwrap_or("-"),
                detail["revision"]["source"]["recordId"]
                    .as_str()
                    .unwrap_or("-"),
                match detail["sample"]["available"].as_bool() {
                    Some(true) => format!(
                        "可用（{} 字节）",
                        detail["sample"]["bytes"].as_u64().unwrap_or(0)
                    ),
                    _ => "不可用".to_string(),
                },
            );
            if action["kind"].as_str() == Some("processor") {
                text.push_str(&format!(
                    "\n处理器脚本（超时 {}ms）:\n{}",
                    action["timeoutMs"].as_u64().unwrap_or(200),
                    action["script"].as_str().unwrap_or_default()
                ));
            } else {
                let body = self
                    .state
                    .network
                    .rule_body_window(rule, body_offset, body_limit.min(512 * 1024))
                    .await
                    .map_err(|error| host_error("rule_get", error))?;
                if let Some(body) = body {
                    text.push_str(&format!(
                        "\nbody（offset {} / total {}, {}）:\n{}",
                        body.offset, body.total, body.encoding, body.data
                    ));
                }
            }
            Ok(json!({ "text": text, "detail": detail }))
        })
    }

    fn rule_status<'a>(
        &'a self,
        rule_id: Option<&'a str>,
        handoff_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let resolved = match (rule_id, handoff_id) {
                (Some(rule_id), _) => Some(rule_id.to_string()),
                (None, Some(handoff_id)) => self
                    .state
                    .network
                    .rule_by_handoff(handoff_id)
                    .await
                    .map_err(|error| host_error("rule_status", error))?
                    .map(|rule| rule.id),
                (None, None) => {
                    return Err("rule_status requires ruleId or handoffId".to_string());
                }
            };
            let Some(rule_id) = resolved else {
                return Ok(json!({
                    "text": "该引用尚未创建规则。",
                    "ruleId": null,
                    "nearMisses": [{"reason": "no_request_seen"}],
                }));
            };
            let detail = self
                .state
                .network
                .rule_detail_view(&rule_id)
                .await
                .map_err(|error| host_error("rule_status", error))
                .and_then(|view| {
                    serde_json::to_value(view)
                        .map_err(|error| format!("rule_status failed: internal_error: {error}"))
                })?;
            let hits = detail["hits"].as_array().cloned().unwrap_or_default();
            let mut near_misses = self
                .state
                .network
                .near_misses(&rule_id)
                .await
                .map_err(|error| host_error("rule_status", error))?;
            if hits.is_empty() && near_misses.is_empty() {
                near_misses.push(NetworkNearMissView::reason_only("no_request_seen"));
            }
            let mut text = format!(
                "规则 {} · 状态 {} · revision {}\n",
                rule_id, detail["rule"]["state"], detail["rule"]["currentRevision"]
            );
            if hits.is_empty() {
                text.push_str("最近没有命中。若页面未变，确认是否刷新或重新操作。\n");
                for miss in near_misses.iter().take(3) {
                    text.push_str(&format!(
                        "- near miss: {} {} · {}\n",
                        miss.method.as_deref().unwrap_or("?"),
                        miss.url.as_deref().unwrap_or("?"),
                        miss.reason
                    ));
                }
            } else {
                text.push_str("最近命中:\n");
                for hit in hits.iter().take(10) {
                    text.push_str(&format!(
                        "- {} {} · {} · {}\n",
                        hit["method"].as_str().unwrap_or("?"),
                        hit["url"].as_str().unwrap_or("?"),
                        hit["outcome"].as_str().unwrap_or("?"),
                        hit["at"].as_i64().unwrap_or_default()
                    ));
                }
            }
            Ok(json!({
                "text": text,
                "rule": rule_id,
                "rule": detail["rule"],
                "revision": detail["revision"],
                "hits": hits,
                "nearMisses": near_misses,
            }))
        })
    }

    fn cleanup<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let target = args
                .get("target")
                .and_then(Value::as_str)
                .ok_or_else(|| "cleanup requires target".to_string())?;
            let rule_id = args.get("rule").and_then(Value::as_str);
            let handoff_id = args.get("handoffId").and_then(Value::as_str);
            let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);

            match target {
                "rule" => {
                    let Some(id) = rule_id.or(handoff_id) else {
                        return Err("cleanup rule requires rule".to_string());
                    };
                    let id = if id.starts_with("rule_") {
                        id.to_string()
                    } else {
                        self.state
                            .network
                            .rule_by_handoff(id)
                            .await
                            .map_err(|error| host_error("cleanup", error))?
                            .map(|rule| rule.id)
                            .ok_or_else(|| "not_found: no rule for this handoff".to_string())?
                    };
                    let value = self
                        .state
                        .network
                        .delete_rule(&id)
                        .await
                        .map_err(|error| host_error("cleanup", error))?;
                    Ok(json!({
                        "text": format!("已删除规则 {id}。页面下一次请求将恢复真实数据；8 秒内可在 GUI 撤销。"),
                        "deleted": 1,
                        "target": "rule",
                        "undoAvailable": true,
                        "value": value,
                    }))
                }
                "handoff" => {
                    let Some(handoff_id) = handoff_id else {
                        return Err("cleanup handoff requires handoffId".to_string());
                    };
                    let value = self
                        .state
                        .network
                        .delete_handoff(handoff_id)
                        .await
                        .map_err(|error| host_error("cleanup", error))?;
                    Ok(json!({
                        "text": format!("已删除引用 {handoff_id}。"),
                        "deleted": 1,
                        "target": "handoff",
                        "value": value,
                    }))
                }
                "record" => {
                    let Some(record_id) = args.get("recordId").and_then(Value::as_str) else {
                        return Err("cleanup record requires recordId".to_string());
                    };
                    let value = self
                        .state
                        .network
                        .delete_record(record_id)
                        .await
                        .map_err(|error| host_error("cleanup", error))?;
                    Ok(json!({
                        "text": format!("已删除记录 {record_id}。"),
                        "deleted": 1,
                        "target": "record",
                        "value": value,
                    }))
                }
                "hits" => {
                    let Some(rule_id) = rule_id else {
                        return Err("cleanup hits requires rule".to_string());
                    };
                    let value = self
                        .state
                        .network
                        .delete_hits(Some(rule_id))
                        .await
                        .map_err(|error| host_error("cleanup", error))?;
                    Ok(json!({
                        "text": format!("已清空规则 {} 的命中历史。", rule_id),
                        "deleted": value["deleted"].clone(),
                        "target": "hits",
                        "value": value,
                    }))
                }
                "rules" | "handoffs" | "records" if all => {
                    let status = self.state.network.status_json().await;
                    let count = match target {
                        "rules" => status["rules"]["total"].as_u64().unwrap_or(0),
                        "records" => status["capture"]["records"].as_u64().unwrap_or(0),
                        _ => self
                            .state
                            .network
                            .list_handoffs()
                            .await
                            .map_err(|error| host_error("cleanup", error))?
                            .len() as u64,
                    };
                    Ok(json!({
                        "text": format!(
                            "本版本不支持批量删除（{target} 共 {count} 个）；请改为逐个删除。"
                        ),
                        "error": "confirm_required",
                        "count": count,
                        "deleted": 0,
                        "canExecute": false,
                        "target": target,
                    }))
                }
                "rules" | "handoffs" | "records" => Ok(json!({
                    "text": format!("cleanup {target} requires all: true after the user confirms the count."),
                    "error": "confirm_required",
                    "deleted": 0,
                    "target": target,
                })),
                other => Err(format!("unsupported cleanup target: {other}")),
            }
        })
    }
}
