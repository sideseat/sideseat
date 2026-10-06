//! Trace ingestion pipeline.
//!
//! Orchestrates the 5-stage trace processing pipeline:
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────────────┐
//! │                          TRACE PROCESSING PIPELINE                               │
//! ├──────────────────────────────────────────────────────────────────────────────────┤
//! │                                                                                  │
//! │  ┌──────────┐   ┌──────────┐   ┌─────────┐   ┌────────┐   ┌──────────┐           │
//! │  │1a.ATTRS  │──▶│1b.MSGS   │──▶│2. SIDEML│──▶│3.ENRICH│──▶│4. PERSIST│           │
//! │  │          │   │          │   │         │   │        │   │          │           │
//! │  │ Protobuf │   │ Events   │   │ Raw →   │   │ Costs  │   │ Raw JSON │           │
//! │  │ GenAI    │   │ Attrs    │   │ SideML  │   │Previews│   │ SSE pub  │           │
//! │  │ Classify │   │ Extract  │   │ msgs    │   │        │   │ Storage  │           │
//! │  └──────────┘   └──────────┘   └─────────┘   └────────┘   └──────────┘           │
//! │                                                                                  │
//! └──────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Stage Details
//!
//! | Stage       | Input                                        | Output                                              | Module         |
//! |-------------|----------------------------------------------|-----------------------------------------------------|----------------|
//! | 1a. Attrs   | `ExportTraceServiceRequest`                  | `Vec<SpanData>`                                     | `extract/`     |
//! | 1b. Msgs    | `ExportTraceServiceRequest`, `&[SpanData]`   | `(Vec<Vec<RawMessage>>, Vec<Vec<RawToolDefinition>>, Vec<Vec<RawToolNames>>)` | `extract/`     |
//! | 2. SideML   | `&[Vec<RawMessage>]`                         | `Vec<Vec<SideMLMessage>>`                           | `sideml`       |
//! | 3. Enrich   | `&[SpanData]`, `&[Vec<SideMLMessage>]`       | `Vec<SpanEnrichment>`                               | `enrich.rs`    |
//! | 4. Persist  | `&Request`, `SpanData`, `RawMessage`, ...    | `()`                                                | `persist.rs`   |

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::enrich::enrich_batch;
use super::extract::files::FileExtractionCache;
use super::extract::{ExtractionMode, extract_attributes_batch, extract_messages_batch};
use super::persist::{
    BatchInput, IncomingReference, PendingFileWrite, SseSpanEvent, note_unstored_files,
    persist_extracted_files, prepare_batch, publish_sse_events, reconcile_incoming_references,
    write_to_duckdb,
};
use crate::received::ReceivedPayload;
use crate::staging::{StagedPayloadRef, StagingDisposition, StagingService};
use raw::RawDraft;
pub use raw_lifecycle::Reconciled;
use raw_lifecycle::WrittenRecord;
use sideseat_core::constants::{DEFAULT_PROJECT_ID, PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES};
use sideseat_core::utils::time::is_storable;
use sideseat_domain::content_bodies::ContentBodyService;
use sideseat_domain::files::FileService;
use sideseat_domain::pricing::PricingService;
use sideseat_domain::sideml::to_sideml_batch;
use sideseat_domain::storage_governance::StorageGovernanceService;
use sideseat_messaging::{StreamAcker, StreamClaimer, StreamTopic, TopicService};
use sideseat_ports::queue::TopicError;
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::{NormalizedSpan, ProjectId, StagedPayload, StagedSignal};

/// Consumer group name for trace pipeline
const CONSUMER_GROUP: &str = "trace_pipeline";

/// Interval for claiming stuck messages (seconds)
const CLAIM_INTERVAL_SECS: u64 = 30;

/// Minimum idle time before claiming a message (milliseconds)
const CLAIM_MIN_IDLE_MS: u64 = 60_000;

/// Maximum number of messages to claim at once
const CLAIM_MAX_COUNT: usize = 100;

/// Maximum number of requests to batch before processing
const PIPELINE_BATCH_MAX_SIZE: usize = 1024;

/// Bound on delivered-but-not-yet-selected requests held by the fair scheduler.
///
/// Looking ahead farther than one write batch is what lets a quiet partition get selected ahead of a hot
/// partition's backlog. Four batches bounds both memory and the number of unacknowledged deliveries held by one
/// consumer while still giving the scheduler enough choice to make the policy meaningful.
const PIPELINE_PREFETCH_MAX_SIZE: usize = PIPELINE_BATCH_MAX_SIZE * 4;

/// Timeout for collecting additional messages into a batch (microseconds)
const PIPELINE_BATCH_DRAIN_TIMEOUT_US: u64 = 5_000;

/// How long to wait before retrying a failed subscription.
///
/// The queue keeps accepting publishes while nothing consumes it, so the cost of a slow retry is a growing
/// backlog rather than a lost export - and the backlog is bounded by `stream_publish`'s refusal threshold.
const SUBSCRIBE_RETRY_DELAY: Duration = Duration::from_secs(5);

struct PartitionLane<T> {
    weight: usize,
    items: VecDeque<T>,
}

/// Bounded weighted round-robin over queue partitions.
///
/// FIFO is preserved inside each partition. Across partitions, one round takes `weight` items from each active
/// lane before returning to the first, so a continuously hot partition cannot occupy every selected batch while
/// another assigned partition has work waiting.
struct WeightedFairQueue<T> {
    lanes: BTreeMap<u32, PartitionLane<T>>,
    rotation: VecDeque<u32>,
    len: usize,
}

impl<T> WeightedFairQueue<T> {
    fn new() -> Self {
        Self {
            lanes: BTreeMap::new(),
            rotation: VecDeque::new(),
            len: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn len(&self) -> usize {
        self.len
    }

    fn push(&mut self, partition: u32, item: T) {
        self.push_with_weight(partition, 1, item);
    }

    fn push_with_weight(&mut self, partition: u32, weight: usize, item: T) {
        let weight = weight.max(1);
        match self.lanes.entry(partition) {
            std::collections::btree_map::Entry::Occupied(mut lane) => {
                lane.get_mut().weight = weight;
                lane.get_mut().items.push_back(item);
            }
            std::collections::btree_map::Entry::Vacant(lane) => {
                self.rotation.push_back(partition);
                lane.insert(PartitionLane {
                    weight,
                    items: VecDeque::from([item]),
                });
            }
        }
        self.len += 1;
    }

    fn pop_batch(&mut self, limit: usize) -> Vec<T> {
        let mut selected = Vec::with_capacity(limit.min(self.len));
        while selected.len() < limit {
            let Some(partition) = self.rotation.pop_front() else {
                break;
            };
            let mut remove_lane = false;
            if let Some(lane) = self.lanes.get_mut(&partition) {
                for _ in 0..lane.weight {
                    let Some(item) = lane.items.pop_front() else {
                        break;
                    };
                    selected.push(item);
                    self.len -= 1;
                    if selected.len() == limit {
                        break;
                    }
                }
                remove_lane = lane.items.is_empty();
            }
            if remove_lane {
                self.lanes.remove(&partition);
            } else {
                self.rotation.push_back(partition);
            }
        }
        selected
    }
}

#[cfg(test)]
mod weighted_fair_queue_tests {
    use super::WeightedFairQueue;

    #[test]
    fn a_quiet_partition_enters_the_batch_ahead_of_a_hot_backlog() {
        let mut queue = WeightedFairQueue::new();
        for item in 0..4096 {
            queue.push(0, item);
        }
        queue.push(1, 10_000);

        assert_eq!(queue.pop_batch(4), vec![0, 10_000, 1, 2]);
    }

    #[test]
    fn weights_set_each_partitions_quantum_without_breaking_local_fifo() {
        let mut queue = WeightedFairQueue::new();
        for item in ["a0", "a1", "a2", "a3"] {
            queue.push_with_weight(0, 2, item);
        }
        for item in ["b0", "b1"] {
            queue.push_with_weight(1, 1, item);
        }

        assert_eq!(queue.pop_batch(6), vec!["a0", "a1", "b0", "a2", "a3", "b1"]);
    }
}

// ============================================================================
// PIPELINE PROCESSOR
// ============================================================================

/// Trace processing pipeline orchestrator.
///
/// Receives OTLP traces from a topic and processes them through:
/// 1a. Extract Attributes (parse protobuf, extract GenAI attributes, classify)
/// 1b. Extract Messages (extract raw messages from events and attributes)
/// 2. SideML (raw messages to SideML format)
/// 3. Enrich (costs, previews)
/// 4. Persist (SSE publish, DuckDB write, file extraction)
/// What became of one request's spans.
///
/// `Dropped` exists because "stored" and "discarded because the project is going away" were both a `true`,
/// and a caller told success for records that were dropped has no way to learn otherwise. The queue may
/// acknowledge either - both are final - but a synchronous caller must be able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    Stored,
    /// Nothing was stored, and `reason` says why - which decides the *answer*, not just the log line.
    Dropped {
        spans: usize,
        reason: DropReason,
    },
    /// Some spans were stored and some dropped - a request naming a live project and a dying one, or one
    /// mixing storable and unstorable spans. Distinct from `Dropped` because the answers differ: nothing
    /// stored is a 404, something stored is a success that reports what it rejected.
    ///
    /// `reason` describes the drops, because the message shown to the exporter has to be true: a batch of one
    /// valid span and one year-2300 span stored the valid one and then blamed project deletion.
    PartlyDropped {
        spans: usize,
        reason: DropReason,
    },
    Failed,
}

/// Why a request's spans were discarded, because the honest HTTP answer differs per cause.
///
/// Reporting every drop as "unknown project" was wrong in a way that costs the exporter: a live project
/// exporting a span with an unstorable timestamp got a 404, which says *retry against a different project* -
/// so it retried the same doomed payload indefinitely while its operator hunted a project that was in fact
/// fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The target is gone: the project does not exist or is being deleted, or the trace or session was
    /// deleted. A retry against the same target cannot help, and 404 is the truth.
    Gone,
    /// The payload itself cannot be stored - an out-of-range timestamp, say. A retry cannot help either, but
    /// the project is fine, so the answer is a success that *reports the rejection* through the field OTLP
    /// provides for it rather than blaming the project.
    Unstorable,
}

impl IngestOutcome {
    /// Whether the queue may acknowledge this message: nothing more will be done with it either way.
    fn is_final(self) -> bool {
        !matches!(self, Self::Failed)
    }
}

pub struct TracePipeline {
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    pricing: Arc<PricingService>,
    topics: Arc<TopicService>,
    file_service: Arc<FileService>,
    staging: Arc<StagingService>,
    storage_governance: Option<Arc<StorageGovernanceService>>,
    content_bodies: ContentBodyService,
    /// Cross-batch cache for base64 extraction.
    /// Avoids redundant decode + BLAKE3 for repeated images across spans/batches.
    file_cache: FileExtractionCache,
}

mod batch;
mod consumer;
mod fences_early;
mod fences_late;
mod raw;
mod raw_lifecycle;
mod single;

// ============================================================================
// PER-REQUEST PROCESSING (free function for thread safety)
// ============================================================================

/// Test-only wrapper over `process_request`, which is private to this module.
///
/// Used by `message_goldens_tests` to replay captured OTLP payloads through the real
/// pipeline. File extraction is off: it performs disk writes and no message property under
/// test depends on it.
/// As [`process_request_for_test`], with the extraction mode chosen by the caller.
///
/// The metamorphic test runs a fixture through both modes and compares: the answer under `PerCarrier`
/// must contain the answer under `FirstMatch`, in the same relative order, plus whatever the carriers
/// nobody read were holding.
#[cfg(any(test, feature = "test-support"))]
pub fn process_request_for_test_with_mode(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    mode: ExtractionMode,
) -> Option<Vec<NormalizedSpan>> {
    process_request_for_test_with_files(request, pricing, mode, false)
}

/// As [`process_request_for_test_with_mode`], with file extraction on or off.
///
/// Extraction changes what a stored string holds - base64 or a `#!B64!#` reference - so a test comparing a
/// stored field with something rendered later has to be able to ask for both.
#[cfg(any(test, feature = "test-support"))]
pub fn process_request_for_test_with_files(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    mode: ExtractionMode,
    files_enabled: bool,
) -> Option<Vec<NormalizedSpan>> {
    process_request(
        request,
        pricing,
        files_enabled,
        &FileExtractionCache::new(),
        mode,
    )
    .map(|(spans, _, _)| spans)
}

/// What became of one request in the CPU phase.
///
/// Distinguishes a normal empty request from a panic: only the panic makes the batch unsafe to acknowledge.
enum Prepared {
    /// Spans ready to persist, with the files they reference and any references that arrived formed.
    Ready(
        Vec<NormalizedSpan>,
        Vec<PendingFileWrite>,
        Vec<IncomingReference>,
    ),
    /// The request held no spans. Not an error.
    Nothing,
    /// The request panicked. Its spans, if any, are unknown and must not be acknowledged.
    Panicked,
}

/// Group requests into consecutive waves whose summed decoded size stays under the in-flight budget.
///
/// The byte bound caps concurrent expansion independently of host CPU count. The count bound keeps a wave
/// within the available parallelism.
///
/// Order is preserved: waves are consecutive slices of `requests`, so the results concatenate in request
/// order, which the cardinality check downstream relies on.
///
/// A request larger than the whole budget forms a wave of one because edge admission already accepted it.
fn byte_bounded_waves(
    requests: &[ExportTraceServiceRequest],
    max_per_wave: usize,
) -> Vec<&[ExportTraceServiceRequest]> {
    let max_per_wave = max_per_wave.max(1);
    let mut waves = Vec::new();
    let mut wave_start = 0usize;
    let mut wave_bytes = 0u64;

    for (index, request) in requests.iter().enumerate() {
        let size = request.encoded_len() as u64;
        let would_be = wave_bytes.saturating_add(size);
        let full_by_count = index - wave_start >= max_per_wave;
        // An oversized request starts a one-item wave; an empty wave is never emitted.
        let full_by_bytes = index > wave_start && would_be > PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES;

        if full_by_count || full_by_bytes {
            waves.push(&requests[wave_start..index]);
            wave_start = index;
            wave_bytes = 0;
        }
        wave_bytes = wave_bytes.saturating_add(size);
    }

    if wave_start < requests.len() {
        waves.push(&requests[wave_start..]);
    }
    waves
}

/// Process a single OTLP request through stages 1-4.
///
/// Pure CPU work: extract attributes, messages, sideml, enrich, prepare.
/// Returns NormalizedSpans + pending file writes, or None if no spans.
fn process_request(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    files_enabled: bool,
    file_cache: &FileExtractionCache,
    mode: crate::traces::extract::ExtractionMode,
) -> Option<(
    Vec<NormalizedSpan>,
    Vec<PendingFileWrite>,
    Vec<IncomingReference>,
)> {
    // Stage 1a: Extract Attributes
    let spans = extract_attributes_batch(request);
    if spans.is_empty() {
        return None;
    }

    // Stage 1b: Extract Messages, Tool Definitions, and Tool Names
    let (raw_messages, tool_definitions, tool_names) =
        extract_messages_batch(request, &spans, mode);

    // Stage 2: SideML Conversion
    let messages = to_sideml_batch(&raw_messages);

    // Stage 3: Enrich
    let enrichments = enrich_batch(&spans, &messages, pricing);

    // Stage 4: Prepare (CPU-only file extraction + flatten to NormalizedSpan)
    let (db_spans, pending_files, incoming_references) = prepare_batch(
        request,
        BatchInput {
            spans,
            messages: raw_messages,
            tool_definitions,
            tool_names,
            enrichments,
        },
        files_enabled,
        Some(file_cache),
    );

    Some((db_spans, pending_files, incoming_references))
}

/// Strip spans whose timestamps no analytics backend can store, **before** the request is queued.
///
/// Returns how many were removed, so the route can report them.
///
/// Deterministic checks belong at the edge. A durable queue answers 200 the moment the payload is safely
/// published, which is the right promise - but the consumer later discards an unstorable span, so a live
/// project exporting a year-2300 timestamp got an unqualified success and then found nothing stored, with no
/// `partial_success` to explain it. Nothing downstream can report back to a request that has already
/// returned. Storability is a property of the payload alone - not of the project's state, the queue's, or
/// anything that can change between here and the write - so it can be settled now and reported now, and only
/// what can actually be stored is queued.
///
/// The consumer keeps its own check: this path is skipped when the queue is not durable, the pipeline also
/// runs on redelivery and at shutdown, and a check that exists in one place only is one refactor from being
/// absent.
pub fn strip_unstorable_spans(request: &mut ExportTraceServiceRequest) -> usize {
    let mut removed = 0usize;
    for resource in &mut request.resource_spans {
        for scope in &mut resource.scope_spans {
            let before = scope.spans.len();
            scope.spans.retain(|span| {
                let start =
                    sideseat_core::utils::time::nanos_to_datetime(span.start_time_unix_nano);
                let end = (span.end_time_unix_nano > 0).then(|| {
                    sideseat_core::utils::time::nanos_to_datetime(span.end_time_unix_nano)
                });
                is_storable(start) && end.is_none_or(is_storable)
            });
            removed += before - scope.spans.len();
        }
    }
    if removed > 0 {
        tracing::warn!(
            removed,
            "Rejected spans with timestamps outside the storable range before queueing them"
        );
    }
    removed
}

/// SSE events for a batch, each stamped with its trace's **canonical** session rather than the span's own.
///
/// A subscriber filtered by session compares `event.session_id`, and a span carries a session only if it is
/// the span that knew one - usually the root alone. So every child span produced an event a session
/// subscription discarded, and a span naming a *different* session sent an event to a page that session's
/// trace does not appear on while the page it does appear on received nothing.
///
/// The batch's answer is the starting point; `stamp_stored_sessions` supersedes it from the store once the
/// rows are written, which is what makes the live stream agree with what a subsequent read returns.
fn sse_events_for(spans: &[NormalizedSpan]) -> Vec<SseSpanEvent> {
    let sessions = canonical_session_of_traces(spans);
    spans
        .iter()
        .map(|span| {
            let mut event = SseSpanEvent::from(span);
            let key = (
                span.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                span.trace_id.clone(),
            );
            // Only when this batch knows the trace's session; otherwise the span's own value stands, which
            // is what a batch of orphan children can say.
            if let Some(session) = sessions.get(&key) {
                event.session_id = Some(session.clone());
            }
            event
        })
        .collect()
}

/// Which `(project, trace)` pairs in this batch belong to one of `sessions`.
///
/// Session membership is a property of the **trace**, not of each span, and this is the whole reason the
/// function exists separately: a framework records the session id on the span that knows it - usually the
/// root alone - so filtering spans by their own session id kept every child of a deleted session's trace.
/// What reached the store was a headless trace, readable through the trace and span views, for a session the
/// caller had been told was gone.
///
/// A batch carrying only children of an unseen root names no session at all, so nothing here can attribute
/// it. That case is covered by the trace tombstones (for traces the deletion resolved) and by the deletion
/// sweep (for anything that arrives later) - the two other steps of the four-step protocol.
fn traces_of_sessions(
    spans: &[NormalizedSpan],
    sessions: &HashSet<(String, String)>,
) -> HashSet<(String, String)> {
    canonical_session_of_traces(spans)
        .into_iter()
        .filter(|((project, _), session)| sessions.contains(&(project.clone(), session.clone())))
        .map(|(trace, _)| trace)
        .collect()
}

/// The session each `(project, trace)` in this batch belongs to: the one on its **earliest** span.
///
/// The same rule the reads use - `arg_min(session_id, (timestamp_start, span_id))` - so ingestion and
/// retrieval cannot disagree about which session a trace is in. Taking whichever span happened to come first
/// in the batch made that a function of iteration order; taking *any* span that named a deleted session was
/// worse still, and it is what let a redelivery of a trace canonically in session A be tombstoned and
/// dropped in full because one of its child spans named a deleted session B.
///
/// Bounded by what a batch can see: if an earlier span of the trace arrives in a *later* batch, this answers
/// from the spans in hand. That is inherent - the fence cannot consult a span nobody has sent - and the
/// remaining cases are what the trace tombstones and the deletion sweep exist for. The store-side resolution
/// is authoritative once the rows are written.
fn canonical_session_of_traces(spans: &[NormalizedSpan]) -> HashMap<(String, String), String> {
    /// `(project, trace)`.
    type TraceKey = (String, String);
    /// `(project, trace, span)` - one delivery survives per span, as storage keeps one row per span.
    type SpanKey = (String, String, String);
    /// `(timestamp_start, span_id)` - the same total order the reads use.
    type EarliestBy = (chrono::DateTime<chrono::Utc>, String);

    // Two passes, mirroring what the store actually does.
    //
    // First the latest delivery of each span wins, exactly as `DEDUP_SPANS` keeps one row per span ordered
    // by `ingested_at DESC, rowid DESC` - within a batch that is the last occurrence, because insertion
    // order is the order iterated here. This is the pass that was missing: a span re-delivered *without* a
    // session still counted with its earlier session, so a trace whose session had been removed was fenced
    // against the deleted one.
    //
    // Then the session on the earliest surviving span that has one, which is `arg_min(session_id, …)` after
    // `WHERE session_id IS NOT NULL`. A sessionless span is not a candidate - folding it into the first pass
    // as a competing value made the *earliest span* depend on spans that say nothing about membership.
    let mut latest: HashMap<SpanKey, (usize, Option<String>, chrono::DateTime<chrono::Utc>)> =
        HashMap::new();
    for (position, span) in spans.iter().enumerate() {
        let project = span
            .project_id
            .as_deref()
            .unwrap_or(DEFAULT_PROJECT_ID)
            .to_string();
        let session = span
            .session_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        latest.insert(
            (project, span.trace_id.clone(), span.span_id.clone()),
            (position, session, span.timestamp_start),
        );
    }

    let mut best: HashMap<TraceKey, (EarliestBy, String)> = HashMap::new();
    for ((project, trace, span_id), (_, session, timestamp_start)) in latest {
        let Some(session_id) = session else {
            continue;
        };
        let key = (timestamp_start, span_id);
        best.entry((project, trace))
            .and_modify(|current| {
                if key < current.0 {
                    *current = (key.clone(), session_id.clone());
                }
            })
            .or_insert((key, session_id));
    }

    best.into_iter()
        .map(|(trace, (_, session))| (trace, session))
        .collect()
}

/// Drop spans whose timestamps no analytics backend can store, returning how many went.
///
/// The same rule the metrics path applies, for the same reason and with the same consequence if it is left
/// out: ClickHouse's row conversion reached its `DateTime64(6)` column through `timestamp_nanos_opt`, whose
/// range is narrower than the column's and whose `None` fell back to the **epoch** - where the schema's
/// 90-day TTL deletes the row, after the export was answered 200. DuckDB stored the same span at its stated
/// time, so the two backends disagreed about one export as well.
///
/// Dropped rather than relocated, and counted so the caller reports it: an exporter with a broken clock can
/// act on a rejection, and cannot act on a record silently moved to 1970.
fn drop_unstorable_spans(spans: &mut Vec<NormalizedSpan>) -> usize {
    let before = spans.len();
    spans.retain(|s| {
        let ok = is_storable(s.timestamp_start) && s.timestamp_end.is_none_or(is_storable);
        if !ok {
            tracing::warn!(
                trace_id = %s.trace_id,
                span_id = %s.span_id,
                timestamp_start = %s.timestamp_start,
                timestamp_end = ?s.timestamp_end,
                "Rejecting a span whose timestamp is outside the range every analytics backend can store"
            );
        }
        ok
    });
    before - spans.len()
}

#[cfg(test)]
mod association_leak_tests;
#[cfg(test)]
mod fan_out_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod raw_lifecycle_tests;
#[cfg(test)]
mod storable_timestamp_tests;
#[cfg(test)]
mod strip_unstorable_tests;
