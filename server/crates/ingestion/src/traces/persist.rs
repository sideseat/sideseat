//! Trace persistence.
//!
//! Handles analytics writes, file associations, and SSE publication.
//! Stores raw messages (not normalized) for data preservation.
//! SideML conversion happens at query time in feed pipeline (process_spans).
//! Builds raw span JSON from original OTLP request (deferred for performance).
//!
//! ## File Extraction
//!
//! Before persisting analytics rows, eligible base64 data is extracted from messages
//! and replaced with `#!B64!#[mime]::hash` URIs. Files are stored separately
//! with reference counting for cleanup.

use std::collections::{HashMap, HashSet};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::Span;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use super::enrich::SpanEnrichment;
use super::extract::files::{
    FileExtractionCache, extract_and_replace_files, extract_and_replace_files_cached,
};
use super::extract::{RawMessage, RawToolDefinition, RawToolNames, SpanData};
// The analytics port keeps ingestion independent of concrete adapters.
use crate::otlp::build_attributes_json;
use sideseat_core::constants::DEFAULT_PROJECT_ID;
use sideseat_core::utils::retry::{DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS};
use sideseat_core::utils::time::nanos_to_iso;
use sideseat_domain::files::collect_file_references_in_str;
use sideseat_messaging::TopicService;

use crate::media::{IncomingReference, PendingFileWrite, to_pending_files};
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::{NormalizedSpan, json_to_pre_serialized};

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

// ============================================================================
// BATCH PERSISTENCE
// ============================================================================

/// Extracted data from a batch of spans, ready for flattening and persistence.
pub(super) struct BatchInput {
    pub spans: Vec<SpanData>,
    pub messages: Vec<Vec<RawMessage>>,
    pub tool_definitions: Vec<Vec<RawToolDefinition>>,
    pub tool_names: Vec<Vec<RawToolNames>>,
    pub enrichments: Vec<SpanEnrichment>,
}

/// Prepare spans for persistence: file extraction (CPU only) + flatten.
///
/// Returns NormalizedSpans ready for DuckDB write, plus pending file writes
/// that should be persisted in a background task via `persist_extracted_files`.
///
/// File extraction (CPU phase) replaces base64 data with `#!B64!#` URIs
/// in the JSON fields so DuckDB stores compact references, not raw base64.
/// The actual file I/O (temp write, metadata upsert, finalization) is deferred.
pub(super) fn prepare_batch(
    request: &ExportTraceServiceRequest,
    input: BatchInput,
    files_enabled: bool,
    file_cache: Option<&FileExtractionCache>,
) -> (
    Vec<NormalizedSpan>,
    Vec<PendingFileWrite>,
    Vec<IncomingReference>,
) {
    let mut pending_files = Vec::new();
    let mut incoming_references = Vec::new();

    // Extract files from messages (CPU only: replace base64 with URIs)
    let processed_messages = if files_enabled {
        let (msgs, files) = extract_files_cpu_messages(input.messages, &input.spans, file_cache);
        pending_files = files;
        msgs
    } else {
        input.messages
    };

    // Convert SpanData + Enrichment to NormalizedSpan, build raw span JSON.
    // File extraction from the tool definitions and the metadata is done inline
    // BEFORE serialization, eliminating the serialize→deserialize→re-serialize round-trip.
    let (db_spans, extracted_files) = flatten(
        request,
        input.spans,
        processed_messages,
        input.tool_definitions,
        input.tool_names,
        input.enrichments,
        files_enabled,
        file_cache,
    );
    pending_files.extend(extracted_files);

    // References that arrived already formed, read from the rows *about to be written* rather than from
    // any one extraction step.
    //
    // A reference can appear in messages, tool definitions or metadata, and each is
    // extracted by a different path - so collecting per path missed whichever path was not covered.
    // Scanning the committed strings states the invariant directly: every reference in a row that is
    // written is either one this batch produced, or one that has been verified.
    // Keyed by `(project, trace, uri)`, not by URI alone.
    //
    // Global, one trace extracting URI `U` exempted *every* row containing `U` from reconciliation - so
    // a trace that supplied `U` already formed got no association, and cross-project it could be
    // dangling immediately. Same project, it survived only until the trace that really produced it was
    // deleted.
    let ours: HashSet<(&str, &str, String)> = pending_files
        .iter()
        .map(|f| {
            (
                f.project_id.as_str(),
                f.trace_id.as_str(),
                sideseat_core::utils::file_uri::build_file_uri(&f.hash, f.media_type.as_deref()),
            )
        })
        .collect();
    for span in &db_spans {
        let project_id = span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID);
        let mut present = Vec::new();
        for field in [
            span.messages.as_deref(),
            span.tool_definitions.as_deref(),
            span.metadata.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            collect_file_references_in_str(field, &mut present);
        }
        for uri in present {
            // The whole URI, not the hash: one carrying our hash with a different media type is not
            // ours, and it would also miss the exact-string rewrite if it turned out unbacked.
            if !ours.contains(&(project_id, span.trace_id.as_str(), uri.clone())) {
                incoming_references.push((project_id.to_string(), span.trace_id.clone(), uri));
            }
        }
    }
    incoming_references.sort_unstable();
    incoming_references.dedup();

    (db_spans, pending_files, incoming_references)
}

// ============================================================================
// SSE PUBLISHING
// ============================================================================

/// Publish pre-built SSE events to per-project topics for real-time streaming.
///
/// Uses BroadcastTopic for distributed pub/sub. In Redis mode, events are
/// published via Redis Pub/Sub so all SSE endpoints receive them.
///
/// SSE events are built before DuckDB write so clients only receive notifications
/// after both the DuckDB write and file persistence are complete.
pub(super) async fn publish_sse_events(events: &[SseSpanEvent], topics: &TopicService) {
    for event in events {
        if let Some(ref project_id) = event.project_id {
            let topic = topics.broadcast_topic::<SseSpanEvent>(
                sideseat_ports::queue::broadcast_kinds::SSE_SPANS,
                project_id,
            );
            if let Err(e) = topic.publish(event).await {
                tracing::warn!(error = %e, project_id, "Failed to publish SSE event");
            }
        }
    }
}

// ============================================================================
// FILE EXTRACTION
// ============================================================================

/// CPU-only: extract files from all messages in a batch.
///
/// Replaces base64 data with `#!B64!#` URIs in message content.
/// Returns modified messages and pending file writes for I/O phase.
fn extract_files_cpu_messages(
    mut messages: Vec<Vec<RawMessage>>,
    spans: &[SpanData],
    cache: Option<&FileExtractionCache>,
) -> (Vec<Vec<RawMessage>>, Vec<PendingFileWrite>) {
    let mut pending = Vec::new();

    for (span_idx, span_messages) in messages.iter_mut().enumerate() {
        let span = &spans[span_idx];
        let project_id = span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID);
        let trace_id = &span.trace_id;

        for raw_message in span_messages.iter_mut() {
            let result = match cache {
                Some(c) => extract_and_replace_files_cached(&mut raw_message.content, c),
                None => extract_and_replace_files(&mut raw_message.content),
            };
            pending.extend(to_pending_files(result.files, project_id, trace_id));
        }
    }

    (messages, pending)
}

/// Replace references to files that were not stored with a placeholder a reader can understand.
///
/// A `#!B64!#` reference means "the bytes are in the file store". When a quota rejection means they
/// never got there, committing the reference unchanged leaves a reader unable to tell a *rejected*
/// file from a corrupt one - it renders as a broken image either way, and nothing on the span says
/// which. Neither is acceptable, and keeping the base64 inline is not either: that defeats the quota
/// by moving the bytes into the analytics store, where they are harder to reclaim.
///
/// So the reference is replaced by text that says what happened. It is a string substitution over the
/// already-serialised JSON deliberately: a reference can appear in messages, tool definitions, raw
/// span JSON or metadata, and re-parsing all of them to find it would cost more than the rejection.
pub(super) fn note_unstored_files(
    spans: &mut [NormalizedSpan],
    skipped: &[(String, String)],
) -> usize {
    if skipped.is_empty() {
        return 0;
    }
    let mut rewritten = 0usize;
    // Whole references only: a longer reference that begins with this one is another file's, and stays.
    let replace = |field: &mut String, uri: &str, note: &str, count: &mut usize| {
        if let Some(rewritten) = sideseat_domain::files::replace_file_reference(field, uri, note) {
            *count += 1;
            *field = rewritten;
        }
    };
    for span in spans.iter_mut() {
        let span_project = span.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID);
        for (project_id, uri) in skipped {
            // Only the project whose quota was exceeded. The same content-addressed URI can be stored
            // in another project, and rewriting it there would replace a working reference with a note.
            if project_id != span_project {
                continue;
            }
            // Says only what is known. This set holds two kinds - content rejected for quota, and a
            // reference whose bytes this project does not have - and naming quota for both told readers
            // something that may be untrue of their case.
            let note = sideseat_domain::files::missing_content_note(uri);
            for field in [
                span.messages.as_mut(),
                span.tool_definitions.as_mut(),
                span.metadata.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
                replace(field, uri, &note, &mut rewritten);
            }
        }
    }
    rewritten
}

// ============================================================================
// DUCKDB WRITES
// ============================================================================

/// What a span write left behind, per project.
pub(super) struct SpanWrite {
    /// Projects whose rows committed.
    pub(super) committed: HashSet<String>,
    /// Projects that did not commit, but one of whose attempts failed without settling
    /// (`DataError::write_in_doubt`): their rows may yet land, so a read made now cannot prove them absent.
    pub(super) in_doubt: HashSet<String>,
}

/// Write spans, retrying with backoff, and report which projects' rows committed - all of them, some, or none.
///
/// A backend that writes projects separately can commit some before failing another
/// (`DataError::PartiallyWritten`); a retry then sends only the projects that did not commit, so a committed
/// project is neither written twice nor reported as failed - its rows are readable, and the caller keeps the
/// bookkeeping they need. An atomic backend returns every project or none. Each attempt clones the spans,
/// because `insert_spans` takes ownership.
pub(super) async fn write_to_duckdb(
    spans: Vec<NormalizedSpan>,
    repo: &(dyn AnalyticsRepository + Send + Sync),
) -> SpanWrite {
    let project_of = |span: &NormalizedSpan| {
        span.project_id
            .clone()
            .unwrap_or_else(|| sideseat_core::constants::DEFAULT_PROJECT_ID.to_string())
    };
    let span_count = spans.len();
    let mut committed: HashSet<String> = HashSet::new();
    let mut in_doubt: HashSet<String> = HashSet::new();
    let mut remaining = spans;
    for attempt in 1..=DEFAULT_MAX_ATTEMPTS {
        match repo.insert_spans(remaining.clone()).await {
            Ok(()) => {
                committed.extend(remaining.iter().map(project_of));
                tracing::trace!(
                    spans = span_count,
                    attempt,
                    "Wrote traces to analytics backend"
                );
                in_doubt.retain(|project| !committed.contains(project));
                return SpanWrite {
                    committed,
                    in_doubt,
                };
            }
            Err(error) => {
                if let sideseat_ports::error::DataError::PartiallyWritten {
                    committed_projects,
                    ..
                } = &error
                {
                    tracing::warn!(
                        %error,
                        committed = committed_projects.len(),
                        attempt,
                        "Some projects' spans committed before the write failed; retrying the rest"
                    );
                    committed.extend(committed_projects.iter().cloned());
                } else {
                    tracing::warn!(%error, attempt, "Failed to write spans to analytics backend");
                }
                // The project an in-doubt failure names, or every project still unwritten when it names none.
                let unwritten: Vec<String> = remaining
                    .iter()
                    .map(project_of)
                    .filter(|project| !committed.contains(project))
                    .collect();
                in_doubt.extend(error.projects_in_doubt(unwritten.iter().map(String::as_str)));
                remaining.retain(|span| !committed.contains(&project_of(span)));
            }
        }
        if attempt < DEFAULT_MAX_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(
                DEFAULT_BASE_DELAY_MS * 2_u64.pow(attempt - 1),
            ))
            .await;
        }
    }
    tracing::error!(
        spans = remaining.len(),
        attempts = DEFAULT_MAX_ATTEMPTS,
        "Failed to write spans to analytics backend after retries"
    );
    in_doubt.retain(|project| !committed.contains(project));
    SpanWrite {
        committed,
        in_doubt,
    }
}

// ============================================================================
// FLATTEN TO DB FORMAT
// ============================================================================

mod flattening;
pub(in crate::traces) use flattening::build_raw_span_json;
use flattening::flatten;
pub(crate) use flattening::span_content_digest;

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
