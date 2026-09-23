//! gRPC OTLP server

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

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

use crate::extractors::is_valid_project_id;
use sideseat_core::config::OtelConfig;
use sideseat_core::constants::{OTLP_BODY_LIMIT, TOPIC_TRACES};
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_domain::signals::{
    LogSignal, MetricsSignal, SignalContext, SignalExportError, TraceSignal, export_signal,
};
use sideseat_domain::staging::{StagedPayloadRef, StagingService};
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_domain::topics::TopicService;
use sideseat_ports::clock::Clock;

const PROJECT_ID_HEADER: &str = "x-sideseat-project-id";
const DEFAULT_PROJECT_ID: &str = "default";

pub struct OtlpGrpcServer {
    addr: SocketAddr,
    trace_signal: Arc<TraceSignal>,
    metrics_signal: Arc<MetricsSignal>,
    log_signal: Arc<LogSignal>,
    database: Arc<crate::dependencies::TransactionalStore>,
    debug_path: Option<PathBuf>,
    /// What `otel.auth.required` demands of *this* transport.
    ///
    /// The setting used to apply to HTTP only: the gRPC server had no interceptor at all and took the
    /// project id from an untrusted `x-sideseat-project-id` metadata entry, so with auth required an
    /// unauthenticated client could still write traces or metrics into any existing project. A setting that
    /// is enforced on one of two equivalent transports is not a setting.
    guards: GrpcIngestGuards,
    clock: Arc<dyn Clock>,
    staging: Arc<StagingService>,
    storage_governance: Arc<StorageGovernanceService>,
}

/// What a gRPC ingest call must present when `otel.auth.required` is set.
#[derive(Clone)]
pub struct GrpcIngestAuth {
    pub cache: Arc<crate::dependencies::SharedCache>,
    pub database: Arc<crate::dependencies::TransactionalStore>,
    pub api_key_secret: Arc<Vec<u8>>,
    /// Present when per-IP limiting is on, so a brute force here costs what it costs over HTTP.
    ///
    /// Without it the two transports were asymmetric in the attacker's favour: invalid HTTP attempts are
    /// counted and eventually answered 429, while unlimited gRPC attempts kept reaching key validation. The
    /// bound on guessing has to be the same on both, or the weaker one is the only one that matters.
    pub rate_limiter: Option<Arc<sideseat_domain::rate_limit::RateLimiter>>,
    /// Whose forwarded-for metadata may be believed - shared with the HTTP transport.
    pub trusted_proxies: Arc<sideseat_core::utils::client_ip::TrustedProxies>,
    pub clock: Arc<dyn Clock>,
}

/// What every gRPC ingest call passes through before its payload is read.
///
/// The two travel together because they are the same kind of thing - a gate the HTTP transport already has -
/// and because each was added separately and the second one was missed: `otel.auth.required` applied to HTTP
/// only until it was fixed, and then `rate_limit.ingestion_rpm` did. Grouping them means a third gate is one
/// field on one struct rather than another parameter nobody threads through.
#[derive(Clone, Default)]
pub struct GrpcIngestGuards {
    /// Set exactly when `otel.auth.required` is on.
    pub auth: Option<GrpcIngestAuth>,
    /// Set when rate limiting is on.
    pub limit: Option<GrpcIngestLimit>,
}

/// The per-project ingestion limit, applied to gRPC exactly as the HTTP middleware applies it.
///
/// It was missing entirely: HTTP OTLP routes carry `RateLimitBucket::ingestion` keyed by project, while the
/// gRPC server had only the auth-failure limiter - so `rate_limit.ingestion_rpm` was a setting enforced on
/// one of two equivalent transports, and an exporter that got 429s over HTTP was unlimited over gRPC.
///
/// **The same bucket as HTTP**, not a separate namespace. The auth-failure buckets are deliberately separate
/// because their key is a spoofable address and one transport must not exhaust the other's counters; this
/// key is the project the data is being written to, so the quota belongs to the project rather than to a
/// transport - separate buckets would hand a client twice the limit for splitting its traffic.
#[derive(Clone)]
pub struct GrpcIngestLimit {
    pub limiter: Arc<sideseat_domain::rate_limit::RateLimiter>,
    pub ingestion_rpm: u32,
}

impl GrpcIngestLimit {
    /// Applied before authorisation, as the HTTP layer is, so an unauthenticated flood is bounded too.
    async fn check(limit: Option<&Self>, project_id: &str) -> Result<(), Status> {
        let Some(limit) = limit else {
            return Ok(());
        };
        let bucket = sideseat_domain::rate_limit::RateLimitBucket::ingestion(limit.ingestion_rpm);
        let result = limit.limiter.check(&bucket, project_id).await;
        if !result.allowed {
            tracing::warn!(
                project_id,
                retry_after = ?result.retry_after,
                "gRPC OTLP export refused: ingestion rate limit"
            );
            return Err(Status::resource_exhausted(
                "ingestion rate limit exceeded for this project",
            ));
        }
        Ok(())
    }
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
        let client_ip = sideseat_core::utils::client_ip::attributable_ip(
            request.remote_addr().map(|a| a.ip()),
            request
                .metadata()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok()),
            &self.trusted_proxies,
        );
        if let (Some(limiter), Some(ip)) = (&self.rate_limiter, &client_ip) {
            let bucket = sideseat_domain::rate_limit::RateLimitBucket::grpc_auth_failures(
                sideseat_core::constants::DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM,
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
        let result = crate::auth::validate_api_key_for_project(
            Arc::clone(&self.database),
            &self.api_key_secret,
            header,
            project_id,
            sideseat_ports::types::ApiKeyScope::Ingest,
            self.clock.as_ref(),
        )
        .await;

        // Counted on failure only, exactly as the HTTP middleware does.
        if let Err(ref e) = result
            && matches!(
                e,
                crate::auth::ApiKeyAuthError::InvalidKey | crate::auth::ApiKeyAuthError::Expired
            )
            && let (Some(limiter), Some(ip)) = (&self.rate_limiter, &client_ip)
        {
            let bucket = sideseat_domain::rate_limit::RateLimitBucket::grpc_auth_failures(
                sideseat_core::constants::DEFAULT_RATE_LIMIT_AUTH_FAILURES_RPM,
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
        guards: GrpcIngestGuards,
    ) -> Result<Self> {
        let IngestStores {
            analytics,
            database,
            trace_pipeline,
            clock,
            staging,
            storage_governance,
        } = stores;
        let addr = SocketAddr::new(host.parse()?, config.grpc_port);
        let debug_path = if debug {
            Some(storage.subdir(DataSubdir::Debug))
        } else {
            None
        };
        // Use stream topic for traces (at-least-once delivery)
        let trace_topic = Arc::new(topics.stream_topic::<StagedPayloadRef>(TOPIC_TRACES));
        let trace_signal = Arc::new(TraceSignal::new(trace_topic, trace_pipeline));
        let metrics_signal = Arc::new(MetricsSignal::new(
            Arc::clone(&analytics),
            Arc::clone(&database),
        ));
        let log_signal = Arc::new(LogSignal::new(analytics, Arc::clone(&database)));
        Ok(Self {
            addr,
            trace_signal,
            metrics_signal,
            database,
            log_signal,
            debug_path,
            guards,
            clock,
            staging,
            storage_governance,
        })
    }

    pub async fn start(self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        let addr = self.addr;
        let debug_path = self.debug_path;

        tracing::debug!(%addr, "Starting OTLP gRPC server");

        TonicServer::builder()
            .add_service(
                TraceServiceServer::new(OtlpTraceService::new(
                    self.trace_signal,
                    debug_path.clone(),
                    Arc::clone(&self.database),
                    self.guards.clone(),
                    Arc::clone(&self.clock),
                    Arc::clone(&self.staging),
                    Arc::clone(&self.storage_governance),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                MetricsServiceServer::new(OtlpMetricsService::new(
                    self.metrics_signal,
                    Arc::clone(&self.database),
                    debug_path.clone(),
                    self.guards.clone(),
                    Arc::clone(&self.clock),
                    Arc::clone(&self.staging),
                    Arc::clone(&self.storage_governance),
                ))
                .max_decoding_message_size(OTLP_BODY_LIMIT)
                .max_encoding_message_size(OTLP_BODY_LIMIT),
            )
            .add_service(
                LogsServiceServer::new(OtlpLogsService::new(
                    self.log_signal,
                    Arc::clone(&self.database),
                    debug_path,
                    self.guards,
                    self.clock,
                    self.staging,
                    self.storage_governance,
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
    pub analytics: Arc<crate::dependencies::AnalyticsStore>,
    pub database: Arc<crate::dependencies::TransactionalStore>,
    /// Present exactly when the queue is not durable, in which case traces are written in the request.
    pub trace_pipeline: Option<Arc<sideseat_domain::traces::TracePipeline>>,
    pub clock: Arc<dyn Clock>,
    pub staging: Arc<StagingService>,
    pub storage_governance: Arc<StorageGovernanceService>,
}

/// Whether this project exists and is not being deleted, for the gRPC services.
///
/// HTTP had this and gRPC did not, which meant the same deployment refused an unknown project on one
/// transport and answered success on the other while the write path dropped the records. The authoritative
/// check is still next to the write; this is where an exporter can be *told*.
async fn project_accepts_writes(
    database: &Arc<crate::dependencies::TransactionalStore>,
    project_id: &str,
) -> bool {
    match database.as_ref().get_project(project_id).await {
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
    signal: Arc<TraceSignal>,
    debug_path: Option<PathBuf>,
    /// For telling an exporter now that its project will not accept writes.
    database: Arc<crate::dependencies::TransactionalStore>,
    /// The gates every export passes through - see `GrpcIngestGuards`.
    guards: GrpcIngestGuards,
    clock: Arc<dyn Clock>,
    staging: Arc<StagingService>,
    storage_governance: Arc<StorageGovernanceService>,
}

impl OtlpTraceService {
    fn new(
        signal: Arc<TraceSignal>,
        debug_path: Option<PathBuf>,
        database: Arc<crate::dependencies::TransactionalStore>,
        guards: GrpcIngestGuards,
        clock: Arc<dyn Clock>,
        staging: Arc<StagingService>,
        storage_governance: Arc<StorageGovernanceService>,
    ) -> Self {
        Self {
            signal,
            debug_path,
            database,
            guards,
            clock,
            staging,
            storage_governance,
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
        // Limited before authorisation, as the HTTP layer is - see `GrpcIngestLimit`.
        GrpcIngestLimit::check(self.guards.limit.as_ref(), &project_id).await?;
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.guards.auth {
            auth.authorize(&request, &project_id).await?;
        }
        if !project_accepts_writes(&self.database, &project_id).await {
            return Err(Status::not_found("unknown project, or it is being deleted"));
        }
        let req = request.into_inner();
        export_signal(
            self.signal.as_ref(),
            req,
            SignalContext {
                project_id: &project_id,
                debug_path: self.debug_path.as_deref(),
                clock: self.clock.as_ref(),
                staging: self.staging.as_ref(),
                storage_governance: self.storage_governance.as_ref(),
            },
        )
        .await
        .map(Response::new)
        .map_err(|error| match error {
            SignalExportError::Gone => {
                Status::not_found("unknown project, trace or session, or it is being deleted")
            }
            SignalExportError::StoreUnavailable => Status::unavailable("could not store traces"),
            SignalExportError::QueueFull => Status::resource_exhausted("trace buffer full"),
            SignalExportError::QuotaExceeded => {
                Status::resource_exhausted("project storage quota exceeded")
            }
        })
    }
}

/// gRPC metrics service
struct OtlpMetricsService {
    signal: Arc<MetricsSignal>,
    database: Arc<crate::dependencies::TransactionalStore>,
    debug_path: Option<PathBuf>,
    /// The gates every export passes through - see `GrpcIngestGuards`.
    guards: GrpcIngestGuards,
    clock: Arc<dyn Clock>,
    staging: Arc<StagingService>,
    storage_governance: Arc<StorageGovernanceService>,
}

impl OtlpMetricsService {
    fn new(
        signal: Arc<MetricsSignal>,
        database: Arc<crate::dependencies::TransactionalStore>,
        debug_path: Option<PathBuf>,
        guards: GrpcIngestGuards,
        clock: Arc<dyn Clock>,
        staging: Arc<StagingService>,
        storage_governance: Arc<StorageGovernanceService>,
    ) -> Self {
        Self {
            signal,
            database,
            debug_path,
            guards,
            clock,
            staging,
            storage_governance,
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
        // Limited before authorisation, as the HTTP layer is - see `GrpcIngestLimit`.
        GrpcIngestLimit::check(self.guards.limit.as_ref(), &project_id).await?;
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.guards.auth {
            auth.authorize(&request, &project_id).await?;
        }
        if !project_accepts_writes(&self.database, &project_id).await {
            return Err(Status::not_found("unknown project, or it is being deleted"));
        }
        let req = request.into_inner();
        export_signal(
            self.signal.as_ref(),
            req,
            SignalContext {
                project_id: &project_id,
                debug_path: self.debug_path.as_deref(),
                clock: self.clock.as_ref(),
                staging: self.staging.as_ref(),
                storage_governance: self.storage_governance.as_ref(),
            },
        )
        .await
        .map(Response::new)
        .map_err(|error| match error {
            SignalExportError::Gone => Status::not_found("unknown project, or it is being deleted"),
            SignalExportError::StoreUnavailable => Status::unavailable("could not store metrics"),
            SignalExportError::QueueFull => Status::resource_exhausted("metrics buffer full"),
            SignalExportError::QuotaExceeded => {
                Status::resource_exhausted("project storage quota exceeded")
            }
        })
    }
}

/// gRPC logs service
struct OtlpLogsService {
    signal: Arc<LogSignal>,
    database: Arc<crate::dependencies::TransactionalStore>,
    debug_path: Option<PathBuf>,
    /// The gates every export passes through - see `GrpcIngestGuards`.
    guards: GrpcIngestGuards,
    clock: Arc<dyn Clock>,
    staging: Arc<StagingService>,
    storage_governance: Arc<StorageGovernanceService>,
}

impl OtlpLogsService {
    fn new(
        signal: Arc<LogSignal>,
        database: Arc<crate::dependencies::TransactionalStore>,
        debug_path: Option<PathBuf>,
        guards: GrpcIngestGuards,
        clock: Arc<dyn Clock>,
        staging: Arc<StagingService>,
        storage_governance: Arc<StorageGovernanceService>,
    ) -> Self {
        Self {
            signal,
            database,
            debug_path,
            guards,
            clock,
            staging,
            storage_governance,
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
        // Limited before authorisation, as the HTTP layer is - see `GrpcIngestLimit`.
        GrpcIngestLimit::check(self.guards.limit.as_ref(), &project_id).await?;
        // Authorised *before* anything is read from the payload, and against the project the call names -
        // so `otel.auth.required` refuses an unauthenticated write here exactly as it does over HTTP.
        if let Some(auth) = &self.guards.auth {
            auth.authorize(&request, &project_id).await?;
        }
        if !project_accepts_writes(&self.database, &project_id).await {
            return Err(Status::not_found("unknown project, or it is being deleted"));
        }
        let req = request.into_inner();
        export_signal(
            self.signal.as_ref(),
            req,
            SignalContext {
                project_id: &project_id,
                debug_path: self.debug_path.as_deref(),
                clock: self.clock.as_ref(),
                staging: self.staging.as_ref(),
                storage_governance: self.storage_governance.as_ref(),
            },
        )
        .await
        .map(Response::new)
        .map_err(|error| match error {
            SignalExportError::Gone => Status::not_found("unknown project, or it is being deleted"),
            SignalExportError::StoreUnavailable => Status::unavailable("could not store logs"),
            SignalExportError::QueueFull => Status::resource_exhausted("logs buffer full"),
            SignalExportError::QuotaExceeded => {
                Status::resource_exhausted("project storage quota exceeded")
            }
        })
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
            // And the ingestion limit, for the same reason and in the same window: it was missing from this
            // transport entirely, so `rate_limit.ingestion_rpm` bounded HTTP exporters and not gRPC ones.
            assert!(
                window.contains("GrpcIngestLimit::check(self.guards.limit.as_ref(), &project_id)"),
                "a gRPC export reads its payload without applying the project's ingestion limit; \
                 rate_limit.ingestion_rpm would then apply to HTTP only"
            );
        }
    }
}
