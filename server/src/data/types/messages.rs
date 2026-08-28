//! Shared message types for all database backends
//!
//! This module contains message query result types and parameters.

use chrono::{DateTime, Utc};

use super::analytics::SpanIdentity;

/// Columns a filter-options request may ask for, shared by both analytics backends.
///
/// These were declared separately in the DuckDB and ClickHouse repositories and had drifted in
/// both directions: ClickHouse omitted `gen_ai_agent_name`, so the Agent filter dropdown was
/// empty there, while DuckDB omitted `span_name`, `session_id` and `user_id`, so those were
/// empty on DuckDB. Which filters a user sees depended on the storage backend.
///
/// Every name here is a column on `otel_spans` in both schemas. The allowlist exists to keep
/// a caller-supplied column name out of the SQL, so adding a name that is not a real column
/// turns a filter into an error rather than an injection.
/// The trace name a list row displays: the root span's name, else the earliest named span's.
///
/// The same expression in both dialects, because three places have to agree on it - the projection
/// that displays it, the filter options that offer it, and the filter that matches it. `alias`
/// qualifies the column for queries that name their table.
///
/// A `trace_name` filter has to be evaluated against *this*, per trace. Matching the raw
/// `span_name` of any span meant selecting "agent" also returned traces displayed under another
/// name that merely contained an agent span.
pub fn trace_display_name(alias: &str, dialect_first: DisplayNameDialect) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match dialect_first {
        DisplayNameDialect::DuckDb => format!(
            "COALESCE(\
             FIRST({prefix}span_name ORDER BY {prefix}timestamp_start) \
             FILTER (WHERE {prefix}parent_span_id IS NULL AND {prefix}span_name IS NOT NULL), \
             FIRST({prefix}span_name ORDER BY {prefix}timestamp_start) \
             FILTER (WHERE {prefix}span_name IS NOT NULL))"
        ),
        DisplayNameDialect::ClickHouse => format!(
            "coalesce(\
             argMinIf({prefix}span_name, {prefix}timestamp_start, \
             {prefix}parent_span_id IS NULL AND {prefix}span_name IS NOT NULL), \
             argMinIf({prefix}span_name, {prefix}timestamp_start, {prefix}span_name IS NOT NULL))"
        ),
    }
}

/// Which aggregate syntax to render the display name in.
#[derive(Clone, Copy)]
pub enum DisplayNameDialect {
    DuckDb,
    ClickHouse,
}

/// The single value a trace row displays for a column that only some of its spans carry: the
/// earliest span that has one.
///
/// A session id, a user id and an environment are recorded on the spans that know them - often the
/// root alone - so the row shows one value chosen this way. A filter on such a column has to be
/// evaluated against that value, not against "some span": a trace whose root names a session and
/// whose children do not was returned by `session IS NULL` while displaying the session, and
/// excluded by nothing.
///
/// The column name comes from this crate's allowlists, never from a request.
pub fn trace_display_first(column: &str, alias: &str, dialect_first: DisplayNameDialect) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match dialect_first {
        DisplayNameDialect::DuckDb => format!(
            "FIRST({prefix}{column} ORDER BY {prefix}timestamp_start) \
             FILTER (WHERE {prefix}{column} IS NOT NULL)"
        ),
        DisplayNameDialect::ClickHouse => format!(
            "argMinIf({prefix}{column}, {prefix}timestamp_start, {prefix}{column} IS NOT NULL)"
        ),
    }
}

/// What makes a span a GenAI span, for the "GenAI only" trace and session lists.
///
/// A span qualifies through its observation type or through any GenAI attribute. Recognising only
/// the provider and the request model missed transport-level instrumentation that records an
/// operation name, a response model, an agent or tool name, or token usage and nothing else - those
/// traces vanished from the default list.
///
/// One definition, used by both backends, because it read differently in each and the same project
/// showed a different trace list depending on which one served it. `alias` qualifies the columns for
/// queries that name their table; pass "" when they do not.
pub fn genai_span_predicate(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    [
        format!("{prefix}observation_type != 'span'"),
        format!("{prefix}gen_ai_system IS NOT NULL"),
        format!("{prefix}gen_ai_operation_name IS NOT NULL"),
        format!("{prefix}gen_ai_request_model IS NOT NULL"),
        format!("{prefix}gen_ai_response_model IS NOT NULL"),
        format!("{prefix}gen_ai_agent_name IS NOT NULL"),
        format!("{prefix}gen_ai_tool_name IS NOT NULL"),
        format!("{prefix}gen_ai_usage_total_tokens > 0"),
    ]
    .join(" OR ")
}

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

/// Which rows a trace or session message query returns.
///
/// One definition shared by both backends: it was declared identically in the DuckDB and
/// ClickHouse repositories, so a change to one silently changed what the other returned. The
/// predicate is plain SQL that both dialects accept; if one ever needs to diverge, that
/// divergence should be visible here rather than by two constants drifting apart.
///
/// Rows with no messages, no tools and no error are never returned, which is why the
/// message-parsing harness applies the same filter when it builds its row sets.
pub const MESSAGE_CONTENT_FILTER: &str =
    "(messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR')";

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
    pub project_id: String,
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
}

/// Unified parameters for message queries (trace, span, or session).
///
/// Priority: span_id > session_id > trace_id
#[derive(Debug, Default, Clone)]
pub struct MessageQueryParams {
    pub project_id: String,
    pub span_id: Option<String>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    pub from_timestamp: Option<DateTime<Utc>>,
    pub to_timestamp: Option<DateTime<Utc>>,
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
