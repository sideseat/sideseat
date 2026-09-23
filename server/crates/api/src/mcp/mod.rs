use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use sideseat_ports::clock::Clock;

use crate::auth::ProjectRead;

mod tools;
mod types;

use self::tools::McpServer;

// rmcp 3 parameterises the service by session manager as well as handler.
type McpService = StreamableHttpService<McpServer, LocalSessionManager>;

/// Shared state for MCP routes. Sessions are managed by a single shared
/// `LocalSessionManager`; the per-request `StreamableHttpService` is cheap
/// to construct (three Arc clones) and its factory captures the project_id
/// extracted from the URL.
#[derive(Clone)]
struct McpRouterState {
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    clock: Arc<dyn Clock>,
    ct: CancellationToken,
    /// Session IDs are resolved within their authorised project, never across
    /// tenant boundaries.
    session_managers: Arc<dashmap::DashMap<String, Arc<LocalSessionManager>>>,
}

pub fn routes(
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    clock: Arc<dyn Clock>,
    ct: CancellationToken,
) -> Router<()> {
    let state = McpRouterState {
        analytics,
        clock,
        ct,
        session_managers: Arc::new(dashmap::DashMap::new()),
    };

    Router::new().fallback(mcp_proxy).with_state(state)
}

async fn mcp_proxy(
    State(state): State<McpRouterState>,
    access: ProjectRead,
    req: axum::extract::Request,
) -> Response {
    let project_id = access.project_id.into_inner();
    let analytics = state.analytics.clone();
    let clock = state.clock.clone();
    let session_manager = state
        .session_managers
        .entry(project_id.clone())
        .or_insert_with(|| Arc::new(LocalSessionManager::default()))
        .clone();
    let svc = McpService::new(
        move || {
            Ok(McpServer::new(
                analytics.clone(),
                clock.clone(),
                project_id.clone(),
            ))
        },
        session_manager,
        {
            // rmcp 3 marks the config #[non_exhaustive]. Its defaults matter here: Host
            // validation is restricted to loopback, which is the DNS-rebinding
            // mitigation that RUSTSEC-2026-0189 was filed against. Left at the default
            // deliberately - widen allowed_hosts only for a deployment that is reached
            // by hostname.
            let mut config = StreamableHttpServerConfig::default();
            config.cancellation_token = state.ct.clone();
            config
        },
    );
    svc.oneshot(req).await.unwrap().into_response()
}

pub fn cancellation_token_from_shutdown(
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.wait_for(|&v| v).await;
        t.cancel();
    });
    token
}
