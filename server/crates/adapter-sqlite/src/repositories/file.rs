//! SQLite file metadata, trace associations, deletion fencing, and retention cleanup.
//!
//! Reference counts are derived from durable association rows. Deletion claims fence ingestion while object
//! bytes are being removed, and compare-and-set recovery prevents stale workers from deleting live content.

use sqlx::SqlitePool;

use crate::SqliteError;
use sideseat_core::constants::FILE_CLEANUP_BATCH_SIZE;
use sideseat_ports::traits::retention_cleanup_logical_bytes;
use sideseat_ports::types::FileRow;

#[cfg(test)]
async fn setup_test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    // Execute the schema as a script: semicolons inside SQL comments are not statement boundaries.
    sqlx::raw_sql(crate::schema::SCHEMA)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

/// Insert a file or increment its reference count atomically.
///
/// Returns the resulting reference count.
pub async fn upsert_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
    now: i64,
) -> Result<i64, SqliteError> {
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

/// Decrement a positive reference count atomically.
///
/// Returns `None` when the file is absent or already has no references.
pub async fn decrement_ref_count(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    now: i64,
) -> Result<Option<i64>, SqliteError> {
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

/// Get a file by project and hash.
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

/// Check whether a file exists.
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

/// Delete a file metadata record.
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

/// Delete all file records for a project.
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
    now: i64,
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
        // One in-flight writer added, whether the row is new or shared: a reference that arrived already
        // formed is still only justified once the span carrying it commits, and a second batch referencing
        // the same association must be counted so neither can release the row from under the other.
        "INSERT INTO trace_files (trace_id, project_id, file_hash, pending_writers, durable) \
         VALUES (?, ?, ?, 1, 0) \
         ON CONFLICT(project_id, trace_id, file_hash) \
         DO UPDATE SET pending_writers = pending_writers + 1",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;

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
/// The claim's own value comes back with each row: the recovery path must be able to prove, at the moment it
/// deletes the bytes, that the claim is still the one it saw. See [`reclaim_stale_file`].
pub async fn get_stale_claimed_files(
    pool: &SqlitePool,
    older_than_secs: i64,
    now: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    let cutoff = now - older_than_secs;
    Ok(sqlx::query_as(
        "SELECT project_id, file_hash, deleting_at FROM files \
         WHERE deleting_at IS NOT NULL AND deleting_at <= ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

/// Re-take an abandoned claim, atomically against the value that was observed.
///
/// The recovery path acts on a *snapshot*: between the scan and the byte deletion the row can be released
/// (a worker whose object delete failed), or deleted and recreated by an ingestion that then associates the
/// same content hash. The stale reading must never authorize deletion of content referenced by a committed span.
///
/// So the byte deletion is gated on this compare-and-set: the claim must still be exactly the one observed,
/// and nothing may reference the file. Success also refreshes the claim, which re-leases it so a second
/// worker does not act on the same stale reading.
pub async fn reclaim_stale_file(
    pool: &SqlitePool,
    project_id: &str,
    file_hash: &str,
    observed_deleting_at: i64,
    now: i64,
) -> Result<bool, SqliteError> {
    // The refreshed claim must be strictly newer than the observed value so the same snapshot authorizes at
    // most one worker. Because timestamps have one-second resolution, `now` alone may equal the old claim.
    // Advancing by one can delay expiry slightly but can never make it expire early.
    let now = now.max(observed_deleting_at + 1);
    let result = sqlx::query(
        r#"
        UPDATE files SET deleting_at = ?
        WHERE project_id = ? AND file_hash = ? AND deleting_at = ?
          AND NOT EXISTS (
              SELECT 1 FROM trace_files
              WHERE project_id = files.project_id AND file_hash = files.file_hash
          )
        "#,
    )
    .bind(now)
    .bind(project_id)
    .bind(file_hash)
    .bind(observed_deleting_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
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
    now: i64,
) -> Result<bool, SqliteError> {
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
    now: i64,
) -> Result<(), SqliteError> {
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
    now: i64,
) -> Result<Option<i64>, SqliteError> {
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
#[allow(clippy::too_many_arguments)]
pub async fn associate_file(
    pool: &SqlitePool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
    now: i64,
) -> Result<bool, SqliteError> {
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

    // One in-flight writer added, new row or shared. The analytics row that justifies it has not committed
    // yet, so the row starts non-durable; a batch confirms it on success and decrements it on failure, and
    // the release deletes only a non-durable row with no writer left - so a second batch sharing this
    // association is safe from the first's failure. Returns true whenever a reference was recorded, which is
    // always: every reference must be resolved by exactly one confirm or release.
    sqlx::query(
        "INSERT INTO trace_files (trace_id, project_id, file_hash, pending_writers, durable) \
         VALUES (?, ?, ?, 1, 0) \
         ON CONFLICT(project_id, trace_id, file_hash) \
         DO UPDATE SET pending_writers = pending_writers + 1",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;

    // Recomputed from the associations, not incremented. Same reasoning as `sync_ref_count`: a
    // maintained counter drifts, a derived one cannot. A share does not change the row count, so this is a
    // no-op then; on a fresh row it is the +1. Inside the transaction, so it counts the row just written.
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

/// Release a trace's associations except the ones its surviving spans still reference.
///
/// Two conditions and neither is optional. `file_hash <> ALL(keep)` leaves a survivor's file alone;
/// `pending_writers = 0` protects a batch in flight, which the survivor scan cannot see because referencing
/// a file increments that counter *before* the span row exists. Without it this is a read-then-act race with
/// exactly the window it exists to close.
///
/// `RETURNING`, so the caller reconciles the set this statement produced rather than one it read beforehand.
///
/// The `keep` list is interpolated as bound placeholders rather than a bound array, because SQLite has no
/// array type - the PostgreSQL twin uses `<> ALL($3::text[])`.
pub async fn release_trace_files_except(
    pool: &SqlitePool,
    project_id: &str,
    trace_id: &str,
    keep: &[String],
) -> Result<Vec<String>, SqliteError> {
    let keep_clause = if keep.is_empty() {
        String::new()
    } else {
        format!(
            " AND file_hash NOT IN ({})",
            keep.iter().map(|_| "?").collect::<Vec<_>>().join(",")
        )
    };
    let sql = format!(
        "DELETE FROM trace_files WHERE project_id = ? AND trace_id = ? AND pending_writers = 0\
         {keep_clause} RETURNING file_hash"
    );

    let mut query = sqlx::query_scalar::<_, String>(&sql)
        .bind(project_id)
        .bind(trace_id);
    for hash in keep {
        query = query.bind(hash);
    }
    Ok(query.fetch_all(pool).await?)
}

/// Get total storage used by a project
pub async fn get_project_storage_bytes(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<i64, SqliteError> {
    let result: (i64,) = sqlx::query_as(
        "SELECT
             COALESCE((SELECT SUM(size_bytes) FROM files WHERE project_id = ?), 0)
           + COALESCE((SELECT SUM(logical_bytes) FROM content_bodies WHERE project_id = ?), 0)",
    )
    .bind(project_id)
    .bind(project_id)
    .fetch_one(pool)
    .await?;

    Ok(result.0)
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

/// Get total file storage used across all organizations a user belongs to.
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

/// Get files with zero references across all projects for global cleanup.
///
/// Returns `(project_id, file_hash)` pairs for orphaned files.
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
    // Decrement this writer, then delete only if no writer is left and the association never became
    // durable. That two-fact test is what makes it safe under concurrency: a boolean could not tell "the
    // last provisional writer, delete it" from "another batch is still in flight, keep it". A batch that
    // committed set `durable`, so the delete skips it; a batch still in flight left `pending_writers`
    // positive, so the delete skips it too. In a transaction, so the decrement and the delete see one state.
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE trace_files SET pending_writers = MAX(pending_writers - 1, 0) \
         WHERE project_id = ? AND trace_id = ? AND file_hash = ?",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "DELETE FROM trace_files \
         WHERE project_id = ? AND trace_id = ? AND file_hash = ? AND durable = 0 AND pending_writers = 0",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Mark a batch's associations durable, now that its analytics rows are committed.
///
/// Sets `durable` and resolves this writer (`pending_writers - 1`). Durable is monotonic and permanently
/// blocks the release-delete, so the association survives any number of *other* batches failing and
/// releasing - which is the whole point: once one referencing batch has committed, the file is backed.
/// Idempotent, and called after a successful write, before anything else can observe the batch as failed.
pub async fn confirm_trace_file_associations(
    pool: &SqlitePool,
    associations: &[(String, String, String)],
) -> Result<u64, SqliteError> {
    if associations.is_empty() {
        return Ok(0);
    }
    // Deduped, so a tuple listed twice decrements `pending_writers` once - matching the PostgreSQL twin's
    // `IN (UNNEST(...))`, which touches each row once regardless of repetition. Each batch increments an
    // association once, so one decrement per distinct tuple is the correct resolution.
    let mut unique: Vec<&(String, String, String)> = associations.iter().collect();
    unique.sort_unstable();
    unique.dedup();
    let mut tx = pool.begin().await?;
    let mut confirmed = 0u64;
    for (project_id, trace_id, file_hash) in unique {
        let result = sqlx::query(
            "UPDATE trace_files SET durable = 1, pending_writers = MAX(pending_writers - 1, 0) \
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

/// Record cleanup intent for these traces. Idempotent: a trace already queued keeps its schedule.
pub async fn record_retention_cleanup(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
    now: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut written = Vec::with_capacity(trace_ids.len());
    for trace_id in trace_ids {
        // The token moves on a re-record, so a claim an earlier worker still holds no longer matches: new work
        // behind the same identity must not be discarded by a stale completion.
        let token: i64 = sqlx::query_scalar(
            "INSERT INTO retention_cleanup \
             (project_id, trace_id, created_at, attempts, next_attempt_at, claim_token, logical_bytes) \
             VALUES (?, ?, ?, 0, 0, 1, ?) \
             ON CONFLICT(project_id, trace_id) DO UPDATE SET \
                 claim_token = claim_token + 1, next_attempt_at = 0 \
             RETURNING claim_token",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(now)
        .bind(
            i64::try_from(retention_cleanup_logical_bytes(project_id, trace_id))
                .unwrap_or(i64::MAX),
        )
        .fetch_one(pool)
        .await?;
        written.push((trace_id.clone(), token));
    }
    Ok(written)
}

/// Claim due candidates, pushing their next attempt out before returning them.
///
/// `WHERE (project_id, trace_id) IN (SELECT ... LIMIT n)` rather than a bare `LIMIT` on the update: the inner
/// select fixes the set, and the update then leases exactly those. The backoff is geometric to a ceiling, so a
/// candidate whose reconciliation keeps failing is retried more slowly rather than spinning - and it is
/// **never dropped**, because the record is the only thing that knows the cleanup is owed.
pub async fn claim_retention_cleanup(
    pool: &SqlitePool,
    limit: i64,
    lease_secs: i64,
    now: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    // The token is **returned**, so completion can require it: without that a stale worker's completion deletes
    // a newer intent. The claim does not bump it - bumping is what a *re-record* means - so a worker holding a
    // token whose row was re-recorded finds no match and leaves the new work alone.
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "UPDATE retention_cleanup \
         SET attempts = attempts + 1, \
             next_attempt_at = ? + ? * MIN(attempts + 1, 8) \
         WHERE (project_id, trace_id) IN ( \
             SELECT project_id, trace_id FROM retention_cleanup \
             WHERE next_attempt_at <= ? ORDER BY next_attempt_at LIMIT ? \
         ) \
         RETURNING project_id, trace_id, claim_token",
    )
    .bind(now)
    .bind(lease_secs)
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Drop candidates whose cleanup completed.
pub async fn complete_retention_cleanup(
    pool: &SqlitePool,
    project_id: &str,
    completed: &[(String, i64)],
) -> Result<(), SqliteError> {
    // Per row, because the token is per row. A single statement would need a values list; the batch is at most
    // one claim's worth.
    for (trace_id, token) in completed {
        sqlx::query(
            "DELETE FROM retention_cleanup \
             WHERE project_id = ? AND trace_id = ? AND claim_token = ?",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Restore an association a survivor scan released, as **durable**.
///
/// The compensation path of survivor reconciliation: a span committed between the scan and the release, so its
/// association was deleted and must come back before any byte is reclaimed.
///
/// `durable = 1` is the whole point, and `insert_trace_file` was the first choice and is wrong here: it leaves
/// the row provisional, so a *later* batch that references the same file and then fails would decrement
/// `pending_writers` to zero, find a non-durable row, and delete it - taking the association of a span that
/// committed long before. The reference being restored is owned by a committed span, which is exactly what
/// `durable` means.
pub async fn restore_durable_trace_file(
    pool: &SqlitePool,
    project_id: &str,
    trace_id: &str,
    file_hash: &str,
) -> Result<(), SqliteError> {
    sqlx::query(
        "INSERT INTO trace_files (trace_id, project_id, file_hash, pending_writers, durable) \
         VALUES (?, ?, ?, 0, 1) \
         ON CONFLICT(project_id, trace_id, file_hash) DO UPDATE SET durable = 1",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "file_confirm_dedup_tests.rs"]
mod confirm_dedup_tests;
