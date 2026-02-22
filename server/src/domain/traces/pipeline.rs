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

use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::enrich::enrich_batch;
use super::extract::files::FileExtractionCache;
use super::extract::{extract_attributes_batch, extract_messages_batch};
use super::persist::{
    PendingFileWrite, prepare_batch, persist_extracted_files, persist_prepared,
};
use crate::core::TopicService;
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
const PIPELINE_BATCH_MAX_SIZE: usize = 256;

/// Timeout for collecting additional messages into a batch (microseconds)
const PIPELINE_BATCH_DRAIN_TIMEOUT_US: u64 = 500;

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
    /// Avoids redundant decode + SHA-256 for repeated images across spans/batches.
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
                            self.run(&msg).await;
                            if let Err(e) = acker.ack(&msg_id).await {
                                tracing::warn!(error = %e, msg_id = %msg_id, "Failed to ack during drain");
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
                self.run_batch(&requests).await;

                // Phase 4: Acknowledge all messages
                for msg_id in &msg_ids {
                    if let Err(e) = acker.ack(msg_id).await {
                        tracing::warn!(error = %e, msg_id = %msg_id, "Failed to ack message");
                    }
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
                            self.run(&request).await;
                            if let Err(e) = acker.ack(&msg.id).await {
                                tracing::warn!(error = %e, msg_id = %msg.id, "Failed to ack claimed message");
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
    /// base64 extraction are all CPU-bound), then batches DuckDB write + SSE.
    /// File I/O is spawned as a background task.
    async fn run_batch(&self, requests: &[ExportTraceServiceRequest]) {
        let t_batch_start = std::time::Instant::now();

        let pricing = &self.pricing;
        let files_enabled = self.file_service.is_enabled();
        let file_cache = &self.file_cache;

        // Process requests in parallel using scoped threads.
        // base64 extraction can take 100ms-1s per request for image-heavy spans,
        // so parallel processing across CPU cores significantly reduces batch time.
        // The FileExtractionCache is shared across threads (RwLock) to skip
        // redundant decode + SHA-256 for the same base64 content.
        let results: Vec<Option<(Vec<NormalizedSpan>, Vec<PendingFileWrite>)>> =
            tokio::task::block_in_place(|| {
                let num_workers = std::thread::available_parallelism()
                    .map(|p| p.get())
                    .unwrap_or(4);

                if requests.len() <= num_workers {
                    // Few requests: one thread per request
                    std::thread::scope(|s| {
                        let handles: Vec<_> = requests
                            .iter()
                            .map(|request| {
                                s.spawn(|| {
                                    process_request(request, pricing, files_enabled, file_cache)
                                })
                            })
                            .collect();
                        handles
                            .into_iter()
                            .map(|h| h.join().expect("process_request panicked"))
                            .collect()
                    })
                } else {
                    // Many requests: chunk into worker-sized groups
                    let chunk_size = requests.len().div_ceil(num_workers);
                    std::thread::scope(|s| {
                        let handles: Vec<_> = requests
                            .chunks(chunk_size)
                            .map(|chunk| {
                                s.spawn(|| {
                                    chunk
                                        .iter()
                                        .map(|request| {
                                            process_request(
                                                request,
                                                pricing,
                                                files_enabled,
                                                file_cache,
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                })
                            })
                            .collect();

                        handles
                            .into_iter()
                            .flat_map(|h| h.join().expect("process_request panicked"))
                            .collect()
                    })
                }
            });

        let mut all_db_spans: Vec<NormalizedSpan> = Vec::new();
        let mut all_pending_files: Vec<PendingFileWrite> = Vec::new();
        for (db_spans, pending_files) in results.into_iter().flatten() {
            all_db_spans.extend(db_spans);
            all_pending_files.extend(pending_files);
        }

        if all_db_spans.is_empty() {
            return;
        }

        let t_prepare_done = std::time::Instant::now();
        let span_count = all_db_spans.len();

        // Single DuckDB write + SSE publish for entire batch
        persist_prepared(all_db_spans, &self.topics, &self.analytics).await;

        let t_persist_done = std::time::Instant::now();

        // File I/O in background (doesn't block pipeline for next batch)
        if !all_pending_files.is_empty() {
            let file_service = Arc::clone(&self.file_service);
            let file_count = all_pending_files.len();
            tracing::debug!(files = file_count, "Spawning background file persistence");
            tokio::spawn(async move {
                persist_extracted_files(all_pending_files, &file_service).await;
            });
        }

        tracing::debug!(
            requests = requests.len(),
            spans = span_count,
            prepare_ms = t_prepare_done.duration_since(t_batch_start).as_millis() as u64,
            persist_ms = t_persist_done.duration_since(t_prepare_done).as_millis() as u64,
            total_ms = t_persist_done.duration_since(t_batch_start).as_millis() as u64,
            "Pipeline batch completed"
        );
    }

    /// Run the complete pipeline for a single request (used during shutdown drain
    /// and claimed message recovery). File I/O is done inline for reliability.
    async fn run(&self, request: &ExportTraceServiceRequest) {
        let result = process_request(
            request,
            &self.pricing,
            self.file_service.is_enabled(),
            &self.file_cache,
        );
        if let Some((db_spans, pending_files)) = result {
            if db_spans.is_empty() {
                return;
            }
            persist_prepared(db_spans, &self.topics, &self.analytics).await;
            persist_extracted_files(pending_files, &self.file_service).await;
        }
    }
}

// ============================================================================
// PER-REQUEST PROCESSING (free function for thread safety)
// ============================================================================

/// Process a single OTLP request through stages 1-4.
///
/// Pure CPU work: extract attributes, messages, sideml, enrich, prepare.
/// Returns NormalizedSpans + pending file writes, or None if no spans.
fn process_request(
    request: &ExportTraceServiceRequest,
    pricing: &PricingService,
    files_enabled: bool,
    file_cache: &FileExtractionCache,
) -> Option<(Vec<NormalizedSpan>, Vec<PendingFileWrite>)> {
    // Stage 1a: Extract Attributes
    let spans = extract_attributes_batch(request);
    if spans.is_empty() {
        return None;
    }

    // Stage 1b: Extract Messages, Tool Definitions, and Tool Names
    let (raw_messages, tool_definitions, tool_names) = extract_messages_batch(request, &spans);

    // Debug: Log raw_messages counts for VercelAISDK
    for (i, (span, raw_msgs)) in spans.iter().zip(raw_messages.iter()).enumerate() {
        if matches!(
            span.framework,
            Some(crate::data::types::Framework::VercelAISdk)
        ) {
            tracing::debug!(
                idx = i,
                span_id = %span.span_id,
                span_name = %span.span_name,
                raw_msgs_count = raw_msgs.len(),
                "VercelAISDK: after extract_messages_batch"
            );
        }
    }

    // Stage 2: SideML Conversion
    let messages = to_sideml_batch(&raw_messages);

    // Debug: Log SideML messages counts for VercelAISDK
    for (i, (span, sideml_msgs)) in spans.iter().zip(messages.iter()).enumerate() {
        if matches!(
            span.framework,
            Some(crate::data::types::Framework::VercelAISdk)
        ) {
            tracing::debug!(
                idx = i,
                span_id = %span.span_id,
                span_name = %span.span_name,
                sideml_msgs_count = sideml_msgs.len(),
                "VercelAISDK: after to_sideml_batch"
            );
        }
    }

    // Stage 3: Enrich
    let enrichments = enrich_batch(&spans, &messages, pricing);

    // Stage 4: Prepare (CPU-only file extraction + flatten to NormalizedSpan)
    let (db_spans, pending_files) = prepare_batch(
        request,
        spans,
        raw_messages,
        tool_definitions,
        tool_names,
        enrichments,
        files_enabled,
        Some(file_cache),
    );

    Some((db_spans, pending_files))
}
