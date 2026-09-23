//! OTEL query API endpoints

pub mod feed;
pub mod files;
pub mod filters;
mod json_stream;
pub mod logs;
pub mod messages;
pub mod metrics;
pub mod search;
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

use sideseat_domain::content_bodies::ContentBodyService;
use sideseat_domain::files::FileService;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_domain::topics::TopicService;
use sideseat_ports::clock::Clock;
use sideseat_ports::types::ProjectId;

use crate::types::ApiError;

/// Shared state for OTEL API endpoints
#[derive(Clone)]
pub struct OtelApiState {
    pub analytics: Arc<crate::dependencies::AnalyticsStore>,
    pub topics: Arc<TopicService>,
    pub file_service: Arc<FileService>,
    pub content_bodies: ContentBodyService,
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub clock: Arc<dyn Clock>,
    pub storage_governance: Arc<StorageGovernanceService>,
    /// Reconstructions already computed, keyed by the rows they came from. Process-local, so a new
    /// build never serves an answer the previous pipeline produced.
    pub reconstruction: sideseat_domain::sideml::feed::cache::ReconstructionCache,
    pub shutdown_rx: watch::Receiver<bool>,
}

/// Build OTEL API routes
#[allow(clippy::too_many_arguments)]
pub fn routes(
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    topics: Arc<TopicService>,
    file_service: Arc<FileService>,
    database: Arc<crate::dependencies::TransactionalStore>,
    cache: Arc<crate::dependencies::SharedCache>,
    clock: Arc<dyn Clock>,
    storage_governance: Arc<StorageGovernanceService>,
    shutdown_rx: watch::Receiver<bool>,
) -> Router<()> {
    let content_bodies = ContentBodyService::from_file_service(&file_service);
    let state = OtelApiState {
        analytics,
        topics,
        file_service,
        content_bodies,
        database,
        cache,
        clock,
        storage_governance,
        reconstruction: sideseat_domain::sideml::feed::cache::ReconstructionCache::new(),
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
        .route("/traces/{trace_id}/logs", get(logs::list_trace_logs))
        // Spans (nested under traces)
        .route("/traces/{trace_id}/spans", get(spans::list_trace_spans))
        .route("/traces/{trace_id}/spans/{span_id}", get(spans::get_span))
        .route(
            "/traces/{trace_id}/spans/{span_id}/messages",
            get(messages::get_span_messages),
        )
        .route(
            "/traces/{trace_id}/spans/{span_id}/logs",
            get(logs::list_span_logs),
        )
        // Logs
        .route("/logs", get(logs::list_logs))
        .route("/logs/filter-options", get(logs::get_log_filter_options))
        .route("/search", get(search::search))
        // Metrics
        .route("/metrics", get(metrics::list_metrics))
        .route("/metrics/aggregates", get(metrics::aggregate_metrics))
        .route(
            "/metrics/filter-options",
            get(metrics::get_metric_filter_options),
        )
        .route("/metrics/{datapoint_id}", get(metrics::get_metric))
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

async fn acquire_deletion_fence(
    state: &OtelApiState,
    project_id: &ProjectId,
    purpose: &str,
) -> Result<String, ApiError> {
    let owner = state
        .storage_governance
        .acquire_maintenance(project_id, purpose)
        .await
        .map_err(|error| ApiError::conflict("PROJECT_MAINTENANCE_BUSY", error.to_string()))?;
    match state.storage_governance.current_hold(project_id).await {
        Ok(None) => Ok(owner),
        Ok(Some(_)) => {
            state
                .storage_governance
                .release_maintenance(project_id, &owner)
                .await;
            Err(ApiError::conflict(
                "PROJECT_LEGAL_HOLD",
                "Project data cannot be deleted while a legal hold is active",
            ))
        }
        Err(error) => {
            state
                .storage_governance
                .release_maintenance(project_id, &owner)
                .await;
            Err(ApiError::internal(error.to_string()))
        }
    }
}

async fn reserve_deletion_journal(
    state: &OtelApiState,
    project_id: &ProjectId,
    logical_bytes: u64,
) -> Result<(), ApiError> {
    state
        .storage_governance
        .reserve_maintenance_under_fence(project_id, logical_bytes)
        .await
        .map(|_| ())
        .map_err(|error| ApiError::conflict("MAINTENANCE_RESERVE_EXHAUSTED", error.to_string()))
}
