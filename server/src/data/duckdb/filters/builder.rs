//! SQL filter builder
//!
//! Builds SQL WHERE clauses from Filter structs.
//! Includes column whitelists and mapping functions.

use super::types::{OptionsOp, SqlParams};

/// Build tags filter using DuckDB array_contains
/// The alias parameter is prepended to the column name (e.g., "sp" → "sp.tags").
pub fn build_tags_filter(
    tags: &[String],
    operator: &OptionsOp,
    params: &mut SqlParams,
    alias: &str,
) -> String {
    if tags.is_empty() {
        return "1=1".to_string();
    }

    let col = if alias.is_empty() {
        "tags".to_string()
    } else {
        format!("{}.tags", alias)
    };

    let conditions: Vec<String> = tags
        .iter()
        .map(|t| {
            params.values.push(t.clone());
            match operator {
                OptionsOp::AnyOf => format!("array_contains({}, ?)", col),
                OptionsOp::NoneOf => format!("NOT array_contains({}, ?)", col),
            }
        })
        .collect();

    let join_op = match operator {
        OptionsOp::AnyOf => " OR ",
        OptionsOp::NoneOf => " AND ",
    };

    format!("({})", conditions.join(join_op))
}

/// Column whitelists for different entities
pub mod columns {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tags_filter_any_of() {
        let tags = vec!["important".to_string(), "urgent".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "");

        assert_eq!(sql, "(array_contains(tags, ?) OR array_contains(tags, ?))");
        assert_eq!(params.values, vec!["important", "urgent"]);
    }

    #[test]
    fn build_tags_filter_any_of_with_alias() {
        let tags = vec!["important".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "sp");

        assert_eq!(sql, "(array_contains(sp.tags, ?))");
        assert_eq!(params.values, vec!["important"]);
    }

    #[test]
    fn build_tags_filter_none_of() {
        let tags = vec!["spam".to_string()];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::NoneOf, &mut params, "");

        assert_eq!(sql, "(NOT array_contains(tags, ?))");
        assert_eq!(params.values, vec!["spam"]);
    }

    #[test]
    fn build_tags_filter_empty() {
        let tags: Vec<String> = vec![];
        let mut params = SqlParams::default();
        let sql = build_tags_filter(&tags, &OptionsOp::AnyOf, &mut params, "");

        assert_eq!(sql, "1=1");
        assert!(params.values.is_empty());
    }

    #[test]
    fn map_trace_columns() {
        assert_eq!(
            columns::map_trace_column_to_spans("input_tokens"),
            "gen_ai_usage_input_tokens"
        );
        assert_eq!(
            columns::map_trace_column_to_spans("total_cost"),
            "gen_ai_cost_total"
        );
        assert_eq!(
            columns::map_trace_column_to_spans("start_time"),
            "timestamp_start"
        );
        assert_eq!(columns::map_trace_column_to_spans("unknown"), "unknown");
    }

    #[test]
    fn map_session_columns() {
        assert_eq!(
            columns::map_session_column_to_spans("start_time"),
            "timestamp_start"
        );
        assert_eq!(
            columns::map_session_column_to_spans("end_time"),
            "timestamp_end"
        );
        assert_eq!(
            columns::map_session_column_to_spans("session_id"),
            "session_id"
        );
    }

    #[test]
    fn map_span_columns() {
        // API aliases should map to DB column names
        assert_eq!(columns::map_span_column("start_time"), "timestamp_start");
        assert_eq!(columns::map_span_column("end_time"), "timestamp_end");
        // Native DB column names should pass through unchanged
        assert_eq!(
            columns::map_span_column("timestamp_start"),
            "timestamp_start"
        );
        assert_eq!(columns::map_span_column("timestamp_end"), "timestamp_end");
        assert_eq!(columns::map_span_column("span_name"), "span_name");
        assert_eq!(columns::map_span_column("duration_ms"), "duration_ms");
    }
}
