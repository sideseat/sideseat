//! Query engine implementation

use sqlx::SqlitePool;

use super::filters::{AttributeFilter, FilterOperator, SpanFilter, TraceFilter};
use super::pagination::{Cursor, PageResult};
use crate::otel::error::OtelError;
use crate::otel::storage::sqlite::{AttributeKeyCache, SpanIndex, TraceSummary};

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
        attr_cache: Option<&AttributeKeyCache>,
    ) -> Result<PageResult<TraceSummary>, OtelError> {
        let mut sql = String::from(
            "SELECT t.trace_id, t.session_id, t.root_span_id, t.root_span_name, t.service_name, t.detected_framework, \
             t.span_count, t.start_time_ns, t.end_time_ns, t.duration_ns, \
             t.total_input_tokens, t.total_output_tokens, t.total_tokens, t.has_errors \
             FROM traces t WHERE t.deleted_at IS NULL",
        );

        let mut args: Vec<String> = Vec::new();

        if let Some(service) = &filter.service_name {
            sql.push_str(" AND t.service_name = ?");
            args.push(service.clone());
        }

        if let Some(framework) = &filter.framework {
            sql.push_str(" AND t.detected_framework = ?");
            args.push(framework.clone());
        }

        if let Some(start) = filter.start_time_ns {
            sql.push_str(" AND t.start_time_ns >= ?");
            args.push(start.to_string());
        }

        if let Some(end) = filter.end_time_ns {
            sql.push_str(" AND t.start_time_ns <= ?");
            args.push(end.to_string());
        }

        if filter.has_errors == Some(true) {
            sql.push_str(" AND t.has_errors = 1");
        }

        if let Some(min_dur) = filter.min_duration_ns {
            sql.push_str(" AND t.duration_ns >= ?");
            args.push(min_dur.to_string());
        }

        if let Some(max_dur) = filter.max_duration_ns {
            sql.push_str(" AND t.duration_ns <= ?");
            args.push(max_dur.to_string());
        }

        if let Some(search) = &filter.search {
            sql.push_str(
                " AND (t.trace_id LIKE ? ESCAPE '\\' OR t.service_name LIKE ? ESCAPE '\\')",
            );
            let pattern = escape_like_pattern(search);
            args.push(pattern.clone());
            args.push(pattern);
        }

        // Add EAV attribute filters using EXISTS subqueries
        if let Some(cache) = attr_cache {
            for (i, attr_filter) in filter.attributes.iter().enumerate() {
                let key_id = cache.get_key_id(&attr_filter.key, "trace");

                if let Some(kid) = key_id {
                    self.append_attribute_filter(&mut sql, &mut args, i, kid, attr_filter)?;
                }
                // If key doesn't exist, skip the filter (no matching attributes)
            }
        }

        if let Some(c) = cursor {
            sql.push_str(" AND (t.start_time_ns, t.trace_id) < (?, ?)");
            args.push(c.timestamp.to_string());
            args.push(c.id.clone());
        }

        sql.push_str(" ORDER BY t.start_time_ns DESC, t.trace_id DESC LIMIT ?");
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

    /// Append attribute filter as EXISTS subquery
    fn append_attribute_filter(
        &self,
        sql: &mut String,
        args: &mut Vec<String>,
        index: usize,
        key_id: i64,
        filter: &AttributeFilter,
    ) -> Result<(), OtelError> {
        match filter.op {
            FilterOperator::Eq => {
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_str = ?)",
                    index, index, index, index
                ));
                args.push(key_id.to_string());
                args.push(filter.value.as_str().unwrap_or("").to_string());
            }
            FilterOperator::Ne => {
                sql.push_str(&format!(
                    " AND NOT EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_str = ?)",
                    index, index, index, index
                ));
                args.push(key_id.to_string());
                args.push(filter.value.as_str().unwrap_or("").to_string());
            }
            FilterOperator::Contains => {
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_str LIKE ? ESCAPE '\\')",
                    index, index, index, index
                ));
                args.push(key_id.to_string());
                let pattern = escape_like_pattern(filter.value.as_str().unwrap_or(""));
                args.push(pattern);
            }
            FilterOperator::StartsWith => {
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_str LIKE ? ESCAPE '\\')",
                    index, index, index, index
                ));
                args.push(key_id.to_string());
                let val = filter.value.as_str().unwrap_or("");
                let escaped = val.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                args.push(format!("{}%", escaped));
            }
            FilterOperator::In => {
                let values: Vec<&str> = filter
                    .value
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                if !values.is_empty() {
                    let placeholders = vec!["?"; values.len()].join(",");
                    sql.push_str(&format!(
                        " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                         WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_str IN ({}))",
                        index, index, index, index, placeholders
                    ));
                    args.push(key_id.to_string());
                    args.extend(values.into_iter().map(String::from));
                }
            }
            FilterOperator::Gt | FilterOperator::Lt | FilterOperator::Gte | FilterOperator::Lte => {
                let op = filter.op.to_sql_str();
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ? AND ta{}.value_num {} ?)",
                    index, index, index, index, op
                ));
                args.push(key_id.to_string());
                let num = filter.value.as_f64().unwrap_or(0.0);
                args.push(num.to_string());
            }
            FilterOperator::IsNull => {
                sql.push_str(&format!(
                    " AND NOT EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ?)",
                    index, index, index
                ));
                args.push(key_id.to_string());
            }
            FilterOperator::IsNotNull => {
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM trace_attributes ta{} \
                     WHERE ta{}.trace_id = t.trace_id AND ta{}.key_id = ?)",
                    index, index, index
                ));
                args.push(key_id.to_string());
            }
        }
        Ok(())
    }

    /// Query spans with filters and cursor-based pagination
    pub async fn query_spans(
        &self,
        filter: &SpanFilter,
        limit: usize,
    ) -> Result<Vec<SpanIndex>, OtelError> {
        let mut sql = String::from(
            "SELECT span_id, trace_id, session_id, parent_span_id, span_name, service_name, \
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

        // Cursor-based pagination
        if filter.cursor_timestamp.is_some() && filter.cursor_id.is_some() {
            sql.push_str(" AND (start_time_ns, span_id) > (?, ?)");
        }

        sql.push_str(" ORDER BY start_time_ns, span_id LIMIT ?");

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
        if let (Some(cursor_ts), Some(cursor_id)) = (filter.cursor_timestamp, &filter.cursor_id) {
            query = query.bind(cursor_ts).bind(cursor_id);
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
                Option<String>,
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
                session_id: r.1,
                root_span_id: r.2,
                root_span_name: r.3,
                service_name: r.4,
                detected_framework: r.5,
                span_count: r.6,
                start_time_ns: r.7,
                end_time_ns: r.8,
                duration_ns: r.9,
                total_input_tokens: r.10,
                total_output_tokens: r.11,
                total_tokens: r.12,
                has_errors: r.13,
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
