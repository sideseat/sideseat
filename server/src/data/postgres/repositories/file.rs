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
    hash_algo: &str,
) -> Result<i64, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    // Use INSERT ... ON CONFLICT with RETURNING for atomic upsert
    // If file exists, increment ref_count; otherwise insert with ref_count=1
    let result: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 1, $6, $7)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET
            ref_count = files.ref_count + 1,
            updated_at = $8
        RETURNING ref_count::bigint
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
        WHERE project_id = $2 AND file_hash = $3 AND ref_count > 0
        RETURNING ref_count::bigint
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
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>, i64, String, i64, i64, i64)>(
        r#"
        SELECT id::bigint, project_id, file_hash, media_type, size_bytes, hash_algo, ref_count::bigint, created_at, updated_at
        FROM files
        WHERE project_id = $1 AND file_hash = $2
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

/// Associate a trace with a file that already exists, without inventing metadata - see the SQLite twin.
pub async fn associate_existing_file(
    pool: &PgPool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    let mut tx = pool.begin().await?;

    let row: Option<(Option<i64>,)> =
        // `FOR UPDATE`, not a bare read. Reading and then writing is a race that loses: ingestion sees
        // no claim, cleanup claims the still-unreferenced file and commits, ingestion then associates
        // and finalises the bytes, and cleanup deletes them - leaving a committed reference to nothing.
        // The row lock makes the claim and the association serialise on the same object.
        sqlx::query_as(
            "SELECT deleting_at FROM files WHERE project_id = $1 AND file_hash = $2 FOR UPDATE",
        )
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
        // One in-flight writer added, new row or shared - see the SQLite twin.
        "INSERT INTO trace_files (trace_id, project_id, file_hash, pending_writers, durable) \
         VALUES ($1, $2, $3, 1, FALSE) \
         ON CONFLICT (project_id, trace_id, file_hash) \
         DO UPDATE SET pending_writers = trace_files.pending_writers + 1",
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
            updated_at = $1
        WHERE project_id = $2 AND file_hash = $3
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

/// Files claimed for deletion longer ago than `older_than`, so an abandoned claim can be resumed - see
/// the SQLite twin for why a durable claim needs this.
pub async fn get_stale_claimed_files(
    pool: &PgPool,
    older_than_secs: i64,
) -> Result<Vec<(String, String)>, PostgresError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    Ok(sqlx::query_as(
        "SELECT project_id, file_hash FROM files WHERE deleting_at IS NOT NULL AND deleting_at <= $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?)
}

/// Claim a file for deletion - see the SQLite twin for why a count cannot replace this.
pub async fn claim_file_for_deletion(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;

    // Lock the row *before* asking whether anything references it, in a statement of its own.
    //
    // As one UPDATE with `NOT EXISTS (SELECT ... FROM trace_files ...)` this was unsound in a way only a
    // concurrent writer shows. Under READ COMMITTED an UPDATE that blocks on a locked row re-checks its
    // qualification when the lock frees, but the re-check evaluates the subquery against the *statement's
    // original* snapshot - so an `associate_file` that committed a `trace_files` row while this statement
    // waited is invisible to it, and the claim succeeds on a file that is now referenced. Cleanup then
    // deletes bytes a committed row points at. `the_file_fence_holds_against_a_concurrent_association`
    // reproduces exactly that.
    //
    // Locking first and asking second fixes it because the second statement gets its own snapshot, taken
    // after the lock was granted, which by then includes anything the other writer committed.
    let existing: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT deleting_at FROM files WHERE project_id = $1 AND file_hash = $2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(file_hash)
    .fetch_optional(&mut *tx)
    .await?;
    match existing {
        None => return Ok(false),             // no such file
        Some((Some(_),)) => return Ok(false), // already claimed
        Some((None,)) => {}
    }

    let result = sqlx::query(
        r#"
        UPDATE files SET deleting_at = $1
        WHERE project_id = $2 AND file_hash = $3 AND deleting_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM trace_files
              WHERE project_id = files.project_id AND file_hash = files.file_hash
          )
        "#,
    )
    .bind(now)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;
    let claimed = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(claimed)
}

/// Give up a deletion claim, leaving the file in place.
pub async fn release_deletion_claim(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query("UPDATE files SET deleting_at = NULL WHERE project_id = $1 AND file_hash = $2")
        .bind(project_id)
        .bind(file_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Put back a metadata row whose bytes could not be deleted - see the SQLite twin.
pub async fn restore_orphan_metadata(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<(), PostgresError> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 0, $6, $6)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET updated_at = $6
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .bind(media_type)
    .bind(size_bytes)
    .bind(hash_algo)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a file's metadata only if nothing references it - see the SQLite twin for why the condition
/// has to be inside the statement.
pub async fn delete_file_if_unreferenced(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    let result = sqlx::query(
        r#"
        DELETE FROM files
        WHERE project_id = $1 AND file_hash = $2
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
/// See the SQLite twin: `ref_count` is a cached `COUNT(*)`, and recomputing it is the only form immune
/// to concurrent cleanups both subtracting the same references.
pub async fn sync_ref_count(
    pool: &PgPool,
    project_id: &str,
    file_hash: &str,
) -> Result<Option<i64>, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let result: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE files
        SET ref_count = (
                SELECT COUNT(*) FROM trace_files
                WHERE project_id = files.project_id AND file_hash = files.file_hash
            ),
            updated_at = $1
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

/// Associate a file with a trace, counting the reference only if the association is new.
///
/// See the SQLite twin for why this is one transaction: `ref_count` must equal the number of
/// associations, because that is what deletion decrements.
pub async fn associate_file(
    pool: &PgPool,
    trace_id: &str,
    project_id: &str,
    file_hash: &str,
    media_type: Option<&str>,
    size_bytes: i64,
    hash_algo: &str,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;

    // Refuse through the fence - see the SQLite twin.
    let claimed: Option<(Option<i64>,)> =
        // `FOR UPDATE`, not a bare read. Reading and then writing is a race that loses: ingestion sees
        // no claim, cleanup claims the still-unreferenced file and commits, ingestion then associates
        // and finalises the bytes, and cleanup deletes them - leaving a committed reference to nothing.
        // The row lock makes the claim and the association serialise on the same object.
        sqlx::query_as(
            "SELECT deleting_at FROM files WHERE project_id = $1 AND file_hash = $2 FOR UPDATE",
        )
            .bind(project_id)
            .bind(file_hash)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((Some(_),)) = claimed {
        return Err(PostgresError::Conflict(format!(
            "file {file_hash} in project {project_id} is being deleted"
        )));
    }

    sqlx::query(
        r#"
        INSERT INTO files (project_id, file_hash, media_type, size_bytes, hash_algo, ref_count, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 0, $6, $6)
        ON CONFLICT(project_id, file_hash) DO UPDATE SET updated_at = $6
        "#,
    )
    .bind(project_id)
    .bind(file_hash)
    .bind(media_type)
    .bind(size_bytes)
    .bind(hash_algo)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // One in-flight writer added, new row or shared - see the schema comment and the SQLite twin. Returns
    // true whenever a reference was recorded, which is always: each reference is resolved by one confirm or
    // release.
    sqlx::query(
        "INSERT INTO trace_files (trace_id, project_id, file_hash, pending_writers, durable) \
         VALUES ($1, $2, $3, 1, FALSE) \
         ON CONFLICT (project_id, trace_id, file_hash) \
         DO UPDATE SET pending_writers = trace_files.pending_writers + 1",
    )
    .bind(trace_id)
    .bind(project_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;

    // Recomputed from the associations, not incremented - see `sync_ref_count`. A share leaves the row
    // count unchanged, so this is a no-op then; on a fresh row it is the +1.
    sqlx::query(
        r#"
        UPDATE files
        SET ref_count = (
                SELECT COUNT(*) FROM trace_files
                WHERE project_id = files.project_id AND file_hash = files.file_hash
            ),
            updated_at = $1
        WHERE project_id = $2 AND file_hash = $3
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
pub async fn get_file_reference_counts_for_traces(
    pool: &PgPool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<(String, i64)>, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT file_hash, COUNT(*)::bigint FROM trace_files \
         WHERE project_id = $1 AND trace_id = ANY($2) GROUP BY file_hash",
    )
    .bind(project_id)
    .bind(trace_ids)
    .fetch_all(pool)
    .await?)
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
) -> Result<Vec<String>, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    // `RETURNING`, so the caller reconciles exactly the hashes this statement removed - see the SQLite twin
    // for the association that a read-then-delete pair misses.
    let rows: Vec<String> = sqlx::query_scalar(
        "DELETE FROM trace_files WHERE project_id = $1 AND trace_id = ANY($2::text[]) \
         RETURNING file_hash",
    )
    .bind(project_id)
    .bind(trace_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get total storage used by a project
pub async fn get_project_storage_bytes(
    pool: &PgPool,
    project_id: &str,
) -> Result<i64, PostgresError> {
    let result: (Option<i64>,) =
        sqlx::query_as("SELECT SUM(size_bytes)::bigint FROM files WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get total file storage used by all projects in an organization
pub async fn get_org_file_storage_bytes(pool: &PgPool, org_id: &str) -> Result<i64, PostgresError> {
    let result: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(f.size_bytes), 0)::bigint
        FROM files f
        JOIN projects p ON f.project_id = p.id
        WHERE p.organization_id = $1
        "#,
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0))
}

/// Get total file storage used across all orgs a user belongs to
pub async fn get_user_file_storage_bytes(
    pool: &PgPool,
    user_id: &str,
) -> Result<i64, PostgresError> {
    let result: (Option<i64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(f.size_bytes), 0)::bigint
        FROM files f
        JOIN projects p ON f.project_id = p.id
        JOIN organization_members m ON p.organization_id = m.organization_id
        WHERE m.user_id = $1
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
pub async fn get_orphan_files(pool: &PgPool) -> Result<Vec<(String, String)>, PostgresError> {
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
/// `sync_ref_count`, which recomputes the count from the associations that remain.
pub async fn release_trace_file_association(
    pool: &PgPool,
    project_id: &str,
    trace_id: &str,
    file_hash: &str,
) -> Result<bool, PostgresError> {
    // Decrement this writer, then delete only a non-durable row with none left - the two-fact test. Two
    // statements in a transaction, not one CTE: a data-modifying CTE that updates a row and then deletes
    // the same row in the outer statement modifies one tuple twice in a single command, which Postgres
    // leaves the delete a silent no-op for. See the SQLite twin for why a boolean could not distinguish the
    // last provisional writer from a still-in-flight peer.
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE trace_files SET pending_writers = GREATEST(pending_writers - 1, 0) \
         WHERE project_id = $1 AND trace_id = $2 AND file_hash = $3",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query(
        "DELETE FROM trace_files \
         WHERE project_id = $1 AND trace_id = $2 AND file_hash = $3 \
           AND durable = FALSE AND pending_writers = 0",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(file_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}
/// Mark a batch's associations durable. See the SQLite twin.
pub async fn confirm_trace_file_associations(
    pool: &PgPool,
    associations: &[(String, String, String)],
) -> Result<u64, PostgresError> {
    if associations.is_empty() {
        return Ok(0);
    }
    // Deduped first, so a tuple that appears twice decrements `pending_writers` once - the same as the
    // SQLite twin's per-element loop over a deduped set. Without this, `IN (UNNEST(...))` touches each row
    // once however many times the tuple is listed, while the loop would touch it per listing: the two
    // backends would drift on a caller that passed a duplicate. Each referencing batch increments once, so
    // one decrement per distinct tuple is the correct resolution on both.
    let mut unique: Vec<&(String, String, String)> = associations.iter().collect();
    unique.sort_unstable();
    unique.dedup();
    // One statement for the batch, via three parallel arrays.
    let projects: Vec<&str> = unique.iter().map(|(p, _, _)| p.as_str()).collect();
    let traces: Vec<&str> = unique.iter().map(|(_, t, _)| t.as_str()).collect();
    let hashes: Vec<&str> = unique.iter().map(|(_, _, h)| h.as_str()).collect();
    let result = sqlx::query(
        "UPDATE trace_files SET durable = TRUE, pending_writers = GREATEST(pending_writers - 1, 0) \
         WHERE (project_id, trace_id, file_hash) IN ( \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[]) \
         )",
    )
    .bind(&projects)
    .bind(&traces)
    .bind(&hashes)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
