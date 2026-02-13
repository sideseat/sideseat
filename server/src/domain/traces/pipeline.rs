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
use super::extract::{extract_attributes_batch, extract_messages_batch};
use super::persist::persist_batch;
use crate::core::TopicService;
use crate::data::AnalyticsService;
use crate::data::files::FileService;
use crate::data::topics::{StreamTopic, TopicError};
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

                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("TracePipeline received shutdown, draining...");
                            shutdown_requested = true;
                        }
                    }
                    result = subscriber.recv() => {
                        match result {
                            Ok((msg_id, msg)) => {
                                self.run(&msg).await;
                                // Acknowledge after successful processing
                                if let Err(e) = acker.ack(&msg_id).await {
                                    tracing::warn!(error = %e, msg_id = %msg_id, "Failed to ack message");
                                }
                            }
                            Err(TopicError::Lagged(n)) => {
                                tracing::warn!(lagged = n, "TracePipeline lagged");
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

    /// Run the complete pipeline for a batch of traces.
    async fn run(&self, request: &ExportTraceServiceRequest) {
        // Stage 1a: Extract Attributes
        let spans = extract_attributes_batch(request);
        if spans.is_empty() {
            return;
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
        let enrichments = enrich_batch(&spans, &messages, &self.pricing);

        // Stage 4: Persist
        // - Extracts base64 files from messages, replaces with #!B64!# URIs
        // - Stores raw messages (not SideML) for data preservation
        // - SideML conversion happens at query time in feed pipeline (process_spans)
        // - SSE events published after DB write using NormalizedSpan
        persist_batch(
            request,
            spans,
            raw_messages,
            tool_definitions,
            tool_names,
            enrichments,
            &self.topics,
            &self.analytics,
            &self.file_service,
        )
        .await;
    }
}
