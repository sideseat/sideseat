//! OpenTelemetry Protocol (OTLP) HTTP and gRPC endpoints

mod encoding;
mod grpc;
mod logs;
mod metrics;
mod traces;

pub use grpc::OtlpGrpcServer;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};

use crate::core::constants::{TOPIC_LOGS, TOPIC_METRICS, TOPIC_TRACES};
use crate::core::{Publisher, TopicService};
use crate::data::TransactionalService;
use crate::data::cache::CacheService;
use crate::data::topics::StreamTopic;
pub use crate::utils::otlp::{
    inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces,
};

#[derive(Clone)]
pub struct OtlpState {
    /// Stream topic for traces (at-least-once delivery)
    pub trace_topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
    /// Local publishers for metrics and logs (backward compatible)
    pub metrics_publisher: Publisher<ExportMetricsServiceRequest>,
    pub logs_publisher: Publisher<ExportLogsServiceRequest>,
    pub debug_path: Option<PathBuf>,
    /// For telling an exporter, now, that its project will not accept writes.
    pub database: Arc<TransactionalService>,
    pub cache: Arc<CacheService>,
}

/// Whether this project exists and is not being deleted, answered from the project cache.
///
/// The authoritative check is next to the write (`TracePipeline`), because a request that passes here
/// goes onto a topic and persists seconds later. This one exists so the exporter is *told*: without it a
/// project id that does not exist gets 200 OK and its spans are dropped later, which is silent data loss
/// dressed as success. A cached answer is fine for that job - being wrong for a few minutes costs a
/// misleading status code, not a bad write.
async fn project_accepts_writes(state: &OtlpState, project_id: &str) -> bool {
    // `get_project` reads the project cache and its query filters out a claimed project, and claiming
    // now invalidates that cache - so this is fence-aware without a second lookup path.
    match state
        .database
        .repository()
        .get_project(Some(&state.cache), project_id)
        .await
    {
        Ok(found) => found.is_some(),
        // Unknown: let it through and let the write path decide. Refusing on a lookup failure would
        // turn a database blip into rejected telemetry.
        Err(e) => {
            tracing::warn!(project_id, error = %e, "Could not check the project at ingest");
            true
        }
    }
}

pub fn routes(
    topics: &Arc<TopicService>,
    debug_path: Option<PathBuf>,
    database: Arc<TransactionalService>,
    cache: Arc<CacheService>,
) -> Router {
    // Use stream topic for traces (at-least-once delivery)
    let trace_topic = Arc::new(topics.stream_topic::<ExportTraceServiceRequest>(TOPIC_TRACES));

    // Use local topics for metrics and logs (backward compatible)
    let metrics_topic = topics
        .topic::<ExportMetricsServiceRequest>(TOPIC_METRICS)
        .expect("Failed to create metrics topic");
    let logs_topic = topics
        .topic::<ExportLogsServiceRequest>(TOPIC_LOGS)
        .expect("Failed to create logs topic");

    let state = OtlpState {
        trace_topic,
        metrics_publisher: metrics_topic.publisher(),
        logs_publisher: logs_topic.publisher(),
        debug_path,
        database,
        cache,
    };

    Router::new()
        .route("/traces", post(traces::export))
        .route("/metrics", post(metrics::export))
        .route("/logs", post(logs::export))
        .with_state(state)
}
