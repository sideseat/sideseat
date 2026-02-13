//! gRPC OTLP server

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::watch;
use tonic::transport::Server as TonicServer;
use tonic::{Request, Response, Status};

use opentelemetry_proto::tonic::collector::{
    logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    },
    metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    },
    trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    },
};

use crate::api::extractors::is_valid_project_id;
use crate::core::config::OtelConfig;
use crate::core::constants::{OTLP_BODY_LIMIT, TOPIC_LOGS, TOPIC_METRICS, TOPIC_TRACES};
use crate::core::storage::{AppStorage, DataSubdir};
use crate::core::{Publisher, TopicService};
use crate::data::topics::StreamTopic;
use crate::utils::debug::write_debug;
use crate::utils::otlp::{
    inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces,
};

const PROJECT_ID_HEADER: &str = "x-sideseat-project-id";
const DEFAULT_PROJECT_ID: &str = "default";

/// Maximum retry attempts for trace publish
const PUBLISH_MAX_ATTEMPTS: u32 = 3;

/// Base delay in milliseconds for exponential backoff
const PUBLISH_BASE_DELAY_MS: u64 = 50;

pub struct OtlpGrpcServer {
    addr: SocketAddr,
    trace_topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
    metrics_publisher: Publisher<ExportMetricsServiceRequest>,
    logs_publisher: Publisher<ExportLogsServiceRequest>,
    debug_path: Option<PathBuf>,
}

impl OtlpGrpcServer {
    pub fn new(
        config: &OtelConfig,
        host: &str,
        topics: &Arc<TopicService>,
        storage: &AppStorage,
        debug: bool,
    ) -> Result<Self> {
        let addr = SocketAddr::new(host.parse()?, config.grpc_port);
        let debug_path = if debug {
            Some(storage.subdir(DataSubdir::Debug))
        } else {
            None
        };
        // Use stream topic for traces (at-least-once delivery)
        let trace_topic = Arc::new(topics.stream_topic::<ExportTraceServiceRequest>(TOPIC_TRACES));
        let metrics_publisher = topics
            .topic::<ExportMetricsServiceRequest>(TOPIC_METRICS)
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .publisher();
        let logs_publisher = topics
            .topic::<ExportLogsServiceRequest>(TOPIC_LOGS)
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .publisher();
        Ok(Self {
            addr,
            trace_topic,
            metrics_publisher,
            logs_publisher,
            debug_path,
        })
    }

    pub async fn start(self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        let addr = self.addr;
        let debug_path = self.debug_path;

        tracing::debug!(%addr, "Starting OTLP gRPC server");

        TonicServer::builder()
            .add_service(
                TraceServiceServer::new(OtlpTraceService::new(
                    self.trace_topic,
                    debug_path.clone(),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                MetricsServiceServer::new(OtlpMetricsService::new(
                    self.metrics_publisher,
                    debug_path.clone(),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                LogsServiceServer::new(OtlpLogsService::new(self.logs_publisher, debug_path))
                    .max_decoding_message_size(OTLP_BODY_LIMIT)
                    .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .serve_with_shutdown(addr, async move {
                let _ = shutdown_rx.wait_for(|&v| v).await;
                tracing::debug!("OTLP gRPC server shutting down");
            })
            .await?;

        Ok(())
    }
}

/// Extract project_id from gRPC metadata, defaulting to "default"
/// Returns None if the provided project_id is invalid
fn extract_project_id<T>(request: &Request<T>) -> Option<String> {
    let project_id = request
        .metadata()
        .get(PROJECT_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PROJECT_ID);

    if is_valid_project_id(project_id) {
        Some(project_id.to_string())
    } else {
        None
    }
}

/// gRPC trace service
struct OtlpTraceService {
    topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
    debug_path: Option<PathBuf>,
}

impl OtlpTraceService {
    fn new(
        topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
        debug_path: Option<PathBuf>,
    ) -> Self {
        Self { topic, debug_path }
    }
}

#[tonic::async_trait]
impl TraceService for OtlpTraceService {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let project_id = extract_project_id(&request)
            .ok_or_else(|| Status::invalid_argument("Invalid project_id"))?;
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_traces(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "traces.jsonl", &project_id, &req).await;
        }

        // Publish to stream topic with retry (at-least-once delivery)
        let mut last_error = None;
        for attempt in 1..=PUBLISH_MAX_ATTEMPTS {
            match self.topic.publish(&req).await {
                Ok(_) => {
                    if attempt > 1 {
                        tracing::debug!(attempt, "Trace publish succeeded after retry");
                    }
                    last_error = None;
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < PUBLISH_MAX_ATTEMPTS {
                        let delay =
                            Duration::from_millis(PUBLISH_BASE_DELAY_MS * 2_u64.pow(attempt - 1));
                        tracing::warn!(
                            error = %last_error.as_ref().unwrap(),
                            attempt,
                            delay_ms = delay.as_millis(),
                            "Retrying trace publish after transient error"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        if let Some(e) = last_error {
            tracing::warn!(error = %e, attempts = PUBLISH_MAX_ATTEMPTS, "Failed to publish traces after retries");
            return Err(Status::resource_exhausted("trace buffer full"));
        }

        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

/// gRPC metrics service
struct OtlpMetricsService {
    publisher: Publisher<ExportMetricsServiceRequest>,
    debug_path: Option<PathBuf>,
}

impl OtlpMetricsService {
    fn new(publisher: Publisher<ExportMetricsServiceRequest>, debug_path: Option<PathBuf>) -> Self {
        Self {
            publisher,
            debug_path,
        }
    }
}

#[tonic::async_trait]
impl MetricsService for OtlpMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let project_id = extract_project_id(&request)
            .ok_or_else(|| Status::invalid_argument("Invalid project_id"))?;
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_metrics(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "metrics.jsonl", &project_id, &req).await;
        }

        if let Err(e) = self.publisher.publish(req) {
            tracing::warn!(error = %e, "Failed to publish metrics to topic");
            return Err(Status::resource_exhausted("metrics buffer full"));
        }

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

/// gRPC logs service
struct OtlpLogsService {
    publisher: Publisher<ExportLogsServiceRequest>,
    debug_path: Option<PathBuf>,
}

impl OtlpLogsService {
    fn new(publisher: Publisher<ExportLogsServiceRequest>, debug_path: Option<PathBuf>) -> Self {
        Self {
            publisher,
            debug_path,
        }
    }
}

#[tonic::async_trait]
impl LogsService for OtlpLogsService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let project_id = extract_project_id(&request)
            .ok_or_else(|| Status::invalid_argument("Invalid project_id"))?;
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_logs(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "logs.jsonl", &project_id, &req).await;
        }

        if let Err(e) = self.publisher.publish(req) {
            tracing::warn!(error = %e, "Failed to publish logs to topic");
            return Err(Status::resource_exhausted("logs buffer full"));
        }

        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}
