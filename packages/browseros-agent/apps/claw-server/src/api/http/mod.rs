//! Canonical NeoB HTTP API and shared request middleware.

use super::mcp::streamable_http_service;
use crate::{
    AppState,
    error::{AppError, CanonicalError, RequestId},
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use std::time::Instant;
use tracing::{Instrument, info_span};
use ulid::Ulid;

pub(crate) mod audit;
mod claude_desktop;
mod cockpit;
mod connections;
mod feedback;
mod help_events;
mod live;
mod network;
mod plan;
mod previews;
mod recordings;
mod replay;
mod screencasts;
mod screenshots;
mod sessions;
mod settings;
mod skills;
mod system;

/// UTF-8 request-body ceiling enforced before recording ingest and advertised by
/// `/api/v1/system` so the relay can reject oversized queued batches before POSTing.
pub(super) const RECORDING_INGEST_MAX_BYTES: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/system/health", get(system::health))
        .route("/system/ready", get(system::ready))
        .route("/system/diagnostics", get(system::diagnostics))
        .route("/system/shutdown", post(system::shutdown))
        .route("/api/v1/system", get(system::info))
        .route(
            "/api/v1/extension/update-ready",
            post(extension_update_ready),
        )
        .route("/api/v1/cockpit/stats", get(cockpit::stats))
        .route(
            "/api/v1/feedback/invitation",
            get(feedback::invitation).post(feedback::respond),
        )
        .route("/api/v1/audit/storage", get(audit::storage))
        .route("/api/v1/audit/retention", put(audit::set_retention))
        .route("/api/v1/audit/cleanup", post(audit::cleanup))
        .route("/api/v1/audit/agents", get(audit::agents))
        .route(
            "/api/v1/settings/telemetry",
            get(settings::telemetry).put(settings::update_telemetry),
        )
        .route(
            "/api/v1/settings/agent",
            get(settings::agent).put(settings::update_agent),
        )
        .route("/api/v1/help/events", get(help_events::events))
        .route("/api/v1/screencasts", get(screencasts::screencasts))
        .route("/api/v1/network/state", get(network::state))
        .route(
            "/api/v1/network/captures",
            post(network::start_capture).delete(network::stop_capture),
        )
        .route(
            "/api/v1/network/records",
            get(network::records).delete(network::delete_records),
        )
        .route(
            "/api/v1/network/records/restore",
            post(network::restore_record),
        )
        .route(
            "/api/v1/network/records/{record_id}",
            get(network::record).delete(network::delete_record),
        )
        .route(
            "/api/v1/network/records/{record_id}/request-body",
            get(network::request_body),
        )
        .route(
            "/api/v1/network/records/{record_id}/response-body",
            get(network::response_body),
        )
        .route(
            "/api/v1/network/handoffs",
            get(network::list_handoffs).post(network::create_handoff),
        )
        .route(
            "/api/v1/network/handoffs/{id}",
            get(network::get_handoff).delete(network::delete_handoff),
        )
        .route(
            "/api/v1/network/rules",
            get(network::rules).delete(network::delete_rules),
        )
        .route(
            "/api/v1/network/rules/{id}",
            get(network::rule)
                .patch(network::patch_rule)
                .delete(network::delete_rule),
        )
        .route(
            "/api/v1/network/rules/{id}/restore",
            post(network::restore_rule),
        )
        .route("/api/v1/network/rules/{id}/test", post(network::test_rule))
        .route(
            "/api/v1/network/rules/{id}/processor-test",
            post(network::processor_test),
        )
        .route(
            "/api/v1/network/tabs/{tabId}/reload",
            post(network::reload_tab),
        )
        .route(
            "/api/v1/network/rules/{id}/revisions",
            get(network::revisions),
        )
        .route(
            "/api/v1/network/rules/{id}/revisions/{revision}/restore",
            post(network::restore_revision),
        )
        .route("/api/v1/network/stop-all", post(network::stop_all))
        .route(
            "/api/v1/network/hits",
            get(network::hits).delete(network::delete_hits),
        )
        .route("/api/v1/network/events", get(network::events))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{session_id}", get(sessions::get))
        .route(
            "/api/v1/sessions/{session_id}/preview",
            get(previews::preview),
        )
        .route(
            "/api/v1/sessions/{session_id}/cancel",
            post(sessions::cancel),
        )
        .route(
            "/api/v1/sessions/{session_id}/help/resolve",
            post(sessions::resolve_help),
        )
        .route(
            "/api/v1/sessions/{session_id}/screenshots",
            get(screenshots::list),
        )
        .route(
            "/api/v1/sessions/{session_id}/screenshots/{screenshot_id}",
            get(screenshots::get),
        )
        .route(
            "/api/v1/sessions/{session_id}/recording",
            get(replay::recording),
        )
        .route(
            "/api/v1/sessions/{session_id}/recording/events",
            get(replay::download_events),
        )
        .route(
            "/api/v1/sessions/{session_id}/recording/live",
            get(live::live),
        )
        .route(
            "/api/v1/recordings/events",
            post(recordings::append_document_events)
                .layer(DefaultBodyLimit::max(RECORDING_INGEST_MAX_BYTES)),
        )
        .route(
            "/api/v1/claude-desktop/extension",
            get(claude_desktop::extension),
        )
        .route(
            "/api/v1/claude-desktop/extension/install",
            post(claude_desktop::install),
        )
        .route("/api/v1/connections", get(connections::list))
        .route(
            "/api/v1/connections/{harness}",
            put(connections::connect).delete(connections::disconnect),
        )
        .route("/api/v1/skills", get(skills::list).post(skills::create))
        .route(
            "/api/v1/skills/{name}",
            get(skills::get).put(skills::update).delete(skills::delete),
        )
        .route("/api/v1/skills/{name}/runs", get(skills::list_runs))
        .route("/api/v1/plan", get(plan::plan))
        .route("/api/v1/plan/pricing", get(plan::pricing))
        .route("/api/v1/plan/savings", get(sessions::savings))
        .route("/api/v1/plan/checkout", post(plan::start_checkout))
        .route("/api/v1/plan/checkout/{checkout_id}", get(plan::checkout))
        .route("/api/v1/plan/key", get(plan::key))
        .route("/api/v1/plan/key/activate", post(plan::activate))
        .route(
            "/api/v1/plan/key/computers/{install_id}/remove",
            post(plan::remove_computer),
        )
        .route(
            "/api/v1/plan/key/computers/{install_id}/rename",
            post(plan::rename_computer),
        )
        .route("/api/v1/plan/key/reset", post(plan::reset_key))
        .route("/api/v1/plan/cancel", post(plan::cancel))
        .route("/api/v1/plan/resume", post(plan::resume))
        .route("/api/v1/plan/portal", post(plan::portal))
        .route("/api/v1/plan/refresh", post(plan::refresh))
        .route("/api/v1/skills/{name}/flow", get(skills::flow))
        .route(
            "/api/v1/skills/{name}/ultrafast/approve-write",
            post(skills::approve_write).delete(skills::revoke_write),
        )
        .nest_service(
            "/mcp",
            Router::new()
                .fallback_service(streamable_http_service(state))
                .layer(middleware::from_fn(mcp_request_hygiene)),
        )
        .fallback(route_fallback)
        .layer(middleware::from_fn(options_preflight))
}

pub(super) fn error(
    request_id: &RequestId,
    status: StatusCode,
    code: &str,
    message: &str,
) -> CanonicalError {
    CanonicalError::new(status, code, message, Some(request_id))
}

pub(super) fn internal(request_id: &RequestId, source: AppError) -> CanonicalError {
    tracing::error!(request_id = %request_id.0, error = %source, "canonical route failed");
    error(
        request_id,
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "internal server error",
    )
}

/// Rejects browser-page requests to the loopback MCP endpoint. Browser fetches
/// carry `origin` or `sec-fetch-site`; native MCP clients do not.
async fn mcp_request_hygiene(req: Request, next: Next) -> Response {
    // The nested /mcp service shadows the router's `/{*path}` preflight route,
    // so answer OPTIONS here to keep loopback preflight behavior consistent.
    if *req.method() == Method::OPTIONS {
        return StatusCode::NO_CONTENT.into_response();
    }
    let headers = req.headers();
    if headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return AppError::forbidden("unsupported request").into_response();
    }
    let needs_json = match *req.method() {
        Method::POST | Method::PUT | Method::PATCH => true,
        // rmcp's DELETE /mcp session teardown carries no body and no
        // content-type, so exempt only that case.
        Method::DELETE => headers.contains_key(header::CONTENT_TYPE),
        _ => false,
    };
    if needs_json {
        let is_json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("application/json"));
        if !is_json {
            return AppError::unsupported_media_type("unsupported content type").into_response();
        }
    }
    next.run(req).await
}

async fn options_preflight(req: Request, next: Next) -> Response {
    if *req.method() == Method::OPTIONS {
        return StatusCode::NO_CONTENT.into_response();
    }
    next.run(req).await
}

pub async fn request_context(mut req: Request, next: Next) -> Response {
    let request_id = RequestId(Ulid::new().to_string());
    req.extensions_mut().insert(request_id.clone());
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let reject_origin = (path == "/api/v1/recordings/events"
        && !trusted_recording_origin(req.headers()))
        || ((path == "/api/v1/extension/update-ready"
            || path == "/api/v1/screencasts"
            || is_plan_path(&path))
            && !trusted_extension_origin(req.headers()))
        || (path.starts_with("/api/v1/network") && !trusted_network_origin(req.headers()));
    let span = info_span!("http_request", request_id = %request_id.0, %method, %path);
    async move {
        let start = Instant::now();
        let mut response = if reject_origin {
            CanonicalError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                concat!("this endpoint is restricted to ", crate::product_name!()),
                Some(&request_id),
            )
            .into_response()
        } else {
            next.run(req).await
        };
        // One structured line per failed request; sub-400 traffic stays
        // unlogged on purpose (claw-app polls several endpoints).
        let status = response.status().as_u16();
        if status >= 400 {
            let duration_ms = start.elapsed().as_millis() as u64;
            if status >= 500 {
                tracing::error!(%method, %path, status, duration_ms, "request failed");
            } else {
                tracing::warn!(%method, %path, status, duration_ms, "request failed");
            }
        }
        let headers = response.headers_mut();
        if !reject_origin {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET,POST,PUT,PATCH,DELETE,OPTIONS"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(
                    "accept,content-type,authorization,mcp-session-id,mcp-protocol-version,last-event-id,x-recording-batch-id,x-recording-tab-id,x-recording-document-id,x-recording-has-gap",
                ),
            );
        }
        if let Ok(value) = HeaderValue::from_str(&request_id.0) {
            headers.insert("x-request-id", value);
        }
        response
    }
    .instrument(span)
    .await
}

/// Loopback hosts the network API accepts, so a DNS-rebinding page cannot
/// reach it by pointing some other name at 127.0.0.1.
fn host_is_loopback(headers: &axum::http::HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    // `[::1]:9310` keeps everything up to the closing bracket; anything else
    // drops a trailing `:port`.
    let address = match host.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(address, _)| address),
        None => host.rsplit_once(':').map_or(host, |(address, _)| address),
    };
    matches!(address, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// The network API serves the extension and native MCP clients only.
/// Network data is readable by the extension and by native (originless) clients
/// only. Any other page origin, including the opaque `null`, is rejected.
///
/// A browser request without `Origin` is still checked against
/// `Sec-Fetch-Site`: an `<img>` from another site would otherwise be able to
/// read capture data if DNS ever resolved a remote name to loopback.
fn trusted_network_origin(headers: &axum::http::HeaderMap) -> bool {
    if !host_is_loopback(headers) {
        return false;
    }
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(BROWSERCLAW_EXTENSION_ORIGIN) => true,
        Some(_) => false,
        // Native clients (curl, MCP) send no Origin and no Sec-Fetch-Site.
        None => {
            !headers.contains_key("sec-fetch-site")
                || matches!(
                    headers
                        .get("sec-fetch-site")
                        .and_then(|value| value.to_str().ok()),
                    Some("none") | Some("same-origin")
                )
        }
    }
}

/// The ultrafast routes can buy, cancel, reset a key and remove computers, and their answers name
/// the computers on the key, so no web page may reach them.
fn is_plan_path(path: &str) -> bool {
    path == "/api/v1/plan" || path.starts_with("/api/v1/plan/")
}

/// Stable origin derived from claw-app's manifest signing key.
const BROWSERCLAW_EXTENSION_ORIGIN: &str = "chrome-extension://pjimfkbpehlcllblajnpfamdfjhhlgkc";

fn trusted_recording_origin(headers: &axum::http::HeaderMap) -> bool {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        None => true,
        Some(BROWSERCLAW_EXTENSION_ORIGIN) => true,
        Some("null") => {
            // The native opaque recorder may send `Origin: null`; accept it only with
            // `Sec-Fetch-Site: none`, not as general trust of null origins.
            headers
                .get("sec-fetch-site")
                .and_then(|value| value.to_str().ok())
                == Some("none")
        }
        Some(_) => false,
    }
}

async fn route_fallback(request: Request) -> StatusCode {
    if *request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Notifications carry no authority or version: wake the coordinator, which
/// independently reads the extension's native staged state over CDP.
async fn extension_update_ready(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> StatusCode {
    state.extension_updates.notify();
    StatusCode::NO_CONTENT
}

/// claw-app, or a native client (no `Origin` and no `Sec-Fetch-Site`). Every other web page is
/// refused: responses here carry `Access-Control-Allow-Origin: *`, and a WebSocket has no CORS at
/// all, so the origin check is the only thing keeping a site from these endpoints.
///
/// Chrome sends no `Origin` on a GET from an extension page that holds host permission, only
/// `Sec-Fetch-Site: none`. A web page cannot set that header, and its requests here say
/// `cross-site`.
fn trusted_extension_origin(headers: &axum::http::HeaderMap) -> bool {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(BROWSERCLAW_EXTENSION_ORIGIN) => true,
        None => headers
            .get("sec-fetch-site")
            .is_none_or(|value| value.as_bytes() == b"none"),
        Some(_) => false,
    }
}

#[cfg(test)]
mod network_origin_tests {
    #![allow(clippy::expect_used)]
    use super::{BROWSERCLAW_EXTENSION_ORIGIN, host_is_loopback, trusted_network_origin};
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        map
    }

    #[test]
    fn loopback_hosts_are_accepted_and_others_rejected() {
        assert!(host_is_loopback(&headers(&[("host", "127.0.0.1:9310")])));
        assert!(host_is_loopback(&headers(&[("host", "localhost:9310")])));
        assert!(host_is_loopback(&headers(&[("host", "[::1]:9310")])));
        assert!(!host_is_loopback(&headers(&[("host", "evil.example")])));
        assert!(!host_is_loopback(&HeaderMap::new()));
    }

    #[test]
    fn extension_origin_is_always_trusted_on_loopback() {
        assert!(trusted_network_origin(&headers(&[
            ("host", "127.0.0.1:9310"),
            ("origin", BROWSERCLAW_EXTENSION_ORIGIN),
        ])));
    }

    #[test]
    fn foreign_origin_is_rejected_even_on_loopback() {
        assert!(!trusted_network_origin(&headers(&[
            ("host", "127.0.0.1:9310"),
            ("origin", "http://evil.example"),
        ])));
        assert!(!trusted_network_origin(&headers(&[
            ("host", "127.0.0.1:9310"),
            ("origin", "null"),
        ])));
    }

    #[test]
    fn without_origin_the_site_header_decides() {
        assert!(trusted_network_origin(&headers(&[(
            "host",
            "127.0.0.1:9310"
        )])));
        assert!(!trusted_network_origin(&headers(&[
            ("host", "127.0.0.1:9310"),
            ("sec-fetch-site", "cross-site"),
        ])));
        assert!(trusted_network_origin(&headers(&[
            ("host", "127.0.0.1:9310"),
            ("sec-fetch-site", "same-origin"),
        ])));
    }

    #[test]
    fn a_rebound_host_is_rejected_even_without_origin() {
        assert!(!trusted_network_origin(&headers(&[(
            "host",
            "evil.example:9310"
        )])));
    }
}
