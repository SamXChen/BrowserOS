//! REST and SSE surface for network capture, handoffs and mock rules.

use super::{error, internal};
use crate::{
    AppState,
    error::{AppError, CanonicalError, RequestId},
};
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchRule {
    enabled: Option<bool>,
    scope: Option<String>,
    tab_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IgnoreKeysRequest {
    keys: Vec<String>,
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

fn fail(request_id: &RequestId, source: AppError) -> CanonicalError {
    if matches!(
        source,
        AppError::Internal(_) | AppError::Db(_) | AppError::Io { .. } | AppError::Join(_)
    ) {
        return internal(request_id, source);
    }
    let status = source.status();
    let code = match status.as_u16() {
        400 | 422 => "invalid_request",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        410 => "gone",
        503 => "unavailable",
        _ => "internal_error",
    };
    error(request_id, status, code, &source.to_string())
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
    Ok(axum::Json(json!({ "records": records })))
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
        Ok(Some(page)) => Ok(axum::Json(page)),
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
        Ok(Some(page)) => Ok(axum::Json(page)),
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
                    "state": handoff.state,
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
        .map(|rules| axum::Json(json!({ "rules": rules })))
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

pub async fn near_misses(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .near_misses(&id)
        .await
        .map(|near_misses| axum::Json(json!({ "nearMisses": near_misses })))
        .map_err(|error| fail(&request_id, error))
}

pub async fn ignore_keys(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<IgnoreKeysRequest>,
) -> Result<axum::Json<Value>, CanonicalError> {
    state
        .network
        .ignore_keys(&id, &body.keys)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
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
        .map_err(|error| fail(&request_id, error))
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
        .map(|revisions| axum::Json(json!({ "revisions": revisions })))
        .map_err(|error| fail(&request_id, error))
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
        .map_err(|error| fail(&request_id, error))
}

pub async fn patch_rule(
    Extension(request_id): Extension<RequestId>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<PatchRule>,
) -> Result<axum::Json<Value>, CanonicalError> {
    if let Some(enabled) = body.enabled {
        return state
            .network
            .set_rule_enabled(&id, enabled)
            .await
            .map(axum::Json)
            .map_err(|error| fail(&request_id, error));
    }
    if let Some(scope) = body.scope {
        return state
            .network
            .set_rule_scope(&id, &scope, body.tab_id)
            .await
            .map(axum::Json)
            .map_err(|error| fail(&request_id, error));
    }
    Err(fail(
        &request_id,
        AppError::bad_request("enabled_or_scope_required"),
    ))
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
        .delete_hits(&rule_id)
        .await
        .map(axum::Json)
        .map_err(|error| fail(&request_id, error))
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
            let event = Event::default()
                .event("record")
                .id(record.seq.to_string())
                .data(json!({ "record": record }).to_string());
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
                            let mut sse = Event::default().event(event.event).data(event.data.to_string());
                            if let Some(seq) = event.data.get("record").and_then(|record| record.get("seq")).and_then(Value::as_u64) {
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
