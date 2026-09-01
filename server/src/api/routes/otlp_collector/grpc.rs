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
use crate::core::constants::{OTLP_BODY_LIMIT, TOPIC_LOGS, TOPIC_TRACES};
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
    logs_publisher: Publisher<ExportLogsServiceRequest>,
    /// Metrics are written in the request rather than queued, so this service needs the stores.
    analytics: Arc<crate::data::AnalyticsService>,
    database: Arc<crate::data::TransactionalService>,
    /// Present exactly when the topic backend cannot promise durability, in which case traces are also
    /// written in the request.
    trace_pipeline: Option<Arc<crate::domain::TracePipeline>>,
    debug_path: Option<PathBuf>,
    /// What `otel.auth.required` demands of *this* transport.
    ///
    /// The setting used to apply to HTTP only: the gRPC server had no interceptor at all and took the
    /// project id from an untrusted `x-sideseat-project-id` metadata entry, so with auth required an
    /// unauthenticated client could still write traces or metrics into any existing project. A setting that
    /// is enforced on one of two equivalent transports is not a setting.
    auth: Option<GrpcIngestAuth>,
}

/// What a gRPC ingest call must present when `otel.auth.required` is set.
#[derive(Clone)]
pub struct GrpcIngestAuth {
    pub cache: Arc<crate::data::cache::CacheService>,
    pub database: Arc<crate::data::TransactionalService>,
    pub api_key_secret: Arc<Vec<u8>>,
    /// Present when per-IP limiting is on, so a brute force here costs what it costs over HTTP.
    ///
    /// Without it the two transports were asymmetric in the attacker's favour: invalid HTTP attempts are
    /// counted and eventually answered 429, while unlimited gRPC attempts kept reaching key validation. The
    /// bound on guessing has to be the same on both, or the weaker one is the only one that matters.
    pub rate_limiter: Option<Arc<crate::data::cache::RateLimiter>>,
    /// Whose forwarded-for metadata may be believed - shared with the HTTP transport.
    pub trusted_proxies: Arc<crate::utils::client_ip::TrustedProxies>,
}

impl GrpcIngestAuth {
    /// Authorise one request against the project it names, mirroring the HTTP middleware.
    ///
    /// The same `validate_api_key_for_project` with the same `Ingest` scope, so the two transports cannot
    /// drift apart on what a key is allowed to do. The project id comes from the metadata the call also uses
    /// to route its data, so a key valid for another organisation's project cannot write here.
    async fn authorize<T>(&self, request: &Request<T>, project_id: &str) -> Result<(), Status> {
        // The same attribution the HTTP transport uses, through the same helper - see
        // `utils::client_ip` for why a forwarded address is believed only from a configured trusted proxy,
        // and why both alternatives (peer-only, or unconditional trust) are wrong.
        //
        // The bucket name differs (`grpc_auth_fail`), so the two transports cannot exhaust each other's
        // counters: sharing one namespace let a spoofable value on either side reach a peer on the other.
        let client_ip = crate::utils::client_ip::attributable_ip(
            request.remote_addr().map(|a| a.ip()),
            request
                .metadata()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok()),
            &self.trusted_proxies,
        );
        if let (Some(limiter), Some(ip)) = (&self.rate_limiter, &client_ip) {
            let bucket = crate::data::cache::RateLimitBucket::grpc_auth_failures(
                crate::core::constants::DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM,
            );
            if limiter.is_blocked(&bucket, ip).await {
                tracing::warn!(ip = %ip, "gRPC OTLP auth blocked due to too many failures");
                return Err(Status::resource_exhausted(
                    "too many authentication failures",
                ));
            }
        }

        let header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                Status::unauthenticated(
                    "OTLP ingestion requires an API key in the authorization metadata",
                )
            })?;
        let result = crate::api::auth::validate_api_key_for_project(
            &self.cache,
            Arc::clone(&self.database),
            &self.api_key_secret,
            header,
            project_id,
            crate::data::types::ApiKeyScope::Ingest,
        )
        .await;

        // Counted on failure only, exactly as the HTTP middleware does.
        if let Err(ref e) = result
            && matches!(
                e,
                crate::api::auth::ApiKeyAuthError::InvalidKey
                    | crate::api::auth::ApiKeyAuthError::Expired
            )
            && let (Some(limiter), Some(ip)) = (&self.rate_limiter, &client_ip)
        {
            let bucket = crate::data::cache::RateLimitBucket::grpc_auth_failures(
                crate::core::constants::DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM,
            );
            let _ = limiter.check(&bucket, ip).await;
        }

        result.map(|_| ()).map_err(|e| {
            tracing::debug!(project_id, error = %e, "Refused a gRPC OTLP export");
            Status::unauthenticated("invalid or unauthorized API key")
        })
    }
}

impl OtlpGrpcServer {
    pub fn new(
        config: &OtelConfig,
        host: &str,
        topics: &Arc<TopicService>,
        storage: &AppStorage,
        stores: IngestStores,
        debug: bool,
        auth: Option<GrpcIngestAuth>,
    ) -> Result<Self> {
        let IngestStores {
            analytics,
            database,
            trace_pipeline,
        } = stores;
        let addr = SocketAddr::new(host.parse()?, config.grpc_port);
        let debug_path = if debug {
            Some(storage.subdir(DataSubdir::Debug))
        } else {
            None
        };
        // Use stream topic for traces (at-least-once delivery)
        let trace_topic = Arc::new(topics.stream_topic::<ExportTraceServiceRequest>(TOPIC_TRACES));
        let logs_publisher = topics
            .topic::<ExportLogsServiceRequest>(TOPIC_LOGS)
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .publisher();
        Ok(Self {
            addr,
            trace_topic,
            analytics,
            database,
            trace_pipeline,
            logs_publisher,
            debug_path,
            auth,
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
                    self.trace_pipeline,
                    Arc::clone(&self.database),
                    self.auth.clone(),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                MetricsServiceServer::new(OtlpMetricsService::new(
                    self.analytics,
                    self.database,
                    debug_path.clone(),
                    self.auth.clone(),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                LogsServiceServer::new(OtlpLogsService::new(
                    self.logs_publisher,
                    debug_path,
                    self.auth,
                ))
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

/// What the ingest services write through.
///
/// Grouped because they travel together and are passed as a unit: metrics are written inside their
/// request, and traces are too whenever the topic backend cannot promise durability.
pub struct IngestStores {
    pub analytics: Arc<crate::data::AnalyticsService>,
    pub database: Arc<crate::data::TransactionalService>,
    /// Present exactly when the queue is not durable, in which case traces are written in the request.
    pub trace_pipeline: Option<Arc<crate::domain::TracePipeline>>,
}

/// Whether this project exists and is not being deleted, for the gRPC services.
///
/// HTTP had this and gRPC did not, which meant the same deployment refused an unknown project on one
/// transport and answered success on the other while the write path dropped the records. The authoritative
/// check is still next to the write; this is where an exporter can be *told*.
async fn project_accepts_writes(
    database: &Arc<crate::data::TransactionalService>,
    project_id: &str,
) -> bool {
    match database.repository().get_project(None, project_id).await {
        Ok(found) => found.is_some(),
        // Unknown: let the write path decide, as the HTTP twin does. Refusing on a lookup failure would
        // turn a database blip into rejected telemetry.
        Err(e) => {
            tracing::warn!(project_id, error = %e, "Could not check the project at ingest");
            true
        }
    }
}

/// gRPC trace service
struct OtlpTraceService {
    topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
    debug_path: Option<PathBuf>,
    /// The synchronous path, present exactly when the queue cannot promise durability. The HTTP handler
    /// had this and gRPC did not, so the same deployment answered honestly on one transport and not the
    /// other.
    pipeline: Option<Arc<crate::domain::TracePipeline>>,
    /// For telling an exporter now that its project will not accept writes.
    database: Arc<crate::data::TransactionalService>,
    /// Set exactly when `otel.auth.required` is on - see `GrpcIngestAuth`.
    auth: Option<GrpcIngestAuth>,
}

impl OtlpTraceService {
    fn new(
        topic: Arc<StreamTopic<ExportTraceServiceRequest>>,
        debug_path: Option<PathBuf>,
        pipeline: Option<Arc<crate::domain::TracePipeline>>,
        database: Arc<crate::data::TransactionalService>,
        auth: Option<GrpcIngestAuth>,
    ) -> Self {
        Self {
            topic,
            debug_path,
            pipeline,
            database,
            auth,
        }
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
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.auth {
            auth.authorize(&request, &project_id).await?;
        }
        if !project_accepts_writes(&self.database, &project_id).await {
            return Err(Status::not_found("unknown project, or it is being deleted"));
        }
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_traces(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "traces.jsonl", &project_id, &req).await;
        }

        // Acknowledge only what is durable - see the HTTP twin. When the queue is in memory, a published
        // trace is lost by the crash that this transport's success message has already denied.
        if let Some(pipeline) = self.pipeline.as_ref() {
            match pipeline.ingest_now(&req).await {
                crate::domain::traces::IngestOutcome::Stored => {}
                // Reported, not swallowed as success: an exporter told success for records that were
                // discarded has no way to learn otherwise.
                // *Why* decides the answer - see the HTTP twin. A live project whose span cannot be stored
                // must not be told "unknown project", which sends the exporter to retry a doomed payload
                // forever against a project that is healthy.
                crate::domain::traces::IngestOutcome::Dropped { spans, reason } => match reason {
                    crate::domain::traces::DropReason::Gone => {
                        return Err(Status::not_found(
                            "unknown project, trace or session, or it is being deleted",
                        ));
                    }
                    crate::domain::traces::DropReason::Unstorable => {
                        return Ok(Response::new(ExportTraceServiceResponse {
                            partial_success: Some(
                                opentelemetry_proto::tonic::collector::trace::v1::ExportTracePartialSuccess {
                                    rejected_spans: spans as i64,
                                    error_message: "spans were rejected as unstorable; check their \
                                                    timestamps are within 1900-2299"
                                        .to_string(),
                                },
                            ),
                        }));
                    }
                },
                // Something was stored, so this is a success reporting what it rejected - see the HTTP
                // twin.
                crate::domain::traces::IngestOutcome::PartlyDropped { spans, reason } => {
                    return Ok(Response::new(ExportTraceServiceResponse {
                        partial_success: Some(
                            opentelemetry_proto::tonic::collector::trace::v1::ExportTracePartialSuccess {
                                rejected_spans: spans as i64,
                                // True to the cause - see the HTTP twin.
                                error_message: match reason {
                                    crate::domain::traces::DropReason::Gone => {
                                        "some spans' project, trace or session is unknown or is being deleted"
                                    }
                                    crate::domain::traces::DropReason::Unstorable => {
                                        "some spans were rejected as unstorable; check their timestamps are \
                                         within 1900-2299"
                                    }
                                }
                                .to_string(),
                            },
                        ),
                    }));
                }
                crate::domain::traces::IngestOutcome::Failed => {
                    tracing::error!(%project_id, "Failed to store traces");
                    return Err(Status::unavailable("could not store traces"));
                }
            }
            return Ok(Response::new(ExportTraceServiceResponse {
                partial_success: None,
            }));
        }

        // Unstorable spans are rejected before the queue answers 200 for them, exactly as on the HTTP
        // transport - the durable acknowledgement is honest only about what can actually be stored, and
        // nothing downstream can report back to a call that has already returned. Having this on one of two
        // equivalent transports was the same class of gap as `otel.auth.required` gating HTTP only.
        let unstorable = crate::domain::traces::strip_unstorable_spans(&mut req);
        let remaining: usize = req
            .resource_spans
            .iter()
            .flat_map(|r| r.scope_spans.iter())
            .map(|s| s.spans.len())
            .sum();
        if unstorable > 0 && remaining == 0 {
            return Ok(Response::new(ExportTraceServiceResponse {
                partial_success: Some(
                    opentelemetry_proto::tonic::collector::trace::v1::ExportTracePartialSuccess {
                        rejected_spans: unstorable as i64,
                        error_message:
                            "spans were rejected as unstorable; check their timestamps are \
                                        within 1900-2299"
                                .to_string(),
                    },
                ),
            }));
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

        // Reporting what was stripped, if any - see the HTTP twin.
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: (unstorable > 0).then(|| {
                opentelemetry_proto::tonic::collector::trace::v1::ExportTracePartialSuccess {
                    rejected_spans: unstorable as i64,
                    error_message:
                        "some spans were rejected as unstorable; check their timestamps are within 1900-2299"
                            .to_string(),
                }
            }),
        }))
    }
}

/// gRPC metrics service
struct OtlpMetricsService {
    analytics: Arc<crate::data::AnalyticsService>,
    database: Arc<crate::data::TransactionalService>,
    debug_path: Option<PathBuf>,
    /// Set exactly when `otel.auth.required` is on - see `GrpcIngestAuth`.
    auth: Option<GrpcIngestAuth>,
}

impl OtlpMetricsService {
    fn new(
        analytics: Arc<crate::data::AnalyticsService>,
        database: Arc<crate::data::TransactionalService>,
        debug_path: Option<PathBuf>,
        auth: Option<GrpcIngestAuth>,
    ) -> Self {
        Self {
            analytics,
            database,
            debug_path,
            auth,
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
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.auth {
            auth.authorize(&request, &project_id).await?;
        }
        if !project_accepts_writes(&self.database, &project_id).await {
            return Err(Status::not_found("unknown project, or it is being deleted"));
        }
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_metrics(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "metrics.jsonl", &project_id, &req).await;
        }

        // Written before the answer - see the HTTP twin: a queued acknowledgement was a 200 for records
        // an in-process buffer could lose.
        let stored =
            match crate::domain::ingest_metrics(&req, &self.analytics, &self.database).await {
                Ok(stored) => stored,
                Err(e) => {
                    tracing::error!(error = %e, %project_id, "Failed to store metrics");
                    return Err(Status::unavailable("could not store metrics"));
                }
            };

        // Reported, not swallowed - see the HTTP twin.
        let rejected = stored.total.saturating_sub(stored.stored);
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: (rejected > 0).then(|| {
                opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsPartialSuccess {
                    rejected_data_points: rejected as i64,
                    error_message: "the project is unknown or is being deleted".to_string(),
                }
            }),
        }))
    }
}

/// gRPC logs service
struct OtlpLogsService {
    publisher: Publisher<ExportLogsServiceRequest>,
    debug_path: Option<PathBuf>,
    /// Set exactly when `otel.auth.required` is on - see `GrpcIngestAuth`.
    auth: Option<GrpcIngestAuth>,
}

impl OtlpLogsService {
    fn new(
        publisher: Publisher<ExportLogsServiceRequest>,
        debug_path: Option<PathBuf>,
        auth: Option<GrpcIngestAuth>,
    ) -> Self {
        Self {
            publisher,
            debug_path,
            auth,
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
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.auth {
            auth.authorize(&request, &project_id).await?;
        }
        let mut req = request.into_inner();

        // Inject project_id into resource attributes
        inject_project_id_logs(&mut req, &project_id);

        // Write to debug file if debug mode is enabled
        if let Some(ref debug_path) = self.debug_path {
            write_debug(debug_path, "logs.jsonl", &project_id, &req).await;
        }

        // Rejected with a reason rather than silently accepted - see the HTTP twin: SideSeat stores no
        // logs, and answering with an unqualified success made an exporter count them as delivered.
        let rejected: i64 = req
            .resource_logs
            .iter()
            .flat_map(|resource| resource.scope_logs.iter())
            .map(|scope| scope.log_records.len() as i64)
            .sum();
        if let Err(e) = self.publisher.publish(req) {
            tracing::debug!(error = %e, "Failed to publish logs to topic");
        }

        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: Some(
                opentelemetry_proto::tonic::collector::logs::v1::ExportLogsPartialSuccess {
                    rejected_log_records: rejected,
                    error_message: "SideSeat does not store logs; send traces to /v1/traces"
                        .to_string(),
                },
            ),
        }))
    }
}

#[cfg(test)]
mod grpc_auth_tests {
    /// Every gRPC ingest service authorises before it reads its payload.
    ///
    /// `otel.auth.required` used to apply to the HTTP transport only: this server had no interceptor at all
    /// and took the project id from an untrusted `x-sideseat-project-id` metadata entry, so an
    /// unauthenticated client could write traces, metrics or logs into any existing project while the
    /// operator believed ingestion was locked down. A setting enforced on one of two equivalent transports is
    /// not a setting.
    ///
    /// The compiler is the first guard: each service owns its own `auth` field, so deleting any one gate makes
    /// that field dead and fails the build. This test covers what the compiler cannot - that the gate runs
    /// *before* the payload is consumed, and that a fourth signal added later carries it too. The rule: each `extract_project_id` in an `export` handler is followed
    /// by the authorisation call before anything else happens.
    #[test]
    fn every_grpc_export_authorizes_before_reading_its_payload() {
        let whole = include_str!("grpc.rs");
        // Only the code above this test module: the assertions below quote the very strings they look for,
        // so scanning the whole file would count them too.
        let source = whole
            .split_once("mod grpc_auth_tests {")
            .map(|(code, _)| code)
            .unwrap_or(whole);
        let extractions: Vec<usize> = source
            .match_indices("let project_id = extract_project_id(&request)")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            extractions.len(),
            3,
            "expected the trace, metrics and logs services; found {} extraction sites - a new signal must \
             carry the same gate",
            extractions.len()
        );
        for start in extractions {
            // The gate has to appear before the payload is consumed, so look only at the window between the
            // extraction and the first `into_inner()` that follows it.
            let rest = &source[start..];
            let consumed = rest.find("into_inner()").unwrap_or(rest.len());
            let window = &rest[..consumed];
            assert!(
                window.contains("auth.authorize(&request, &project_id)"),
                "a gRPC export reads its payload without authorising the project it names; \
                 otel.auth.required would then apply to HTTP only"
            );
        }
    }
}
