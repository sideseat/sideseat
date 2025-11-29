use crate::api;
use crate::auth::AuthManager;
use crate::otel::OtelManager;
use axum::{Router, routing::get};
use std::sync::Arc;

pub fn create_routes(
    auth_manager: Arc<AuthManager>,
    otel_manager: Option<Arc<OtelManager>>,
) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/health", get(api::health::health_check))
        .with_state(otel_manager.clone())
        .nest("/auth", api::auth::create_routes(auth_manager.clone()));

    // OTel query routes at /traces, /spans, /sessions, /traces/sse (no auth per user request)
    let router = if let Some(otel) = otel_manager {
        Router::new()
            .merge(public_routes)
            .nest("/traces", api::otel::create_query_routes(otel.clone()))
            .nest("/spans", api::otel::create_spans_routes(otel.clone()))
            .nest("/sessions", api::otel::create_sessions_routes(otel))
    } else {
        Router::new().merge(public_routes)
    };

    let _ = auth_manager;

    router
}
