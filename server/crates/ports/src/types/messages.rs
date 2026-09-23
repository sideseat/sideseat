//! Shared message types for all database backends
//!
//! This module contains message query result types and parameters.

use chrono::{DateTime, Utc};

use super::ProjectId;
use super::analytics::SpanIdentity;

pub const SPAN_FILTER_OPTION_COLUMNS: &[&str] = &[
    "environment",
    "framework",
    "gen_ai_agent_name",
    "gen_ai_request_model",
    "gen_ai_system",
    "observation_type",
    "session_id",
    "span_category",
    "span_name",
    "status_code",
    "user_id",
];

/// Trace filter options, as (view column, underlying span column).
pub const TRACE_FILTER_OPTION_COLUMNS: &[(&str, &str)] = &[
    ("environment", "environment"),
    ("session_id", "session_id"),
    ("trace_name", "span_name"),
    ("user_id", "user_id"),
];

/// Session filter options.
pub const SESSION_FILTER_OPTION_COLUMNS: &[&str] = &["environment", "user_id"];

// ============================================================================
// Row types
// ============================================================================

/// Raw span row from database for message queries.
///
/// Messages are stored as raw JSON at ingestion time.
/// The feed pipeline (process_spans) handles parsing, SideML conversion, and all processing.
#[derive(Debug, Clone)]
pub struct MessageSpanRow {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_timestamp: DateTime<Utc>,
    /// Span end time (for OUTPUT message ordering)
    pub span_end_timestamp: Option<DateTime<Utc>>,
    /// Raw messages (JSON string, converted to SideML at query time)
    pub messages_json: String,
    /// Tool definitions (JSON string)
    pub tool_definitions_json: String,
    /// Tool names (JSON string)
    pub tool_names_json: String,
    /// Compact digest of the interpretation-bearing body inputs.
    ///
    /// Body hydration derives this from content-addressed hashes, with inline bytes used for fields
    /// not migrated yet. `None` means the cache must hash the three inline payloads directly.
    pub body_cache_key: Option<String>,
    /// Span metadata
    pub model: Option<String>,
    pub provider: Option<String>,
    pub status_code: Option<String>,
    pub exception_type: Option<String>,
    pub exception_message: Option<String>,
    pub exception_stacktrace: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cost_total: f64,
    /// Observation type for query-time role derivation (e.g., "Tool", "Generation")
    pub observation_type: Option<String>,
    /// Session ID for conversation grouping in feed API
    pub session_id: Option<String>,
    /// Ingestion time for cursor-based pagination in feed API
    pub ingested_at: DateTime<Utc>,
    /// Instrumentation scope: the library that produced the span, versioned. What makes a rule keyed
    /// on a producer's identity-and-version expressible at read time - the fact the design record's
    /// persistence audit found was never captured for spans. `None` on pre-v4 rows.
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    /// The compact envelope: the facts a debugging caller needs beside the messages, loaded in the
    /// same query because a second span-sized request doubles the measured p50 and introduces a
    /// snapshot-consistency problem between the two reads. One envelope per span in the response,
    /// never repeated per block.
    pub span_name: Option<String>,
    pub framework: Option<String>,
    pub response_model: Option<String>,
    pub response_id: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<i64>,
    /// Span-level finish reasons, as stored (a JSON array rendered to text).
    pub finish_reasons: Option<String>,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_input: f64,
    pub cost_output: f64,
}

impl SpanIdentity for MessageSpanRow {
    fn trace_id(&self) -> &str {
        &self.trace_id
    }
    fn span_id(&self) -> &str {
        &self.span_id
    }
    fn ordering_timestamp(&self) -> DateTime<Utc> {
        self.span_timestamp
    }
}

// ============================================================================
// Query results
// ============================================================================

/// Query result containing raw span rows.
///
/// Use process_spans() to process into messages.
#[derive(Debug)]
pub struct MessageQueryResult {
    pub rows: Vec<MessageSpanRow>,
}

// ============================================================================
// Query parameters
// ============================================================================

/// Parameters for project-wide message feed query.
#[derive(Debug, Default, Clone)]
pub struct FeedMessagesParams {
    pub project_id: ProjectId,
    /// Maximum number of spans to return
    pub limit: u32,
    /// Cursor for pagination: (ingested_at_us, span_id, trace_id).
    ///
    /// The trace id is part of the key because a span id is unique only within a trace.
    pub cursor: Option<(i64, String, String)>,
    /// Filter by event time >= start_time
    pub start_time: Option<DateTime<Utc>>,
    /// Filter by event time < end_time
    pub end_time: Option<DateTime<Utc>>,
    /// Ignore spans ingested at or after this instant, in microseconds since the epoch.
    ///
    /// The traversal watermark, established on the first page and carried by the cursor, so a page and the
    /// reconstruction context loaded around it describe the same instant. See
    /// [`MessageQueryParams::ingested_before_us`] for the sequence that made a span vanish from every page.
    pub ingested_before_us: Option<i64>,
}

/// Unified parameters for message queries (trace, span, or session).
///
/// Priority: span_id > session_id > trace_id
#[derive(Debug, Default, Clone)]
pub struct MessageQueryParams {
    pub project_id: ProjectId,
    pub span_id: Option<String>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    /// Several traces at once, for the project feed: a page of the feed holds spans from many
    /// traces, and reconstruction has to see each of those traces whole. Lower priority than the
    /// single-entity selectors.
    ///
    /// An `Option` so that "no trace list given" and "a list that happens to be empty" cannot be
    /// confused. As a bare `Vec`, an empty one read as *unused*, and a caller whose only selector
    /// was this list then asked for the whole project with no content filter - an empty feed page
    /// turning into an unbounded read. `Some(empty)` matches nothing.
    pub trace_ids: Option<Vec<String>>,
    pub from_timestamp: Option<DateTime<Utc>>,
    pub to_timestamp: Option<DateTime<Utc>>,
    /// Ignore rows ingested at or after this instant, in microseconds since the epoch.
    ///
    /// The project feed's traversal watermark. A page is chosen by ingestion time, but the
    /// *reconstruction context* loaded around it was unbounded in that dimension - so a span ingested
    /// after the traversal began could enter the context, win deduplication against a span still to be
    /// paged, and then be scoped off the page it was not selected for. Neither copy was ever returned:
    /// the older one suppressed, the newer one filtered out.
    ///
    /// Bounding the context by the same watermark that bounds page selection makes a traversal a view of
    /// one instant. Only the feed sets it; the span, trace and session views are not paginated and read
    /// whatever is there.
    pub ingested_before_us: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_query_result() {
        let result = MessageQueryResult { rows: vec![] };
        assert!(result.rows.is_empty());
    }
}
