//! REST and SSE surface for network capture, handoffs and mock rules.

use super::{error, internal};
use crate::{
    AppState,
    error::{AppError, CanonicalError, RequestId},
    services::network::NetworkRecordListView,
};
use axum::{
    Extension,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, convert::Infallible, time::Duration};
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordQuery {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    500
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandoffRequest {
    record_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreRequest {
    undo_token: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyQuery {
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_body_limit")]
    limit: u64,
}

fn default_body_limit() -> u64 {
    64 * 1024
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitsQuery {
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default = "default_hits_limit")]
    limit: u32,
}

fn default_hits_limit() -> u32 {
    50
}

/// Stable error codes. `AppError` messages start with their
/// code, so the REST body reports the same code as the MCP text instead of a
/// status-derived generic one.
const ERROR_CODES: &[&str] = &[
    "rule_not_found",
    "rule_name_taken",
    "invalid_rule_name",
    "invalid_request",
    "invalid_scope",
    "tab_unbound",
    "tab_required",
    "handoff_not_found",
    "handoff_expired",
    "sample_body_unavailable",
    "invalid_response",
    "invalid_processor",
    "processor_failed_on_sample",
    "superseded",
    "fetch_not_disabled",
    "not_a_processor",
    "undo_expired",
    "tab_not_found",
    "browser_unavailable",
];

fn fail(request_id: &RequestId, source: AppError) -> CanonicalError {
    if matches!(
        source,
        AppError::Internal(_) | AppError::Db(_) | AppError::Io { .. } | AppError::Join(_)
    ) {
        return internal(request_id, source);
    }
    let status = source.status();
    let message = source.to_string();
    let code = ERROR_CODES
        .iter()
        .find(|code| message.starts_with(**code))
        .copied()
        .unwrap_or(match status.as_u16() {
            400 | 422 => "invalid_request",
            403 => "forbidden",
            404 => "not_found",
            409 => "conflict",
            410 => "gone",
            503 => "unavailable",
            _ => "internal_error",
        });
    error(request_id, status, code, &message)
}

pub async fn state(
    Extension(_request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    Ok(axum::Json(state.network.status_json().await))
}

pub async fn start_capture(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .start_capture()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn stop_capture(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .stop_capture()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn records(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Query(query): Query<RecordQuery>,
) -> Result<axum::Json<Value>, CanonicalError> {
    let records = state
        .network
        .list_records(query.since, query.limit.clamp(1, 2000))
        .await;
    let _ = request_id;
    Ok(axum::Json(json!(NetworkRecordListView { records })))
}

pub async fn record(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(record_id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    match state.network.get_record(&record_id).await {
        Some(record) => Ok(axum::Json(json!(record))),
        None => Err(super::error(
            &request_id,
            axum::http::StatusCode::NOT_FOUND,
            "not_found",
            "record_not_found",
        )),
    }
}

pub async fn request_body(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(record_id): Path<String>,
    Query(query): Query<BodyQuery>,
) -> Result<axum::Json<Value>, CanonicalError> {
    match state
        .network
        .request_body_page(&record_id, query.offset, query.limit.clamp(1, 262_144))
        .await
    {
        Ok(Some(page)) => Ok(axum::Json(json!(page))),
        Ok(None) => Err(super::error(
            &request_id,
            axum::http::StatusCode::NOT_FOUND,
            "not_found",
            "record_not_found",
        )),
        Err(error) => Err(fail(&request_id, error)),
    }
}

pub async fn response_body(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(record_id): Path<String>,
    Query(query): Query<BodyQuery>,
) -> Result<axum::Json<Value>, CanonicalError> {
    match state
        .network
        .response_body_page(&record_id, query.offset, query.limit.clamp(1, 262_144))
        .await
    {
        Ok(Some(page)) => Ok(axum::Json(json!(page))),
        Ok(None) => Err(super::error(
            &request_id,
            axum::http::StatusCode::NOT_FOUND,
            "not_found",
            "record_not_found",
        )),
        Err(error) => Err(fail(&request_id, error)),
    }
}

pub async fn delete_record(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(record_id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .delete_record(&record_id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn delete_records(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .delete_all_records()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn restore_record(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    axum::Json(body): axum::Json<RestoreRequest>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .restore_record(&body.undo_token)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn create_handoff(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    axum::Json(body): axum::Json<HandoffRequest>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .create_handoff(&body.record_id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn list_handoffs(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .list_handoffs()
        .await
        .map(|handoffs| axum::Json(json!({ "handoffs": handoffs })))
        .map_err(|error| fail(&request_id, error))
}

pub async fn get_handoff(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .get_handoff(&id)
        .await
        .map(|handoff| {
            axum::Json(json!({
                "handoff": {
                    "id": handoff.id,
                    "createdAt": handoff.created_at,
                    "expiresAt": handoff.expires_at,
                    "ruleId": handoff.rule_id,
                    "source": handoff.source_meta,
                    "requestHeaders": handoff.request_headers,
                    "responseHeaders": handoff.response_headers,
                    "requestBody": handoff.request_body,
                    "bodyState": handoff.body_state,
                    "matcherSeed": handoff.matcher_seed,
                    "suggestedIgnoreKeys": handoff.suggested_ignore,
                    "relatedRecords": handoff.related,
                }
            }))
        })
        .map_err(|error| fail(&request_id, error))
}

pub async fn delete_handoff(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .delete_handoff(&id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn rules(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .list_rules()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn patch_rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<axum::Json<Value>, JsonRejection>,
) -> Result<axum::Json<Value>, CanonicalError> {
    let axum::Json(patch) = body.map_err(|rejection| {
        error(
            &request_id,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            &rejection.body_text(),
        )
    })?;
    state
        .network
        .patch_rule(&id, &patch)
        .await
        .map(axum::Json)
        .map_err(|source| fail(&request_id, source))
}

pub async fn test_rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .test_rule(&id)
        .await
        .map(axum::Json)
        .map_err(|source| fail(&request_id, source))
}

pub async fn revisions(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .list_revisions(&id)
        .await
        .map(axum::Json)
        .map_err(|source| fail(&request_id, source))
}

pub async fn restore_revision(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path((id, revision)): Path<(String, u32)>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .restore_revision(&id, revision)
        .await
        .map(axum::Json)
        .map_err(|source| fail(&request_id, source))
}

pub async fn delete_rules(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .delete_all_rules()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn processor_test(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .processor_test(&id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn reload_tab(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(tab_id): Path<i64>,
    body: Result<axum::Json<Value>, JsonRejection>,
) -> Result<StatusCode, CanonicalError> {
    let axum::Json(value) = body.map_err(|rejection| {
        error(
            &request_id,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            &rejection.body_text(),
        )
    })?;
    let ignore_cache = value
        .get("ignoreCache")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    state
        .network
        .reload_tab(tab_id, ignore_cache)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|error| fail(&request_id, error))
}

pub async fn rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .get_rule_detail(&id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn delete_rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .delete_rule(&id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn restore_rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .restore_rule(&id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn stop_all(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .stop_all()
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

pub async fn hits(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Query(query): Query<HitsQuery>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .list_hits(query.rule_id.as_deref(), query.limit.clamp(1, 200))
        .await
        .map(|hits| axum::Json(json!({ "hits": hits })))
        .map_err(|error| fail(&request_id, error))
}

pub async fn delete_hits(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Query(query): Query<HitsQuery>,
) -> Result<axum::Json<Value>, CanonicalError> {
    let Some(rule_id) = query.rule_id else {
        return Err(super::error(
            &request_id,
            axum::http::StatusCode::BAD_REQUEST,
            "invalid_request",
            "ruleId is required",
        ));
    };
    state
        .network
        .delete_hits(Some(&rule_id))
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
}

/// Monotonic progress rank for a record; a live event must never move a record
/// backwards in state.
#[must_use]
pub fn state_rank(value: &Value) -> u8 {
    match value.get("state").and_then(Value::as_str) {
        Some("created") => 0,
        Some("response_ready") => 1,
        Some("done") | Some("failed") => 2,
        _ => 2,
    }
}

/// `true` when a live `record` event would repeat what the backlog already
/// sent. The backlog is read after `subscribe()`, so the same seq can arrive
/// twice; without this the GUI rewinds a finished record to its initial state.
#[must_use]
pub fn should_skip(sent: &HashMap<u64, u8>, seq: Option<u64>, data: &Value) -> bool {
    let Some(seq) = seq else {
        return false;
    };
    let Some(previous) = sent.get(&seq) else {
        return false;
    };
    state_rank(data) <= *previous
}

pub async fn events(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl stream::Stream<Item = Result<Event, Infallible>>>, CanonicalError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut receiver = state.network.subscribe();
    let snapshot = state.network.status_json().await;
    let mut reset = false;
    let backlog = match last_event_id {
        Some(seq) => {
            let oldest = state.network.oldest_seq().await;
            if oldest.is_some_and(|oldest| oldest > seq.saturating_add(1)) {
                reset = true;
                Vec::new()
            } else {
                let records = state.network.list_records(Some(seq), 2000).await;
                if records.len() >= 2000 {
                    reset = true;
                    Vec::new()
                } else {
                    records
                }
            }
        }
        None => Vec::new(),
    };
    let mut sent: HashMap<u64, u8> = HashMap::new();
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);

    tokio::spawn(async move {
        let first = Event::default()
            .event("snapshot")
            .data(snapshot.to_string());
        if tx.send(Ok(first)).await.is_err() {
            return;
        }
        if reset {
            let reset = Event::default()
                .event("gap")
                .data(json!({ "reset": true }).to_string());
            if tx.send(Ok(reset)).await.is_err() {
                return;
            }
        }
        for record in backlog {
            let payload = json!({ "record": record });
            sent.insert(record.seq, state_rank(&payload));
            let event = Event::default()
                .event("record")
                .id(record.seq.to_string())
                .data(payload.to_string());
            if tx.send(Ok(event)).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                () = tx.closed() => return,
                received = receiver.recv() => {
                    match received {
                        Ok(event) => {
                            let seq = event
                                .data
                                .get("record")
                                .and_then(|record| record.get("seq"))
                                .and_then(Value::as_u64);
                            if event.event == "record" && should_skip(&sent, seq, &event.data) {
                                continue;
                            }
                            if let Some(seq) = seq
                                && let Some(rank) = sent.get_mut(&seq)
                            {
                                *rank = (*rank).max(state_rank(&event.data));
                            } else if let Some(seq) = seq {
                                sent.insert(seq, state_rank(&event.data));
                            }
                            let mut sse = Event::default().event(event.event).data(event.data.to_string());
                            if let Some(seq) = seq {
                                sse = sse.id(seq.to_string());
                            }
                            if tx.send(Ok(sse)).await.is_err() {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                            let reset = Event::default()
                                .event("gap")
                                .data(json!({ "lost": lost }).to_string());
                            if tx.send(Ok(reset)).await.is_err() {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    });
    let _ = request_id;
    Ok(Sse::new(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }))
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[cfg(test)]
mod sse_dedupe_tests {
    #![allow(clippy::expect_used)]
    use super::{should_skip, state_rank};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn the_live_event_cannot_rewind_a_record() {
        let mut sent = HashMap::new();
        let finished = json!({ "record": { "seq": 7, "state": "done" } });
        sent.insert(7, state_rank(&finished));

        // Same state again, and every earlier state, are repeats.
        assert!(should_skip(&sent, Some(7), &finished));
        assert!(should_skip(
            &sent,
            Some(7),
            &json!({ "record": { "seq": 7, "state": "created" } })
        ));
        assert!(should_skip(
            &sent,
            Some(7),
            &json!({ "record": { "seq": 7, "state": "response_ready" } })
        ));
    }

    #[test]
    fn unknown_and_new_records_are_forwarded() {
        let sent = HashMap::new();
        assert!(!should_skip(
            &sent,
            Some(9),
            &json!({ "record": { "seq": 9, "state": "created" } })
        ));
        // Events with no record seq (rule, hit, capture) are never filtered.
        assert!(!should_skip(&sent, None, &json!({ "rule": {} })));
        assert_eq!(state_rank(&json!({ "state": "response_ready" })), 1);
    }
}
