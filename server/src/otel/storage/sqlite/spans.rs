//! Span index operations

use sqlx::SqlitePool;

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Insert multiple spans in a single transaction (batch insert)
pub async fn insert_spans_batch(
    pool: &SqlitePool,
    spans: &[NormalizedSpan],
) -> Result<(), OtelError> {
    if spans.is_empty() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    for span in spans {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO spans (
                span_id, trace_id, parent_span_id, span_name, service_name,
                detected_framework, detected_category, gen_ai_agent_name,
                gen_ai_tool_name, gen_ai_request_model, start_time_ns,
                end_time_ns, duration_ns, status_code, usage_input_tokens,
                usage_output_tokens, parquet_file
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&span.span_id)
        .bind(&span.trace_id)
        .bind(&span.parent_span_id)
        .bind(&span.span_name)
        .bind(&span.service_name)
        .bind(&span.detected_framework)
        .bind(&span.detected_category)
        .bind(&span.gen_ai_agent_name)
        .bind(&span.gen_ai_tool_name)
        .bind(&span.gen_ai_request_model)
        .bind(span.start_time_unix_nano)
        .bind(span.end_time_unix_nano)
        .bind(span.duration_ns)
        .bind(span.status_code as i32)
        .bind(span.usage_input_tokens)
        .bind(span.usage_output_tokens)
        .bind(None::<&str>) // parquet_file will be updated later
        .execute(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to insert span: {}", e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    Ok(())
}

/// Insert a span into the index
pub async fn insert_span(
    pool: &SqlitePool,
    span: &NormalizedSpan,
    parquet_file: Option<&str>,
) -> Result<(), OtelError> {
    sqlx::query(
        r#"
        INSERT OR REPLACE INTO spans (
            span_id, trace_id, parent_span_id, span_name, service_name,
            detected_framework, detected_category, gen_ai_agent_name,
            gen_ai_tool_name, gen_ai_request_model, start_time_ns,
            end_time_ns, duration_ns, status_code, usage_input_tokens,
            usage_output_tokens, parquet_file
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&span.span_id)
    .bind(&span.trace_id)
    .bind(&span.parent_span_id)
    .bind(&span.span_name)
    .bind(&span.service_name)
    .bind(&span.detected_framework)
    .bind(&span.detected_category)
    .bind(&span.gen_ai_agent_name)
    .bind(&span.gen_ai_tool_name)
    .bind(&span.gen_ai_request_model)
    .bind(span.start_time_unix_nano)
    .bind(span.end_time_unix_nano)
    .bind(span.duration_ns)
    .bind(span.status_code as i32)
    .bind(span.usage_input_tokens)
    .bind(span.usage_output_tokens)
    .bind(parquet_file)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to insert span: {}", e)))?;

    Ok(())
}

/// Get spans for a trace
pub async fn get_spans_by_trace(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Vec<SpanIndex>, OtelError> {
    let rows = sqlx::query_as::<_, SpanIndex>(
        r#"
        SELECT span_id, trace_id, parent_span_id, span_name, service_name,
               detected_framework, detected_category, gen_ai_agent_name,
               gen_ai_tool_name, gen_ai_request_model, start_time_ns,
               end_time_ns, duration_ns, status_code, usage_input_tokens,
               usage_output_tokens, parquet_file
        FROM spans WHERE trace_id = ?
        ORDER BY start_time_ns
        "#,
    )
    .bind(trace_id)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get spans: {}", e)))?;

    Ok(rows)
}

/// Span index record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SpanIndex {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub service_name: String,
    pub detected_framework: String,
    pub detected_category: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub status_code: i32,
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
    pub parquet_file: Option<String>,
}
