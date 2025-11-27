//! Parquet file tracking

use sqlx::SqlitePool;

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

    // Update storage stats
    sqlx::query(
        r#"
        UPDATE storage_stats SET
            total_spans = total_spans + ?,
            total_parquet_bytes = total_parquet_bytes + ?,
            total_parquet_files = total_parquet_files + 1,
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

/// Remove a file record (after deletion)
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

        // Delete file record
        sqlx::query("DELETE FROM parquet_files WHERE file_path = ?")
            .bind(file_path)
            .execute(pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to remove file: {}", e)))?;

        // Update storage stats
        sqlx::query(
            r#"
            UPDATE storage_stats SET
                total_spans = total_spans - ?,
                total_parquet_bytes = total_parquet_bytes - ?,
                total_parquet_files = total_parquet_files - 1,
                last_updated = ?
            WHERE id = 1
            "#,
        )
        .bind(span_count)
        .bind(file_size)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to update stats: {}", e)))?;
    }

    Ok(())
}
