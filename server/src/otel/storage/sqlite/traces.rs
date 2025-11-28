//! Trace summary operations

use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::HashMap;

use crate::otel::error::OtelError;
use crate::otel::normalize::NormalizedSpan;

/// Trace summary record
#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub trace_id: String,
    pub session_id: Option<String>,
    pub root_span_id: Option<String>,
    pub root_span_name: Option<String>,
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
    // OTLP StatusCode: 0=UNSET, 1=OK, 2=ERROR
    let has_error = span.status_code == 2;

    sqlx::query(
        r#"
        INSERT INTO traces (
            trace_id, root_span_id, root_span_name, service_name, detected_framework,
            span_count, start_time_ns, end_time_ns, duration_ns,
            total_input_tokens, total_output_tokens, total_tokens,
            has_errors, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(trace_id) DO UPDATE SET
            span_count = span_count + 1,
            root_span_id = COALESCE(root_span_id, excluded.root_span_id),
            root_span_name = COALESCE(root_span_name, excluded.root_span_name),
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
    .bind(if span.parent_span_id.is_none() { Some(&span.span_name) } else { None::<&String> })
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

/// Batch upsert multiple trace summaries in a single transaction
pub async fn upsert_traces_batch(
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

    upsert_traces_batch_with_tx(&mut tx, spans).await?;

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    Ok(())
}

/// Batch upsert traces using an existing transaction (for atomic operations)
pub async fn upsert_traces_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    spans: &[NormalizedSpan],
) -> Result<(), OtelError> {
    if spans.is_empty() {
        return Ok(());
    }

    // Group spans by trace_id to aggregate trace-level data
    let mut trace_spans: HashMap<&str, Vec<&NormalizedSpan>> = HashMap::new();
    for span in spans {
        trace_spans.entry(&span.trace_id).or_default().push(span);
    }

    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    for (trace_id, trace_span_list) in trace_spans {
        // Find root span (no parent) or use first span
        let root_span = trace_span_list
            .iter()
            .find(|s| s.parent_span_id.is_none())
            .unwrap_or(&trace_span_list[0]);

        // Find session_id from first span that has one
        let session_id = trace_span_list.iter().find_map(|s| s.session_id.as_ref());

        // Aggregate values across all spans in this batch for this trace
        let span_count = trace_span_list.len() as i32;
        let min_start = trace_span_list.iter().map(|s| s.start_time_unix_nano).min().unwrap_or(0);
        let max_end = trace_span_list.iter().filter_map(|s| s.end_time_unix_nano).max();
        let total_input: i64 = trace_span_list.iter().filter_map(|s| s.usage_input_tokens).sum();
        let total_output: i64 = trace_span_list.iter().filter_map(|s| s.usage_output_tokens).sum();
        let total_tokens: i64 = trace_span_list.iter().filter_map(|s| s.usage_total_tokens).sum();
        // OTLP StatusCode: 0=UNSET, 1=OK, 2=ERROR
        let has_error = trace_span_list.iter().any(|s| s.status_code == 2);

        sqlx::query(
            r#"
            INSERT INTO traces (
                trace_id, session_id, root_span_id, root_span_name, service_name, detected_framework,
                span_count, start_time_ns, end_time_ns, duration_ns,
                total_input_tokens, total_output_tokens, total_tokens,
                has_errors, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(trace_id) DO UPDATE SET
                session_id = COALESCE(session_id, excluded.session_id),
                span_count = span_count + excluded.span_count,
                root_span_id = COALESCE(root_span_id, excluded.root_span_id),
                root_span_name = COALESCE(root_span_name, excluded.root_span_name),
                end_time_ns = MAX(COALESCE(end_time_ns, 0), COALESCE(excluded.end_time_ns, 0)),
                duration_ns = MAX(COALESCE(end_time_ns, 0), COALESCE(excluded.end_time_ns, 0)) - start_time_ns,
                total_input_tokens = COALESCE(total_input_tokens, 0) + COALESCE(excluded.total_input_tokens, 0),
                total_output_tokens = COALESCE(total_output_tokens, 0) + COALESCE(excluded.total_output_tokens, 0),
                total_tokens = COALESCE(total_tokens, 0) + COALESCE(excluded.total_tokens, 0),
                has_errors = has_errors OR excluded.has_errors,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(trace_id)
        .bind(session_id)
        .bind(if root_span.parent_span_id.is_none() {
            Some(&root_span.span_id)
        } else {
            None::<&String>
        })
        .bind(if root_span.parent_span_id.is_none() {
            Some(&root_span.span_name)
        } else {
            None::<&String>
        })
        .bind(&root_span.service_name)
        .bind(&root_span.detected_framework)
        .bind(span_count)
        .bind(min_start)
        .bind(max_end)
        .bind(max_end.map(|e| e - min_start))
        .bind(if total_input > 0 { Some(total_input) } else { None })
        .bind(if total_output > 0 { Some(total_output) } else { None })
        .bind(if total_tokens > 0 { Some(total_tokens) } else { None })
        .bind(has_error)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to upsert trace: {}", e)))?;
    }

    Ok(())
}

/// Soft delete a trace by setting deleted_at timestamp
/// Also deletes associated EAV attributes (since soft delete doesn't trigger CASCADE)
pub async fn soft_delete_trace(pool: &SqlitePool, trace_id: &str) -> Result<bool, OtelError> {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    // Start transaction for atomic delete
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    // Soft delete the trace
    let result = sqlx::query(
        r#"
        UPDATE traces SET deleted_at = ?, updated_at = ?
        WHERE trace_id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(now)
    .bind(now)
    .bind(trace_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to delete trace: {}", e)))?;

    if result.rows_affected() > 0 {
        // Delete associated trace attributes (EAV cleanup)
        sqlx::query("DELETE FROM trace_attributes WHERE trace_id = ?")
            .bind(trace_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                OtelError::StorageError(format!("Failed to delete trace attributes: {}", e))
            })?;

        // Get all span_ids for this trace to delete their attributes
        let span_ids: Vec<String> =
            sqlx::query_scalar("SELECT span_id FROM spans WHERE trace_id = ?")
                .bind(trace_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| OtelError::StorageError(format!("Failed to get span ids: {}", e)))?;

        // Delete span attributes for all spans in this trace
        if !span_ids.is_empty() {
            let placeholders = vec!["?"; span_ids.len()].join(",");
            let sql = format!("DELETE FROM span_attributes WHERE span_id IN ({})", placeholders);
            let mut query = sqlx::query(&sql);
            for span_id in &span_ids {
                query = query.bind(span_id);
            }
            query.execute(&mut *tx).await.map_err(|e| {
                OtelError::StorageError(format!("Failed to delete span attributes: {}", e))
            })?;
        }
    }

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    Ok(result.rows_affected() > 0)
}

/// Get a trace summary by ID (excludes deleted traces)
pub async fn get_trace(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Option<TraceSummary>, OtelError> {
    let row = sqlx::query_as::<
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
    >(
        r#"
        SELECT trace_id, session_id, root_span_id, root_span_name, service_name, detected_framework,
               span_count, start_time_ns, end_time_ns, duration_ns,
               total_input_tokens, total_output_tokens, total_tokens, has_errors
        FROM traces WHERE trace_id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(trace_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get trace: {}", e)))?;

    Ok(row.map(|r| TraceSummary {
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::schema::SCHEMA;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
        // Initialize storage_stats
        sqlx::query("INSERT INTO storage_stats (id, total_traces, total_spans, total_parquet_bytes, total_parquet_files, last_updated) VALUES (1, 0, 0, 0, 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn insert_test_trace(pool: &SqlitePool, trace_id: &str) {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query(
            r#"
            INSERT INTO traces (
                trace_id, root_span_id, service_name, detected_framework,
                span_count, start_time_ns, end_time_ns, duration_ns,
                total_input_tokens, total_output_tokens, total_tokens,
                has_errors, created_at, updated_at
            ) VALUES (?, ?, ?, ?, 1, ?, ?, ?, NULL, NULL, NULL, 0, ?, ?)
            "#,
        )
        .bind(trace_id)
        .bind(format!("span_{}", trace_id))
        .bind("test-service")
        .bind("unknown")
        .bind(now)
        .bind(now + 1000000)
        .bind(1000000i64)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_soft_delete_trace_success() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        // Verify trace exists
        let trace = get_trace(&pool, "trace-1").await.unwrap();
        assert!(trace.is_some());

        // Soft delete
        let deleted = soft_delete_trace(&pool, "trace-1").await.unwrap();
        assert!(deleted);

        // Verify trace is no longer visible
        let trace = get_trace(&pool, "trace-1").await.unwrap();
        assert!(trace.is_none());
    }

    #[tokio::test]
    async fn test_soft_delete_trace_not_found() {
        let pool = setup_test_db().await;

        // Try to delete non-existent trace
        let deleted = soft_delete_trace(&pool, "non-existent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_soft_delete_trace_already_deleted() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        // First delete should succeed
        let deleted = soft_delete_trace(&pool, "trace-1").await.unwrap();
        assert!(deleted);

        // Second delete should return false (already deleted)
        let deleted = soft_delete_trace(&pool, "trace-1").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_get_trace_excludes_deleted() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;
        insert_test_trace(&pool, "trace-2").await;

        // Both traces exist
        assert!(get_trace(&pool, "trace-1").await.unwrap().is_some());
        assert!(get_trace(&pool, "trace-2").await.unwrap().is_some());

        // Delete one trace
        soft_delete_trace(&pool, "trace-1").await.unwrap();

        // Deleted trace is not visible
        assert!(get_trace(&pool, "trace-1").await.unwrap().is_none());
        // Other trace still visible
        assert!(get_trace(&pool, "trace-2").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_trace_summary_fields() {
        let pool = setup_test_db().await;
        insert_test_trace(&pool, "trace-1").await;

        let trace = get_trace(&pool, "trace-1").await.unwrap().unwrap();
        assert_eq!(trace.trace_id, "trace-1");
        assert_eq!(trace.root_span_id, Some("span_trace-1".to_string()));
        assert_eq!(trace.service_name, "test-service");
        assert_eq!(trace.detected_framework, "unknown");
        assert_eq!(trace.span_count, 1);
        assert!(!trace.has_errors);
    }
}
