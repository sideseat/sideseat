//! Transport-neutral OTLP signal ingestion.
//!
//! HTTP and gRPC differ in framing, authentication and status mapping. Once a request is decoded,
//! however, they must make the same decisions about project injection, debug capture, storability,
//! durability and partial success. This module owns that shared decision tree.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use opentelemetry_proto::tonic::collector::{
    logs::v1::{ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse},
    metrics::v1::{
        ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    },
    trace::v1::{ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse},
};
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::metric;
use prost::Message;
use serde::Serialize;

use crate::otlp::{
    PROJECT_ID_ATTR, inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces,
};
use crate::staging::{StagedPayloadRef, StagingDisposition, StagingService};
use crate::storage_governance::{GovernanceError, StorageGovernanceService};
use crate::topics::{StreamTopic, TopicMessage};
use crate::traces::{DropReason, IngestOutcome, TracePipeline, strip_unstorable_spans};
use sideseat_core::core::constants::{TOPIC_LOGS, TOPIC_METRICS, TOPIC_TRACES};
use sideseat_core::utils::debug::write_debug;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::{StagedRecord, StagedSignal};

const PUBLISH_MAX_ATTEMPTS: u32 = 3;
const PUBLISH_BASE_DELAY_MS: u64 = 50;

/// Signals whose complete lifecycle has moved through [`export_signal`].
///
/// The repository gate uses this registry to require both HTTP and gRPC registration. Adding an
/// implementation here before both transports use it is intentionally a failing change.
pub const REGISTERED_SIGNAL_NAMES: &[&str] = &["traces", "metrics", "logs"];

/// The durability promise made before an OTLP success is returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityRequirement {
    /// The signal must be committed to its store or to a durable queue before acknowledgement.
    DurableBeforeAck,
}

/// How this deployment satisfies a signal's durability requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleStrategy {
    /// Persist in the request and answer only after the write finishes.
    PersistBeforeAck,
    /// Publish to an at-least-once stream and let its consumer persist asynchronously.
    DurableQueue,
}

/// Stable identity used by a signal's confirmation predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalIdentity {
    /// Trace id, span id and the position inside that span's own carrier.
    TraceSpanCarrierPosition,
    /// The metric datapoint digest.
    MetricDatapointId,
    /// The semantic log digest and its ordinal within the export.
    LogDigestAndOrdinal,
}

/// What a staged delivery must prove before its payload may be released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationPredicate {
    pub identity: SignalIdentity,
    /// Confirmation compares the stored producer-content digest for strict equality.
    pub strict_content_digest: bool,
    /// System-managed fields are deliberately absent from the digest.
    pub excludes_system_metadata: bool,
}

/// Static facts every signal must declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalDescriptor {
    pub name: &'static str,
    pub queue_topic: &'static str,
    pub debug_file: &'static str,
    pub durability: DurabilityRequirement,
    pub confirmation: ConfirmationPredicate,
}

/// Why a decoded request could not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalExportError {
    Gone,
    StoreUnavailable,
    QueueFull,
    QuotaExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalRejection {
    Gone,
    Unstorable,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistOutcome {
    Stored,
    Dropped {
        records: usize,
        reason: SignalRejection,
    },
    PartlyDropped {
        records: usize,
        reason: SignalRejection,
    },
}

/// Inputs shared by the two OTLP transports after framing and authorization.
pub struct SignalContext<'a> {
    pub project_id: &'a str,
    pub debug_path: Option<&'a Path>,
    pub clock: &'a dyn Clock,
    pub staging: &'a StagingService,
    pub storage_governance: &'a StorageGovernanceService,
}

#[async_trait]
pub trait Signal: Send + Sync {
    type Request: Serialize + Message + Default + Send + Sync;
    type Response: Send;

    fn descriptor(&self) -> SignalDescriptor;
    fn lifecycle(&self) -> LifecycleStrategy;
    fn inject_project_id(&self, request: &mut Self::Request, project_id: &str);
    fn strip_unstorable(&self, request: &mut Self::Request) -> usize;
    fn record_count(&self, request: &Self::Request) -> usize;
    fn staged_signal(&self) -> StagedSignal;
    fn staged_records(
        &self,
        request: &Self::Request,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> Vec<StagedRecord>;
    fn partition_key(&self, request: &Self::Request) -> String;
    fn response(&self, rejected: Option<(usize, SignalRejection, bool)>) -> Self::Response;
    fn total_drop_is_error(&self, reason: SignalRejection) -> bool;

    async fn persist(
        &self,
        request: &Self::Request,
        _context: &SignalContext<'_>,
    ) -> Result<PersistOutcome, String>;
    async fn publish(&self, payload: &StagedPayloadRef) -> Result<(), String>;
}

/// Process one decoded OTLP request through the signal's declared lifecycle.
pub async fn export_signal<S: Signal>(
    signal: &S,
    mut request: S::Request,
    context: SignalContext<'_>,
) -> Result<S::Response, SignalExportError> {
    signal.inject_project_id(&mut request, context.project_id);

    let descriptor = signal.descriptor();
    if let Some(debug_path) = context.debug_path {
        write_debug(
            debug_path,
            descriptor.debug_file,
            context.project_id,
            context.clock.now(),
            &request,
        )
        .await;
    }

    let rejected_unstorable = signal.strip_unstorable(&mut request);
    let remaining = signal.record_count(&request);
    if remaining == 0 {
        return Ok(signal.response((rejected_unstorable > 0).then_some((
            rejected_unstorable,
            SignalRejection::Unstorable,
            false,
        ))));
    }

    let records = signal.staged_records(&request, context.clock.now());
    if records.len() != remaining {
        tracing::error!(
            signal = descriptor.name,
            records = records.len(),
            remaining,
            "Signal extraction disagrees with its record count"
        );
        return Err(SignalExportError::StoreUnavailable);
    }
    let encoded = request.encode_to_vec();
    context
        .storage_governance
        .admit(
            &sideseat_ports::types::ProjectId::from(context.project_id),
            u64::try_from(encoded.len()).unwrap_or(u64::MAX),
        )
        .await
        .map_err(|error| match error {
            GovernanceError::QuotaExceeded { .. } => SignalExportError::QuotaExceeded,
            _ => SignalExportError::StoreUnavailable,
        })?;
    let payload_ref = context
        .staging
        .stage(
            context.project_id,
            signal.staged_signal(),
            &encoded,
            records,
            signal.partition_key(&request),
        )
        .await
        .map_err(|error| {
            tracing::error!(
                signal = descriptor.name,
                project_id = context.project_id,
                %error,
                "Failed to durably stage OTLP signal"
            );
            SignalExportError::StoreUnavailable
        })?;

    match signal.lifecycle() {
        LifecycleStrategy::PersistBeforeAck => {
            let mut last_outcome = None;
            loop {
                match signal.persist(&request, &context).await {
                    Ok(outcome) => last_outcome = Some(outcome),
                    Err(error) => tracing::error!(
                        signal = descriptor.name,
                        project_id = context.project_id,
                        %error,
                        "Failed to persist staged OTLP signal"
                    ),
                }

                let Some((payload, _)) = context
                    .staging
                    .load(&payload_ref.id)
                    .await
                    .map_err(|_| SignalExportError::StoreUnavailable)?
                else {
                    // Another worker/sweep already proved it terminal.
                    break;
                };
                match context.staging.settle(&payload).await {
                    Ok(StagingDisposition::Confirmed | StagingDisposition::DeliberatelyAbsent) => {
                        break;
                    }
                    Ok(StagingDisposition::Pending) | Err(_) => {
                        let exhausted = context
                            .staging
                            .note_failed_attempt(&payload_ref.id)
                            .await
                            .map_err(|_| SignalExportError::StoreUnavailable)?;
                        if exhausted {
                            return Err(SignalExportError::StoreUnavailable);
                        }
                        tokio::time::sleep(Duration::from_millis(PUBLISH_BASE_DELAY_MS)).await;
                    }
                }
            }

            match last_outcome.unwrap_or(PersistOutcome::Stored) {
                PersistOutcome::Stored => {
                    Ok(signal.response((rejected_unstorable > 0).then_some((
                        rejected_unstorable,
                        SignalRejection::Unstorable,
                        true,
                    ))))
                }
                PersistOutcome::Dropped { records, reason } => {
                    let (records, reason) =
                        combine_rejections(rejected_unstorable, records, reason);
                    if signal.total_drop_is_error(reason) {
                        Err(SignalExportError::Gone)
                    } else {
                        Ok(signal.response(Some((records, reason, false))))
                    }
                }
                PersistOutcome::PartlyDropped { records, reason } => {
                    let (records, reason) =
                        combine_rejections(rejected_unstorable, records, reason);
                    Ok(signal.response(Some((records, reason, true))))
                }
            }
        }
        LifecycleStrategy::DurableQueue => {
            let mut last_error = None;
            for attempt in 1..=PUBLISH_MAX_ATTEMPTS {
                match signal.publish(&payload_ref).await {
                    Ok(()) => {
                        if attempt > 1 {
                            tracing::debug!(
                                signal = descriptor.name,
                                attempt,
                                "Signal publish succeeded after retry"
                            );
                        }
                        last_error = None;
                        break;
                    }
                    Err(error) => {
                        last_error = Some(error);
                        if attempt < PUBLISH_MAX_ATTEMPTS {
                            let delay = Duration::from_millis(
                                PUBLISH_BASE_DELAY_MS * 2_u64.pow(attempt - 1),
                            );
                            tracing::warn!(
                                signal = descriptor.name,
                                error = %last_error.as_deref().unwrap_or_default(),
                                attempt,
                                delay_ms = delay.as_millis(),
                                "Retrying signal publish after transient error"
                            );
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
            if let Some(error) = last_error {
                tracing::warn!(
                    signal = descriptor.name,
                    %error,
                    attempts = PUBLISH_MAX_ATTEMPTS,
                    "Failed to publish signal after retries"
                );
                return Err(SignalExportError::QueueFull);
            }

            Ok(signal.response((rejected_unstorable > 0).then_some((
                rejected_unstorable,
                SignalRejection::Unstorable,
                true,
            ))))
        }
    }
}

fn combine_rejections(
    unstorable: usize,
    records: usize,
    reason: SignalRejection,
) -> (usize, SignalRejection) {
    if unstorable == 0 {
        return (records, reason);
    }
    (
        unstorable.saturating_add(records),
        if reason == SignalRejection::Unstorable {
            SignalRejection::Unstorable
        } else {
            SignalRejection::Mixed
        },
    )
}

#[derive(Clone)]
pub struct TraceSignal {
    topic: Arc<StreamTopic<StagedPayloadRef>>,
    pipeline: Option<Arc<TracePipeline>>,
}

impl TraceSignal {
    pub fn new(
        topic: Arc<StreamTopic<StagedPayloadRef>>,
        pipeline: Option<Arc<TracePipeline>>,
    ) -> Self {
        Self { topic, pipeline }
    }
}

#[async_trait]
impl Signal for TraceSignal {
    type Request = ExportTraceServiceRequest;
    type Response = ExportTraceServiceResponse;

    fn descriptor(&self) -> SignalDescriptor {
        SignalDescriptor {
            name: "traces",
            queue_topic: TOPIC_TRACES,
            debug_file: "traces.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::TraceSpanCarrierPosition,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        }
    }

    fn lifecycle(&self) -> LifecycleStrategy {
        if self.pipeline.is_some() {
            LifecycleStrategy::PersistBeforeAck
        } else {
            LifecycleStrategy::DurableQueue
        }
    }

    fn inject_project_id(&self, request: &mut Self::Request, project_id: &str) {
        for resource_spans in &request.resource_spans {
            if let Some(resource) = &resource_spans.resource {
                for attribute in &resource.attributes {
                    if attribute.key == PROJECT_ID_ATTR
                        && let Some(value) = &attribute.value
                        && let Some(any_value::Value::StringValue(existing_id)) = &value.value
                        && existing_id != project_id
                    {
                        tracing::warn!(
                            path_project_id = project_id,
                            request_project_id = %existing_id,
                            "Project ID mismatch: request contains different project_id than transport"
                        );
                    }
                }
            }
        }
        inject_project_id_traces(request, project_id);
    }

    fn strip_unstorable(&self, request: &mut Self::Request) -> usize {
        strip_unstorable_spans(request)
    }

    fn record_count(&self, request: &Self::Request) -> usize {
        request
            .resource_spans
            .iter()
            .flat_map(|resource| &resource.scope_spans)
            .map(|scope| scope.spans.len())
            .sum()
    }

    fn staged_signal(&self) -> StagedSignal {
        StagedSignal::Traces
    }

    fn staged_records(
        &self,
        request: &Self::Request,
        _received_at: chrono::DateTime<chrono::Utc>,
    ) -> Vec<StagedRecord> {
        crate::traces::confirmation_records(request)
    }

    fn partition_key(&self, request: &Self::Request) -> String {
        TopicMessage::partition_key(request)
    }

    fn response(&self, rejected: Option<(usize, SignalRejection, bool)>) -> Self::Response {
        let partial_success = rejected.map(|(records, reason, partial)| ExportTracePartialSuccess {
            rejected_spans: records as i64,
            error_message: match (reason, partial) {
                (SignalRejection::Gone, true) => {
                    "some spans' project, trace or session is unknown or is being deleted"
                }
                (SignalRejection::Gone, false) => {
                    "the project, trace or session is unknown or is being deleted"
                }
                (SignalRejection::Unstorable, true) => {
                    "some spans were rejected as unstorable; check their timestamps are within 1900-2299"
                }
                (SignalRejection::Unstorable, false) => {
                    "spans were rejected as unstorable; check their timestamps are within 1900-2299"
                }
                (SignalRejection::Mixed, true) => {
                    "some spans were rejected for multiple reasons"
                }
                (SignalRejection::Mixed, false) => "spans were rejected for multiple reasons",
            }
            .to_string(),
        });
        ExportTraceServiceResponse { partial_success }
    }

    fn total_drop_is_error(&self, reason: SignalRejection) -> bool {
        reason == SignalRejection::Gone
    }

    async fn persist(
        &self,
        request: &Self::Request,
        _context: &SignalContext<'_>,
    ) -> Result<PersistOutcome, String> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| "trace persistence lifecycle has no pipeline".to_string())?;
        match pipeline.ingest_now(request).await {
            IngestOutcome::Stored => Ok(PersistOutcome::Stored),
            IngestOutcome::Dropped { spans, reason } => Ok(PersistOutcome::Dropped {
                records: spans,
                reason: map_trace_rejection(reason),
            }),
            IngestOutcome::PartlyDropped { spans, reason } => Ok(PersistOutcome::PartlyDropped {
                records: spans,
                reason: map_trace_rejection(reason),
            }),
            IngestOutcome::Failed => Err("trace pipeline failed".to_string()),
        }
    }

    async fn publish(&self, request: &StagedPayloadRef) -> Result<(), String> {
        self.topic
            .publish(request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn map_trace_rejection(reason: DropReason) -> SignalRejection {
    match reason {
        DropReason::Gone => SignalRejection::Gone,
        DropReason::Unstorable => SignalRejection::Unstorable,
    }
}

#[derive(Clone)]
pub struct MetricsSignal {
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
}

impl MetricsSignal {
    pub fn new(
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
    ) -> Self {
        Self {
            analytics,
            database,
        }
    }
}

#[async_trait]
impl Signal for MetricsSignal {
    type Request = ExportMetricsServiceRequest;
    type Response = ExportMetricsServiceResponse;

    fn descriptor(&self) -> SignalDescriptor {
        SignalDescriptor {
            name: "metrics",
            queue_topic: TOPIC_METRICS,
            debug_file: "metrics.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::MetricDatapointId,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        }
    }

    fn lifecycle(&self) -> LifecycleStrategy {
        LifecycleStrategy::PersistBeforeAck
    }

    fn inject_project_id(&self, request: &mut Self::Request, project_id: &str) {
        inject_project_id_metrics(request, project_id);
    }

    fn strip_unstorable(&self, request: &mut Self::Request) -> usize {
        strip_unstorable_metrics(request)
    }

    fn record_count(&self, request: &Self::Request) -> usize {
        request
            .resource_metrics
            .iter()
            .flat_map(|resource| &resource.scope_metrics)
            .flat_map(|scope| &scope.metrics)
            .map(|metric| match metric.data.as_ref() {
                Some(metric::Data::Gauge(value)) => value.data_points.len(),
                Some(metric::Data::Sum(value)) => value.data_points.len(),
                Some(metric::Data::Histogram(value)) => value.data_points.len(),
                Some(metric::Data::ExponentialHistogram(value)) => value.data_points.len(),
                Some(metric::Data::Summary(value)) => value.data_points.len(),
                None => 0,
            })
            .sum()
    }

    fn staged_signal(&self) -> StagedSignal {
        StagedSignal::Metrics
    }

    fn staged_records(
        &self,
        request: &Self::Request,
        _received_at: chrono::DateTime<chrono::Utc>,
    ) -> Vec<StagedRecord> {
        crate::metrics::extract_metrics_batch(request)
            .into_iter()
            .map(|metric| StagedRecord::Metric {
                datapoint_id: metric.datapoint_id,
                content_digest: metric.content_digest,
                timestamp: metric.timestamp,
            })
            .collect()
    }

    fn partition_key(&self, request: &Self::Request) -> String {
        TopicMessage::partition_key(request)
    }

    fn response(&self, rejected: Option<(usize, SignalRejection, bool)>) -> Self::Response {
        ExportMetricsServiceResponse {
            partial_success: rejected.map(
                |(records, reason, _partial)| ExportMetricsPartialSuccess {
                    rejected_data_points: records as i64,
                    error_message: match reason {
                        SignalRejection::Gone => {
                            "the project is unknown or is being deleted"
                        }
                        SignalRejection::Unstorable => {
                            "the data point timestamps are outside the storable range"
                        }
                        SignalRejection::Mixed => {
                            "some projects are unknown or being deleted, and some data point timestamps \
                             are outside the storable range"
                        }
                    }
                    .to_string(),
                },
            ),
        }
    }

    fn total_drop_is_error(&self, _reason: SignalRejection) -> bool {
        false
    }

    async fn persist(
        &self,
        request: &Self::Request,
        context: &SignalContext<'_>,
    ) -> Result<PersistOutcome, String> {
        let stored = crate::metrics::ingest_governed(
            request,
            self.analytics.as_ref(),
            self.database.as_ref(),
            context.storage_governance,
        )
        .await?;
        let rejected = stored.total.saturating_sub(stored.stored);
        if rejected == 0 {
            return Ok(PersistOutcome::Stored);
        }
        let reason = match (stored.gone > 0, stored.unstorable > 0) {
            (true, true) => SignalRejection::Mixed,
            (true, false) => SignalRejection::Gone,
            (false, true) => SignalRejection::Unstorable,
            (false, false) => SignalRejection::Mixed,
        };
        if stored.stored == 0 {
            Ok(PersistOutcome::Dropped {
                records: rejected,
                reason,
            })
        } else {
            Ok(PersistOutcome::PartlyDropped {
                records: rejected,
                reason,
            })
        }
    }

    async fn publish(&self, _request: &StagedPayloadRef) -> Result<(), String> {
        Err("metrics use persist-before-ack, not a queue".to_string())
    }
}

#[derive(Clone)]
pub struct LogSignal {
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
}

impl LogSignal {
    pub fn new(
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
    ) -> Self {
        Self {
            analytics,
            database,
        }
    }
}

#[async_trait]
impl Signal for LogSignal {
    type Request = ExportLogsServiceRequest;
    type Response = ExportLogsServiceResponse;

    fn descriptor(&self) -> SignalDescriptor {
        SignalDescriptor {
            name: "logs",
            queue_topic: TOPIC_LOGS,
            debug_file: "logs.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::LogDigestAndOrdinal,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        }
    }

    fn lifecycle(&self) -> LifecycleStrategy {
        LifecycleStrategy::PersistBeforeAck
    }

    fn inject_project_id(&self, request: &mut Self::Request, project_id: &str) {
        inject_project_id_logs(request, project_id);
    }

    fn strip_unstorable(&self, request: &mut Self::Request) -> usize {
        strip_unstorable_logs(request)
    }

    fn record_count(&self, request: &Self::Request) -> usize {
        request
            .resource_logs
            .iter()
            .flat_map(|resource| &resource.scope_logs)
            .map(|scope| scope.log_records.len())
            .sum()
    }

    fn staged_signal(&self) -> StagedSignal {
        StagedSignal::Logs
    }

    fn staged_records(
        &self,
        request: &Self::Request,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> Vec<StagedRecord> {
        crate::logs::extract_logs_batch(request, received_at)
            .into_iter()
            .map(|log| StagedRecord::Log {
                log_digest: log.log_digest,
                ordinal: log.ordinal,
                timestamp: log.timestamp,
                trace_id: log.trace_id,
                span_id: log.span_id,
            })
            .collect()
    }

    fn partition_key(&self, request: &Self::Request) -> String {
        TopicMessage::partition_key(request)
    }

    fn response(&self, rejected: Option<(usize, SignalRejection, bool)>) -> Self::Response {
        ExportLogsServiceResponse {
            partial_success: rejected.map(|(records, reason, _)| ExportLogsPartialSuccess {
                rejected_log_records: records as i64,
                error_message: match reason {
                    SignalRejection::Gone => "the project is unknown or is being deleted",
                    SignalRejection::Unstorable => {
                        "the log timestamps are outside the storable range"
                    }
                    SignalRejection::Mixed => {
                        "some projects are unknown or being deleted, and some log timestamps are \
                         outside the storable range"
                    }
                }
                .to_string(),
            }),
        }
    }

    fn total_drop_is_error(&self, _reason: SignalRejection) -> bool {
        false
    }

    async fn persist(
        &self,
        request: &Self::Request,
        context: &SignalContext<'_>,
    ) -> Result<PersistOutcome, String> {
        let stored = crate::logs::ingest_governed(
            request,
            self.analytics.as_ref(),
            self.database.as_ref(),
            context.clock.now(),
            context.storage_governance,
        )
        .await?;
        let rejected = stored.total.saturating_sub(stored.stored);
        if rejected == 0 {
            return Ok(PersistOutcome::Stored);
        }
        let reason = match (stored.gone > 0, stored.unstorable > 0) {
            (true, true) => SignalRejection::Mixed,
            (true, false) => SignalRejection::Gone,
            (false, true) => SignalRejection::Unstorable,
            (false, false) => SignalRejection::Mixed,
        };
        if stored.stored == 0 {
            Ok(PersistOutcome::Dropped {
                records: rejected,
                reason,
            })
        } else {
            Ok(PersistOutcome::PartlyDropped {
                records: rejected,
                reason,
            })
        }
    }

    async fn publish(&self, _request: &StagedPayloadRef) -> Result<(), String> {
        Err("logs use persist-before-ack, not a queue".to_string())
    }
}

fn storable_nanos(value: u64) -> bool {
    value == 0
        || sideseat_core::utils::time::is_storable(sideseat_core::utils::time::nanos_to_datetime(
            value,
        ))
}

fn strip_unstorable_metrics(request: &mut ExportMetricsServiceRequest) -> usize {
    let mut removed = 0usize;
    for resource in &mut request.resource_metrics {
        for scope in &mut resource.scope_metrics {
            for metric in &mut scope.metrics {
                let keep = |time: u64, start: u64| storable_nanos(time) && storable_nanos(start);
                match metric.data.as_mut() {
                    Some(metric::Data::Gauge(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Sum(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Histogram(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::ExponentialHistogram(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    Some(metric::Data::Summary(data)) => {
                        let before = data.data_points.len();
                        data.data_points
                            .retain(|point| keep(point.time_unix_nano, point.start_time_unix_nano));
                        removed += before - data.data_points.len();
                    }
                    None => {}
                }
            }
            scope.metrics.retain(|metric| match metric.data.as_ref() {
                Some(metric::Data::Gauge(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Sum(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Histogram(data)) => !data.data_points.is_empty(),
                Some(metric::Data::ExponentialHistogram(data)) => !data.data_points.is_empty(),
                Some(metric::Data::Summary(data)) => !data.data_points.is_empty(),
                None => false,
            });
        }
    }
    removed
}

fn strip_unstorable_logs(request: &mut ExportLogsServiceRequest) -> usize {
    let mut removed = 0usize;
    for resource in &mut request.resource_logs {
        for scope in &mut resource.scope_logs {
            let before = scope.log_records.len();
            scope.log_records.retain(|record| {
                storable_nanos(record.time_unix_nano)
                    && storable_nanos(record.observed_time_unix_nano)
            });
            removed += before - scope.log_records.len();
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_declare_the_full_signal_contract() {
        fn assert_signal<T: Signal>() {}
        assert_signal::<TraceSignal>();

        let descriptor = SignalDescriptor {
            name: "traces",
            queue_topic: TOPIC_TRACES,
            debug_file: "traces.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::TraceSpanCarrierPosition,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        };
        assert_eq!(descriptor.queue_topic, TOPIC_TRACES);
        assert!(descriptor.confirmation.strict_content_digest);
        assert!(descriptor.confirmation.excludes_system_metadata);
    }

    #[test]
    fn metrics_declare_the_full_signal_contract() {
        fn assert_signal<T: Signal>() {}
        assert_signal::<MetricsSignal>();
        let descriptor = SignalDescriptor {
            name: "metrics",
            queue_topic: TOPIC_METRICS,
            debug_file: "metrics.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::MetricDatapointId,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        };
        assert_eq!(descriptor.queue_topic, TOPIC_METRICS);
        assert!(descriptor.confirmation.strict_content_digest);
    }

    #[test]
    fn logs_declare_the_full_signal_contract() {
        fn assert_signal<T: Signal>() {}
        assert_signal::<LogSignal>();
        let descriptor = SignalDescriptor {
            name: "logs",
            queue_topic: TOPIC_LOGS,
            debug_file: "logs.jsonl",
            durability: DurabilityRequirement::DurableBeforeAck,
            confirmation: ConfirmationPredicate {
                identity: SignalIdentity::LogDigestAndOrdinal,
                strict_content_digest: true,
                excludes_system_metadata: true,
            },
        };
        assert_eq!(descriptor.queue_topic, TOPIC_LOGS);
        assert!(descriptor.confirmation.strict_content_digest);
    }
}
