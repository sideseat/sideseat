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

/// Associate a trace with a file that **already exists**, without inventing metadata for it.
///
/// For a reference that arrived already formed: its bytes are in storage, so the size and media type are
/// facts this process does not have. `associate_file` would create a row with size 0, undercounting the
/// project's quota forever - and checking for the row first and then calling it is a race, because the
/// row can vanish in between.
///
/// Returns false when there is no such file, or when it is claimed for deletion. Both mean the caller
/// must not commit a reference to it.
pub async fn associate_existing_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    let mut tx = pool.begin().await?;

    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM files WHERE project_id = ? AND file_hash = ?")
            .bind(project_id)
            .bind(file_hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((claim,)) = row else {
        return Ok(false);
    };
    if claim.is_some() {
        return Ok(false);
    }

    sqlx::query(
        // Provisional, like every association a batch creates: a reference that arrived already formed is
        // still only justified once the span carrying it commits.
        "INSERT OR IGNORE INTO trace_files (trace_id, project_id, file_hash, provisional) \
         VALUES (?, ?, ?, 1)",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;

    let now = chrono::Utc::now().timestamp();
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

    tx.commit().await?;
    Ok(true)
}

/// Files claimed for deletion longer ago than `older_than`, so an abandoned claim can be resumed.
///
/// A claim is durable on purpose - that is what makes it a fence - which means a crash between claiming
/// and finishing leaves it set. Without this the file is stuck: the sweep sees a zero-reference row,
/// tries to claim it, gets refused by the claim already there, and skips it forever, while ingestion
/// keeps failing its batch on the same fence.
pub async fn get_stale_claimed_files(
    pool: &SqlitePool,
    older_than_secs: i64,
) -> Result<Vec<(String, String)>, SqliteError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    Ok(sqlx::query_as(
        "SELECT project_id, file_hash FROM files WHERE deleting_at IS NOT NULL AND deleting_at <= ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

/// Claim a file for deletion, if nothing references it and nobody else has claimed it.
///
/// The fence. Deleting the metadata row and then the bytes leaves a window in which ingestion recreates
/// the row, writes an association and finalises the bytes - and the byte delete then removes content a
/// committed row references. A reference count cannot express "deletion in progress"; this claim can,
/// and `associate_file` refuses while it is set.
pub async fn claim_file_for_deletion(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        r#"
        UPDATE files SET deleting_at = ?
        WHERE project_id = ? AND file_hash = ? AND deleting_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM trace_files
              WHERE project_id = files.project_id AND file_hash = files.file_hash
          )
        "#,
    )
    .bind(now)
    .bind(project_id)
    .bind(file_hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Give up a deletion claim, leaving the file in place.
pub async fn release_deletion_claim(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<(), SqliteError> {
    sqlx::query("UPDATE files SET deleting_at = NULL WHERE project_id = ? AND file_hash = ?")
        .bind(project_id)
        .bind(file_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Put back a metadata row whose bytes could not be deleted, as an orphan for a later sweep.
///
/// Deleting the row before the bytes is deliberate - the surviving failure is then a leak rather than a
/// row pointing at nothing a reader can fetch. But `get_orphan_files` selects on `ref_count`, so with
/// the row gone the leak is invisible and nothing would ever retry. Restoring it with no references
/// makes it an orphan again, which is exactly what the sweep looks for.
pub async fn restore_orphan_metadata(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<(), SqliteError> {
    let now = chrono::Utc::now().timestamp();
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
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a file's metadata **only if nothing references it**, and say whether it was deleted.
///
/// The condition is part of the statement, which is what makes it safe against a concurrent
/// association. Reading a count of zero and then deleting is not: cleanup can recompute zero,
/// ingestion can associate a new trace, and the delete still fires - taking the bytes out from under a
/// span that was just committed. Here the association makes the delete match nothing, cleanup sees
/// that it deleted nothing, and the file stays.
pub async fn delete_file_if_unreferenced(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    let result = sqlx::query(
        r#"
        DELETE FROM files
        WHERE project_id = ? AND file_hash = ?
          AND NOT EXISTS (
              SELECT 1 FROM trace_files
              WHERE project_id = files.project_id AND file_hash = files.file_hash
          )
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
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
/// becomes the truth. `idx_trace_files_project_hash` is what makes it cheap: the primary key leads
/// with `project_id` but separates it from `file_hash` by `trace_id`, so the count needs its own index.
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

    // Refuse through the fence. A claimed file is mid-deletion and its bytes may already be gone, so
    // associating with it would commit a reference to nothing. Refusing fails the batch, and the retry
    // finds the file either deleted - and writes it again, with the bytes in hand - or released.
    //
    // SQLite has one writer, and a transaction that reads and then writes fails with a busy error if
    // another connection wrote in between - so the read-then-check pattern fails *safe* here, refusing
    // the batch. Postgres needs `FOR UPDATE` to get the same guarantee; see the twin.
    let claimed: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM files WHERE project_id = ? AND file_hash = ?")
            .bind(project_id)
            .bind(file_hash)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((Some(_),)) = claimed {
        return Err(SqliteError::Conflict(format!(
            "file {file_hash} in project {project_id} is being deleted"
        )));
    }

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

    // Created *provisional*: the analytics row that justifies it has not committed yet. The creating batch
    // confirms it on success (`confirm_trace_file_associations`) and the failure path deletes only rows that
    // are still provisional - so a second batch that committed in between is not affected.
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO trace_files (trace_id, project_id, file_hash, provisional) \
         VALUES (?, ?, ?, 1)",
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
) -> Result<Vec<String>, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = trace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // `RETURNING`, so the caller reconciles exactly the hashes this statement removed.
    //
    // Reading the hashes first and deleting afterwards is a different set: an association added in between
    // is deleted here but absent from the read, so its file's stored count is never recomputed - and the
    // orphan sweeper selects on that count, so nothing ever reclaims it. Taking the set *from the delete*
    // cannot miss one.
    let query = format!(
        "DELETE FROM trace_files WHERE project_id = ? AND trace_id IN ({}) RETURNING file_hash",
        placeholders
    );

    let mut query_builder = sqlx::query_scalar::<_, String>(&query).bind(project_id);

    for trace_id in trace_ids {
        query_builder = query_builder.bind(trace_id);
    }

    Ok(query_builder.fetch_all(pool).await?)
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

/// Release one association created by a batch whose analytics write then failed.
///
/// See `TransactionalRepository::release_trace_file_association`. The caller follows this with
/// `sync_ref_count`, which recomputes the count from the associations that remain - so the count cannot
/// drift from the truth however the two interleave with a concurrent batch.
pub async fn release_trace_file_association(
    pool: &SqlitePool,
    project_id: &str,
    trace_id: &str,
    file_hash: &str,
) -> Result<bool, SqliteError> {
    // `provisional = 1` in the predicate is what makes this safe rather than merely precise. Two batches
    // can carry the same association; the second sees it present, commits its span, and clears the flag - so
    // this delete matches nothing and its file stays. A read-then-delete pair could not promise that.
    let result = sqlx::query(
        "DELETE FROM trace_files \
         WHERE project_id = ? AND trace_id = ? AND file_hash = ? AND provisional = 1",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(file_hash)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark a batch's associations as no longer provisional, now that its analytics rows are committed.
///
/// One statement per association, and idempotent: clearing an already-cleared flag is a no-op. Called after
/// a successful write, before anything else can observe the batch as failed.
pub async fn confirm_trace_file_associations(
    pool: &SqlitePool,
    associations: &[(String, String, String)],
) -> Result<u64, SqliteError> {
    if associations.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut confirmed = 0u64;
    for (project_id, trace_id, file_hash) in associations {
        let result = sqlx::query(
            "UPDATE trace_files SET provisional = 0 \
             WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(file_hash)
        .execute(&mut *tx)
        .await?;
        confirmed += result.rows_affected();
    }
    tx.commit().await?;
    Ok(confirmed)
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
        assert_eq!(
            deleted.len(),
            1,
            "the delete reports the hashes it removed, which is the set the caller must reconcile"
        );

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

    /// A claimed file cannot be associated, so its bytes cannot be deleted out from under a new row.
    ///
    /// The interleaving a conditional delete alone leaves open: the row is deleted, ingestion recreates
    /// it, associates and finalises the bytes, and cleanup then deletes those bytes - leaving a
    /// committed reference to nothing. Refusing association through the claim is what closes it; the
    /// batch fails and its retry finds the file either gone, and writes it again, or released.
    #[tokio::test]
    async fn a_claimed_file_refuses_association() {
        let pool = setup_test_pool().await;
        associate_file(
            &pool,
            "old-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        delete_trace_files(&pool, "default", &["old-trace".to_string()])
            .await
            .unwrap();

        assert!(
            claim_file_for_deletion(&pool, "default", test_hash())
                .await
                .unwrap(),
            "nothing references it, so it can be claimed"
        );
        assert!(
            !claim_file_for_deletion(&pool, "default", test_hash())
                .await
                .unwrap(),
            "and a second cleanup cannot claim it as well"
        );

        // Ingestion must not be able to reference bytes that are being deleted.
        let associated = associate_file(
            &pool,
            "new-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await;
        assert!(
            associated.is_err(),
            "associating with a claimed file must fail so the batch is refused"
        );

        // Released, it is available again.
        release_deletion_claim(&pool, "default", test_hash())
            .await
            .unwrap();
        assert!(
            associate_file(
                &pool,
                "new-trace",
                "default",
                test_hash(),
                Some("image/png"),
                1024,
                "sha256",
            )
            .await
            .is_ok(),
            "once the claim is released the file can be referenced again"
        );
    }

    /// An abandoned claim is findable, so a crash mid-deletion does not strand the file forever.
    ///
    /// The claim is durable by design - that is the whole point of a fence - so nothing releases it if the
    /// process dies holding it. Then every sweep sees a zero-reference row it cannot claim and skips it,
    /// and every ingestion naming that file fails its batch. It has to be discoverable by age.
    #[tokio::test]
    async fn an_abandoned_claim_is_found_by_age_and_a_fresh_one_is_not() {
        let pool = setup_test_pool().await;
        associate_file(
            &pool,
            "old-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();
        delete_trace_files(&pool, "default", &["old-trace".to_string()])
            .await
            .unwrap();
        assert!(
            claim_file_for_deletion(&pool, "default", test_hash())
                .await
                .unwrap()
        );

        assert!(
            get_stale_claimed_files(&pool, 900)
                .await
                .unwrap()
                .is_empty(),
            "a claim taken a moment ago is a deletion in progress, not an abandoned one"
        );

        // Age it past the threshold rather than sleeping.
        sqlx::query("UPDATE files SET deleting_at = deleting_at - 1000 WHERE file_hash = ?")
            .bind(test_hash())
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            get_stale_claimed_files(&pool, 900).await.unwrap(),
            vec![("default".to_string(), test_hash().to_string())],
            "an old claim is reported so the sweep can finish what the crash left"
        );

        // And finishing it is exactly the normal path: the row goes, nothing is left claimed.
        assert!(
            delete_file_if_unreferenced(&pool, "default", test_hash())
                .await
                .unwrap()
        );
        assert!(
            get_stale_claimed_files(&pool, 900)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A file re-associated between the count and the delete must survive.
    ///
    /// The interleaving a count-then-delete cannot survive: cleanup recomputes zero, ingestion
    /// associates a new trace, and the delete fires anyway - taking the bytes from under a span that was
    /// just committed. With the condition inside the delete, the association makes it match nothing.
    #[tokio::test]
    async fn a_file_referenced_again_before_deletion_survives() {
        let pool = setup_test_pool().await;
        associate_file(
            &pool,
            "old-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();

        // Cleanup: associations for the doomed trace are gone, and the count reads zero.
        delete_trace_files(&pool, "default", &["old-trace".to_string()])
            .await
            .unwrap();
        assert_eq!(
            sync_ref_count(&pool, "default", test_hash()).await.unwrap(),
            Some(0)
        );

        // Ingestion gets in first, associating a new trace with the same content.
        associate_file(
            &pool,
            "new-trace",
            "default",
            test_hash(),
            Some("image/png"),
            1024,
            "sha256",
        )
        .await
        .unwrap();

        // The delete must now refuse.
        let deleted = delete_file_if_unreferenced(&pool, "default", test_hash())
            .await
            .unwrap();
        assert!(
            !deleted,
            "the file is referenced by new-trace, so it must not be deleted"
        );
        assert!(
            get_file(&pool, "default", test_hash())
                .await
                .unwrap()
                .is_some(),
            "and its metadata must still be there"
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
