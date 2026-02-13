//! File repository for SQLite operations
//!
//! Manages file metadata and trace-file associations for the file storage system.

use sqlx::SqlitePool;

use crate::core::constants::FILE_CLEANUP_BATCH_SIZE;
use crate::data::sqlite::SqliteError;
use crate::data::types::FileRow;

/// Upsert a file record (insert or increment ref_count)
///
/// Returns the new ref_count value.
/// Uses RETURNING for atomic operation to avoid race conditions.
pub async fn upsert_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
) -> Result<i64, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    // Use INSERT ... ON CONFLICT with RETURNING for atomic upsert
    // If file exists, increment ref_count; otherwise insert with ref_count=1
    let result: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, ref_count, created_at, updated_at)
        VALUES (?, ?, ?, ?, 1, ?, ?)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET
            ref_count = ref_count + 1,
            updated_at = ?
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
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    // Use RETURNING for atomic operation
    let result: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE files
        SET ref_count = ref_count - 1, updated_at = ?
        WHERE project_id = ? AND file_hash = ?
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
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<FileRow>, SqliteError> {
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>, i64, i64, i64, i64)>(
        r#"
        SELECT id, project_id, file_hash, media_type, size_bytes, ref_count, created_at, updated_at
        FROM files
        WHERE project_id = ? AND file_hash = ?
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
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    let result: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM files WHERE project_id = ? AND file_hash = ?")
            .bind(project_id)
            .bind(file_hash)
            .fetch_one(pool)
            .await?;

    Ok(result.0 > 0)
}

/// Delete a file metadata record
pub async fn delete_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    let result = sqlx::query("DELETE FROM files WHERE project_id = ? AND file_hash = ?")
        .bind(project_id)
        .bind(file_hash)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all file records for a project
///
/// Returns the number of files deleted.
pub async fn delete_project_files(pool: &SqlitePool, project_id: &str) -> Result<u64, SqliteError> {
    let result = sqlx::query("DELETE FROM files WHERE project_id = ?")
        .bind(project_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

/// Insert a trace-file association
pub async fn insert_trace_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
) -> Result<(), SqliteError> {
    sqlx::query(
        "INSERT OR IGNORE INTO trace_files (trace_id, project_id, file_hash) VALUES (?, ?, ?)",
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
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Build placeholders for IN clause
    let placeholders = trace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    let query = format!(
        "SELECT DISTINCT file_hash FROM trace_files WHERE project_id = ? AND trace_id IN ({})",
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
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(0);
    }

    let placeholders = trace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    let query = format!(
        "DELETE FROM trace_files WHERE project_id = ? AND trace_id IN ({})",
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
    pool: &SqlitePool,
    project_id: &str,
) -> Result<i64, SqliteError> {
    let result: (Option<i64>,) =
        sqlx::query_as("SELECT SUM(size_bytes) FROM files WHERE project_id = ?")
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get all files with zero ref_count across all projects (for global cleanup)
///
/// Returns (project_id, file_hash) pairs for orphaned files.
pub async fn get_orphan_files(pool: &SqlitePool) -> Result<Vec<(String, String)>, SqliteError> {
    let sql = format!(
        "SELECT project_id, file_hash FROM files WHERE ref_count = 0 LIMIT {}",
        FILE_CLEANUP_BATCH_SIZE
    );
    let rows = sqlx::query_as::<_, (String, String)>(&sql)
        .fetch_all(pool)
        .await?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Apply full schema (includes files and trace_files tables)
        for statement in crate::data::sqlite::schema::SCHEMA
            .split(';')
            .filter(|s| !s.trim().is_empty())
        {
            sqlx::query(statement.trim()).execute(&pool).await.unwrap();
        }

        pool
    }

    fn test_hash() -> &'static str {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
    }

    #[tokio::test]
    async fn test_upsert_file_new() {
        let pool = setup_test_pool().await;

        let ref_count = upsert_file(&pool, "default", test_hash(), Some("image/png"), 1024)
            .await
            .unwrap();

        assert_eq!(ref_count, 1);

        let file = get_file(&pool, "default", test_hash()).await.unwrap();
        assert!(file.is_some());
        let file = file.unwrap();
        assert_eq!(file.project_id, "default");
        assert_eq!(file.file_hash, test_hash());
        assert_eq!(file.media_type, Some("image/png".to_string()));
        assert_eq!(file.size_bytes, 1024);
        assert_eq!(file.ref_count, 1);
    }

    #[tokio::test]
    async fn test_upsert_file_increments_ref_count() {
        let pool = setup_test_pool().await;

        let ref1 = upsert_file(&pool, "default", test_hash(), Some("image/png"), 1024)
            .await
            .unwrap();
        assert_eq!(ref1, 1);

        let ref2 = upsert_file(&pool, "default", test_hash(), Some("image/png"), 1024)
            .await
            .unwrap();
        assert_eq!(ref2, 2);

        let ref3 = upsert_file(&pool, "default", test_hash(), Some("image/png"), 1024)
            .await
            .unwrap();
        assert_eq!(ref3, 3);
    }

    #[tokio::test]
    async fn test_decrement_ref_count() {
        let pool = setup_test_pool().await;

        upsert_file(&pool, "default", test_hash(), None, 1024)
            .await
            .unwrap();
        upsert_file(&pool, "default", test_hash(), None, 1024)
            .await
            .unwrap();

        let new_count = decrement_ref_count(&pool, "default", test_hash())
            .await
            .unwrap();
        assert_eq!(new_count, Some(1));

        let new_count = decrement_ref_count(&pool, "default", test_hash())
            .await
            .unwrap();
        assert_eq!(new_count, Some(0));
    }

    #[tokio::test]
    async fn test_decrement_ref_count_not_found() {
        let pool = setup_test_pool().await;

        let result = decrement_ref_count(&pool, "default", test_hash())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_file_exists() {
        let pool = setup_test_pool().await;

        assert!(!file_exists(&pool, "default", test_hash()).await.unwrap());

        upsert_file(&pool, "default", test_hash(), None, 1024)
            .await
            .unwrap();

        assert!(file_exists(&pool, "default", test_hash()).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_file() {
        let pool = setup_test_pool().await;

        upsert_file(&pool, "default", test_hash(), None, 1024)
            .await
            .unwrap();

        let deleted = delete_file(&pool, "default", test_hash()).await.unwrap();
        assert!(deleted);

        assert!(!file_exists(&pool, "default", test_hash()).await.unwrap());
    }

    #[tokio::test]
    async fn test_trace_file_associations() {
        let pool = setup_test_pool().await;
        let hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let hash2 = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";

        // Insert file records first
        upsert_file(&pool, "default", hash1, None, 1024)
            .await
            .unwrap();
        upsert_file(&pool, "default", hash2, None, 2048)
            .await
            .unwrap();

        // Associate with trace
        insert_trace_file(&pool, "trace1", "default", hash1)
            .await
            .unwrap();
        insert_trace_file(&pool, "trace1", "default", hash2)
            .await
            .unwrap();
        insert_trace_file(&pool, "trace2", "default", hash1)
            .await
            .unwrap();

        // Get hashes for trace1
        let hashes = get_file_hashes_for_traces(&pool, "default", &["trace1".to_string()])
            .await
            .unwrap();
        assert_eq!(hashes.len(), 2);

        // Get hashes for both traces
        let hashes = get_file_hashes_for_traces(
            &pool,
            "default",
            &["trace1".to_string(), "trace2".to_string()],
        )
        .await
        .unwrap();
        // hash1 appears in both, hash2 only in trace1 - should deduplicate
        assert_eq!(hashes.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_trace_files() {
        let pool = setup_test_pool().await;
        let hash = test_hash();

        upsert_file(&pool, "default", hash, None, 1024)
            .await
            .unwrap();
        insert_trace_file(&pool, "trace1", "default", hash)
            .await
            .unwrap();

        let deleted = delete_trace_files(&pool, "default", &["trace1".to_string()])
            .await
            .unwrap();
        assert_eq!(deleted, 1);

        let hashes = get_file_hashes_for_traces(&pool, "default", &["trace1".to_string()])
            .await
            .unwrap();
        assert!(hashes.is_empty());
    }

    #[tokio::test]
    async fn test_get_project_storage_bytes() {
        let pool = setup_test_pool().await;

        let bytes = get_project_storage_bytes(&pool, "default").await.unwrap();
        assert_eq!(bytes, 0);

        upsert_file(
            &pool,
            "default",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            None,
            1024,
        )
        .await
        .unwrap();
        upsert_file(
            &pool,
            "default",
            "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
            None,
            2048,
        )
        .await
        .unwrap();

        let bytes = get_project_storage_bytes(&pool, "default").await.unwrap();
        assert_eq!(bytes, 3072);
    }

    #[tokio::test]
    async fn test_project_isolation() {
        let pool = setup_test_pool().await;
        let hash = test_hash();

        upsert_file(&pool, "project1", hash, None, 1024)
            .await
            .unwrap();
        upsert_file(&pool, "project2", hash, None, 2048)
            .await
            .unwrap();

        let file1 = get_file(&pool, "project1", hash).await.unwrap().unwrap();
        let file2 = get_file(&pool, "project2", hash).await.unwrap().unwrap();

        assert_eq!(file1.size_bytes, 1024);
        assert_eq!(file2.size_bytes, 2048);

        // Storage should be separate
        assert_eq!(
            get_project_storage_bytes(&pool, "project1").await.unwrap(),
            1024
        );
        assert_eq!(
            get_project_storage_bytes(&pool, "project2").await.unwrap(),
            2048
        );
    }
}
