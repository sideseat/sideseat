//! Trace-level aggregations

use sqlx::SqlitePool;

use crate::otel::error::OtelError;

/// Aggregate token usage for a trace
#[derive(Debug, Clone, Default)]
pub struct TraceTokenSummary {
    pub trace_id: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub llm_call_count: i64,
}

/// Aggregate duration stats for a trace
#[derive(Debug, Clone, Default)]
pub struct TraceDurationSummary {
    pub trace_id: String,
    pub total_duration_ns: i64,
    pub span_count: i64,
    pub avg_span_duration_ns: i64,
    pub max_span_duration_ns: i64,
}

/// Get token usage summary for a trace
pub async fn get_trace_token_summary(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<TraceTokenSummary, OtelError> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT
            COALESCE(SUM(usage_input_tokens), 0) as input_tokens,
            COALESCE(SUM(usage_output_tokens), 0) as output_tokens,
            COALESCE(SUM(usage_total_tokens), 0) as total_tokens,
            COUNT(*) FILTER (WHERE usage_total_tokens IS NOT NULL) as llm_calls
        FROM spans
        WHERE trace_id = ?
        "#,
    )
    .bind(trace_id)
    .fetch_one(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(TraceTokenSummary {
        trace_id: trace_id.to_string(),
        total_input_tokens: row.0,
        total_output_tokens: row.1,
        total_tokens: row.2,
        llm_call_count: row.3,
    })
}

/// Get duration summary for a trace
pub async fn get_trace_duration_summary(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<TraceDurationSummary, OtelError> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT
            COALESCE(MAX(end_time_ns) - MIN(start_time_ns), 0) as total_duration,
            COUNT(*) as span_count,
            COALESCE(AVG(duration_ns), 0) as avg_duration,
            COALESCE(MAX(duration_ns), 0) as max_duration
        FROM spans
        WHERE trace_id = ?
        "#,
    )
    .bind(trace_id)
    .fetch_one(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(TraceDurationSummary {
        trace_id: trace_id.to_string(),
        total_duration_ns: row.0,
        span_count: row.1,
        avg_span_duration_ns: row.2,
        max_span_duration_ns: row.3,
    })
}

/// Get category breakdown for a trace
pub async fn get_trace_category_breakdown(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Vec<CategoryCount>, OtelError> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT
            COALESCE(detected_category, 'unknown') as category,
            COUNT(*) as count
        FROM spans
        WHERE trace_id = ?
        GROUP BY detected_category
        ORDER BY count DESC
        "#,
    )
    .bind(trace_id)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(rows.into_iter().map(|(category, count)| CategoryCount { category, count }).collect())
}

/// Category count
#[derive(Debug, Clone)]
pub struct CategoryCount {
    pub category: String,
    pub count: i64,
}

/// Global aggregations across all traces
pub async fn get_global_stats(pool: &SqlitePool) -> Result<GlobalStats, OtelError> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64)>(
        r#"
        SELECT
            COUNT(DISTINCT trace_id) as trace_count,
            COUNT(*) as span_count,
            COALESCE(SUM(usage_input_tokens), 0) as total_input_tokens,
            COALESCE(SUM(usage_output_tokens), 0) as total_output_tokens,
            COALESCE(SUM(usage_total_tokens), 0) as total_tokens
        FROM spans
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(GlobalStats {
        trace_count: row.0,
        span_count: row.1,
        total_input_tokens: row.2,
        total_output_tokens: row.3,
        total_tokens: row.4,
    })
}

/// Global statistics
#[derive(Debug, Clone, Default)]
pub struct GlobalStats {
    pub trace_count: i64,
    pub span_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
}
