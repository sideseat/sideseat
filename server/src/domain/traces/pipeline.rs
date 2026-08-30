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

use std::collections::HashSet;
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
            // Subscribe with consumer group
            let mut subscriber = match topic.subscribe(CONSUMER_GROUP, &consumer).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to subscribe to trace topic");
                    return;
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
                            if self.run(&msg).await {
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
                let first = tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("TracePipeline received shutdown, draining...");
                            shutdown_requested = true;
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
                            Err(e) => {
                                tracing::error!(error = %e, "TracePipeline receive error");
                                break;
                            }
                        }
                    }
                    _ = claim_interval.tick() => {
                        // Periodically claim stuck messages from other consumers
                        self.claim_stuck_messages(&claimer, &acker, &consumer).await;
                        continue;
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
                            if self.run(&request).await {
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

        // Spans for a project being deleted are dropped, not written.
        //
        // This is the authoritative half of the project deletion fence, and admission at the HTTP edge
        // is only the fast half: a request passes admission, goes onto a topic, and persists seconds
        // later, so the claim can land in between. Checked here the check is adjacent to the write,
        // which is the only place it can be sound.
        //
        // Dropped rather than refused. A refusal invites a retry, and the project is not coming back -
        // the exporter would retry a doomed batch until it gave up, and a batch mixing a live project
        // with a dying one would lose the live project's spans too.
        let claimed = self.claimed_projects(&all_db_spans).await;
        if !claimed.is_empty() {
            let before = all_db_spans.len();
            all_db_spans.retain(|s| {
                !claimed.contains(s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            });
            all_pending_files.retain(|f| !claimed.contains(f.project_id.as_str()));
            all_incoming.retain(|(project, _, _)| !claimed.contains(project.as_str()));
            tracing::warn!(
                dropped = before - all_db_spans.len(),
                projects = ?claimed,
                "Dropped spans for projects being deleted"
            );
            if all_db_spans.is_empty() {
                return true;
            }
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
        let (unbacked, reconcile_failed) =
            reconcile_incoming_references(&all_incoming, &self.file_service).await;
        if reconcile_failed > 0 {
            self.file_cache.invalidate_all();
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
        let db_ok = write_to_duckdb(all_db_spans, &self.analytics).await;

        let t_persist_done = std::time::Instant::now();

        if db_ok {
            publish_sse_events(&sse_events, &self.topics).await;
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

    /// Which of a batch's projects are claimed for deletion.
    ///
    /// One query per distinct project, and a batch almost always names one - the SDK sends a project's
    /// spans to that project's endpoint. A lookup failure reports *not* claimed: dropping a live
    /// project's spans because a query failed would be a data loss caused by the fence, while writing
    /// to a dying project leaves rows the sweep's verification still catches.
    async fn claimed_projects(&self, spans: &[NormalizedSpan]) -> HashSet<String> {
        // Same default the write path applies, so the fence and the write agree on which project a
        // span without one belongs to.
        let mut projects: Vec<&str> = spans
            .iter()
            .map(|s| s.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            .collect();
        projects.sort_unstable();
        projects.dedup();
        let repo = self.file_service.database().repository();
        let mut claimed = HashSet::new();
        for project in projects {
            match repo.project_is_claimed(project).await {
                Ok(true) => {
                    claimed.insert(project.to_string());
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(project, error = %e, "Could not check the project deletion fence")
                }
            }
        }
        claimed
    }

    /// Run the complete pipeline for a single request (used during shutdown drain
    /// and claimed message recovery). File I/O is done inline for reliability.
    ///
    /// Returns true if the DuckDB write succeeded, false otherwise.
    async fn run(&self, request: &ExportTraceServiceRequest) -> bool {
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
                return false;
            }
        };
        if let Some((db_spans, pending_files, incoming)) = result {
            if db_spans.is_empty() {
                return true;
            }
            let sse_events: Vec<SseSpanEvent> = db_spans.iter().map(SseSpanEvent::from).collect();
            // Ordered, exactly as the batch path is: this path ran the two concurrently, so it could
            // commit a row referencing a file whose write had failed - the same defect, in the path
            // that runs at shutdown and recovery, where it is least likely to be noticed.
            let mut db_spans = db_spans;
            let files = persist_extracted_files(pending_files, &self.file_service).await;
            if files.failed > 0 {
                self.file_cache.invalidate_all();
                tracing::error!(
                    failed = files.failed,
                    "Refusing to commit spans whose extracted files could not be stored"
                );
                return false;
            }
            let mut unresolvable = files.quota_skipped;
            let (unbacked, reconcile_failed) =
                reconcile_incoming_references(&incoming, &self.file_service).await;
            if reconcile_failed > 0 {
                self.file_cache.invalidate_all();
                tracing::error!(
                    failed = reconcile_failed,
                    "Refusing the batch: could not settle whether some file references are backed"
                );
                return false;
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
            let db_ok = write_to_duckdb(db_spans, &self.analytics).await;
            if db_ok {
                publish_sse_events(&sse_events, &self.topics).await;
            }
            db_ok
        } else {
            true
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
