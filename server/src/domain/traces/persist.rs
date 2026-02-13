//! Trace persistence (Stage 4)
//!
//! Handles SSE publishing and DuckDB writes with retry logic.
//! Stores raw messages (not normalized) for data preservation.
//! SideML conversion happens at query time in feed pipeline (process_spans).
//! Builds raw span JSON from original OTLP request (deferred for performance).
//!
//! ## File Extraction
//!
//! Before persisting to DuckDB, base64 data >= 1KB is extracted from messages
//! and replaced with `#!B64!#[mime]::hash` URIs. Files are stored separately
//! with reference counting for cleanup.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::Span;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use super::enrich::SpanEnrichment;
use super::extract::files::extract_and_replace_files;
use super::extract::{RawMessage, RawToolDefinition, RawToolNames, SpanData};
use crate::core::constants::FILES_MAX_CONCURRENT_FINALIZATION;
use crate::core::{TopicMessage, TopicService};
use crate::data::AnalyticsService;
use crate::data::files::FileService;
use crate::data::types::NormalizedSpan;
use crate::utils::otlp::extract_attributes;
use crate::utils::retry::{DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS, retry_with_backoff_async};

// ============================================================================
// SSE EVENT MODEL
// ============================================================================

/// SSE event for notifying clients of new spans.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SseSpanEvent {
    pub project_id: Option<String>,
    pub trace_id: String,
    pub span_id: String,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
}

impl From<&NormalizedSpan> for SseSpanEvent {
    fn from(span: &NormalizedSpan) -> Self {
        Self {
            project_id: span.project_id.clone(),
            trace_id: span.trace_id.clone(),
            span_id: span.span_id.clone(),
            session_id: span.session_id.clone(),
            user_id: span.user_id.clone(),
        }
    }
}

impl TopicMessage for SseSpanEvent {
    fn size_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.project_id.as_ref().map_or(0, |s| s.len())
            + self.trace_id.len()
            + self.span_id.len()
            + self.session_id.as_ref().map_or(0, |s| s.len())
            + self.user_id.as_ref().map_or(0, |s| s.len())
    }
}

// ============================================================================
// BATCH PERSISTENCE
// ============================================================================

/// Persist data to DuckDB and SSE topics (Stage 4).
///
/// Combines spans with enrichments (costs, previews) and converts to DB format.
/// Builds raw span JSON from original OTLP request (deferred for performance).
/// SSE events are published only after successful DuckDB write.
///
/// ## File Extraction Flow
///
/// 1. Extract base64 files from messages, replace with #!B64!# URIs
/// 2. Write temp files for extracted content
/// 3. Upsert file metadata in SQLite
/// 4. Finalize files to permanent storage (synchronous)
/// 5. Write to DuckDB (messages now have #!B64!# URIs)
#[allow(clippy::too_many_arguments)]
pub(super) async fn persist_batch(
    request: &ExportTraceServiceRequest,
    spans: Vec<SpanData>,
    messages: Vec<Vec<RawMessage>>,
    tool_definitions: Vec<Vec<RawToolDefinition>>,
    tool_names: Vec<Vec<RawToolNames>>,
    enrichments: Vec<SpanEnrichment>,
    topics: &TopicService,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
) {
    // Extract files from messages if file storage is enabled
    // Files are finalized synchronously to ensure they exist before DuckDB write
    let processed_messages = if file_service.is_enabled() {
        extract_files_from_batch(messages, &spans, file_service).await
    } else {
        messages
    };

    // DuckDB Write - batch write with retry logic
    // Converts SpanData + Enrichment to NormalizedSpan, builds raw span JSON
    let mut db_spans = flatten(
        request,
        spans,
        processed_messages,
        tool_definitions,
        tool_names,
        enrichments,
    );

    // Extract files from raw_span as well (same base64 data may appear in OTLP attributes)
    if file_service.is_enabled() {
        extract_files_from_raw_spans(&mut db_spans, file_service).await;
    }

    write_to_duckdb(&db_spans, analytics).await;

    // SSE Publish - real-time updates to per-project topics (after persist)
    publish_to_sse(&db_spans, topics).await;
}

// ============================================================================
// SSE PUBLISHING
// ============================================================================

/// Publish spans to per-project SSE topics for real-time streaming.
///
/// Uses BroadcastTopic for distributed pub/sub. In Redis mode, events are
/// published via Redis Pub/Sub so all SSE endpoints receive them.
async fn publish_to_sse(spans: &[NormalizedSpan], topics: &TopicService) {
    for span in spans {
        if let Some(ref project_id) = span.project_id {
            let topic_name = format!("sse_spans:{}", project_id);
            let topic = topics.broadcast_topic::<SseSpanEvent>(&topic_name);
            let event = SseSpanEvent::from(span);
            if let Err(e) = topic.publish(&event).await {
                tracing::warn!(error = %e, project_id, "Failed to publish SSE event");
            }
        }
    }
}

// ============================================================================
// FILE EXTRACTION
// ============================================================================

/// File pending finalization to permanent storage
struct PendingFinalization {
    project_id: String,
    hash: String,
    temp_path: std::path::PathBuf,
}

/// Finalize pending files to permanent storage with bounded concurrency.
///
/// Uses `buffer_unordered` to limit concurrent I/O operations, preventing
/// resource exhaustion when processing batches with many files.
async fn finalize_pending_files(
    pending: Vec<PendingFinalization>,
    file_service: &Arc<FileService>,
    context: &str,
) {
    if pending.is_empty() {
        return;
    }

    let count = pending.len();
    let storage = file_service.storage();

    // Use buffer_unordered for bounded concurrency instead of unbounded join_all
    let results: Vec<_> = futures::stream::iter(pending.into_iter().map(|pf| {
        let storage = Arc::clone(storage);
        async move {
            let result = storage
                .finalize_temp(&pf.project_id, &pf.hash, &pf.temp_path)
                .await;
            (pf, result)
        }
    }))
    .buffer_unordered(FILES_MAX_CONCURRENT_FINALIZATION)
    .collect()
    .await;

    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    for (pf, result) in results {
        match result {
            Ok(()) => success_count += 1,
            Err(e) => {
                failure_count += 1;
                tracing::warn!(
                    error = %e,
                    hash = %pf.hash,
                    project_id = %pf.project_id,
                    "Failed to finalize file to permanent storage"
                );
            }
        }
    }

    if failure_count > 0 {
        tracing::debug!(
            finalized = success_count,
            failed = failure_count,
            total = count,
            context,
            "File finalization complete with failures"
        );
    } else if success_count > 0 {
        tracing::debug!(
            files = success_count,
            context,
            "Files finalized successfully"
        );
    }
}

/// Extract files from all messages in a batch.
///
/// For each span's messages:
/// 1. Extract base64 data >= 1KB
/// 2. Write temp file
/// 3. Upsert metadata in SQLite
/// 4. Record trace-file association
/// 5. Finalize all files to permanent storage (bounded parallel I/O)
///
/// Files are finalized in parallel (with concurrency limit) and confirmed
/// before returning. This prevents broken #!B64!# references while
/// maximizing throughput.
async fn extract_files_from_batch(
    mut messages: Vec<Vec<RawMessage>>,
    spans: &[SpanData],
    file_service: &Arc<FileService>,
) -> Vec<Vec<RawMessage>> {
    let temp_dir = file_service.temp_dir();
    let repo = file_service.database().repository();
    let mut pending_finalizations: Vec<PendingFinalization> = Vec::new();

    for (span_idx, span_messages) in messages.iter_mut().enumerate() {
        let span = &spans[span_idx];
        let project_id = span.project_id.as_deref().unwrap_or("default");
        let trace_id = &span.trace_id;

        // Track unique hashes per trace to avoid duplicate trace_files entries
        let mut trace_hashes: HashSet<String> = HashSet::new();

        for raw_message in span_messages.iter_mut() {
            // Extract files from message content
            let result = extract_and_replace_files(&mut raw_message.content);

            if result.modified {
                for file in result.files {
                    // Write temp file
                    let temp_path = temp_dir.join(format!("{}_{}", project_id, &file.hash));
                    if let Err(e) = tokio::fs::write(&temp_path, &file.data).await {
                        tracing::warn!(
                            error = %e,
                            hash = %file.hash,
                            project_id,
                            "Failed to write temp file, skipping"
                        );
                        continue;
                    }

                    // Upsert file metadata in database via repository trait
                    let upsert_result = repo
                        .upsert_file(
                            project_id,
                            &file.hash,
                            file.media_type.as_deref(),
                            file.size as i64,
                        )
                        .await
                        .map_err(|e| e.to_string());

                    if let Err(e) = upsert_result {
                        tracing::warn!(
                            error = %e,
                            hash = %file.hash,
                            project_id,
                            "Failed to upsert file metadata, cleaning up temp file"
                        );
                        // Clean up temp file since we can't track it
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        continue;
                    }

                    // Record trace-file association (deduplicated per trace)
                    if !trace_hashes.contains(&file.hash) {
                        trace_hashes.insert(file.hash.clone());
                        let insert_result = repo
                            .insert_trace_file(trace_id, project_id, &file.hash)
                            .await
                            .map_err(|e| e.to_string());

                        if let Err(e) = insert_result {
                            tracing::warn!(
                                error = %e,
                                hash = %file.hash,
                                trace_id,
                                "Failed to insert trace-file association"
                            );
                        }
                    }

                    // Queue for parallel finalization
                    pending_finalizations.push(PendingFinalization {
                        project_id: project_id.to_string(),
                        hash: file.hash,
                        temp_path,
                    });
                }
            }
        }
    }

    // Finalize all files with bounded concurrency
    finalize_pending_files(pending_finalizations, file_service, "messages").await;

    messages
}

/// Extract files from all JSON fields in normalized spans.
///
/// Extracts base64 from:
/// - raw_span: OTLP attributes/events
/// - tool_definitions: tool schemas (could have example data)
/// - metadata: user-provided metadata
///
/// Files are finalized with bounded concurrency and confirmed before returning.
/// This ensures ALL base64 is extracted from the database, not just messages.
async fn extract_files_from_raw_spans(
    db_spans: &mut [NormalizedSpan],
    file_service: &Arc<FileService>,
) {
    let temp_dir = file_service.temp_dir();
    let repo = file_service.database().repository();
    let mut pending_finalizations: Vec<PendingFinalization> = Vec::new();

    for span in db_spans.iter_mut() {
        let project_id = span.project_id.as_deref().unwrap_or("default");
        let trace_id = &span.trace_id;

        // Extract from all JSON fields that could contain base64
        let mut all_files = Vec::new();

        // raw_span
        let result = extract_and_replace_files(&mut span.raw_span);
        all_files.extend(result.files);

        // tool_definitions
        let result = extract_and_replace_files(&mut span.tool_definitions);
        all_files.extend(result.files);

        // metadata
        let result = extract_and_replace_files(&mut span.metadata);
        all_files.extend(result.files);

        for file in all_files {
            // Write temp file
            let temp_path = temp_dir.join(format!("{}_{}", project_id, &file.hash));
            if let Err(e) = tokio::fs::write(&temp_path, &file.data).await {
                tracing::warn!(
                    error = %e,
                    hash = %file.hash,
                    project_id,
                    "Failed to write temp file, skipping"
                );
                continue;
            }

            // Upsert file metadata in database via repository trait
            let upsert_result = repo
                .upsert_file(
                    project_id,
                    &file.hash,
                    file.media_type.as_deref(),
                    file.size as i64,
                )
                .await
                .map_err(|e| e.to_string());

            if let Err(e) = upsert_result {
                tracing::warn!(
                    error = %e,
                    hash = %file.hash,
                    project_id,
                    "Failed to upsert file metadata, cleaning up temp file"
                );
                // Clean up temp file since we can't track it
                let _ = tokio::fs::remove_file(&temp_path).await;
                continue;
            }

            // Record trace-file association
            let insert_result = repo
                .insert_trace_file(trace_id, project_id, &file.hash)
                .await
                .map_err(|e| e.to_string());

            if let Err(e) = insert_result {
                tracing::warn!(
                    error = %e,
                    hash = %file.hash,
                    trace_id,
                    "Failed to insert trace-file association"
                );
            }

            // Queue for parallel finalization
            pending_finalizations.push(PendingFinalization {
                project_id: project_id.to_string(),
                hash: file.hash,
                temp_path,
            });
        }
    }

    // Finalize all files with bounded concurrency
    finalize_pending_files(pending_finalizations, file_service, "raw_spans").await;
}

// ============================================================================
// DUCKDB WRITES
// ============================================================================

/// Write batch to analytics backend with exponential backoff retry.
async fn write_to_duckdb(spans: &[NormalizedSpan], analytics: &Arc<AnalyticsService>) {
    let span_count = spans.len();
    let repo = analytics.repository();

    let result = retry_with_backoff_async(DEFAULT_MAX_ATTEMPTS, DEFAULT_BASE_DELAY_MS, || {
        repo.insert_spans(spans)
    })
    .await;

    match result {
        Ok(attempts) => {
            if attempts > 1 {
                tracing::trace!(
                    spans = span_count,
                    attempts,
                    "Wrote traces to analytics backend after retry"
                );
            } else {
                tracing::trace!(spans = span_count, "Wrote traces to analytics backend");
            }
        }
        Err((e, attempts)) => {
            tracing::error!(
                error = %e,
                spans = span_count,
                attempts,
                "Failed to write spans to analytics backend after retries"
            );
        }
    }
}

// ============================================================================
// FLATTEN TO DB FORMAT
// ============================================================================

/// Flatten raw messages, tool definitions, tool names, and enrichments into DB-ready format.
///
/// Iterates request in same order as normalize_batch to match spans with OTLP data.
/// Builds raw span JSON directly from request (no lookup needed).
///
/// # Panics
/// Debug assertion fails if span counts don't match (indicates pipeline bug).
fn flatten(
    request: &ExportTraceServiceRequest,
    span_data: Vec<SpanData>,
    messages: Vec<Vec<RawMessage>>,
    tool_definitions: Vec<Vec<RawToolDefinition>>,
    tool_names: Vec<Vec<RawToolNames>>,
    enrichments: Vec<SpanEnrichment>,
) -> Vec<NormalizedSpan> {
    let span_count = span_data.len();
    let mut result = Vec::with_capacity(span_count);
    let mut iter = span_data
        .into_iter()
        .zip(messages)
        .zip(tool_definitions)
        .zip(tool_names)
        .zip(enrichments);

    // Iterate request in same order as normalize_batch
    for resource_spans in &request.resource_spans {
        let resource_attrs = resource_spans
            .resource
            .as_ref()
            .map(|r| extract_attributes(&r.attributes))
            .unwrap_or_default();

        for scope_spans in &resource_spans.scope_spans {
            for otlp_span in &scope_spans.spans {
                if let Some(((((span, msgs), tools), tnames), enrichment)) = iter.next() {
                    let messages_json =
                        serde_json::to_value(&msgs).unwrap_or(JsonValue::Array(vec![]));

                    // Flatten tool definitions: extract content arrays and merge
                    let tool_definitions_json = flatten_tool_definitions(&tools);

                    // Flatten tool names: extract content arrays and merge into flat string list
                    let tool_names_json = flatten_tool_names(&tnames);

                    let raw_span = build_raw_span_json(otlp_span, &resource_attrs);
                    result.push(to_normalized_span(
                        span,
                        &enrichment,
                        messages_json,
                        tool_definitions_json,
                        tool_names_json,
                        raw_span,
                    ));
                }
            }
        }
    }

    debug_assert_eq!(
        result.len(),
        span_count,
        "Span count mismatch: expected {}, got {}",
        span_count,
        result.len()
    );

    result
}

/// Flatten tool definitions: extract content from each RawToolDefinition and merge arrays.
/// Input: `[{source: {...}, content: [def1, def2]}, {source: {...}, content: [def3]}]`
/// Output: `[def1, def2, def3]`
fn flatten_tool_definitions(tools: &[RawToolDefinition]) -> JsonValue {
    let mut result: Vec<JsonValue> = Vec::new();
    for tool in tools {
        if let Some(arr) = tool.content.as_array() {
            result.extend(arr.iter().cloned());
        } else {
            result.push(tool.content.clone());
        }
    }
    JsonValue::Array(result)
}

/// Flatten tool names: extract content from each RawToolNames and merge into flat string list.
/// Input: `[{source: {...}, content: ["tool1", "tool2"]}, {source: {...}, content: ["tool3"]}]`
/// Output: `["tool1", "tool2", "tool3"]`
fn flatten_tool_names(tnames: &[RawToolNames]) -> JsonValue {
    let mut result: Vec<JsonValue> = Vec::new();
    for tname in tnames {
        if let Some(arr) = tname.content.as_array() {
            result.extend(arr.iter().cloned());
        } else {
            result.push(tname.content.clone());
        }
    }
    JsonValue::Array(result)
}

/// Convert SpanData to NormalizedSpan and apply enrichment.
fn to_normalized_span(
    span: SpanData,
    enrichment: &SpanEnrichment,
    messages: JsonValue,
    tool_definitions: JsonValue,
    tool_names: JsonValue,
    raw_span: JsonValue,
) -> NormalizedSpan {
    NormalizedSpan {
        // Identity
        project_id: span.project_id,
        trace_id: span.trace_id,
        span_id: span.span_id,
        parent_span_id: span.parent_span_id,
        trace_state: span.trace_state,

        // Session and user
        session_id: span.session_id,
        user_id: span.user_id,

        // Naming and classification
        span_name: span.span_name,
        span_kind: span.span_kind,
        span_category: span.span_category,
        observation_type: span.observation_type,
        framework: span.framework,
        status_code: span.status_code,
        status_message: span.status_message,
        exception_type: span.exception_type,
        exception_message: span.exception_message,
        exception_stacktrace: span.exception_stacktrace,

        // Time
        timestamp_start: span.timestamp_start,
        timestamp_end: span.timestamp_end,
        duration_ms: span.duration_ms,

        // Environment
        environment: span.environment,

        // GenAI core fields
        gen_ai_system: span.gen_ai_system,
        gen_ai_operation_name: span.gen_ai_operation_name,
        gen_ai_request_model: span.gen_ai_request_model,
        gen_ai_response_model: span.gen_ai_response_model,
        gen_ai_response_id: span.gen_ai_response_id,

        // GenAI request parameters
        gen_ai_temperature: span.gen_ai_temperature,
        gen_ai_top_p: span.gen_ai_top_p,
        gen_ai_top_k: span.gen_ai_top_k,
        gen_ai_max_tokens: span.gen_ai_max_tokens,
        gen_ai_frequency_penalty: span.gen_ai_frequency_penalty,
        gen_ai_presence_penalty: span.gen_ai_presence_penalty,
        gen_ai_stop_sequences: span.gen_ai_stop_sequences,

        // GenAI response
        gen_ai_finish_reasons: span.gen_ai_finish_reasons,

        // GenAI agent fields
        gen_ai_agent_id: span.gen_ai_agent_id,
        gen_ai_agent_name: span.gen_ai_agent_name,

        // GenAI tool fields
        gen_ai_tool_name: span.gen_ai_tool_name,
        gen_ai_tool_call_id: span.gen_ai_tool_call_id,

        // GenAI performance metrics
        gen_ai_server_ttft_ms: span.gen_ai_server_ttft_ms,
        gen_ai_server_request_duration_ms: span.gen_ai_server_request_duration_ms,

        // Token usage
        gen_ai_usage_input_tokens: span.gen_ai_usage_input_tokens,
        gen_ai_usage_output_tokens: span.gen_ai_usage_output_tokens,
        gen_ai_usage_total_tokens: span.gen_ai_usage_total_tokens,
        gen_ai_usage_cache_read_tokens: span.gen_ai_usage_cache_read_tokens,
        gen_ai_usage_cache_write_tokens: span.gen_ai_usage_cache_write_tokens,
        gen_ai_usage_reasoning_tokens: span.gen_ai_usage_reasoning_tokens,
        gen_ai_usage_details: span.gen_ai_usage_details,

        // Enrichment data (costs)
        gen_ai_cost_input: enrichment.input_cost,
        gen_ai_cost_output: enrichment.output_cost,
        gen_ai_cost_cache_read: enrichment.cache_read_cost,
        gen_ai_cost_cache_write: enrichment.cache_write_cost,
        gen_ai_cost_reasoning: enrichment.reasoning_cost,
        gen_ai_cost_total: enrichment.total_cost,

        // Enrichment data (previews)
        input_preview: enrichment.input_preview.clone(),
        output_preview: enrichment.output_preview.clone(),

        // External services
        http_method: span.http_method,
        http_url: span.http_url,
        http_status_code: span.http_status_code,

        db_system: span.db_system,
        db_name: span.db_name,
        db_operation: span.db_operation,
        db_statement: span.db_statement,

        storage_system: span.storage_system,
        storage_bucket: span.storage_bucket,
        storage_object: span.storage_object,

        messaging_system: span.messaging_system,
        messaging_destination: span.messaging_destination,

        // Tags and metadata
        tags: span.tags,
        metadata: span.metadata,

        // Raw messages (converted to SideML on query)
        messages,

        // Raw tool definitions (separate from conversation messages)
        tool_definitions,

        // Raw tool names (list of tool names, separate from full definitions)
        tool_names,

        // Raw span JSON (includes attributes and resource.attributes)
        raw_span,

        // Ingestion time (populated by DB default, not set during span creation)
        ingested_at: None,
    }
}

// ============================================================================
// RAW SPAN JSON BUILDING
// ============================================================================

/// Convert nanoseconds since epoch to ISO 8601 timestamp string
fn nanos_to_iso(nanos: u64) -> String {
    let secs = (nanos / 1_000_000_000) as i64;
    let nsecs = (nanos % 1_000_000_000) as u32;
    DateTime::<Utc>::from_timestamp(secs, nsecs)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .unwrap_or_default()
}

/// Build a raw JSON representation of an OTLP span for archival.
/// Uses ordered map for better readability (identity -> timing -> content -> metadata).
fn build_raw_span_json(span: &Span, resource_attrs: &HashMap<String, String>) -> JsonValue {
    let mut map = serde_json::Map::new();

    // Identity fields first
    map.insert("trace_id".into(), json!(hex::encode(&span.trace_id)));
    map.insert("span_id".into(), json!(hex::encode(&span.span_id)));
    map.insert(
        "parent_span_id".into(),
        if span.parent_span_id.is_empty() {
            JsonValue::Null
        } else {
            json!(hex::encode(&span.parent_span_id))
        },
    );
    map.insert("name".into(), json!(&span.name));
    map.insert("kind".into(), json!(span.kind));

    // Timing
    map.insert(
        "start_time_unix_nano".into(),
        json!(span.start_time_unix_nano),
    );
    map.insert("end_time_unix_nano".into(), json!(span.end_time_unix_nano));

    // Status
    map.insert(
        "status".into(),
        span.status
            .as_ref()
            .map(|s| {
                let mut status_map = serde_json::Map::new();
                status_map.insert("code".into(), json!(s.code));
                status_map.insert("message".into(), json!(&s.message));
                JsonValue::Object(status_map)
            })
            .unwrap_or(JsonValue::Null),
    );

    // Attributes
    map.insert("attributes".into(), build_attributes_raw(&span.attributes));

    // Events (with ordered fields)
    let events: Vec<JsonValue> = span
        .events
        .iter()
        .map(|e| {
            let mut event_map = serde_json::Map::new();
            event_map.insert("name".into(), json!(&e.name));
            event_map.insert("timestamp".into(), json!(nanos_to_iso(e.time_unix_nano)));
            event_map.insert("attributes".into(), build_attributes_raw(&e.attributes));
            event_map.insert(
                "dropped_attributes_count".into(),
                json!(e.dropped_attributes_count),
            );
            JsonValue::Object(event_map)
        })
        .collect();
    map.insert("events".into(), json!(events));

    // Links (with ordered fields)
    let links: Vec<JsonValue> = span
        .links
        .iter()
        .map(|l| {
            let mut link_map = serde_json::Map::new();
            link_map.insert("trace_id".into(), json!(hex::encode(&l.trace_id)));
            link_map.insert("span_id".into(), json!(hex::encode(&l.span_id)));
            link_map.insert("trace_state".into(), json!(&l.trace_state));
            link_map.insert("attributes".into(), build_attributes_raw(&l.attributes));
            link_map.insert("flags".into(), json!(l.flags));
            link_map.insert(
                "dropped_attributes_count".into(),
                json!(l.dropped_attributes_count),
            );
            JsonValue::Object(link_map)
        })
        .collect();
    map.insert("links".into(), json!(links));

    // Resource
    let mut resource_map = serde_json::Map::new();
    resource_map.insert(
        "attributes".into(),
        build_resource_attributes(resource_attrs),
    );
    map.insert("resource".into(), JsonValue::Object(resource_map));

    // Metadata (less important, at the end)
    map.insert(
        "trace_state".into(),
        if span.trace_state.is_empty() {
            JsonValue::Null
        } else {
            json!(&span.trace_state)
        },
    );
    map.insert("flags".into(), json!(span.flags));
    map.insert(
        "dropped_attributes_count".into(),
        json!(span.dropped_attributes_count),
    );
    map.insert(
        "dropped_events_count".into(),
        json!(span.dropped_events_count),
    );
    map.insert(
        "dropped_links_count".into(),
        json!(span.dropped_links_count),
    );

    JsonValue::Object(map)
}

/// Build JSON from attributes HashMap (for resource attributes)
fn build_resource_attributes(attrs: &HashMap<String, String>) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> =
        attrs.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    JsonValue::Object(map)
}

/// Build JSON from raw KeyValue attributes (preserves types)
fn build_attributes_raw(attrs: &[KeyValue]) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .map(|v| (kv.key.clone(), any_value_to_json(v)))
        })
        .collect();
    JsonValue::Object(map)
}

/// Convert AnyValue to JSON value (preserves types)
fn any_value_to_json(value: &AnyValue) -> JsonValue {
    match &value.value {
        Some(any_value::Value::StringValue(s)) => json!(s),
        Some(any_value::Value::BoolValue(b)) => json!(b),
        Some(any_value::Value::IntValue(i)) => json!(i),
        Some(any_value::Value::DoubleValue(d)) => json!(d),
        Some(any_value::Value::ArrayValue(arr)) => {
            json!(arr.values.iter().map(any_value_to_json).collect::<Vec<_>>())
        }
        Some(any_value::Value::KvlistValue(kvlist)) => {
            let map: serde_json::Map<String, JsonValue> = kvlist
                .values
                .iter()
                .filter_map(|kv| {
                    kv.value
                        .as_ref()
                        .map(|v| (kv.key.clone(), any_value_to_json(v)))
                })
                .collect();
            JsonValue::Object(map)
        }
        Some(any_value::Value::BytesValue(b)) => json!(hex::encode(b)),
        None => JsonValue::Null,
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::traces::MessageSource;
    use chrono::Utc;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
    use serde_json::json;

    fn make_span(id: &str) -> SpanData {
        SpanData {
            project_id: Some("test-project".to_string()),
            trace_id: "trace1".to_string(),
            span_id: id.to_string(),
            span_name: format!("span-{}", id),
            timestamp_start: Utc::now(),
            ..Default::default()
        }
    }

    fn make_otlp_span(id: &str) -> Span {
        Span {
            trace_id: b"trace1__________".to_vec(),
            span_id: id.as_bytes().to_vec(),
            name: format!("span-{}", id),
            ..Default::default()
        }
    }

    fn make_request(span_count: usize) -> ExportTraceServiceRequest {
        let spans: Vec<Span> = (0..span_count)
            .map(|i| make_otlp_span(&format!("span{}", i + 1)))
            .collect();
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    fn make_raw_message(content: &str) -> RawMessage {
        RawMessage {
            source: MessageSource::Attribute {
                key: "test".to_string(),
                time: Utc::now(),
            },
            content: json!({
                "role": "user",
                "content": content
            }),
        }
    }

    fn make_enrichment() -> SpanEnrichment {
        SpanEnrichment::default()
    }

    #[test]
    fn test_flatten_empty() {
        let request = ExportTraceServiceRequest::default();
        let spans: Vec<SpanData> = vec![];
        let messages: Vec<Vec<RawMessage>> = vec![];
        let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![];
        let tool_names: Vec<Vec<RawToolNames>> = vec![];
        let enrichments: Vec<SpanEnrichment> = vec![];
        let result = flatten(
            &request,
            spans,
            messages,
            tool_definitions,
            tool_names,
            enrichments,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn test_flatten_single_span_no_messages() {
        let request = make_request(1);
        let spans = vec![make_span("span1")];
        let messages: Vec<Vec<RawMessage>> = vec![vec![]];
        let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
        let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
        let enrichments = vec![make_enrichment()];
        let result = flatten(
            &request,
            spans,
            messages,
            tool_definitions,
            tool_names,
            enrichments,
        );

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].span_id, "span1");
        assert_eq!(result[0].messages, json!([]));
        assert_eq!(result[0].tool_definitions, json!([]));
        assert_eq!(result[0].tool_names, json!([]));
    }

    #[test]
    fn test_flatten_multiple_spans() {
        let request = make_request(3);
        let spans = vec![make_span("span1"), make_span("span2"), make_span("span3")];
        let messages: Vec<Vec<RawMessage>> = vec![vec![], vec![], vec![]];
        let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![], vec![], vec![]];
        let tool_names: Vec<Vec<RawToolNames>> = vec![vec![], vec![], vec![]];
        let enrichments = vec![make_enrichment(), make_enrichment(), make_enrichment()];
        let result = flatten(
            &request,
            spans,
            messages,
            tool_definitions,
            tool_names,
            enrichments,
        );

        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_flatten_stores_raw_messages() {
        let request = make_request(1);
        let spans = vec![make_span("span1")];
        let messages = vec![vec![
            make_raw_message("Hello"),
            make_raw_message("Hi there"),
        ]];
        let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
        let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
        let enrichments = vec![make_enrichment()];

        let result = flatten(
            &request,
            spans,
            messages,
            tool_definitions,
            tool_names,
            enrichments,
        );

        assert_eq!(result.len(), 1);

        // Verify messages are stored as JSON array with raw structure
        let stored_messages = result[0].messages.as_array().unwrap();
        assert_eq!(stored_messages.len(), 2);
        // Raw messages have "content" field with original message data
        assert_eq!(stored_messages[0]["content"]["content"], "Hello");
        assert_eq!(stored_messages[1]["content"]["content"], "Hi there");
    }

    #[test]
    fn test_flatten_applies_enrichment() {
        let request = make_request(1);
        let spans = vec![make_span("span1")];
        let messages: Vec<Vec<RawMessage>> = vec![vec![]];
        let tool_definitions: Vec<Vec<RawToolDefinition>> = vec![vec![]];
        let tool_names: Vec<Vec<RawToolNames>> = vec![vec![]];
        let enrichments = vec![SpanEnrichment {
            input_cost: 0.001,
            output_cost: 0.002,
            total_cost: 0.003,
            input_preview: Some("Hello".to_string()),
            output_preview: Some("Hi".to_string()),
            ..Default::default()
        }];

        let result = flatten(
            &request,
            spans,
            messages,
            tool_definitions,
            tool_names,
            enrichments,
        );

        assert_eq!(result[0].gen_ai_cost_input, 0.001);
        assert_eq!(result[0].gen_ai_cost_output, 0.002);
        assert_eq!(result[0].gen_ai_cost_total, 0.003);
        assert_eq!(result[0].input_preview, Some("Hello".to_string()));
        assert_eq!(result[0].output_preview, Some("Hi".to_string()));
    }
}
