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

// rmcp 3 parameterises the service by session manager as well as handler.
type McpService = StreamableHttpService<McpServer, LocalSessionManager>;

/// Shared state for MCP routes. Sessions are managed by a single shared
/// `LocalSessionManager`; the per-request `StreamableHttpService` is cheap
/// to construct (three Arc clones) and its factory captures the project_id
/// extracted from the URL.
#[derive(Clone)]
struct McpRouterState {
    analytics: Arc<AnalyticsService>,
    ct: CancellationToken,
    /// One session manager **per project**, which is what binds a session to the project it was authorised
    /// for.
    ///
    /// A single shared manager was an authorisation bypass: authorisation checks the project in the *URL*,
    /// but `rmcp` resolves an existing `Mcp-Session-Id` to its own worker without consulting the factory - so
    /// initialising a session against project A and then replaying that id on an authorised project-B URL
    /// served A's data after only proving access to B. Keyed per project, A's session id does not exist under
    /// B: the request has to initialise a new session, which runs B's factory.
    ///
    /// Bounded by the number of projects that have ever opened an MCP session on this instance, which is the
    /// same order as the sessions themselves.
    session_managers: Arc<dashmap::DashMap<String, Arc<LocalSessionManager>>>,
}

pub fn routes(analytics: Arc<AnalyticsService>, ct: CancellationToken) -> Router<()> {
    let state = McpRouterState {
        analytics,
        ct,
        session_managers: Arc::new(dashmap::DashMap::new()),
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

    // The project comes from the URL, so it must be checked against who is asking.
    //
    // This endpoint was mounted with no auth at all while `auth.enabled` defaults to *true*, and it exposes
    // spans, prompts, raw attributes, sessions and statistics for whatever project the path names - so any
    // caller who could reach it could read another organisation's conversations. `require_auth` (layered in
    // `server.rs`) now rejects an unauthenticated request whenever auth is on and injects the context;
    // `verify_project_access` is what turns a *valid* credential into a valid credential **for this
    // project**, since a key belonging to another organisation is otherwise perfectly valid.
    //
    // With `--no-auth` the injected context is `LocalDefault`, which `verify_project_access` admits - so the
    // documented development flow is unchanged.
    if let (Some(auth), Some(service)) = (
        req.extensions()
            .get::<crate::api::auth::AuthContext>()
            .cloned(),
        req.extensions()
            .get::<Arc<crate::api::auth::AuthService>>()
            .cloned(),
    ) {
        if let Err(e) = service
            .verify_project_access(&auth, &project_id, crate::data::types::ApiKeyScope::Read)
            .await
        {
            return e.into_response();
        }
    } else {
        // No context means the auth layer did not run, which must never be a way in.
        tracing::error!(
            project_id,
            "An MCP request arrived with no authentication context; refusing it. The auth layer is missing \
             from the MCP routes."
        );
        return crate::api::types::ApiError::forbidden(
            "AUTH_CONTEXT_MISSING",
            "authentication context missing",
        )
        .into_response();
    }
    let analytics = state.analytics.clone();
    // The manager for *this* project only - see `session_managers`.
    let session_manager = state
        .session_managers
        .entry(project_id.clone())
        .or_insert_with(|| Arc::new(LocalSessionManager::default()))
        .clone();
    let svc = McpService::new(
        move || Ok(McpServer::new(analytics.clone(), project_id.clone())),
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
