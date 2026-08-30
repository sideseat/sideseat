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
    hash_algo: &str,
) -> Result<i64, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    // Use INSERT ... ON CONFLICT with RETURNING for atomic upsert
    // If file exists, increment ref_count; otherwise insert with ref_count=1
    let result: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, 1, ?, ?)
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
    .bind(hash_algo)
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
        WHERE project_id = ? AND file_hash = ? AND ref_count > 0
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
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>, i64, String, i64, i64, i64)>(
        r#"
        SELECT id, project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at
        FROM files
        WHERE project_id = ? AND file_hash = ?
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            project_id,
            file_hash,
            media_type,
            size_bytes,
            hash_algo,
            ref_count,
            created_at,
            updated_at,
        )| {
            FileRow {
                id,
                project_id,
                file_hash,
                media_type,
                size_bytes,
                hash_algo,
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

/// Set a file's reference count to the number of associations that actually exist.
///
/// Derived, not maintained. `ref_count` is a cached `COUNT(*)` over `trace_files`, and every way it
/// drifted came from trying to keep the cache in step by hand: increment once per batch instead of per
/// association, decrement once per hash instead of per association, or - the one no careful pairing can
/// fix - two concurrent cleanups that both read a count of three and both subtract three, taking a file
/// that four traces referenced down to zero.
///
/// Recomputing is idempotent and immune to all of it: whatever else happened concurrently, the count
/// becomes the truth. Cheap, because `(project_id, file_hash)` is the association table's key prefix.
pub async fn sync_ref_count(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let result: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE files
        SET ref_count = (
                SELECT COUNT(*) FROM trace_files
                WHERE project_id = files.project_id AND file_hash = files.file_hash
            ),
            updated_at = ?
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

/// Associate a file with a trace, and count the reference **only if the association is new**.
///
/// One transaction, because the two halves have to agree: `ref_count` must equal the number of
/// associations, since that is what deletion decrements. Doing them separately drifted both ways -
/// a retry re-incremented while `INSERT OR IGNORE` kept the existing association, and a failure
/// between the two left an increment with no association, so the file outlived every trace that
/// referenced it.
///
/// Returns whether the association was new, which is also whether the count moved.
pub async fn associate_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;

    // The file row must exist before the association can reference it, and must not be counted here.
    sqlx::query(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, 0, ?, ?)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET updated_at = ?
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .bind(media_type)
    .bind(size_bytes)
    .bind(hash_algo)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO trace_files (trace_id, project_id, file_hash) VALUES (?, ?, ?)",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    // Recomputed from the associations, not incremented. Same reasoning as `sync_ref_count`: a
    // maintained counter drifts, a derived one cannot. Inside the transaction, so the association it
    // counts is the one just inserted.
    if inserted {
        sqlx::query(
            r#"
            UPDATE files
            SET ref_count = (
                    SELECT COUNT(*) FROM trace_files
                    WHERE project_id = files.project_id AND file_hash = files.file_hash
                ),
                updated_at = ?
            WHERE project_id = ? AND file_hash = ?
            "#,
        )
        .bind(now)
        .bind(project_id)
        .bind(file_hash)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(inserted)
}

/// How many of these traces reference each file, so deletion can decrement by that many.
///
/// `get_file_hashes_for_traces` returns each hash once, and deletion decremented once per hash - so
/// deleting three traces that all referenced one file removed three associations and one reference.
pub async fn get_file_reference_counts_for_traces(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<(String, i64)>, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = trace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let query = format!(
        "SELECT file_hash, COUNT(*) FROM trace_files \
         WHERE project_id = ? AND trace_id IN ({placeholders}) GROUP BY file_hash"
    );
    let mut q = sqlx::query_as(&query).bind(project_id);
    for trace_id in trace_ids {
        q = q.bind(trace_id);
    }
    Ok(q.fetch_all(pool).await?)
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

/// Get total file storage used by all projects in an organization
pub async fn get_org_file_storage_bytes(
    pool: &SqlitePool,
    org_id: &str,
) -> Result<i64, SqliteError> {
    let result: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(f.size_bytes), 0)
        FROM files f
        JOIN projects p ON f.project_id = p.id
        WHERE p.organization_id = ?
        "#,
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get total file storage used across all orgs a user belongs to
pub async fn get_user_file_storage_bytes(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<i64, SqliteError> {
    let result: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(f.size_bytes), 0)
        FROM files f
        JOIN projects p ON f.project_id = p.id
        JOIN organization_members m ON p.organization_id = m.organization_id
        WHERE m.user_id = ?
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get all files with zero ref_count across all projects (for global cleanup)
///
/// Returns (project_id, file_hash) pairs for orphaned files.
pub async fn get_orphan_files(pool: &SqlitePool) -> Result<Vec<(String, String)>, SqliteError> {
    let sql = format!(
        "SELECT project_id, file_hash FROM files WHERE ref_count = 0 ORDER BY created_at ASC LIMIT {}",
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

        let ref_count = upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
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

        let ref1 = upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        assert_eq!(ref1, 1);

        let ref2 = upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        assert_eq!(ref2, 2);

        let ref3 = upsert_file(
            &pool,
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        assert_eq!(ref3, 3);
    }

    #[tokio::test]
    async fn test_decrement_ref_count() {
        let pool = setup_test_pool().await;

        upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
            .await
            .unwrap();
        upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
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

        upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
            .await
            .unwrap();

        assert!(file_exists(&pool, "default", test_hash()).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_file() {
        let pool = setup_test_pool().await;

        upsert_file(&pool, "default", test_hash(), None, 1024, "sha256")
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
        upsert_file(&pool, "default", hash1, None, 1024, "sha256")
            .await
            .unwrap();
        upsert_file(&pool, "default", hash2, None, 2048, "sha256")
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

        upsert_file(&pool, "default", hash, None, 1024, "sha256")
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
            "sha256",
        )
        .await
        .unwrap();
        upsert_file(
            &pool,
            "default",
            "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
            None,
            2048,
            "sha256",
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

        upsert_file(&pool, "project1", hash, None, 1024, "sha256")
            .await
            .unwrap();
        upsert_file(&pool, "project2", hash, None, 2048, "sha256")
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

    /// Two cleanups deleting overlapping trace sets must not release a reference twice.
    ///
    /// The case a maintained counter cannot survive: four traces reference one file, two cleanups both
    /// read a count of three for the same three traces, and both subtract three - taking the count to
    /// zero and deleting a file the fourth trace still shows. Derived from the associations, the count
    /// is simply what remains, however many times it is recomputed.
    #[tokio::test]
    async fn recomputing_a_reference_count_is_idempotent() {
        let pool = setup_test_pool().await;
        let all = ["t1", "t2", "t3", "t4"];
        for trace in all {
            associate_file(
                &pool,
                trace,
                "default",
                test_hash(),
                Some("image/png"),
                1024,
                "sha256",
            )
            .await
            .unwrap();
        }

        // Both cleanups target the same three traces; only one deletion actually removes rows.
        let doomed: Vec<String> = ["t1", "t2", "t3"].iter().map(|t| t.to_string()).collect();
        delete_trace_files(&pool, "default", &doomed).await.unwrap();

        // Two recomputations, as two concurrent cleanups would each do.
        let first = sync_ref_count(&pool, "default", test_hash()).await.unwrap();
        let second = sync_ref_count(&pool, "default", test_hash()).await.unwrap();

        assert_eq!(first, Some(1), "t4 still references the file");
        assert_eq!(
            second, first,
            "recomputing again must not release t4's reference a second time"
        );
    }

    /// Two projects can present the same trace id, and their associations must not collide.
    ///
    /// A trace id comes from the client. Keyed without the project, the first project's association
    /// satisfied `INSERT OR IGNORE` for the second - so the second got no association, `associate_file`
    /// reported "not new" and skipped the increment, and the second project's file was left with a
    /// reference count nothing would ever release.
    #[tokio::test]
    async fn two_projects_sharing_a_trace_id_each_get_their_own_association() {
        let pool = setup_test_pool().await;

        for project in ["project-a", "project-b"] {
            let inserted = associate_file(
                &pool,
                "same-trace-id",
                project,
                test_hash(),
                Some("image/png"),
                1024,
                "sha256",
            )
            .await
            .unwrap();
            assert!(
                inserted,
                "{project} must get its own association for a trace id it happens to share"
            );
            let file = get_file(&pool, project, test_hash())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(file.ref_count, 1, "{project} holds exactly one reference");
        }
    }

    /// A retry must not count the same reference twice.
    ///
    /// With the increment separate from the association, `INSERT OR IGNORE` kept the existing
    /// association while the increment ran again - so a redelivered batch inflated the count and the
    /// file became uncollectable after its traces were gone.
    #[tokio::test]
    async fn associating_the_same_file_twice_counts_it_once() {
        let pool = setup_test_pool().await;

        let first = associate_file(
            &pool,
            "trace-a",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        let second = associate_file(
            &pool,
            "trace-a",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();

        assert!(first, "the first association is new");
        assert!(!second, "the second is a retry of the same association");

        let file = get_file(&pool, "default", test_hash())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            file.ref_count, 1,
            "one association means one reference, however many times the batch is delivered"
        );
    }

    /// Deleting several traces that share a file must release every reference they held.
    ///
    /// The loop decremented once per distinct hash, so deleting three traces that referenced one file
    /// removed three associations and one reference - leaving a file nothing points at and nothing will
    /// ever collect.
    #[tokio::test]
    async fn deleting_traces_releases_every_reference_they_held() {
        let pool = setup_test_pool().await;
        let traces = ["trace-a", "trace-b", "trace-c"];
        for trace in traces {
            associate_file(
                &pool,
                trace,
                "default",
                test_hash(),
                Some("image/png"),
                1024,
                "sha256",
            )
            .await
            .unwrap();
        }

        let counts = get_file_reference_counts_for_traces(
            &pool,
            "default",
            &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert_eq!(counts, vec![(test_hash().to_string(), 3)]);

        delete_trace_files(
            &pool,
            "default",
            &traces.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let released = sync_ref_count(&pool, "default", test_hash()).await.unwrap();
        assert_eq!(
            released,
            Some(0),
            "all three references released at once, so the file is collectable"
        );
    }

    /// `ref_count` must equal the number of trace associations, or deleting one trace deletes a file
    /// another trace still shows.
    ///
    /// The defect this pins: ingestion incremented once per batch-unique hash while associating per
    /// trace, so two traces of one batch sharing a file held two associations and one count. Deleting
    /// either took the count to zero and removed the bytes from under the other.
    #[tokio::test]
    async fn ref_count_matches_the_number_of_trace_associations() {
        let pool = setup_test_pool().await;

        // Two traces of one batch reference the same content-addressed file. Ingestion increments
        // once per association, which is what the loop in `write_and_record_files` now does.
        for trace in ["trace-a", "trace-b"] {
            upsert_file(
                &pool,
                "default",
                test_hash(),
                Some("image/png"),
                1024,
                "sha256",
            )
            .await
            .unwrap();
            insert_trace_file(&pool, trace, "default", test_hash())
                .await
                .unwrap();
        }

        // Deleting the first trace must leave the file alive for the second.
        delete_trace_files(&pool, "default", &["trace-a".to_string()])
            .await
            .unwrap();
        let after_first = decrement_ref_count(&pool, "default", test_hash())
            .await
            .unwrap();
        assert_eq!(
            after_first,
            Some(1),
            "the file is still referenced by trace-b, so it must not be collectable"
        );

        // Deleting the second releases it.
        delete_trace_files(&pool, "default", &["trace-b".to_string()])
            .await
            .unwrap();
        let after_second = decrement_ref_count(&pool, "default", test_hash())
            .await
            .unwrap();
        assert_eq!(
            after_second,
            Some(0),
            "with no trace referencing it the file is collectable"
        );
    }
}
