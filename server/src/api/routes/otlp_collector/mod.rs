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
}

pub fn routes(topics: &Arc<TopicService>, debug_path: Option<PathBuf>) -> Router {
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
    };

    Router::new()
        .route("/traces", post(traces::export))
        .route("/metrics", post(metrics::export))
        .route("/logs", post(logs::export))
        .with_state(state)
}
