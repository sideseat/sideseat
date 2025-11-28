//! Fast indexed SQLite queries

use sqlx::SqlitePool;

use crate::otel::error::OtelError;
use crate::otel::storage::sqlite::{SpanIndex, TraceSummary};

/// List traces with basic filters (fast indexed query)
pub async fn list_traces(
    pool: &SqlitePool,
    service_name: Option<&str>,
    framework: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TraceSummary>, OtelError> {
    let mut sql = String::from(
        "SELECT trace_id, session_id, root_span_id, root_span_name, service_name, detected_framework, \
         span_count, start_time_ns, end_time_ns, duration_ns, \
         total_input_tokens, total_output_tokens, total_tokens, has_errors \
         FROM traces WHERE deleted_at IS NULL",
    );

    if service_name.is_some() {
        sql.push_str(" AND service_name = ?");
    }
    if framework.is_some() {
        sql.push_str(" AND detected_framework = ?");
    }

    sql.push_str(" ORDER BY start_time_ns DESC LIMIT ? OFFSET ?");

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
    >(&sql);

    if let Some(s) = service_name {
        query = query.bind(s);
    }
    if let Some(f) = framework {
        query = query.bind(f);
    }
    query = query.bind(limit).bind(offset);

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

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

/// Get trace by ID
pub async fn get_trace_by_id(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Option<TraceSummary>, OtelError> {
    crate::otel::storage::sqlite::traces::get_trace(pool, trace_id).await
}

/// Get spans by trace ID
pub async fn get_spans_by_trace_id(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<Vec<SpanIndex>, OtelError> {
    crate::otel::storage::sqlite::spans::get_spans_by_trace(pool, trace_id).await
}

/// Count traces by framework
pub async fn count_traces_by_framework(pool: &SqlitePool) -> Result<Vec<(String, i64)>, OtelError> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT detected_framework, COUNT(*) as count FROM traces GROUP BY detected_framework ORDER BY count DESC"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(rows)
}

/// Count traces by service
pub async fn count_traces_by_service(pool: &SqlitePool) -> Result<Vec<(String, i64)>, OtelError> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT service_name, COUNT(*) as count FROM traces GROUP BY service_name ORDER BY count DESC"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(rows)
}

/// Get storage stats
pub async fn get_storage_stats(pool: &SqlitePool) -> Result<StorageStats, OtelError> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT total_traces, total_spans, total_parquet_bytes, total_parquet_files FROM storage_stats WHERE id = 1"
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Query failed: {}", e)))?;

    Ok(row
        .map(|(traces, spans, bytes, files)| StorageStats {
            total_traces: traces,
            total_spans: spans,
            total_parquet_bytes: bytes,
            total_parquet_files: files,
        })
        .unwrap_or_default())
}

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    pub total_traces: i64,
    pub total_spans: i64,
    pub total_parquet_bytes: i64,
    pub total_parquet_files: i64,
}
