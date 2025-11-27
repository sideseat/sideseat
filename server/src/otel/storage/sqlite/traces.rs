//! Trace summary operations

use sqlx::SqlitePool;

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Trace summary record
#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub trace_id: String,
    pub root_span_id: Option<String>,
    pub service_name: String,
    pub detected_framework: String,
    pub span_count: i32,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub has_errors: bool,
}

/// Insert or update a trace summary
pub async fn upsert_trace(pool: &SqlitePool, span: &NormalizedSpan) -> Result<(), OtelError> {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let has_error = span.status_code != 0;

    sqlx::query(
        r#"
        INSERT INTO traces (
            trace_id, root_span_id, service_name, detected_framework,
            span_count, start_time_ns, end_time_ns, duration_ns,
            total_input_tokens, total_output_tokens, total_tokens,
            has_errors, created_at, updated_at
        ) VALUES (?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(trace_id) DO UPDATE SET
            span_count = span_count + 1,
            end_time_ns = MAX(COALESCE(end_time_ns, 0), COALESCE(excluded.end_time_ns, 0)),
            duration_ns = MAX(COALESCE(end_time_ns, 0), COALESCE(excluded.end_time_ns, 0)) - start_time_ns,
            total_input_tokens = COALESCE(total_input_tokens, 0) + COALESCE(excluded.total_input_tokens, 0),
            total_output_tokens = COALESCE(total_output_tokens, 0) + COALESCE(excluded.total_output_tokens, 0),
            total_tokens = COALESCE(total_tokens, 0) + COALESCE(excluded.total_tokens, 0),
            has_errors = has_errors OR excluded.has_errors,
            updated_at = excluded.updated_at
        "#
    )
    .bind(&span.trace_id)
    .bind(if span.parent_span_id.is_none() { Some(&span.span_id) } else { None::<&String> })
    .bind(&span.service_name)
    .bind(&span.detected_framework)
    .bind(span.start_time_unix_nano)
    .bind(span.end_time_unix_nano)
    .bind(span.duration_ns)
    .bind(span.usage_input_tokens)
    .bind(span.usage_output_tokens)
    .bind(span.usage_total_tokens)
    .bind(has_error)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to upsert trace: {}", e)))?;

    Ok(())
}

/// Get a trace summary by ID
pub async fn get_trace(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Option<TraceSummary>, OtelError> {
    let row = sqlx::query_as::<
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
    >(
        r#"
        SELECT trace_id, root_span_id, service_name, detected_framework,
               span_count, start_time_ns, end_time_ns, duration_ns,
               total_input_tokens, total_output_tokens, total_tokens, has_errors
        FROM traces WHERE trace_id = ?
        "#,
    )
    .bind(trace_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get trace: {}", e)))?;

    Ok(row.map(|r| TraceSummary {
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
    }))
}
