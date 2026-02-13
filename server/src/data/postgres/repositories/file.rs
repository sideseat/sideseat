//! File repository for PostgreSQL operations
//!
//! Manages file metadata and trace-file associations for the file storage system.

use sqlx::PgPool;

use crate::core::constants::FILE_CLEANUP_BATCH_SIZE;
use crate::data::postgres::PostgresError;
use crate::data::types::FileRow;

/// Upsert a file record (insert or increment ref_count)
///
/// Returns the new ref_count value.
/// Uses RETURNING for atomic operation to avoid race conditions.
pub async fn upsert_file(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
) -> Result<i64, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    // Use INSERT ... ON CONFLICT with RETURNING for atomic upsert
    // If file exists, increment ref_count; otherwise insert with ref_count=1
    let result: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, ref_count, created_at, updated_at)
        VALUES ($1, $2, $3, $4, 1, $5, $6)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET
            ref_count = files.ref_count + 1,
            updated_at = $7
        RETURNING ref_count
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .bind(media_type)
    .bind(size_bytes)
    .bind(now)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?;

    Ok(result.0)
}

/// Decrement ref_count atomically and return the new value
///
/// Returns None if file doesn't exist, Some(new_ref_count) otherwise.
/// Caller should delete the file if ref_count reaches 0.
pub async fn decrement_ref_count(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    // Use RETURNING for atomic operation
    let result: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE files
        SET ref_count = ref_count - 1, updated_at = $1
        WHERE project_id = $2 AND file_hash = $3
        RETURNING ref_count
        "#,
    )
    .bind(now)
    .bind(project_id)
    .bind(file_hash)
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|(count,)| count))
}

/// Get a file by project and hash
pub async fn get_file(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<FileRow>, PostgresError> {
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>, i64, i64, i64, i64)>(
        r#"
        SELECT id, project_id, file_hash, media_type, size_bytes, ref_count, created_at, updated_at
        FROM files
        WHERE project_id = $1 AND file_hash = $2
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, project_id, file_hash, media_type, size_bytes, ref_count, created_at, updated_at)| {
            FileRow {
                id,
                project_id,
                file_hash,
                media_type,
                size_bytes,
                ref_count,
                created_at,
                updated_at,
            }
        },
    ))
}

/// Check if a file exists
pub async fn file_exists(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    let result: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM files WHERE project_id = $1 AND file_hash = $2")
            .bind(project_id)
            .bind(file_hash)
            .fetch_one(pool)
            .await?;

    Ok(result.0 > 0)
}

/// Delete a file metadata record
pub async fn delete_file(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    let result = sqlx::query("DELETE FROM files WHERE project_id = $1 AND file_hash = $2")
        .bind(project_id)
        .bind(file_hash)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all file records for a project
///
/// Returns the number of files deleted.
pub async fn delete_project_files(pool: &PgPool, project_id: &str) -> Result<u64, PostgresError> {
    let result = sqlx::query("DELETE FROM files WHERE project_id = $1")
        .bind(project_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

/// Insert a trace-file association
pub async fn insert_trace_file(
    pool: &PgPool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO trace_files (trace_id, project_id, file_hash) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get file hashes for traces
///
/// Returns unique file hashes associated with the given trace IDs.
pub async fn get_file_hashes_for_traces(
    pool: &PgPool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Build placeholders for IN clause with numbered parameters
    let placeholders = trace_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 2))
        .collect::<Vec<_>>()
        .join(",");

    let query = format!(
        "SELECT DISTINCT file_hash FROM trace_files WHERE project_id = $1 AND trace_id IN ({})",
        placeholders
    );

    let mut query_builder = sqlx::query_as::<_, (String,)>(&query).bind(project_id);

    for trace_id in trace_ids {
        query_builder = query_builder.bind(trace_id);
    }

    let rows = query_builder.fetch_all(pool).await?;

    Ok(rows.into_iter().map(|(hash,)| hash).collect())
}

/// Delete trace-file associations for traces
pub async fn delete_trace_files(
    pool: &PgPool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(0);
    }

    let placeholders = trace_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 2))
        .collect::<Vec<_>>()
        .join(",");

    let query = format!(
        "DELETE FROM trace_files WHERE project_id = $1 AND trace_id IN ({})",
        placeholders
    );

    let mut query_builder = sqlx::query(&query).bind(project_id);

    for trace_id in trace_ids {
        query_builder = query_builder.bind(trace_id);
    }

    let result = query_builder.execute(pool).await?;

    Ok(result.rows_affected())
}

/// Get total storage used by a project
pub async fn get_project_storage_bytes(
    pool: &PgPool,
    project_id: &str,
) -> Result<i64, PostgresError> {
    let result: (Option<i64>,) =
        sqlx::query_as("SELECT SUM(size_bytes) FROM files WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get all files with zero ref_count across all projects (for global cleanup)
///
/// Returns (project_id, file_hash) pairs for orphaned files.
pub async fn get_orphan_files(pool: &PgPool) -> Result<Vec<(String, String)>, PostgresError> {
    let sql = format!(
        "SELECT project_id, file_hash FROM files WHERE ref_count = 0 LIMIT {}",
        FILE_CLEANUP_BATCH_SIZE
    );
    let rows = sqlx::query_as::<_, (String, String)>(&sql)
        .fetch_all(pool)
        .await?;

    Ok(rows)
}
