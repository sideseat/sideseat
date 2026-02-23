//! Shared message types for all database backends
//!
//! This module contains message query result types and parameters.

use chrono::{DateTime, Utc};

use super::analytics::SpanIdentity;

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
    /// Cursor for pagination: (ingested_at_us, span_id)
    pub cursor: Option<(i64, String)>,
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
