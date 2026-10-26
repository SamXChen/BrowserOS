pub mod dispatch;
pub mod distill;
pub mod effects;
pub mod guards;
pub mod helper_runtime;
mod identity_text;
mod network_guide;
pub mod neob_tools;
pub mod naming;
pub mod network_host;
pub mod observers;
mod prompt;
pub mod pursue;
pub mod script_hook;
mod service;
mod timeouts;

#[cfg(test)]
pub mod test_support;

use crate::{AppState, services::sessions::RetainedGroupAction};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use std::{sync::Arc, time::Duration};

pub use service::ClawMcpService;
// Re-exported so the route tests assert against the real text instead of keeping a
// copy of it, which is what let the two drift apart.
pub use service::SESSION_ARG_DESCRIPTION;

/// Builds the shared MCP service used by both streamable HTTP and stdio.
#[must_use]
pub fn browser_mcp_service(state: AppState) -> ClawMcpService {
    let browser = Arc::downgrade(&state.browser);
    state
        .sessions
        .set_retained_group_hook(Arc::new(move |ownership, key, action| {
            let browser = browser.clone();
            Box::pin(async move {
                let session = match browser.upgrade() {
                    Some(browser) => browser.session().await,
                    None => None,
                };
                match action {
                    RetainedGroupAction::Collapse => {
                        effects::tab_groups::collapse_agent_tab_group(
                            session.as_ref(),
                            &ownership,
                            &key,
                        )
                        .await
                    }
                    RetainedGroupAction::Close => {
                        effects::tab_groups::close_agent_tab_group(
                            session.as_ref(),
                            &ownership,
                            &key,
                        )
                        .await
                    }
                }
            })
        }));
    ClawMcpService::new(state)
}

/// Builds the rmcp streamable HTTP service mounted at `/mcp`.
///
/// The transport keeps a session alive exactly as long as the application does, so
/// expiring a session has a single owner: the application-level idle sweep.
#[must_use]
pub fn streamable_http_service(
    state: AppState,
) -> StreamableHttpService<ClawMcpService, LocalSessionManager> {
    let sessions = session_manager(state.config.session_idle);
    StreamableHttpService::new(
        move || Ok(browser_mcp_service(state.clone())),
        Arc::new(sessions),
        StreamableHttpServerConfig::default(),
    )
}

/// Transport-level session manager whose idle timeout matches the application's
/// `session_idle`.
///
/// rmcp defaults `keep_alive` to 300 s while the application keeps a session for
/// `DEFAULT_SESSION_IDLE_MS` (30 min). Sessions therefore expired at the transport
/// layer first: clients got `404 Not Found: Session not found`, re-initialized as the
/// MCP spec requires, and continued under a new identity while the old one kept the
/// agent's tab group. Deriving the transport
/// timeout from the same value gives session expiry a single owner.
///
/// `sse_keep_alive` is left at its default: it is a 15 s heartbeat on the stream and
/// has nothing to do with session lifetime.
fn session_manager(idle: Duration) -> LocalSessionManager {
    let mut sessions = LocalSessionManager::default();
    sessions.session_config.keep_alive = Some(idle);
    sessions
}

#[cfg(test)]
mod tests {
    use super::session_manager;
    use std::time::Duration;

    #[test]
    fn session_manager_keeps_sessions_as_long_as_the_application() {
        let idle = Duration::from_secs(30 * 60);
        assert_eq!(session_manager(idle).session_config.keep_alive, Some(idle));
    }

    #[test]
    fn session_manager_follows_the_configured_idle() {
        let idle = Duration::from_secs(7);
        assert_eq!(session_manager(idle).session_config.keep_alive, Some(idle));
    }
}
