//! OpenTelemetry Protocol (OTLP) HTTP and gRPC endpoints

mod encoding;
mod grpc;
mod logs;
mod metrics;
mod traces;

pub use grpc::{GrpcIngestAuth, GrpcIngestGuards, GrpcIngestLimit, IngestStores, OtlpGrpcServer};

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use sideseat_core::constants::TOPIC_TRACES;
use sideseat_domain::storage_governance::StorageGovernanceService;
pub use sideseat_ingestion::otlp::{
    inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces,
};
use sideseat_ingestion::signals::{LogSignal, MetricsSignal, TraceSignal};
use sideseat_ingestion::staging::{StagedPayloadRef, StagingService};
use sideseat_messaging::TopicService;
use sideseat_ports::clock::Clock;

#[derive(Clone)]
pub struct OtlpState {
    /// The transport-neutral trace lifecycle used by HTTP and gRPC.
    pub trace_signal: Arc<TraceSignal>,
    /// The transport-neutral metrics lifecycle used by HTTP and gRPC.
    pub metrics_signal: Arc<MetricsSignal>,
    /// The transport-neutral logs lifecycle used by HTTP and gRPC.
    pub log_signal: Arc<LogSignal>,
    pub debug_path: Option<PathBuf>,
    /// For the metrics write, which happens in the request, and for telling an exporter now that its
    /// project will not accept writes.
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub clock: Arc<dyn Clock>,
    pub staging: Arc<StagingService>,
    pub storage_governance: Arc<StorageGovernanceService>,
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
    match state.database.as_ref().get_project(project_id).await {
        Ok(found) => found.is_some(),
        // Unknown: let it through and let the write path decide. Refusing on a lookup failure would
        // turn a database blip into rejected telemetry.
        Err(e) => {
            tracing::warn!(project_id, error = %e, "Could not check the project at ingest");
            true
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn routes(
    topics: &Arc<TopicService>,
    debug_path: Option<PathBuf>,
    database: Arc<crate::dependencies::TransactionalStore>,
    analytics: Arc<crate::dependencies::AnalyticsStore>,
    cache: Arc<crate::dependencies::SharedCache>,
    clock: Arc<dyn Clock>,
    staging: Arc<StagingService>,
    storage_governance: Arc<StorageGovernanceService>,
    trace_pipeline: Option<Arc<sideseat_ingestion::traces::TracePipeline>>,
) -> Router {
    // Use stream topic for traces (at-least-once delivery)
    let trace_topic = Arc::new(
        topics.stream_topic::<StagedPayloadRef>(TOPIC_TRACES, StagedPayloadRef::partition_key),
    );
    let trace_signal = Arc::new(TraceSignal::new(trace_topic, trace_pipeline));
    let metrics_signal = Arc::new(MetricsSignal::new(
        Arc::clone(&analytics),
        Arc::clone(&database),
    ));
    let log_signal = Arc::new(LogSignal::new(analytics, Arc::clone(&database)));

    let state = OtlpState {
        trace_signal,
        metrics_signal,
        log_signal,
        debug_path,
        database,
        cache,
        clock,
        staging,
        storage_governance,
    };

    Router::new()
        .route("/traces", post(traces::export))
        .route("/metrics", post(metrics::export))
        .route("/logs", post(logs::export))
        .with_state(state)
}
