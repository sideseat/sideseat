use std::sync::Arc;

use axum::Router;
use axum::extract::{OriginalUri, State};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::core::shutdown::ShutdownService;
use crate::data::AnalyticsService;

mod tools;
mod types;

use self::tools::McpServer;

type McpService = StreamableHttpService<McpServer>;

/// Shared state for MCP routes. Sessions are managed by a single shared
/// `LocalSessionManager`; the per-request `StreamableHttpService` is cheap
/// to construct (three Arc clones) and its factory captures the project_id
/// extracted from the URL.
#[derive(Clone)]
struct McpRouterState {
    analytics: Arc<AnalyticsService>,
    ct: CancellationToken,
    session_manager: Arc<LocalSessionManager>,
}

pub fn routes(analytics: Arc<AnalyticsService>, ct: CancellationToken) -> Router<()> {
    let state = McpRouterState {
        analytics,
        ct,
        session_manager: Arc::new(LocalSessionManager::default()),
    };

    Router::new().fallback(mcp_proxy).with_state(state)
}

fn extract_project_id(path: &str) -> String {
    path.split('/')
        .nth(4)
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_string()
}

async fn mcp_proxy(
    OriginalUri(uri): OriginalUri,
    State(state): State<McpRouterState>,
    req: axum::extract::Request,
) -> Response {
    let project_id = extract_project_id(uri.path());
    let analytics = state.analytics.clone();
    let svc = McpService::new(
        move || Ok(McpServer::new(analytics.clone(), project_id.clone())),
        state.session_manager.clone(),
        StreamableHttpServerConfig {
            cancellation_token: state.ct.clone(),
            ..Default::default()
        },
    );
    svc.oneshot(req).await.unwrap().into_response()
}

pub fn cancellation_token_from_shutdown(shutdown: &ShutdownService) -> CancellationToken {
    let token = CancellationToken::new();
    let mut rx = shutdown.subscribe();
    let t = token.clone();
    tokio::spawn(async move {
        let _ = rx.wait_for(|&v| v).await;
        t.cancel();
    });
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_project_id_standard_path() {
        let path = "/api/v1/projects/my-project/mcp";
        assert_eq!(extract_project_id(path), "my-project");
    }

    #[test]
    fn test_extract_project_id_with_subpath() {
        let path = "/api/v1/projects/my-project/mcp/sse";
        assert_eq!(extract_project_id(path), "my-project");
    }

    #[test]
    fn test_extract_project_id_default_on_missing() {
        assert_eq!(extract_project_id("/api/v1/projects"), "default");
        assert_eq!(extract_project_id("/too/short"), "default");
        assert_eq!(extract_project_id(""), "default");
    }

    #[test]
    fn test_extract_project_id_default_on_empty_segment() {
        let path = "/api/v1/projects//mcp";
        assert_eq!(extract_project_id(path), "default");
    }
}
