//! Parquet file tracking

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::otel::error::OtelError;

/// Parquet file record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ParquetFileRecord {
    pub file_path: String,
    pub date_partition: String,
    pub span_count: i32,
    pub file_size_bytes: i64,
    pub min_start_time_ns: i64,
    pub max_end_time_ns: i64,
    pub created_at: i64,
}

/// Register a new parquet file
pub async fn register_file(
    pool: &SqlitePool,
    file_path: &str,
    date_partition: &str,
    span_count: i32,
    file_size_bytes: i64,
    min_start_time_ns: i64,
    max_end_time_ns: i64,
) -> Result<(), OtelError> {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    sqlx::query(
        r#"
        INSERT INTO parquet_files (
            file_path, date_partition, span_count, file_size_bytes,
            min_start_time_ns, max_end_time_ns, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(file_path)
    .bind(date_partition)
    .bind(span_count)
    .bind(file_size_bytes)
    .bind(min_start_time_ns)
    .bind(max_end_time_ns)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to register file: {}", e)))?;

    // Update storage stats including total_traces from actual count
    sqlx::query(
        r#"
        UPDATE storage_stats SET
            total_spans = total_spans + ?,
            total_parquet_bytes = total_parquet_bytes + ?,
            total_parquet_files = total_parquet_files + 1,
            total_traces = (SELECT COUNT(*) FROM traces),
            last_updated = ?
        WHERE id = 1
        "#,
    )
    .bind(span_count)
    .bind(file_size_bytes)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to update stats: {}", e)))?;

    Ok(())
}

/// Register a new parquet file within an existing transaction
pub async fn register_file_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    file_path: &str,
    date_partition: &str,
    span_count: i32,
    file_size_bytes: i64,
    min_start_time_ns: i64,
    max_end_time_ns: i64,
) -> Result<(), OtelError> {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

    sqlx::query(
        r#"
        INSERT INTO parquet_files (
            file_path, date_partition, span_count, file_size_bytes,
            min_start_time_ns, max_end_time_ns, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(file_path)
    .bind(date_partition)
    .bind(span_count)
    .bind(file_size_bytes)
    .bind(min_start_time_ns)
    .bind(max_end_time_ns)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to register file: {}", e)))?;

    // Update storage stats including total_traces from actual count
    sqlx::query(
        r#"
        UPDATE storage_stats SET
            total_spans = total_spans + ?,
            total_parquet_bytes = total_parquet_bytes + ?,
            total_parquet_files = total_parquet_files + 1,
            total_traces = (SELECT COUNT(*) FROM traces WHERE deleted_at IS NULL),
            last_updated = ?
        WHERE id = 1
        "#,
    )
    .bind(span_count)
    .bind(file_size_bytes)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to update stats: {}", e)))?;

    Ok(())
}

/// Get files for a date partition
pub async fn get_files_by_date(
    pool: &SqlitePool,
    date_partition: &str,
) -> Result<Vec<ParquetFileRecord>, OtelError> {
    let rows = sqlx::query_as::<_, ParquetFileRecord>(
        r#"
        SELECT file_path, date_partition, span_count, file_size_bytes,
               min_start_time_ns, max_end_time_ns, created_at
        FROM parquet_files WHERE date_partition = ?
        ORDER BY created_at
        "#,
    )
    .bind(date_partition)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get files: {}", e)))?;

    Ok(rows)
}

/// Remove a file record and associated data (after deletion)
/// This cleans up spans (cascade deletes span_attributes), orphan traces
/// (cascade deletes trace_attributes), and updates stats in a single transaction.
/// Requires PRAGMA foreign_keys = ON for cascade deletes to work.
pub async fn remove_file(pool: &SqlitePool, file_path: &str) -> Result<(), OtelError> {
    // Get file info first
    let file = sqlx::query_as::<_, (i32, i64)>(
        "SELECT span_count, file_size_bytes FROM parquet_files WHERE file_path = ?",
    )
    .bind(file_path)
    .fetch_optional(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get file info: {}", e)))?;

    if let Some((span_count, file_size)) = file {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Use a transaction for all cleanup operations
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

        // Delete span_events for spans in this parquet file
        sqlx::query(
            r#"
            DELETE FROM span_events WHERE span_id IN (
                SELECT span_id FROM spans WHERE parquet_file = ?
            )
            "#,
        )
        .bind(file_path)
        .execute(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to delete span events: {}", e)))?;

        // Delete spans by parquet_file (cascade deletes span_attributes)
        sqlx::query("DELETE FROM spans WHERE parquet_file = ?")
            .bind(file_path)
            .execute(&mut *tx)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to delete spans: {}", e)))?;

        // Delete orphaned traces (cascade deletes trace_attributes)
        sqlx::query(
            r#"
            DELETE FROM traces WHERE trace_id NOT IN (
                SELECT DISTINCT trace_id FROM spans
            )
            "#,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to delete orphan traces: {}", e)))?;

        // Delete file record
        sqlx::query("DELETE FROM parquet_files WHERE file_path = ?")
            .bind(file_path)
            .execute(&mut *tx)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to remove file: {}", e)))?;

        // Update storage stats (spans, bytes, files, and traces count)
        sqlx::query(
            r#"
            UPDATE storage_stats SET
                total_spans = total_spans - ?,
                total_parquet_bytes = total_parquet_bytes - ?,
                total_parquet_files = total_parquet_files - 1,
                total_traces = (SELECT COUNT(*) FROM traces),
                last_updated = ?
            WHERE id = 1
            "#,
        )
        .bind(span_count)
        .bind(file_size)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to update stats: {}", e)))?;

        tx.commit()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;
    }

    Ok(())
}
