//! OpenTelemetry API routes

mod collector;
mod sessions;
mod spans;
mod sse;
mod traces;
mod types;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::post};
use std::sync::Arc;

use crate::otel::OtelManager;

pub use collector::handle_traces;
pub use spans::get_spans;
pub use sse::handle_sse;
pub use traces::{get_trace, get_traces};
pub use types::*;

/// Create OTLP collector routes (mounted at /otel)
/// These are the standard OTLP endpoints for trace/metrics/logs ingestion
pub fn create_collector_routes(otel: Arc<OtelManager>) -> Router {
    Router::new()
        .nest("/v1", collector::create_routes(otel))
        // Stub endpoints that accept and discard (for OTLP compatibility)
        .route("/v1/metrics", post(stub_metrics))
        .route("/v1/logs", post(stub_logs))
        .route("/v1development/profiles", post(stub_profiles))
}

/// Create query routes (mounted at /api/v1/traces)
pub fn create_query_routes(otel: Arc<OtelManager>) -> Router {
    Router::new()
        .merge(traces::create_routes(otel.clone()))
        .nest("/{trace_id}/spans", spans::create_routes(otel.clone()))
        .nest("/sse", sse::create_routes(otel))
}

/// Create spans routes (mounted at /api/v1/spans)
pub fn create_spans_routes(otel: Arc<OtelManager>) -> Router {
    spans::create_routes(otel)
}

/// Create sessions routes (mounted at /api/v1/sessions)
pub fn create_sessions_routes(otel: Arc<OtelManager>) -> Router {
    sessions::create_routes(otel)
}

/// Stub handler for metrics - accepts and discards
async fn stub_metrics() -> impl IntoResponse {
    StatusCode::OK
}

/// Stub handler for logs - accepts and discards
async fn stub_logs() -> impl IntoResponse {
    StatusCode::OK
}

/// Stub handler for profiles - accepts and discards
async fn stub_profiles() -> impl IntoResponse {
    StatusCode::OK
}
