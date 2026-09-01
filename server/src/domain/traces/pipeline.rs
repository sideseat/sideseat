//! Trace Processing Pipeline
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
//! │  │ Classify │   │ Extract  │   │ msgs    │   │        │   │ DuckDB   │           │
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

use std::collections::{HashMap, HashSet};
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
use crate::core::TopicService;
use crate::core::constants::DEFAULT_PROJECT_ID;
use crate::data::AnalyticsService;
use crate::data::files::FileService;
use crate::data::topics::{StreamTopic, TopicError};
use crate::data::types::NormalizedSpan;
use crate::domain::pricing::PricingService;
use crate::domain::sideml::to_sideml_batch;
use crate::utils::time::is_storable;

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

/// Timeout for collecting additional messages into a batch (microseconds)
const PIPELINE_BATCH_DRAIN_TIMEOUT_US: u64 = 5_000;

/// How long to wait before retrying a failed subscription.
///
/// The queue keeps accepting publishes while nothing consumes it, so the cost of a slow retry is a growing
/// backlog rather than a lost export - and the backlog is bounded by `stream_publish`'s refusal threshold.
const SUBSCRIBE_RETRY_DELAY: Duration = Duration::from_secs(5);

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
    /// Every span belonged to a project that will not accept writes, so nothing was stored.
    Dropped {
        spans: usize,
    },
    /// Some spans were stored and some dropped - a request naming a live project and a dying one. Distinct
    /// from `Dropped` because the answers differ: nothing stored is a 404, something stored is a success
    /// that reports what it rejected.
    PartlyDropped {
        spans: usize,
    },
    Failed,
}

impl IngestOutcome {
    /// Whether the queue may acknowledge this message: nothing more will be done with it either way.
    fn is_final(self) -> bool {
        !matches!(self, Self::Failed)
    }
}

pub struct TracePipeline {
    analytics: Arc<AnalyticsService>,
    pricing: Arc<PricingService>,
    topics: Arc<TopicService>,
    file_service: Arc<FileService>,
    /// Cross-batch cache for base64 extraction.
    /// Avoids redundant decode + BLAKE3 for repeated images across spans/batches.
    file_cache: FileExtractionCache,
}

impl TracePipeline {
    pub fn new(
        analytics: Arc<AnalyticsService>,
        pricing: Arc<PricingService>,
        topics: Arc<TopicService>,
        file_service: Arc<FileService>,
    ) -> Self {
        Self {
            analytics,
            pricing,
            topics,
            file_service,
            file_cache: FileExtractionCache::new(),
        }
    }

    /// Start the pipeline processor, consuming from the given stream topic.
    ///
    /// Uses consumer groups for at-least-once delivery:
    /// - Messages are acknowledged after successful processing
    /// - Unacknowledged messages are re-delivered on restart
    /// - Stuck messages are claimed after CLAIM_MIN_IDLE_MS
    pub fn start(
        self,
        topic: StreamTopic<ExportTraceServiceRequest>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        // Generate unique consumer name: {uuid}:{pid}
        let consumer = format!("{}:{}", Uuid::new_v4(), std::process::id());

        tokio::spawn(async move {
            // Subscribe, retrying until it succeeds or the server is shutting down.
            //
            // A single failure used to end the task while the instance stayed healthy and kept accepting
            // writes - so every export was queued and *nothing* consumed it, for the life of the process,
            // with no error after the first line. Redis being briefly unreachable at startup is the
            // ordinary way to reach that, and it is exactly when a retry is obviously right.
            let mut subscriber = loop {
                match topic.subscribe(CONSUMER_GROUP, &consumer).await {
                    Ok(s) => break s,
                    Err(e) => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!(
                                "TracePipeline abandoning subscription during shutdown"
                            );
                            return;
                        }
                        tracing::error!(
                            error = %e,
                            "Failed to subscribe to the trace topic; retrying. Until this succeeds \
                             nothing is consuming the queue."
                        );
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.changed() => {
                                if *shutdown_rx.borrow() {
                                    return;
                                }
                            }
                            _ = tokio::time::sleep(SUBSCRIBE_RETRY_DELAY) => {}
                        }
                    }
                }
            };

            // Get acker and claimer for message operations (Send + Sync)
            let acker = subscriber.acker();
            let claimer = subscriber.claimer();

            tracing::debug!(
                consumer = %consumer,
                group = CONSUMER_GROUP,
                "TracePipeline started"
            );

            // Create interval for periodic claim recovery
            let mut claim_interval =
                tokio::time::interval(Duration::from_secs(CLAIM_INTERVAL_SECS));
            // Don't count the initial tick
            claim_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            let mut shutdown_requested = false;

            loop {
                if shutdown_requested {
                    // Drain remaining messages with timeout
                    match tokio::time::timeout(Duration::from_millis(100), subscriber.recv()).await
                    {
                        Ok(Ok((msg_id, msg))) => {
                            if self.run(&msg).await.is_final() {
                                if let Err(e) = acker.ack(&msg_id).await {
                                    tracing::warn!(error = %e, msg_id = %msg_id, "Failed to ack during drain");
                                }
                            } else {
                                tracing::warn!(msg_id = %msg_id, "Skipping ack during drain: write failed");
                            }
                            continue;
                        }
                        Ok(Err(TopicError::Lagged(n))) => {
                            tracing::warn!(lagged = n, "TracePipeline lagged during drain");
                            continue;
                        }
                        _ => break,
                    }
                }

                // Phase 1: Wait for at least one message (with shutdown/claim handling)
                //
                // Arm order matters. `biased` polls in order and takes the first ready branch, so the
                // *maintenance tick* must come **before** the receive branch: under sustained saturation
                // `subscriber.recv()` is always ready, and putting it first meant claiming and trimming
                // never fired. That is the case they matter most - a full stream is exactly when trim
                // has work to do - so an ordering that starves them there defeats their purpose. The
                // tick still yields to shutdown, which is what `biased` earns its keep for.
                let first = tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("TracePipeline received shutdown, draining...");
                            shutdown_requested = true;
                        }
                        continue;
                    }
                    _ = claim_interval.tick() => {
                        // Periodically claim stuck messages from other consumers
                        self.claim_stuck_messages(&claimer, &acker, &consumer).await;
                        // And discard what every group is finished with. This is the only thing that
                        // bounds the stream: publishing no longer trims by length, because a length
                        // bound deletes the oldest entries whether or not anyone read them - and each
                        // had already been answered 200.
                        match claimer.trim_consumed().await {
                            Ok(0) => {}
                            Ok(trimmed) => tracing::debug!(trimmed, "Trimmed consumed stream entries"),
                            Err(e) => tracing::warn!(error = %e, "Failed to trim consumed stream entries"),
                        }
                        continue;
                    }
                    result = subscriber.recv() => {
                        match result {
                            Ok(pair) => pair,
                            Err(TopicError::Lagged(n)) => {
                                tracing::warn!(lagged = n, "TracePipeline lagged");
                                continue;
                            }
                            Err(TopicError::ChannelClosed) => break,
                            // A payload nothing can parse is acknowledged and skipped, not fatal.
                            //
                            // This used to break out of the loop, so one malformed message stopped all
                            // trace ingestion for the life of the process - and because it was never
                            // acknowledged, a restart was met by the same message. Nothing can store a
                            // payload it cannot decode, so the choice is between discarding one message
                            // loudly and blocking every message behind it silently.
                            Err(TopicError::Undecodable { id, detail }) => {
                                tracing::error!(
                                    message_id = %id,
                                    error = %detail,
                                    "Discarding a queued payload that cannot be decoded"
                                );
                                if let Err(e) = acker.ack(&id).await {
                                    tracing::warn!(message_id = %id, error = %e, "Could not acknowledge an undecodable payload");
                                }
                                continue;
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "TracePipeline receive error");
                                break;
                            }
                        }
                    }
                };

                // Phase 2: Drain additional queued messages into batch
                let mut batch = vec![first];
                while batch.len() < PIPELINE_BATCH_MAX_SIZE {
                    match tokio::time::timeout(
                        Duration::from_micros(PIPELINE_BATCH_DRAIN_TIMEOUT_US),
                        subscriber.recv(),
                    )
                    .await
                    {
                        Ok(Ok(pair)) => batch.push(pair),
                        _ => break,
                    }
                }

                let batch_size = batch.len();
                if batch_size > 1 {
                    tracing::debug!(batch_size, "Processing batched requests");
                }

                // Phase 3: Process entire batch (one DuckDB write)
                let msg_ids: Vec<String> = batch.iter().map(|(id, _)| id.clone()).collect();
                let requests: Vec<ExportTraceServiceRequest> =
                    batch.into_iter().map(|(_, req)| req).collect();
                let db_ok = self.run_batch(&requests).await;

                // Phase 4: Acknowledge only on successful DuckDB write.
                // On failure, messages remain pending for redelivery (at-least-once).
                if db_ok {
                    if let Err(e) = acker.ack_batch(&msg_ids).await {
                        tracing::warn!(error = %e, count = msg_ids.len(), "Failed to batch ack messages");
                    }
                } else {
                    tracing::warn!(
                        count = msg_ids.len(),
                        "Skipping ack: analytics write failed, messages will be redelivered"
                    );
                }
            }

            tracing::debug!("TracePipeline shutdown complete");
        })
    }

    /// Claim and process stuck messages from other consumers.
    ///
    /// Messages that have been pending for longer than CLAIM_MIN_IDLE_MS are
    /// claimed from other (possibly crashed) consumers, processed, and acknowledged.
    async fn claim_stuck_messages(
        &self,
        claimer: &crate::data::topics::StreamClaimer,
        acker: &crate::data::topics::StreamAcker,
        consumer: &str,
    ) {
        match claimer
            .claim(consumer, CLAIM_MIN_IDLE_MS, CLAIM_MAX_COUNT)
            .await
        {
            Ok(messages) if messages.is_empty() => {
                tracing::trace!("No stuck messages to claim");
            }
            Ok(messages) => {
                let count = messages.len();
                tracing::debug!(count, "Claiming stuck messages");

                for msg in messages {
                    // Decode and process the claimed message
                    match ExportTraceServiceRequest::decode(&msg.payload[..]) {
                        Ok(request) => {
                            if self.run(&request).await.is_final() {
                                if let Err(e) = acker.ack(&msg.id).await {
                                    tracing::warn!(error = %e, msg_id = %msg.id, "Failed to ack claimed message");
                                }
                            } else {
                                tracing::warn!(msg_id = %msg.id, "Skipping ack for claimed message: write failed");
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, msg_id = %msg.id, "Failed to decode claimed message, acking to discard");
                            // Ack anyway to prevent infinite retry loop
                            if let Err(ack_err) = acker.ack(&msg.id).await {
                                tracing::warn!(error = %ack_err, msg_id = %msg.id, "Failed to ack invalid message");
                            }
                        }
                    }
                }

                tracing::debug!(count, "Finished processing claimed messages");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to claim stuck messages");
            }
        }
    }

    // ========================================================================
    // PIPELINE EXECUTION
    // ========================================================================

    /// Run the complete pipeline for a batch of OTLP requests.
    ///
    /// Processes requests in parallel across CPU cores (extract, sideml, enrich,
    /// base64 extraction are all CPU-bound), then DuckDB write + file I/O in parallel,
    /// SSE publish after both complete.
    ///
    /// Returns true if the DuckDB write succeeded (messages should be ACKed),
    /// false if it failed (messages should NOT be ACKed for redelivery).
    /// [`Self::run_batch`], for a benchmark that needs the whole write path rather than its CPU half.
    #[cfg(test)]
    pub(crate) async fn run_batch_for_test(&self, requests: &[ExportTraceServiceRequest]) -> bool {
        self.run_batch(requests).await
    }

    async fn run_batch(&self, requests: &[ExportTraceServiceRequest]) -> bool {
        let t_batch_start = std::time::Instant::now();

        let pricing = &self.pricing;
        let files_enabled = self.file_service.is_enabled();
        let file_cache = &self.file_cache;

        // Process requests in parallel using scoped threads.
        // base64 extraction can take 100ms-1s per request for image-heavy spans,
        // so parallel processing across CPU cores significantly reduces batch time.
        // The FileExtractionCache is shared across threads (moka is Send+Sync) to skip
        // redundant decode + BLAKE3 for the same base64 content.
        let results: Vec<Prepared> = tokio::task::block_in_place(|| {
            let num_workers = std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(4);

            if requests.len() <= num_workers {
                // Few requests: one thread per request.
                // Each is wrapped in catch_unwind so a panic in one request
                // doesn't propagate through thread::scope and drop the batch.
                std::thread::scope(|s| {
                    let handles: Vec<_> = requests
                        .iter()
                        .map(|request| {
                            s.spawn(|| {
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    process_request(
                                        request,
                                        pricing,
                                        files_enabled,
                                        file_cache,
                                        ExtractionMode::PerCarrier,
                                    )
                                })) {
                                    Ok(Some((spans, files, incoming))) => {
                                        Prepared::Ready(spans, files, incoming)
                                    }
                                    Ok(None) => Prepared::Nothing,
                                    Err(_) => {
                                        tracing::error!(
                                            "process_request panicked, refusing the batch"
                                        );
                                        Prepared::Panicked
                                    }
                                }
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| match h.join() {
                            Ok(result) => result,
                            Err(_) => {
                                tracing::error!("process_request thread panicked unexpectedly");
                                Prepared::Panicked
                            }
                        })
                        .collect()
                })
            } else {
                // Many requests: chunk into worker-sized groups.
                // Each request is individually wrapped in catch_unwind so a panic
                // in one request doesn't drop the entire chunk.
                let chunk_size = requests.len().div_ceil(num_workers);
                std::thread::scope(|s| {
                    let handles: Vec<_> = requests
                        .chunks(chunk_size)
                        .map(|chunk| {
                            s.spawn(|| {
                                chunk
                                    .iter()
                                    .map(|request| {
                                        match std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| {
                                                process_request(
                                                    request,
                                                    pricing,
                                                    files_enabled,
                                                    file_cache,
                                                    ExtractionMode::PerCarrier,
                                                )
                                            }),
                                        ) {
                                            Ok(Some((spans, files, incoming))) => {
                                                Prepared::Ready(spans, files, incoming)
                                            }
                                            Ok(None) => Prepared::Nothing,
                                            Err(_) => {
                                                tracing::error!(
                                                    "process_request panicked, refusing the batch"
                                                );
                                                Prepared::Panicked
                                            }
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            })
                        })
                        .collect();

                    handles
                        .into_iter()
                        .flat_map(|h| match h.join() {
                            Ok(results) => results,
                            Err(_) => {
                                // A worker thread panicked, so its whole chunk is unaccounted for.
                                // Returning an empty vector hid that: the results would simply be
                                // short, and a count of `Panicked` entries would miss them. The
                                // cardinality check below is what catches it.
                                tracing::error!("process_request thread panicked unexpectedly");
                                Vec::new()
                            }
                        })
                        .collect()
                })
            }
        });

        let mut all_db_spans: Vec<NormalizedSpan> = Vec::new();
        let mut all_pending_files: Vec<PendingFileWrite> = Vec::new();
        let mut all_incoming: Vec<IncomingReference> = Vec::new();
        // Short results mean a worker thread died with its whole chunk, which no per-request outcome can
        // report - so cardinality is checked, not just the outcomes.
        let mut lost_requests = requests.len().saturating_sub(results.len());
        for result in results {
            match result {
                Prepared::Ready(db_spans, pending_files, incoming) => {
                    all_db_spans.extend(db_spans);
                    all_pending_files.extend(pending_files);
                    all_incoming.extend(incoming);
                }
                // Normal: nothing to persist, nothing to refuse.
                Prepared::Nothing => {}
                // `catch_unwind` stops one request's panic taking the batch down, which is right, and
                // the result used to be dropped here - so the batch reported success, the exporter
                // acknowledged, and those spans were gone with nothing but a log line. The batch is
                // refused instead: the exporter retries, and ingestion is idempotent by span id, so
                // re-delivering the requests that did succeed costs a rewrite rather than a duplicate.
                Prepared::Panicked => lost_requests += 1,
            }
        }
        if lost_requests > 0 {
            // The requests that *did* succeed left cache entries but were never persisted, so the retry
            // must not find them claiming otherwise. Every refusal clears the cache for that reason.
            self.file_cache.invalidate_all();
            tracing::error!(
                lost_requests,
                requests = requests.len(),
                "Refusing the batch: a request panicked and its spans would otherwise be acknowledged"
            );
            return false;
        }

        if all_db_spans.is_empty() {
            return true;
        }

        // Spans for a project that will not accept writes are dropped, not written.
        //
        // Checked twice, and the second one is the check that matters. The fence lives in the
        // transactional store and the spans in the analytics store, so no transaction spans both and the
        // gap between reading the fence and committing is real. Making that gap as small as possible is
        // what bounds it: the second check sits immediately before the analytics write, so what remains
        // is the write itself rather than the whole batch - and a batch can take a hundred milliseconds
        // on inlined base64 images, or much longer if object storage is slow.
        //
        // The first check is an optimisation with the same rule: no point storing a dying project's file
        // bytes. Neither can be replaced by an HTTP-edge check, which says nothing about a write that
        // happens seconds later on a topic consumer.
        //
        // "Will not accept writes" covers a project claimed for deletion *and* one whose row is gone.
        // The second is what makes the fence airtight rather than narrow, together with the row
        // outliving its claim: a batch that read the fence before a claim commits within one batch, and
        // after the row goes such writes are refused outright. Spans for a project with no row were
        // never readable anyway - every read path finds data through the project row.
        //
        // Dropped rather than refused: the project is not coming back, so a refusal would have the
        // exporter retry a doomed batch, and a batch mixing a live project with a dying one would lose
        // the live one's spans too.
        match self
            .drop_spans_for_dead_projects(
                &mut all_db_spans,
                &mut all_pending_files,
                &mut all_incoming,
            )
            .await
        {
            Ok(_) if all_db_spans.is_empty() => return true, // nothing left to write
            Ok(_) => {}
            Err(()) => return false,
        }

        // Before the files are written, so a rejected span's attachments are never stored either.
        if drop_unstorable_spans(&mut all_db_spans) > 0 && all_db_spans.is_empty() {
            return true; // nothing storable, and nothing was queued for a retry that cannot help
        }

        let t_prepare_done = std::time::Instant::now();
        let span_count = all_db_spans.len();

        // Build SSE events before write (captures span metadata)
        let sse_events: Vec<SseSpanEvent> = all_db_spans.iter().map(SseSpanEvent::from).collect();

        // Files first, then the rows that reference them.
        //
        // These used to run under one `tokio::join!`, which is faster and admits a state the reader
        // cannot recover from: a span row committed with a `#!B64!#` reference to a file whose write
        // failed. Writing the referenced object before the reference makes that impossible - the
        // remaining failure mode is an orphaned file, which is reclaimable and invisible to a reader.
        //
        // The cost is their sum rather than their maximum. Worth it: parity between the analytics
        // backends proves they *agree*, never that either faithfully represents the OTLP input, so a
        // dangling reference is a class of corruption no read-side test can catch.
        let files = persist_extracted_files(all_pending_files, &self.file_service).await;
        if files.failed > 0 {
            // A row referencing a file that is not there is worse than no row: the reference cannot
            // be repaired by a later delivery, while the batch can. Refusing it lets the exporter
            // retry, and ingestion is idempotent by span id.
            //
            // The cache has to be cleared first. Its entries are written when bytes are *extracted*,
            // before they are stored, and a hit carries no data - which persistence reads as "already
            // in storage". Left in place, the retry this refusal invites would commit exactly the
            // dangling reference the refusal is meant to prevent.
            self.file_cache.invalidate_all();
            // The files that *did* store already have associations, and this batch is not going to write the
            // rows that justify them. Released, or they hold `ref_count` above zero forever: the orphan
            // sweeper selects on `ref_count = 0`, so nothing would reclaim the bytes and the project's quota
            // would shrink permanently. A redelivery re-creates them, so this costs nothing when it succeeds.
            self.release_created_associations(&files.created_associations)
                .await;
            tracing::error!(
                failed = files.failed,
                spans = span_count,
                "Refusing to commit spans whose extracted files could not be stored"
            );
            return false;
        }
        // Deliberate, not transient: retrying will not help, so the spans are still committed - but
        // their references to the rejected files are rewritten first. A reader cannot tell a reference
        // to a rejected file from a corrupt one; both render as a broken image, and nothing on the span
        // says which. The placeholder says which.
        // References that arrived already formed are claims about storage that nothing verified. Checked
        // here, with the ones that hold getting an association and the rest joining the quota-rejected
        // set - both are "a reference a reader cannot resolve", and both are replaced with a note.
        let mut unresolvable = files.quota_skipped;
        let (unbacked, reconcile_failed, incoming_associations) =
            reconcile_incoming_references(&all_incoming, &self.file_service).await;
        // Folded into the batch's own set, so every compensation path - a failed write, a tombstoned trace,
        // the post-write re-check - covers a reference that arrived already formed as well as one whose
        // bytes this batch wrote.
        let mut created_associations = files.created_associations;
        created_associations.extend(incoming_associations);
        if reconcile_failed > 0 {
            self.file_cache.invalidate_all();
            // Same reason as above, and here the set is the merged one: this batch's own file writes plus the
            // references that arrived already formed. Both kinds hold quota until released.
            self.release_created_associations(&created_associations)
                .await;
            tracing::error!(
                failed = reconcile_failed,
                "Refusing the batch: could not settle whether some file references are backed"
            );
            return false;
        }
        unresolvable.extend(unbacked);
        if !unresolvable.is_empty() {
            let rewritten = note_unstored_files(&mut all_db_spans, &unresolvable);
            tracing::error!(
                files = unresolvable.len(),
                references_rewritten = rewritten,
                spans = span_count,
                "File content is not stored for some references; replaced them with a note"
            );
        }
        // Again, with the write next in line. Between the first check and here the batch stored its
        // files, which can take a while, and a claim may have landed in that time.
        let mut no_files: Vec<PendingFileWrite> = Vec::new();
        let mut no_incoming: Vec<IncomingReference> = Vec::new();
        match self
            .drop_spans_for_dead_projects(&mut all_db_spans, &mut no_files, &mut no_incoming)
            .await
        {
            Ok(0) => {}
            Ok(_) => {
                // Whole or partial, the dropped traces' associations go: their spans will not be written,
                // and the count they hold is what the orphan sweeper cannot see past.
                self.release_associations_of_dropped(&created_associations, &all_db_spans)
                    .await;
                if all_db_spans.is_empty() {
                    return true;
                }
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        // And traces deleted individually, which the project fence says nothing about. Checked here, in
        // the same position and for the same reason: this batch's files are already stored, so a trace
        // deleted since they were written must not gain a row pointing at bytes the deletion reclaimed.
        //
        // A dropped span's associations are released, for the same reason a failed write's are: they hold
        // `ref_count` above zero for a row that will never exist, and the orphan sweeper selects on zero.
        // Deleted sessions, first: a trace of a deleted session may itself have no tombstone, because the
        // session's deletion resolved its traces at one instant and this one arrived after.
        match self
            .drop_spans_for_deleted_sessions(&mut all_db_spans)
            .await
        {
            Ok(0) => {}
            Ok(_) if all_db_spans.is_empty() => {
                self.release_created_associations(&created_associations)
                    .await;
                return true;
            }
            Ok(_) => {}
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        match self.drop_spans_for_deleted_traces(&mut all_db_spans).await {
            Ok(0) => {}
            Ok(_) if all_db_spans.is_empty() => {
                self.release_created_associations(&created_associations)
                    .await;
                return true;
            }
            Ok(_) => {
                self.release_associations_of_dropped(&created_associations, &all_db_spans)
                    .await;
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        // Captured before the write consumes the spans: the compensating re-check below needs to know
        // exactly what was written, and only those rows may be removed.
        let written: Vec<(String, String, String)> = all_db_spans
            .iter()
            .map(|s| {
                (
                    s.project_id
                        .as_deref()
                        .unwrap_or(DEFAULT_PROJECT_ID)
                        .to_string(),
                    s.trace_id.clone(),
                    s.span_id.clone(),
                )
            })
            .collect();
        // Which session each written trace belongs to, for the session compensation below. A trace's spans
        // agree about it, so the first one that names a session decides.
        let session_of: HashMap<(String, String), String> = all_db_spans
            .iter()
            .filter_map(|s| {
                let session = s.session_id.as_deref().filter(|v| !v.is_empty())?;
                Some((
                    (
                        s.project_id
                            .as_deref()
                            .unwrap_or(DEFAULT_PROJECT_ID)
                            .to_string(),
                        s.trace_id.clone(),
                    ),
                    session.to_string(),
                ))
            })
            .collect();
        let db_ok = write_to_duckdb(all_db_spans, &self.analytics).await;

        let t_persist_done = std::time::Instant::now();

        if db_ok {
            // Confirm the associations first: the rows that justify them are committed, so they are no
            // longer provisional and no failure path may take them.
            if let Err(e) = self
                .file_service
                .database()
                .repository()
                .confirm_trace_file_associations(&created_associations)
                .await
            {
                tracing::warn!(
                    error = %e,
                    "Could not confirm this batch's file associations as durable; they keep a pending \
                     writer and a later failure path could release them"
                );
            }
            // Then the compensating re-check, before anything is published: a deletion that landed between
            // the pre-write check and the write leaves the trace resurrected, and an SSE event for it would
            // announce it to every connected reader.
            // Sessions first: tombstoning their traces is what makes the trace compensation below see them.
            self.tombstone_traces_of_deleted_sessions(&written, &session_of)
                .await;
            let compensated = self
                .collect_spans_written_for_deleted_traces(&written, &created_associations)
                .await;
            // Published *after* every drop and compensation, and only for spans that survived both. Built
            // before the filtering, `sse_events` announced spans this batch went on to discard - so a
            // reader saw a span appear and then never find it.
            let surviving: HashSet<(&str, &str, &str)> = written
                .iter()
                .map(|(project, trace, span)| (project.as_str(), trace.as_str(), span.as_str()))
                .filter(|(project, trace, span)| {
                    !compensated.contains(&(
                        (*project).to_string(),
                        (*trace).to_string(),
                        (*span).to_string(),
                    ))
                })
                .collect();
            let sse_events: Vec<SseSpanEvent> = sse_events
                .into_iter()
                .filter(|e| {
                    surviving.contains(&(
                        e.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
                        e.trace_id.as_str(),
                        e.span_id.as_str(),
                    ))
                })
                .collect();
            publish_sse_events(&sse_events, &self.topics).await;
        } else {
            // Release the associations this batch created, since the rows that would have justified them
            // are not there. Files are written before the rows deliberately, so a failed write leaves
            // associations holding `ref_count` above zero - and the orphan sweeper selects on
            // `ref_count = 0`, so nothing would ever reclaim those bytes and the project's quota would
            // shrink permanently. A redelivery re-creates them, so this costs nothing when the retry
            // succeeds.
            self.release_created_associations(&created_associations)
                .await;
        }

        tracing::debug!(
            requests = requests.len(),
            spans = span_count,
            db_ok,
            prepare_ms = t_prepare_done.duration_since(t_batch_start).as_millis() as u64,
            persist_ms = t_persist_done.duration_since(t_prepare_done).as_millis() as u64,
            total_ms = t_persist_done.duration_since(t_batch_start).as_millis() as u64,
            "Pipeline batch completed"
        );

        db_ok
    }

    /// Remove everything belonging to a project that will not accept writes.
    ///
    /// Returns how many spans were dropped, so a caller can *report* a partial drop rather than answer an
    /// unqualified success for records it discarded. `Err(())` means the fence could not be read - which
    /// refuses the batch rather than assuming the fence is open, and clears the extraction cache as every
    /// refusal must, because its entries are written when bytes are extracted rather than stored.
    async fn drop_spans_for_dead_projects(
        &self,
        spans: &mut Vec<NormalizedSpan>,
        files: &mut Vec<PendingFileWrite>,
        incoming: &mut Vec<IncomingReference>,
    ) -> Result<usize, ()> {
        let refusing = match self.projects_refusing_writes(spans).await {
            Ok(refusing) => refusing,
            Err(e) => {
                self.file_cache.invalidate_all();
                tracing::error!(
                    error = %e,
                    spans = spans.len(),
                    "Refusing the batch: could not settle whether its project accepts writes"
                );
                return Err(());
            }
        };
        if refusing.is_empty() {
            return Ok(0);
        }
        let before = spans.len();
        spans.retain(|s| !refusing.contains(s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID)));
        files.retain(|f| !refusing.contains(f.project_id.as_str()));
        incoming.retain(|(project, _, _)| !refusing.contains(project.as_str()));
        tracing::warn!(
            dropped = before - spans.len(),
            projects = ?refusing,
            "Dropped spans for projects that do not accept writes (deleted or being deleted)"
        );
        Ok(before - spans.len())
    }

    /// After the write, check the tombstones again and remove anything that slipped through.
    ///
    /// The check before the write cannot be atomic with it - the tombstone is a row in the transactional
    /// store and the spans go to the analytics store, so no transaction spans them. A deletion landing in
    /// that window would leave the trace resurrected: its files reclaimed, its row committed, and the
    /// caller already answered 204.
    ///
    /// So the window is closed by *compensation* rather than by locking: the write is followed by a second
    /// look, and anything now tombstoned is deleted along with the associations this batch created for it.
    /// This is not a substitute for the pre-write check - that is what keeps the common case from ever
    /// writing - and it is not sufficient alone either, since a crash between the write and this line
    /// leaves the spans. The deletion sweep is what covers that, exactly as it covers a project whose
    /// tombstone outlived its claim.
    /// After the write, tombstone the traces of any session deleted in the meantime.
    ///
    /// The session check before the write is not atomic with it, exactly as the trace check is not - and a
    /// session deletion landing in that window leaves a trace the deletion's own snapshot never named. This
    /// hands such traces to the *trace* protocol by tombstoning them, which then removes their rows on the
    /// next line and reconciles their files in the sweep. Doing it this way rather than duplicating the
    /// deletion logic means there is one place that knows how a trace is taken away.
    ///
    /// Returns the trace ids it tombstoned, so the caller can compensate them in the same pass.
    async fn tombstone_traces_of_deleted_sessions(
        &self,
        written: &[(String, String, String)],
        session_of: &HashMap<(String, String), String>,
    ) -> HashSet<(String, String)> {
        let mut affected: HashSet<(String, String)> = HashSet::new();
        if session_of.is_empty() {
            return affected;
        }
        let mut sessions_by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for ((project, _trace), session) in session_of {
            sessions_by_project
                .entry(project.as_str())
                .or_default()
                .push(session.clone());
        }
        let repo = self.file_service.database().repository();
        for (project, mut session_ids) in sessions_by_project {
            session_ids.sort_unstable();
            session_ids.dedup();
            let deleted = match repo.deleted_sessions_among(project, &session_ids).await {
                Ok(found) if found.is_empty() => continue,
                Ok(found) => found,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project,
                        "Could not re-check session tombstones after the write; the deletion sweep will \
                         collect anything that slipped through"
                    );
                    continue;
                }
            };
            let doomed: Vec<String> = written
                .iter()
                .filter(|(p, trace, _)| {
                    p == project
                        && session_of
                            .get(&(p.clone(), trace.clone()))
                            .is_some_and(|s| deleted.contains(s))
                })
                .map(|(_, trace, _)| trace.clone())
                .collect();
            if doomed.is_empty() {
                continue;
            }
            if let Err(e) = repo.record_deleted_traces(project, &doomed).await {
                tracing::warn!(
                    error = %e,
                    project,
                    "Could not tombstone traces of a session deleted during the write; the session sweep \
                     will collect them"
                );
                continue;
            }
            tracing::warn!(
                project,
                traces = doomed.len(),
                "Tombstoned traces of sessions deleted during the write"
            );
            for trace in doomed {
                affected.insert((project.to_string(), trace));
            }
        }
        affected
    }

    async fn collect_spans_written_for_deleted_traces(
        &self,
        written: &[(String, String, String)],
        created_associations: &[(String, String, String)],
    ) -> HashSet<(String, String, String)> {
        // Keyed by (project, trace, span), not (trace, span): a span id is unique only within a trace and a
        // trace id only within a project, so a survivor set that drops the project can let a compensated
        // span in one project suppress the SSE for a same-id survivor in another (gotcha #22). The deletion
        // itself was already per-project; only the returned identity was blind.
        let mut removed: HashSet<(String, String, String)> = HashSet::new();
        if written.is_empty() {
            return removed;
        }
        let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for (project, trace, _span) in written {
            by_project
                .entry(project.as_str())
                .or_default()
                .push(trace.clone());
        }
        let repo = self.file_service.database().repository();
        for (project, mut trace_ids) in by_project {
            trace_ids.sort_unstable();
            trace_ids.dedup();
            let tombstoned = match repo.deleted_traces_among(project, &trace_ids).await {
                Ok(found) if found.is_empty() => continue,
                Ok(found) => found,
                // Nothing to compensate on: the sweep will find these if they are genuinely deleted.
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project,
                        "Could not re-check trace tombstones after the write; the deletion sweep will \
                         collect anything that slipped through"
                    );
                    continue;
                }
            };
            let doomed: Vec<(String, String)> = written
                .iter()
                .filter(|(p, t, _)| p == project && tombstoned.contains(t))
                .map(|(_, t, span)| (t.clone(), span.clone()))
                .collect();
            // The analytics delete is keyed (trace, span) within the project it is scoped to; the returned
            // set carries the project so the SSE filter can tell projects apart.
            // The rows go first, and the associations *only if* they went.
            //
            // Released unconditionally, a failed `delete_spans` leaves readable rows whose files are now
            // eligible for collection - a dangling reference, which is the one outcome the whole
            // write-files-before-rows ordering exists to make impossible. Leaving the association instead
            // leaves a file held by a row that is about to be swept, which the sweep then reconciles.
            if let Err(e) = self
                .analytics
                .repository()
                .delete_spans(project, &doomed)
                .await
            {
                tracing::error!(
                    error = %e,
                    project,
                    spans = doomed.len(),
                    "Could not remove spans written for a deleted trace, so their file associations are \
                     left in place; releasing them would leave a readable row pointing at collectable \
                     bytes. The deletion sweep retries both."
                );
                continue;
            }
            tracing::warn!(
                project,
                spans = doomed.len(),
                traces = tombstoned.len(),
                "Removed spans written for traces deleted during the write"
            );
            removed.extend(
                doomed
                    .iter()
                    .map(|(t, span)| (project.to_string(), t.clone(), span.clone())),
            );
            let orphaned: Vec<(String, String, String)> = created_associations
                .iter()
                .filter(|(p, t, _)| p == project && tombstoned.contains(t))
                .cloned()
                .collect();
            self.release_created_associations(&orphaned).await;
        }
        removed
    }

    /// Release the associations of traces that are no longer in the batch.
    ///
    /// Every drop - a dead project, a deleted session, a deleted trace - leaves behind associations for
    /// spans that will not be written, and an association holds a file's reference count above zero, which
    /// is what the orphan sweeper selects on. Factored out because there are five such sites and each one
    /// that forgot was a permanent leak.
    async fn release_associations_of_dropped(
        &self,
        created: &[(String, String, String)],
        surviving_spans: &[NormalizedSpan],
    ) {
        if created.is_empty() {
            return;
        }
        let surviving: HashSet<(&str, &str)> = surviving_spans
            .iter()
            .map(|s| {
                (
                    s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
                    s.trace_id.as_str(),
                )
            })
            .collect();
        let orphaned: Vec<(String, String, String)> = created
            .iter()
            .filter(|(project, trace, _)| !surviving.contains(&(project.as_str(), trace.as_str())))
            .cloned()
            .collect();
        self.release_created_associations(&orphaned).await;
    }

    /// Release associations this batch created after its analytics write failed.
    ///
    /// Best effort and logged rather than fatal: the batch has already failed and will be redelivered,
    /// so the useful thing is to leave as little behind as possible. A release that itself fails leaves
    /// the association, which is the state this exists to avoid - so it is worth a warning, but not worth
    /// turning a retryable batch into a different failure.
    ///
    /// `sync_ref_count` follows each release, recomputing the count from the associations that remain
    /// rather than subtracting - so a concurrent batch holding its own association keeps the file, and
    /// the count cannot drift however the two interleave.
    async fn release_created_associations(&self, created: &[(String, String, String)]) {
        if created.is_empty() {
            return;
        }
        let repo = self.file_service.database().repository();
        let mut released = 0usize;
        for (project_id, trace_id, hash) in created {
            // No read here, deliberately.
            //
            // An earlier version asked the analytics store whether the trace had rows and skipped the
            // release if it did - which is a read-then-act pair, and therefore not a decision at all: a
            // second batch can commit between the read and the delete, and its file loses its protection.
            // The `provisional` marker replaces that with a single statement. The delete matches only rows
            // still marked provisional, and a batch that committed has already cleared the marker, so
            // "another batch owns this now" is a *stored fact* rather than something observed and hoped to
            // still hold.
            match repo
                .release_trace_file_association(project_id, trace_id, hash)
                .await
            {
                Ok(true) => {
                    released += 1;
                    if let Err(e) = repo.sync_ref_count(project_id, hash).await {
                        tracing::warn!(
                            error = %e,
                            project_id, hash,
                            "Released an association but could not recompute the file's reference count"
                        );
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    project_id, trace_id, hash,
                    "Could not release an association after a failed analytics write; the file it \
                     references will hold quota until the association is removed"
                ),
            }
        }
        if released > 0 {
            tracing::debug!(
                released,
                "Released file associations created by a batch whose analytics write failed"
            );
        }
    }

    /// Remove spans belonging to sessions that have been deleted.
    ///
    /// Separate from the trace check, because they fence different things. A session is deleted by
    /// resolving it to trace ids and deleting those, so a trace of the same session that arrives *after*
    /// that resolution has no trace tombstone - it was never in the snapshot - and would recreate a session
    /// the caller was told was gone. The session id is durable; the trace list is one instant's view.
    ///
    /// `Err` means the table could not be read, which is not the same as empty: the caller refuses the
    /// batch so the exporter retries, exactly as for the project fence and the trace tombstone.
    async fn drop_spans_for_deleted_sessions(
        &self,
        spans: &mut Vec<NormalizedSpan>,
    ) -> Result<usize, ()> {
        let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for span in spans.iter() {
            if let Some(session_id) = span.session_id.as_deref().filter(|s| !s.is_empty()) {
                by_project
                    .entry(span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
                    .or_default()
                    .push(session_id.to_string());
            }
        }
        if by_project.is_empty() {
            return Ok(0);
        }
        let repo = self.file_service.database().repository();
        let mut deleted: HashSet<(String, String)> = HashSet::new();
        for (project, mut session_ids) in by_project {
            session_ids.sort_unstable();
            session_ids.dedup();
            match repo.deleted_sessions_among(project, &session_ids).await {
                Ok(found) => {
                    for session_id in found {
                        deleted.insert((project.to_string(), session_id));
                    }
                }
                Err(e) => {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        error = %e,
                        project = project,
                        "Refusing the batch: could not settle whether its sessions have been deleted"
                    );
                    return Err(());
                }
            }
        }
        if deleted.is_empty() {
            return Ok(0);
        }

        let deleted_traces = traces_of_sessions(spans, &deleted);
        let before = spans.len();
        spans.retain(|s| {
            !deleted_traces.contains(&(
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            ))
        });
        tracing::warn!(
            dropped = before - spans.len(),
            sessions = deleted.len(),
            traces = deleted_traces.len(),
            "Dropped spans for sessions that have been deleted"
        );
        Ok(before - spans.len())
    }

    /// Remove spans belonging to traces that have been deleted.
    ///
    /// The project fence does not cover this: a live project can have an individual trace deleted, and
    /// this batch's files and associations were already written - deliberately, before the analytics row
    /// that references them. So a batch in flight when `delete_traces` ran would commit a span carrying
    /// a `#!B64!#` reference to bytes the deletion has reclaimed, for a trace the caller was told 204
    /// for, and a queued redelivery would do it minutes later.
    ///
    /// `Err` means the tombstone table could not be read, which is not the same as empty: the caller
    /// refuses the batch so the exporter retries, exactly as it does for an unreadable project fence.
    /// Ingestion is idempotent by span id, so a retry costs a rewrite.
    async fn drop_spans_for_deleted_traces(
        &self,
        spans: &mut Vec<NormalizedSpan>,
    ) -> Result<usize, ()> {
        // Grouped by project, because the tombstone is keyed by both and a trace id comes from the
        // client - two projects can legitimately present the same one.
        let mut by_project: HashMap<&str, Vec<String>> = HashMap::new();
        for span in spans.iter() {
            by_project
                .entry(span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
                .or_default()
                .push(span.trace_id.clone());
        }
        let repo = self.file_service.database().repository();
        let mut deleted: HashSet<(String, String)> = HashSet::new();
        for (project, mut trace_ids) in by_project {
            trace_ids.sort_unstable();
            trace_ids.dedup();
            match repo.deleted_traces_among(project, &trace_ids).await {
                Ok(found) => {
                    for trace_id in found {
                        deleted.insert((project.to_string(), trace_id));
                    }
                }
                Err(e) => {
                    self.file_cache.invalidate_all();
                    tracing::error!(
                        error = %e,
                        project = project,
                        "Refusing the batch: could not settle whether its traces have been deleted"
                    );
                    return Err(());
                }
            }
        }
        if deleted.is_empty() {
            return Ok(0);
        }
        let before = spans.len();
        spans.retain(|s| {
            !deleted.contains(&(
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            ))
        });
        tracing::warn!(
            dropped = before - spans.len(),
            traces = deleted.len(),
            "Dropped spans for traces that have been deleted"
        );
        Ok(before - spans.len())
    }

    /// Which of a batch's projects will not accept writes, and whether the question could be answered.
    ///
    /// `Err` means the fence is unknown, which is not the same as open. Dropping a live project's spans
    /// because a query failed would be data loss caused by the fence, and writing to a project that may
    /// be going away is the corruption it exists to prevent - so the caller refuses the batch instead and
    /// the exporter retries. Ingestion is idempotent by span id, so a retry costs a rewrite.
    ///
    /// One query per distinct project, and a batch almost always names one: the SDK sends a project's
    /// spans to that project's endpoint.
    async fn projects_refusing_writes(
        &self,
        spans: &[NormalizedSpan],
    ) -> Result<HashSet<String>, String> {
        // Same default the write path applies, so the fence and the write agree on which project a
        // span without one belongs to.
        let mut projects: Vec<&str> = spans
            .iter()
            .map(|s| s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            .collect();
        projects.sort_unstable();
        projects.dedup();
        let repo = self.file_service.database().repository();
        let mut refusing = HashSet::new();
        for project in projects {
            match repo.project_accepts_writes(project).await {
                Ok(true) => {}
                Ok(false) => {
                    refusing.insert(project.to_string());
                }
                Err(e) => return Err(format!("project {project}: {e}")),
            }
        }
        Ok(refusing)
    }

    /// Extract, enrich and write one request, answering whether it was stored.
    ///
    /// Used by the ingest path when the topic backend is not durable: with an in-memory queue an
    /// acknowledgement before the write is a promise the process cannot keep, so the request writes
    /// first. Measured at roughly nine milliseconds per request against four when batched - which for an
    /// exporter that ships every few seconds is not a cost worth a lost trace.
    pub async fn ingest_now(&self, request: &ExportTraceServiceRequest) -> IngestOutcome {
        self.run(request).await
    }

    /// Run the complete pipeline for a single request (used during shutdown drain
    /// and claimed message recovery). File I/O is done inline for reliability.
    async fn run(&self, request: &ExportTraceServiceRequest) -> IngestOutcome {
        // Wrapped, like every call on the batch path. This one was not, and it is the path that
        // handles *recovery* - so a message the batch path refused for panicking could be claimed here
        // and take down the whole pipeline task rather than one request.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_request(
                request,
                &self.pricing,
                self.file_service.is_enabled(),
                &self.file_cache,
                ExtractionMode::PerCarrier,
            )
        }));
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                self.file_cache.invalidate_all();
                tracing::error!("process_request panicked; refusing this request");
                return IngestOutcome::Failed;
            }
        };
        if let Some((db_spans, pending_files, incoming)) = result {
            if db_spans.is_empty() {
                return IngestOutcome::Stored; // nothing to store, and nothing was refused
            }
            let mut db_spans = db_spans;

            // The project fence, exactly as the batch path applies it. This path had no check at all,
            // which is worse than it sounds: it is the *recovery* path, so it handles messages queued
            // before a restart - precisely the ones most likely to name a project deleted since.
            let mut pending_files = pending_files;
            let mut incoming = incoming;
            let mut partly_dropped = 0usize;
            match self
                .drop_spans_for_dead_projects(&mut db_spans, &mut pending_files, &mut incoming)
                .await
            {
                // Reported as dropped, whether *all* of the request's spans went or only some: a batch can
                // name a live project and a dying one, and answering an unqualified success for the half
                // that was discarded is the failure this path exists to remove.
                // No associations to release here: on this path the project fence runs *before* the files
                // are written, so nothing has been associated yet - which is also why it takes the pending
                // file list and prunes it directly.
                Ok(dropped) if db_spans.is_empty() => {
                    return IngestOutcome::Dropped { spans: dropped };
                }
                Ok(0) => {}
                Ok(dropped) => partly_dropped = dropped,
                Err(()) => return IngestOutcome::Failed,
            }

            // The same rule as the batch path, and reported the same way: a retry cannot fix a clock, so
            // these are dropped rather than refused, and the caller is told how many.
            let unstorable = drop_unstorable_spans(&mut db_spans);
            if unstorable > 0 {
                if db_spans.is_empty() {
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + unstorable,
                    };
                }
                partly_dropped += unstorable;
            }

            let sse_events: Vec<SseSpanEvent> = db_spans.iter().map(SseSpanEvent::from).collect();
            // Ordered, exactly as the batch path is: this path ran the two concurrently, so it could
            // commit a row referencing a file whose write had failed - the same defect, in the path
            // that runs at shutdown and recovery, where it is least likely to be noticed.
            let files = persist_extracted_files(pending_files, &self.file_service).await;
            if files.failed > 0 {
                self.file_cache.invalidate_all();
                // Released for the same reason as on the batch path: the stored files' associations hold
                // quota that nothing would ever reclaim, since the rows justifying them are not being
                // written and the orphan sweeper only sees `ref_count = 0`.
                self.release_created_associations(&files.created_associations)
                    .await;
                tracing::error!(
                    failed = files.failed,
                    "Refusing to commit spans whose extracted files could not be stored"
                );
                return IngestOutcome::Failed;
            }
            let mut unresolvable = files.quota_skipped;
            let (unbacked, reconcile_failed, incoming_associations) =
                reconcile_incoming_references(&incoming, &self.file_service).await;
            let mut created_associations = files.created_associations;
            created_associations.extend(incoming_associations);
            if reconcile_failed > 0 {
                self.file_cache.invalidate_all();
                self.release_created_associations(&created_associations)
                    .await;
                tracing::error!(
                    failed = reconcile_failed,
                    "Refusing the batch: could not settle whether some file references are backed"
                );
                return IngestOutcome::Failed;
            }
            unresolvable.extend(unbacked);
            if !unresolvable.is_empty() {
                let rewritten = note_unstored_files(&mut db_spans, &unresolvable);
                tracing::error!(
                    files = unresolvable.len(),
                    references_rewritten = rewritten,
                    "File content is not stored for some references; replaced them with a note"
                );
            }
            // The trace tombstone, in the same position as the batch path's and for the same reason.
            //
            // This path had no check at all, and it is the *default*: with an in-memory topic the request
            // writes inline, and it is also the shutdown drain and the claimed-message recovery. So a
            // deleted trace could be re-posted through the ordinary configuration and answered success,
            // which is the exact failure the tombstone exists to remove.
            match self.drop_spans_for_deleted_sessions(&mut db_spans).await {
                Ok(dropped) if db_spans.is_empty() => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + dropped,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped += dropped;
                    self.release_associations_of_dropped(&created_associations, &db_spans)
                        .await;
                }
                Err(()) => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            }

            match self.drop_spans_for_deleted_traces(&mut db_spans).await {
                Ok(dropped) if db_spans.is_empty() => {
                    // The associations this batch created are released, or they would hold quota for a
                    // trace that will never have a row.
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + dropped,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped += dropped;
                    self.release_associations_of_dropped(&created_associations, &db_spans)
                        .await;
                }
                Err(()) => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            }

            // Same capture as the batch path, for the same compensating re-check.
            let written: Vec<(String, String, String)> = db_spans
                .iter()
                .map(|s| {
                    (
                        s.project_id
                            .as_deref()
                            .unwrap_or(DEFAULT_PROJECT_ID)
                            .to_string(),
                        s.trace_id.clone(),
                        s.span_id.clone(),
                    )
                })
                .collect();
            let session_of: HashMap<(String, String), String> = db_spans
                .iter()
                .filter_map(|s| {
                    let session = s.session_id.as_deref().filter(|v| !v.is_empty())?;
                    Some((
                        (
                            s.project_id
                                .as_deref()
                                .unwrap_or(DEFAULT_PROJECT_ID)
                                .to_string(),
                            s.trace_id.clone(),
                        ),
                        session.to_string(),
                    ))
                })
                .collect();
            let db_ok = write_to_duckdb(db_spans, &self.analytics).await;
            if !db_ok {
                self.release_created_associations(&created_associations)
                    .await;
            }
            if db_ok {
                if let Err(e) = self
                    .file_service
                    .database()
                    .repository()
                    .confirm_trace_file_associations(&created_associations)
                    .await
                {
                    tracing::warn!(
                        error = %e,
                        "Could not confirm this request's file associations as durable; they keep a \
                         pending writer"
                    );
                }
                self.tombstone_traces_of_deleted_sessions(&written, &session_of)
                    .await;
                let compensated = self
                    .collect_spans_written_for_deleted_traces(&written, &created_associations)
                    .await;
                // Only spans that survived every drop and the compensation - see the batch path, including
                // why the survivor identity carries the project id.
                let surviving: HashSet<(&str, &str, &str)> = written
                    .iter()
                    .map(|(project, trace, span)| (project.as_str(), trace.as_str(), span.as_str()))
                    .filter(|(project, trace, span)| {
                        !compensated.contains(&(
                            (*project).to_string(),
                            (*trace).to_string(),
                            (*span).to_string(),
                        ))
                    })
                    .collect();
                let sse_events: Vec<SseSpanEvent> = sse_events
                    .into_iter()
                    .filter(|e| {
                        surviving.contains(&(
                            e.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
                            e.trace_id.as_str(),
                            e.span_id.as_str(),
                        ))
                    })
                    .collect();
                publish_sse_events(&sse_events, &self.topics).await;
                if partly_dropped > 0 {
                    IngestOutcome::PartlyDropped {
                        spans: partly_dropped,
                    }
                } else {
                    IngestOutcome::Stored
                }
            } else {
                IngestOutcome::Failed
            }
        } else {
            IngestOutcome::Stored
        }
    }
}

// ============================================================================
// PER-REQUEST PROCESSING (free function for thread safety)
// ============================================================================

/// Test-only wrapper over `process_request`, which is private to this module.
///
/// Used by `message_goldens_tests` to replay captured OTLP payloads through the real
/// pipeline. File extraction is off: it performs disk writes and no message property under
/// test depends on it.
#[cfg(test)]
/// As [`process_request_for_test`], with the extraction mode chosen by the caller.
///
/// The metamorphic test runs a fixture through both modes and compares: the answer under `PerCarrier`
/// must contain the answer under `FirstMatch`, in the same relative order, plus whatever the carriers
/// nobody read were holding.
#[cfg(test)]
pub(crate) fn process_request_for_test_with_mode(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    mode: ExtractionMode,
) -> Option<(Vec<NormalizedSpan>, Vec<PendingFileWrite>)> {
    process_request(request, pricing, false, &FileExtractionCache::new(), mode)
        .map(|(spans, files, _)| (spans, files))
}

/// What became of one request in the CPU phase.
///
/// `Option` conflated two outcomes that need opposite handling, and the conflation was live: a request
/// with no spans is perfectly normal, and treating it as a panic made its batch refuse itself forever.
/// A panic must refuse the batch; nothing to do must not.
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

/// Process a single OTLP request through stages 1-4.
///
/// Pure CPU work: extract attributes, messages, sideml, enrich, prepare.
/// Returns NormalizedSpans + pending file writes, or None if no spans.
fn process_request(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    files_enabled: bool,
    file_cache: &FileExtractionCache,
    mode: crate::domain::traces::extract::ExtractionMode,
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
    let mut traces = HashSet::new();
    for span in spans {
        let Some(session_id) = span.session_id.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        let project = span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID);
        if sessions.contains(&(project.to_string(), session_id.to_string())) {
            traces.insert((project.to_string(), span.trace_id.clone()));
        }
    }
    traces
}

#[cfg(test)]
mod session_fence_tests {
    use super::*;

    fn span(project: &str, trace: &str, id: &str, session: Option<&str>) -> NormalizedSpan {
        NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: id.to_string(),
            session_id: session.map(str::to_string),
            ..Default::default()
        }
    }

    /// A deleted session takes its trace's *whole* set of spans, not only the ones naming it.
    ///
    /// The shape that matters, and the one every framework produces: the session id sits on the root and on
    /// nothing else. Matching per span therefore dropped the root and kept the children, which recreated a
    /// deleted session's trace minus its head.
    #[test]
    fn a_deleted_session_takes_the_children_that_do_not_name_it() {
        let spans = vec![
            span("p", "t1", "root", Some("s1")),
            span("p", "t1", "child", None),
            span("p", "t1", "grandchild", None),
            span("p", "t2", "other-root", Some("s2")),
            span("p", "t2", "other-child", None),
        ];
        let deleted = HashSet::from([("p".to_string(), "s1".to_string())]);

        let traces = traces_of_sessions(&spans, &deleted);
        assert_eq!(
            traces,
            HashSet::from([("p".to_string(), "t1".to_string())]),
            "the deleted session must resolve to its whole trace"
        );

        let mut kept = spans.clone();
        kept.retain(|s| {
            !traces.contains(&(
                s.project_id
                    .as_deref()
                    .unwrap_or(DEFAULT_PROJECT_ID)
                    .to_string(),
                s.trace_id.clone(),
            ))
        });
        let ids: Vec<&str> = kept.iter().map(|s| s.span_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["other-root", "other-child"],
            "no span of a deleted session's trace may survive, and no other trace may be touched"
        );
    }

    /// A session id is scoped to its project, so the same id elsewhere is a different session.
    #[test]
    fn a_session_id_in_another_project_is_a_different_session() {
        let spans = vec![
            span("p", "t1", "root", Some("shared")),
            span("q", "t2", "root", Some("shared")),
        ];
        let deleted = HashSet::from([("p".to_string(), "shared".to_string())]);
        assert_eq!(
            traces_of_sessions(&spans, &deleted),
            HashSet::from([("p".to_string(), "t1".to_string())]),
            "a deletion in one project must not reach another project's session of the same name"
        );
    }
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
mod storable_timestamp_tests {
    use super::*;
    use chrono::TimeZone;

    fn span_at(secs: i64) -> NormalizedSpan {
        NormalizedSpan {
            trace_id: "t".to_string(),
            span_id: "s".to_string(),
            timestamp_start: chrono::Utc.timestamp_opt(secs, 0).single().expect("valid"),
            ..Default::default()
        }
    }

    /// A span dated past what the storage column can hold is dropped, not stored at the epoch.
    ///
    /// The epoch is the single worst destination: it is inside the retention window's *past*, so the row is
    /// deleted by the 90-day TTL shortly after the export was answered 200. The old conversion sent every
    /// timestamp beyond 2262 there.
    #[test]
    fn a_span_dated_beyond_the_storable_range_is_rejected() {
        // Year 10000, comfortably past `DateTime64(6)`'s 2299 ceiling and past the nanosecond range that
        // produced the epoch fallback.
        let mut spans = vec![span_at(253_402_300_800), span_at(1_704_067_200)];
        let dropped = drop_unstorable_spans(&mut spans);
        assert_eq!(dropped, 1, "the far-future span must be rejected");
        assert_eq!(spans.len(), 1, "the ordinary span must be kept");
        assert_eq!(
            spans[0].timestamp_start.timestamp(),
            1_704_067_200,
            "the surviving span must be untouched"
        );
    }

    /// An unstorable *end* time is rejected too: it is stored in its own column, with the same TTL.
    #[test]
    fn an_unstorable_end_time_rejects_the_span() {
        let mut span = span_at(1_704_067_200);
        span.timestamp_end = chrono::Utc.timestamp_opt(253_402_300_800, 0).single();
        let mut spans = vec![span];
        assert_eq!(drop_unstorable_spans(&mut spans), 1);
        assert!(spans.is_empty());
    }

    /// The ordinary case costs nothing, and the bounds themselves are inclusive.
    #[test]
    fn timestamps_inside_the_range_are_kept() {
        // 1900-01-01 and 2299-01-01, the two ends of the documented window.
        let mut spans = vec![span_at(-2_208_988_800), span_at(10_382_659_200)];
        assert_eq!(drop_unstorable_spans(&mut spans), 0);
        assert_eq!(spans.len(), 2);
    }
}

#[cfg(test)]
mod association_leak_tests {
    /// Every early return between creating a file association and committing the rows must release them.
    ///
    /// The invariant: **from the moment a file association exists until it is confirmed or released, no path
    /// may return.** A forgotten release is a *permanent* quota leak rather than a transient one - the orphan
    /// sweeper selects on `ref_count = 0`, so an association left above zero means the bytes are never
    /// reclaimed and the project's usable quota shrinks for good. Nothing user-visible happens, which is
    /// exactly why four such returns were written on two separate occasions without anyone noticing.
    ///
    /// Checked structurally, by reading this file, because that is the only form that catches the *next* one:
    /// a new early return added between these two calls compiles, passes every behavioural test, and leaks.
    /// An RAII guard was tried first and rejected - releasing is asynchronous, so `Drop` cannot do it, and
    /// the association set is read several times after the risky region, so a consuming guard does not fit
    /// the flow without restructuring the write path to add a safety net to it.
    ///
    /// The rule is deliberately coarse: *count* returns against releases inside the region. It cannot tell
    /// which release belongs to which return, and a maintainer can defeat it. What it cannot do is stay
    /// silent when someone adds a bare `return` to this region.
    #[test]
    fn every_early_return_between_files_and_the_write_releases_its_associations() {
        let source = include_str!("pipeline.rs");

        // The region is per path: each of the two write paths creates associations with
        // `persist_extracted_files` and ends the risky window at its own `write_to_duckdb`.
        let mut regions = Vec::new();
        let mut cursor = 0usize;
        while let Some(start) = source[cursor..].find("persist_extracted_files(") {
            let start = cursor + start;
            let Some(end) = source[start..].find("write_to_duckdb(") else {
                break;
            };
            let end = start + end;
            regions.push(&source[start..end]);
            cursor = end + "write_to_duckdb(".len();
        }
        assert!(
            regions.len() >= 2,
            "both write paths should have been found; the anchors have moved"
        );

        // Adjacency, not a count. Counting was tried first and is too weak: the region holds two *kinds* of
        // release call, so a total that includes them all leaves slack, and removing one real release still
        // satisfied the sum. Verified by reverting a release and watching the count-based form pass.
        const LOOKBACK: usize = 8;
        for (i, region) in regions.iter().enumerate() {
            let lines: Vec<&str> = region.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if !line.contains("return ") {
                    continue;
                }
                let from = n.saturating_sub(LOOKBACK);
                let discharged = lines[from..n].iter().any(|l| {
                    l.contains("release_created_associations(")
                        || l.contains("release_associations_of_dropped(")
                });
                assert!(
                    discharged,
                    "path {i}: `{}` returns between creating file associations and the write without \
                     releasing them within the preceding {LOOKBACK} lines. An unreleased association holds \
                     its project's quota forever, because the orphan sweeper only reclaims at zero \
                     references.",
                    line.trim()
                );
            }
        }
    }
}
