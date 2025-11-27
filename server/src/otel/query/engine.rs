//! Query engine implementation

use sqlx::SqlitePool;

use super::filters::{SpanFilter, TraceFilter};
use super::pagination::{Cursor, PageResult};
use crate::otel::error::OtelError;
use crate::otel::storage::sqlite::{SpanIndex, TraceSummary};

/// Query engine for traces and spans
pub struct QueryEngine {
    pool: SqlitePool,
}

impl QueryEngine {
    /// Create a new query engine
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Query traces with filters and pagination
    pub async fn query_traces(
        &self,
        filter: &TraceFilter,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> Result<PageResult<TraceSummary>, OtelError> {
        let mut sql = String::from(
            "SELECT trace_id, root_span_id, service_name, detected_framework, \
             span_count, start_time_ns, end_time_ns, duration_ns, \
             total_input_tokens, total_output_tokens, total_tokens, has_errors \
             FROM traces WHERE 1=1",
        );

        let mut args: Vec<String> = Vec::new();

        if let Some(service) = &filter.service_name {
            sql.push_str(" AND service_name = ?");
            args.push(service.clone());
        }

        if let Some(framework) = &filter.framework {
            sql.push_str(" AND detected_framework = ?");
            args.push(framework.clone());
        }

        if let Some(start) = filter.start_time_ns {
            sql.push_str(" AND start_time_ns >= ?");
            args.push(start.to_string());
        }

        if let Some(end) = filter.end_time_ns {
            sql.push_str(" AND start_time_ns <= ?");
            args.push(end.to_string());
        }

        if filter.has_errors == Some(true) {
            sql.push_str(" AND has_errors = 1");
        }

        if let Some(min_dur) = filter.min_duration_ns {
            sql.push_str(" AND duration_ns >= ?");
            args.push(min_dur.to_string());
        }

        if let Some(max_dur) = filter.max_duration_ns {
            sql.push_str(" AND duration_ns <= ?");
            args.push(max_dur.to_string());
        }

        if let Some(search) = &filter.search {
            sql.push_str(" AND (trace_id LIKE ? ESCAPE '\\' OR service_name LIKE ? ESCAPE '\\')");
            let pattern = escape_like_pattern(search);
            args.push(pattern.clone());
            args.push(pattern);
        }

        if let Some(c) = cursor {
            sql.push_str(" AND (start_time_ns, trace_id) < (?, ?)");
            args.push(c.timestamp.to_string());
            args.push(c.id.clone());
        }

        sql.push_str(" ORDER BY start_time_ns DESC, trace_id DESC LIMIT ?");
        args.push((limit + 1).to_string()); // +1 to detect if more pages exist

        let rows = self.execute_trace_query(&sql, &args).await?;

        let has_more = rows.len() > limit;
        let items: Vec<TraceSummary> = rows.into_iter().take(limit).collect();

        let next_cursor = if has_more {
            items
                .last()
                .map(|last| Cursor { timestamp: last.start_time_ns, id: last.trace_id.clone() })
        } else {
            None
        };

        Ok(PageResult { items, next_cursor, has_more })
    }

    /// Query spans with filters
    pub async fn query_spans(
        &self,
        filter: &SpanFilter,
        limit: usize,
    ) -> Result<Vec<SpanIndex>, OtelError> {
        let mut sql = String::from(
            "SELECT span_id, trace_id, parent_span_id, span_name, service_name, \
             detected_framework, detected_category, gen_ai_agent_name, \
             gen_ai_tool_name, gen_ai_request_model, start_time_ns, \
             end_time_ns, duration_ns, status_code, usage_input_tokens, \
             usage_output_tokens, parquet_file \
             FROM spans WHERE 1=1",
        );

        if filter.trace_id.is_some() {
            sql.push_str(" AND trace_id = ?");
        }
        if filter.parent_span_id.is_some() {
            sql.push_str(" AND parent_span_id = ?");
        }
        if filter.service_name.is_some() {
            sql.push_str(" AND service_name = ?");
        }
        if filter.framework.is_some() {
            sql.push_str(" AND detected_framework = ?");
        }
        if filter.category.is_some() {
            sql.push_str(" AND detected_category = ?");
        }
        if filter.agent_name.is_some() {
            sql.push_str(" AND gen_ai_agent_name = ?");
        }
        if filter.tool_name.is_some() {
            sql.push_str(" AND gen_ai_tool_name = ?");
        }
        if filter.model.is_some() {
            sql.push_str(" AND gen_ai_request_model = ?");
        }
        if filter.start_time_ns.is_some() {
            sql.push_str(" AND start_time_ns >= ?");
        }
        if filter.end_time_ns.is_some() {
            sql.push_str(" AND start_time_ns <= ?");
        }
        if filter.status_code.is_some() {
            sql.push_str(" AND status_code = ?");
        }

        sql.push_str(" ORDER BY start_time_ns LIMIT ?");

        let mut query = sqlx::query_as::<_, SpanIndex>(&sql);

        if let Some(trace_id) = &filter.trace_id {
            query = query.bind(trace_id);
        }
        if let Some(parent_span_id) = &filter.parent_span_id {
            query = query.bind(parent_span_id);
        }
        if let Some(service) = &filter.service_name {
            query = query.bind(service);
        }
        if let Some(framework) = &filter.framework {
            query = query.bind(framework);
        }
        if let Some(category) = &filter.category {
            query = query.bind(category);
        }
        if let Some(agent) = &filter.agent_name {
            query = query.bind(agent);
        }
        if let Some(tool) = &filter.tool_name {
            query = query.bind(tool);
        }
        if let Some(model) = &filter.model {
            query = query.bind(model);
        }
        if let Some(start) = filter.start_time_ns {
            query = query.bind(start);
        }
        if let Some(end) = filter.end_time_ns {
            query = query.bind(end);
        }
        if let Some(status) = filter.status_code {
            query = query.bind(status);
        }
        query = query.bind(limit as i64);

        query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to query spans: {}", e)))
    }

    async fn execute_trace_query(
        &self,
        sql: &str,
        args: &[String],
    ) -> Result<Vec<TraceSummary>, OtelError> {
        let mut query = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                String,
                String,
                i32,
                i64,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                bool,
            ),
        >(sql);

        for arg in args {
            query = query.bind(arg);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to query traces: {}", e)))?;

        Ok(rows
            .into_iter()
            .map(|r| TraceSummary {
                trace_id: r.0,
                root_span_id: r.1,
                service_name: r.2,
                detected_framework: r.3,
                span_count: r.4,
                start_time_ns: r.5,
                end_time_ns: r.6,
                duration_ns: r.7,
                total_input_tokens: r.8,
                total_output_tokens: r.9,
                total_tokens: r.10,
                has_errors: r.11,
            })
            .collect())
    }
}

/// Escape LIKE wildcards to prevent pattern injection
/// Escapes \, %, and _ characters for use in SQL LIKE patterns
fn escape_like_pattern(search: &str) -> String {
    let escaped = search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{}%", escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_like_pattern_no_special_chars() {
        assert_eq!(escape_like_pattern("hello"), "%hello%");
    }

    #[test]
    fn test_escape_like_pattern_with_percent() {
        assert_eq!(escape_like_pattern("50%off"), "%50\\%off%");
    }

    #[test]
    fn test_escape_like_pattern_with_underscore() {
        assert_eq!(escape_like_pattern("user_id"), "%user\\_id%");
    }

    #[test]
    fn test_escape_like_pattern_with_backslash() {
        assert_eq!(escape_like_pattern("path\\file"), "%path\\\\file%");
    }

    #[test]
    fn test_escape_like_pattern_with_multiple_special_chars() {
        assert_eq!(escape_like_pattern("100%_test\\"), "%100\\%\\_test\\\\%");
    }

    #[test]
    fn test_escape_like_pattern_empty_string() {
        assert_eq!(escape_like_pattern(""), "%%");
    }

    #[test]
    fn test_escape_like_pattern_only_percent() {
        assert_eq!(escape_like_pattern("%"), "%\\%%");
    }

    #[test]
    fn test_escape_like_pattern_only_underscore() {
        assert_eq!(escape_like_pattern("_"), "%\\_%");
    }

    #[test]
    fn test_escape_like_pattern_consecutive_special_chars() {
        assert_eq!(escape_like_pattern("%%__\\\\"), "%\\%\\%\\_\\_\\\\\\\\%");
    }

    #[test]
    fn test_escape_like_pattern_sql_injection_attempt() {
        // Attempt to break out of LIKE pattern
        assert_eq!(escape_like_pattern("'; DROP TABLE--"), "%'; DROP TABLE--%");
    }

    #[test]
    fn test_escape_like_pattern_wildcard_injection() {
        // Attempt to inject wildcards for broader matching
        assert_eq!(escape_like_pattern("%admin%"), "%\\%admin\\%%");
    }

    #[test]
    fn test_escape_like_pattern_unicode() {
        assert_eq!(escape_like_pattern("日本語"), "%日本語%");
    }

    #[test]
    fn test_escape_like_pattern_mixed_unicode_special() {
        assert_eq!(escape_like_pattern("用户_名%"), "%用户\\_名\\%%");
    }
}
