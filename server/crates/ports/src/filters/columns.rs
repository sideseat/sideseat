//! Which columns a request may filter or sort on.
//!
//! An **allowlist**, so a column name from a request can never reach SQL unchecked - and vocabulary rather than
//! dialect, which is why it lives here and not in either adapter. It sat inside the DuckDB SQL builder, so the
//! ClickHouse adapter reached across into that builder to learn what a caller is allowed to ask for.

pub const TRACE_SORTABLE: &[&str] = &[
    "start_time",
    "end_time",
    "duration_ms",
    "total_tokens",
    "total_cost",
];

pub const TRACE_FILTERABLE: &[&str] = &[
    "trace_name",
    "duration_ms",
    "total_tokens",
    "total_cost",
    "environment",
    "tags",
    "session_id",
    "user_id",
    "trace_id",
    "start_time",
    "end_time",
    "input_cost",
    "output_cost",
    "cache_read_cost",
    "cache_write_cost",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "reasoning_tokens",
    "reasoning_cost",
    "gen_ai_request_model",
    "gen_ai_system",
    "framework",
];

/// Maps trace view column names to otel_spans table column names.
pub fn map_trace_column_to_spans(view_column: &str) -> &str {
    match view_column {
        "input_tokens" => "gen_ai_usage_input_tokens",
        "output_tokens" => "gen_ai_usage_output_tokens",
        "total_tokens" => "gen_ai_usage_total_tokens",
        "cache_read_tokens" => "gen_ai_usage_cache_read_tokens",
        "cache_write_tokens" => "gen_ai_usage_cache_write_tokens",
        "reasoning_tokens" => "gen_ai_usage_reasoning_tokens",
        "input_cost" => "gen_ai_cost_input",
        "output_cost" => "gen_ai_cost_output",
        "cache_read_cost" => "gen_ai_cost_cache_read",
        "cache_write_cost" => "gen_ai_cost_cache_write",
        "reasoning_cost" => "gen_ai_cost_reasoning",
        "total_cost" => "gen_ai_cost_total",
        "start_time" => "timestamp_start",
        "end_time" => "timestamp_end",
        "trace_name" => "span_name",
        _ => view_column,
    }
}

pub const SPAN_SORTABLE: &[&str] = &[
    "timestamp_start",
    "timestamp_end",
    "start_time", // Alias for timestamp_start (API consistency with traces/sessions)
    "end_time",   // Alias for timestamp_end (API consistency with traces/sessions)
    "duration_ms",
    "span_name",
];

/// Maps span API column names to otel_spans table column names.
/// Provides consistent API between spans, traces, and sessions.
pub fn map_span_column(api_column: &str) -> &str {
    match api_column {
        "start_time" => "timestamp_start",
        "end_time" => "timestamp_end",
        _ => api_column,
    }
}

pub const SPAN_FILTERABLE: &[&str] = &[
    "trace_id",
    "span_id",
    "session_id",
    "user_id",
    "environment",
    "span_category",
    "observation_type",
    "framework",
    "status_code",
    "gen_ai_request_model",
    "gen_ai_agent_name",
    // Offered by SPAN_FILTER_CONFIGS in web/src/lib/filters.ts as Provider, Total Tokens and
    // Total Cost. Missing here, so selecting any of them was rejected before the query ran -
    // the dropdown worked and the filter silently did nothing.
    "gen_ai_system",
    "gen_ai_usage_total_tokens",
    "gen_ai_cost_total",
    "timestamp_start",
    "timestamp_end",
    "start_time", // Alias for timestamp_start (API consistency)
    "end_time",   // Alias for timestamp_end (API consistency)
    "duration_ms",
    "span_name",
];

pub const SESSION_SORTABLE: &[&str] = &[
    "start_time",
    "end_time",
    "trace_count",
    "span_count",
    "observation_count",
];

pub const SESSION_FILTERABLE: &[&str] = &[
    "session_id",
    "user_id",
    "environment",
    "start_time",
    "end_time",
];

/// Maps session view column names to otel_spans table column names.
pub fn map_session_column_to_spans(view_column: &str) -> &str {
    match view_column {
        "start_time" => "timestamp_start",
        "end_time" => "timestamp_end",
        _ => view_column,
    }
}
