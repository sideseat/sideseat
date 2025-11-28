//! Span index operations

use sqlx::{Sqlite, SqlitePool, Transaction};

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

    insert_spans_batch_with_tx(&mut tx, spans).await?;

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    Ok(())
}

/// Insert spans batch using an existing transaction (for atomic operations)
/// Uses multi-row INSERT for efficiency (up to 50 spans per statement due to SQLite variable limits)
pub async fn insert_spans_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    spans: &[NormalizedSpan],
) -> Result<(), OtelError> {
    if spans.is_empty() {
        return Ok(());
    }

    // SQLite has a default limit of 999 variables per query
    // With 18 columns per span, we can insert ~55 spans per query
    // Use 50 for safety margin
    const CHUNK_SIZE: usize = 50;

    for chunk in spans.chunks(CHUNK_SIZE) {
        let placeholders: Vec<String> = chunk
            .iter()
            .map(|_| "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".to_string())
            .collect();

        let sql = format!(
            r#"
            INSERT OR REPLACE INTO spans (
                span_id, trace_id, session_id, parent_span_id, span_name, service_name,
                detected_framework, detected_category, gen_ai_agent_name,
                gen_ai_tool_name, gen_ai_request_model, start_time_ns,
                end_time_ns, duration_ns, status_code, usage_input_tokens,
                usage_output_tokens, parquet_file
            ) VALUES {}
            "#,
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for span in chunk {
            query = query
                .bind(&span.span_id)
                .bind(&span.trace_id)
                .bind(&span.session_id)
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
                .bind(None::<&str>); // parquet_file will be updated later
        }

        query
            .execute(&mut **tx)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to insert spans batch: {}", e)))?;
    }

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
            span_id, trace_id, session_id, parent_span_id, span_name, service_name,
            detected_framework, detected_category, gen_ai_agent_name,
            gen_ai_tool_name, gen_ai_request_model, start_time_ns,
            end_time_ns, duration_ns, status_code, usage_input_tokens,
            usage_output_tokens, parquet_file
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&span.span_id)
    .bind(&span.trace_id)
    .bind(&span.session_id)
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
        SELECT span_id, trace_id, session_id, parent_span_id, span_name, service_name,
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

/// Update parquet_file for spans in a time range
pub async fn update_spans_parquet_file(
    pool: &SqlitePool,
    parquet_file: &str,
    min_start_time_ns: i64,
    max_end_time_ns: i64,
) -> Result<u64, OtelError> {
    let result = sqlx::query(
        r#"
        UPDATE spans
        SET parquet_file = ?
        WHERE start_time_ns >= ? AND start_time_ns <= ?
          AND parquet_file IS NULL
        "#,
    )
    .bind(parquet_file)
    .bind(min_start_time_ns)
    .bind(max_end_time_ns)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to update spans parquet_file: {}", e)))?;

    Ok(result.rows_affected())
}

/// Update parquet_file for spans in a time range within an existing transaction
pub async fn update_spans_parquet_file_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    parquet_file: &str,
    min_start_time_ns: i64,
    max_end_time_ns: i64,
) -> Result<u64, OtelError> {
    let result = sqlx::query(
        r#"
        UPDATE spans
        SET parquet_file = ?
        WHERE start_time_ns >= ? AND start_time_ns <= ?
          AND parquet_file IS NULL
        "#,
    )
    .bind(parquet_file)
    .bind(min_start_time_ns)
    .bind(max_end_time_ns)
    .execute(&mut **tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to update spans parquet_file: {}", e)))?;

    Ok(result.rows_affected())
}

/// Span index record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SpanIndex {
    pub span_id: String,
    pub trace_id: String,
    pub session_id: Option<String>,
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
