//! Host implementation of the `network` MCP tool.

use crate::{
    AppState,
    error::AppError,
    services::network::{ApplyInput, handoff::header_lines},
};
use browseros_mcp::NetworkHost;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

pub struct ClawNetworkHost {
    state: AppState,
}

impl ClawNetworkHost {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

fn host_error(operation: &str, error: AppError) -> String {
    format!("{operation} failed: {error}")
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
            let body = self
                .state
                .network
                .handoff_body(handoff_id)
                .await
                .map_err(|error| host_error("handoff_get", error))?
                .unwrap_or_default();
            let total = body.len() as u64;
            let start = body_offset.min(total) as usize;
            let end = start
                .saturating_add((body_limit.min(512 * 1024)) as usize)
                .min(body.len());
            let (encoding, body_text) = text_or_base64(&body[start..end]);
            let (encoding, body_text) = if total > 512 * 1024 {
                (
                    "summary",
                    format!(
                        "body omitted ({total} bytes); use patch_sample to edit without reading it"
                    ),
                )
            } else {
                (encoding, body_text)
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
                "\n\nresponse body (offset {start} / total {total}, {encoding}):\n{body_text}"
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
                "handoffId": handoff.id,
                "source": handoff.source_meta,
                "matcherSeed": handoff.matcher_seed,
                "requestHeaders": handoff.request_headers,
                "requestBody": handoff.request_body,
                "responseHeaders": handoff.response_headers,
                "responseBody": {
                    "encoding": encoding,
                    "offset": start,
                    "total": total,
                    "truncated": (end as u64) < total,
                    "data": body_text,
                },
                "suggestedIgnoreKeys": handoff.suggested_ignore,
                "relatedRecords": handoff.related,
                "ruleId": handoff.rule_id,
            }))
        })
    }

    fn rule_apply<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let input: ApplyInput = serde_json::from_value(args)
                .map_err(|error| format!("invalid_request: {error}"))?;
            let outcome = self
                .state
                .network
                .apply_rule(input)
                .await
                .map_err(|error| host_error("rule_apply", error))?;
            let text = match outcome.state.as_str() {
                "active" => format!(
                    "已生效：{} {}，作用于{}。请刷新页面或重新操作以命中。\n规则 {} · revision {} · 忽略 query: [{}]",
                    outcome.scope,
                    outcome.rule_id,
                    if outcome.scope == "all_tabs" {
                        "所有标签页"
                    } else {
                        "来源标签页"
                    },
                    outcome.rule_id,
                    outcome.revision,
                    if outcome.ignore_keys.is_empty() {
                        "无".to_string()
                    } else {
                        outcome.ignore_keys.join(", ")
                    },
                ),
                "disabled" => format!("规则 {} 已保存但未启用。", outcome.rule_id),
                "dry_run" => format!(
                    "预演完成：{} 个历史样本；将被替换的规则: [{}]；winner: {}。未写入。",
                    outcome.preview.len(),
                    outcome.replaced.join(", "),
                    outcome
                        .preview
                        .iter()
                        .map(|item| format!(
                            "{} -> {}",
                            item["recordId"].as_str().unwrap_or("?"),
                            item["winnerRuleId"].as_str().unwrap_or("?")
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                "superseded" => {
                    "已丢弃：规则应用期间发生了停止或更新的 apply，规则保持停用。".to_string()
                }
                "failed" => format!(
                    "未生效：{}。数据库内容已保留，可重试。",
                    outcome.reason.as_deref().unwrap_or("apply_failed")
                ),
                other => format!("规则状态：{other}"),
            };
            Ok(json!({ "text": text, "result": outcome }))
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
                .get_rule_detail(&rule_id)
                .await
                .map_err(|error| host_error("rule_status", error))?;
            let hits = detail["hits"].as_array().cloned().unwrap_or_default();
            let mut near_misses = self
                .state
                .network
                .near_misses(&rule_id)
                .await
                .map_err(|error| host_error("rule_status", error))?;
            if hits.is_empty() && near_misses.is_empty() {
                near_misses.push(json!({ "reason": "no_request_seen" }));
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
                        miss["method"].as_str().unwrap_or("?"),
                        miss["url"].as_str().unwrap_or("?"),
                        miss["reason"].as_str().unwrap_or("unknown")
                    ));
                    if let Some(diffs) = miss["queryDiffs"].as_array() {
                        for diff in diffs {
                            text.push_str(&format!(
                                "  query {}: sample={} actual={}\n",
                                diff["key"].as_str().unwrap_or("?"),
                                diff["sample"],
                                diff["actual"]
                            ));
                        }
                    }
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
                "ruleId": rule_id,
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
            let rule_id = args.get("ruleId").and_then(Value::as_str);
            let handoff_id = args.get("handoffId").and_then(Value::as_str);
            let hits_rule_id = args.get("hitsRuleId").and_then(Value::as_str);
            let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);

            match target {
                "rule" => {
                    let Some(id) = rule_id.or(handoff_id) else {
                        return Err("cleanup rule requires ruleId".to_string());
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
                    let Some(rule_id) = hits_rule_id.or(rule_id) else {
                        return Err("cleanup hits requires ruleId".to_string());
                    };
                    let value = self
                        .state
                        .network
                        .delete_hits(rule_id)
                        .await
                        .map_err(|error| host_error("cleanup", error))?;
                    Ok(json!({
                        "text": format!("已清空规则 {rule_id} 的命中历史。"),
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

fn text_or_base64(bytes: &[u8]) -> (&'static str, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => ("utf8", text.to_string()),
        Err(_) => ("base64", browseros_core::network::base64_encode(bytes)),
    }
}
