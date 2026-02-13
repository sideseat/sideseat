//! OTEL query API endpoints

pub mod feed;
pub mod files;
pub mod filters;
pub mod messages;
pub mod sessions;
pub mod spans;
mod sse;
pub mod stats;
pub mod traces;
pub mod types;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use tokio::sync::watch;

use crate::core::TopicService;
use crate::data::cache::CacheService;
use crate::data::files::FileService;
use crate::data::{AnalyticsService, TransactionalService};

/// Shared state for OTEL API endpoints
#[derive(Clone)]
pub struct OtelApiState {
    pub analytics: Arc<AnalyticsService>,
    pub topics: Arc<TopicService>,
    pub file_service: Arc<FileService>,
    pub database: Arc<TransactionalService>,
    pub cache: Arc<CacheService>,
    pub shutdown_rx: watch::Receiver<bool>,
}

/// Build OTEL API routes
pub fn routes(
    analytics: Arc<AnalyticsService>,
    topics: Arc<TopicService>,
    file_service: Arc<FileService>,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
    shutdown_rx: watch::Receiver<bool>,
) -> Router<()> {
    let state = OtelApiState {
        analytics,
        topics,
        file_service,
        database,
        cache,
        shutdown_rx,
    };

    Router::new()
        // Traces
        .route(
            "/traces",
            get(traces::list_traces).delete(traces::delete_traces),
        )
        .route(
            "/traces/filter-options",
            get(traces::get_trace_filter_options),
        )
        .route("/traces/{trace_id}", get(traces::get_trace))
        .route(
            "/traces/{trace_id}/messages",
            get(messages::get_trace_messages),
        )
        // Spans (nested under traces)
        .route("/traces/{trace_id}/spans", get(spans::list_trace_spans))
        .route("/traces/{trace_id}/spans/{span_id}", get(spans::get_span))
        .route(
            "/traces/{trace_id}/spans/{span_id}/messages",
            get(messages::get_span_messages),
        )
        // Spans (top-level for cross-trace queries)
        .route("/spans", get(spans::list_spans).delete(spans::delete_spans))
        .route("/spans/filter-options", get(spans::get_span_filter_options))
        // Sessions
        .route(
            "/sessions",
            get(sessions::list_sessions).delete(sessions::delete_sessions),
        )
        .route(
            "/sessions/filter-options",
            get(sessions::get_session_filter_options),
        )
        .route("/sessions/{session_id}", get(sessions::get_session))
        .route(
            "/sessions/{session_id}/messages",
            get(messages::get_session_messages),
        )
        // SSE
        .route("/sse", get(sse::sse))
        // Stats
        .route("/stats", get(stats::get_project_stats))
        // Feed (project-wide message/span activity)
        .route("/feed/messages", get(feed::get_feed_messages))
        .route("/feed/spans", get(feed::get_feed_spans))
        .with_state(state)
}
