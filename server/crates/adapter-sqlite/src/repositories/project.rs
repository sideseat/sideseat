//! SQLite project lifecycle and deletion-journal repository.
//!
//! Project rows are deletion fences and are therefore read directly from the database. Cleanup keeps durable
//! residual records so arbitrarily late writers remain collectable after a project row is removed.

use sqlx::SqlitePool;

use crate::SqliteError;
use sideseat_ports::traits::{
    DeletionCause, DeletionRecord, DeletionScope, retention_cleanup_logical_bytes,
};
use sideseat_ports::types::{ProjectId, ProjectRow};

/// Create a new project with a generated CUID2 ID.
pub async fn create_project(
    pool: &SqlitePool,
    organization_id: &str,
    name: &str,
    now: i64,
) -> Result<ProjectRow, SqliteError> {
    let id = cuid2::create_id();

    // `INSERT ... SELECT`, so the organization's liveness is checked *by the insert* rather than before
    // it. Checked separately it is a lost race: an organization tombstoned between the check and the
    // insert gets a brand-new live project underneath it, the caller is told 201, and the project accepts
    // writes until some later sweep notices - and a client creating projects in a loop could keep the
    // organization's deletion from ever finishing.
    let inserted = sqlx::query(
        "INSERT INTO projects (id, organization_id, name, created_at, updated_at) \
         SELECT ?, id, ?, ?, ? FROM organizations WHERE id = ? AND deleting_at IS NULL",
    )
    .bind(&id)
    .bind(name)
    .bind(now)
    .bind(now)
    .bind(organization_id)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(SqliteError::Conflict(format!(
            "organization {organization_id} does not exist or is being deleted"
        )));
    }

    Ok(ProjectRow {
        id,
        organization_id: organization_id.to_string(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
    })
}

/// Get a project by ID.
///
/// The project row is the deletion fence, so this lookup always reads the database. A process-local cache
/// cannot observe another instance claiming the project for deletion.
pub async fn get_project(pool: &SqlitePool, id: &str) -> Result<Option<ProjectRow>, SqliteError> {
    get_project_from_db(pool, id).await
}

/// Get a project by ID directly from database (no caching)
async fn get_project_from_db(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects \
         WHERE id = ? AND deleting_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, organization_id, name, created_at, updated_at)| ProjectRow {
            id,
            organization_id,
            name,
            created_at,
            updated_at,
        },
    ))
}

/// List live projects with pagination, newest first.
pub async fn list_projects(
    pool: &SqlitePool,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects \
         WHERE deleting_at IS NULL ORDER BY created_at DESC, id LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE deleting_at IS NULL")
        .fetch_one(pool)
        .await?;

    let projects = rows
        .into_iter()
        .map(
            |(id, organization_id, name, created_at, updated_at)| ProjectRow {
                id,
                organization_id,
                name,
                created_at,
                updated_at,
            },
        )
        .collect();

    Ok((projects, total.0 as u64))
}

/// Enumerate live projects and project-owned rows that restore repair can remove.
///
/// Deliberately excludes deletion journals, tombstones, holds, leases, and quota rows: those are durable
/// lifecycle facts rather than unreachable content, and including them would make a completed repair
/// rediscover the same deleted project forever.
pub async fn restore_project_ids(
    pool: &SqlitePool,
    limit: usize,
) -> Result<Vec<ProjectId>, SqliteError> {
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT id AS project_id FROM projects WHERE deleting_at IS NULL
         UNION SELECT project_id FROM files
         UNION SELECT project_id FROM trace_files
         UNION SELECT project_id FROM staged_payloads
         ORDER BY 1
         LIMIT ?",
    )
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().map(ProjectId::from).collect())
}

/// Projects a user can see.
///
/// This always reads the database because list membership depends on the deletion fence of every project.
pub async fn list_for_user(
    pool: &SqlitePool,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    list_for_user_from_db(pool, user_id, page, limit).await
}

/// Query projects visible to a user.
async fn list_for_user_from_db(
    pool: &SqlitePool,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT p.id, p.organization_id, p.name, p.created_at, p.updated_at
        FROM projects p
        JOIN organization_members om ON p.organization_id = om.organization_id
        WHERE om.user_id = ? AND p.deleting_at IS NULL
        ORDER BY p.created_at DESC, p.id
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM projects p
        JOIN organization_members om ON p.organization_id = om.organization_id
        WHERE om.user_id = ? AND p.deleting_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let projects = rows
        .into_iter()
        .map(
            |(id, organization_id, name, created_at, updated_at)| ProjectRow {
                id,
                organization_id,
                name,
                created_at,
                updated_at,
            },
        )
        .collect();

    Ok((projects, total.0 as u64))
}

/// List live projects in an organization.
pub async fn list_for_org(
    pool: &SqlitePool,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    list_for_org_from_db(pool, org_id, page, limit).await
}

/// Query live projects in an organization.
async fn list_for_org_from_db(
    pool: &SqlitePool,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT id, organization_id, name, created_at, updated_at
        FROM projects
        WHERE organization_id = ? AND deleting_at IS NULL
        ORDER BY created_at DESC, id
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM projects WHERE organization_id = ? AND deleting_at IS NULL",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await?;

    let projects = rows
        .into_iter()
        .map(
            |(id, organization_id, name, created_at, updated_at)| ProjectRow {
                id,
                organization_id,
                name,
                created_at,
                updated_at,
            },
        )
        .collect();

    Ok((projects, total.0 as u64))
}

/// Update a project's name by ID. Returns the updated project if found.
pub async fn update_project(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    now: i64,
) -> Result<Option<ProjectRow>, SqliteError> {
    let result = sqlx::query(
        "UPDATE projects SET name = ?, updated_at = ? WHERE id = ? AND deleting_at IS NULL",
    )
    .bind(name)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    get_project_from_db(pool, id).await
}

/// Claim a project for deletion, if it exists and nobody else has claimed it.
///
/// The compare-and-set is the whole mechanism: two admins deleting the same project concurrently, or a
/// sweep racing a request, must not both run the cleanup - and every other path (reads, ingestion,
/// rename) consults the claim, so setting it is what makes the project stop being live.
///
/// Returns false when the project does not exist or is already claimed.
pub async fn claim_project_for_deletion(
    pool: &SqlitePool,
    id: &str,
    now: i64,
) -> Result<bool, SqliteError> {
    let result =
        sqlx::query("UPDATE projects SET deleting_at = ? WHERE id = ? AND deleting_at IS NULL")
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Whether this project currently accepts writes: a row exists and nothing has claimed it.
///
/// Both halves matter, and the second is the one that closes the orphan-span hole. A claimed project is
/// going away; a *missing* project is already gone, or was never created, and in both cases a span
/// written for it is unreachable - every read path finds data through the project row, so those rows
/// are invisible, uncounted against any quota, and inherited by the next project to take the id.
/// Refusing them is what makes "no data outlives its project" true rather than merely likely.
pub async fn project_accepts_writes(pool: &SqlitePool, id: &str) -> Result<bool, SqliteError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(matches!(row, Some((None,))))
}

/// Projects claimed for deletion longer ago than `older_than_secs`, so an abandoned cleanup can resume.
///
/// A project's cleanup spans four stores and can fail or crash part way through any of them. The claim
/// is durable so the project stays fenced across a restart, which means nothing releases it either -
/// and a half-deleted project nothing ever finishes is worse than one that was never deleted, because
/// it is invisible to every read path while its data is still on disk.
pub async fn get_stale_claimed_projects(
    pool: &SqlitePool,
    older_than_secs: i64,
    now: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    let cutoff = now - older_than_secs;
    let mut rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, deleting_at FROM projects WHERE deleting_at IS NOT NULL AND deleting_at <= ? \
         ORDER BY deleting_at LIMIT ?",
    )
    .bind(cutoff)
    .bind(sideseat_core::constants::STALE_CLEANUP_RESUME_BATCH)
    .fetch_all(pool)
    .await?;
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(rows)
}

/// Record what a cleanup sweep observed and, if the evidence is now sufficient, remove the tombstone -
/// as one act.
///
/// # Why counting and deleting cannot be two statements
///
/// They were, and that is a lost race with another instance's sweep: A counts the last window it needs
/// and decides to remove the row; B sweeps, a stalled writer commits, B finds the row and resets the
/// count; A then deletes the row on the strength of a decision that is no longer true, and the writer's
/// spans are orphaned. The decision has to be part of the delete, so it cannot be acted on after
/// something invalidated it.
///
/// # Why the count measures windows rather than sweeps
///
/// Every instance of a horizontally scaled deployment runs the sweep. A bare increment let N instances
/// reach the required number inside one interval - the barrier getting *weaker* the more instances you
/// run. The increment is gated on `last_sweep_at`, and being one statement, concurrent instances race for
/// the row and only one wins per window.
///
/// The times are the **database's**, not the caller's: with per-instance clocks, skew decides which
/// windows count, and every instance reads a different notion of now from the same row.
///
/// Returns whether the row was removed.
pub async fn record_project_sweep(
    pool: &SqlitePool,
    id: &str,
    was_clean: bool,
    required: i64,
    min_gap_secs: i64,
) -> Result<bool, SqliteError> {
    if was_clean {
        sqlx::query(
            "UPDATE projects SET clean_sweeps = clean_sweeps + 1, last_sweep_at = unixepoch() \
             WHERE id = ? AND deleting_at IS NOT NULL \
               AND (last_sweep_at IS NULL OR last_sweep_at <= unixepoch() - ?)",
        )
        .bind(id)
        .bind(min_gap_secs)
        .execute(pool)
        .await?;
    } else {
        // A late writer's spans reset the evidence, and unconditionally: the safe direction is never
        // gated on a window.
        sqlx::query(
            "UPDATE projects SET clean_sweeps = 0, last_sweep_at = unixepoch() \
             WHERE id = ? AND deleting_at IS NOT NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
    }

    // The decision *is* the delete. Another instance that reset the count between this sweep's
    // observation and this statement makes it match nothing, which is exactly what should happen.
    //
    // And in the same transaction, a record that this project existed. The row is removed on finite
    // evidence, which an arbitrarily delayed writer defeats - it can commit after the row is gone, and
    // then nothing knows the project was ever there to collect for. `deleted_projects` is what knows.
    // Recording it separately would lose it to a crash in between, which is the one moment it matters.
    let mut tx = pool.begin().await?;
    let removed = sqlx::query(
        "DELETE FROM projects \
         WHERE id = ? AND deleting_at IS NOT NULL AND clean_sweeps >= ?",
    )
    .bind(id)
    .bind(required)
    .execute(&mut *tx)
    .await?;
    let removed = removed.rows_affected() > 0;
    if removed {
        sqlx::query(
            // `next_check_at` set here, not left to a default: it must be *due*, and null sorted last on
            // PostgreSQL, which queued a fresh deletion behind the entire backlog.
            "INSERT INTO deleted_projects (project_id, deleted_at, next_check_at) \
             VALUES (?, unixepoch(), unixepoch()) \
             ON CONFLICT (project_id) DO NOTHING",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(removed)
}

/// Projects whose rows are gone but whose cleanup is still owed.
///
/// The sweep keeps collecting rows that appear for these ids, so a writer that read the fence before the
/// tombstone has its spans deleted however late it commits - which is what makes the residual a retention
/// rather than a handful of minutes.
/// Claim a bounded batch of deleted-project ids that are due for a cleanup check.
///
/// Four things make this affordable for records that are kept forever.
///
/// **Leased, not merely marked.** The claim pushes `next_check_at` out by `lease_secs` before returning the
/// ids, so a batch that takes longer than a sweep interval - fifty S3 listings have no guaranteed duration -
/// is not picked up again while it is still running. The real next time is set when the check reports.
///
/// **Claimed exclusively.** SQLite has a single writer, so two claims cannot interleave at all.
///
/// **Backed off.** `next_check_at` is materialised from `quiet_checks` on every report, so a project
/// deleted long ago is not re-checked at the same rate as one deleted a minute ago. Without it, a hundred
/// thousand historical deletions meant a hundred thousand storage listings every sweep, forever.
///
/// **Bounded per sweep**, so one pass cannot outlive its window - and the *search* is bounded too, because
/// the index is on the due time rather than on an input to it.
pub async fn claim_deleted_projects_for_check(
    pool: &SqlitePool,
    lease_secs: i64,
    limit: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "UPDATE deleted_projects \
         SET last_checked_at = unixepoch(), next_check_at = unixepoch() + ?, \
             claim_token = claim_token + 1 \
         WHERE project_id IN ( \
             SELECT project_id FROM deleted_projects \
             WHERE next_check_at <= unixepoch() \
             ORDER BY next_check_at, project_id \
             LIMIT ? \
         ) \
         RETURNING project_id, claim_token",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Record what a deleted project's check found, and when to look again.
///
/// Quiet means *nothing at all* was found - no analytics rows, no files, and no error looking. Anything
/// else brings the next check back to the base interval: files arriving while spans are dropped, or a
/// storage delete that failed, are both reasons to look again soon rather than to conclude the project has
/// gone quiet.
pub async fn record_deleted_project_check(
    pool: &SqlitePool,
    project_id: &str,
    claim_token: i64,
    was_quiet: bool,
    base_gap_secs: i64,
    max_gap_secs: i64,
) -> Result<(), SqliteError> {
    // Matched on the token, so a worker whose lease expired part way through its batch updates nothing: the
    // id has been claimed again since, the token has moved on, and overwriting the new holder's schedule -
    // or its result - would let a third worker claim an id that is still being processed.
    if was_quiet {
        sqlx::query(
            "UPDATE deleted_projects \
             SET quiet_checks = quiet_checks + 1, \
                 next_check_at = unixepoch() + MIN(? * (1 << MIN(quiet_checks, 20)), ?) \
             WHERE project_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(max_gap_secs)
        .bind(project_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE deleted_projects SET quiet_checks = 0, next_check_at = unixepoch() + ? \
             WHERE project_id = ? AND claim_token = ?",
        )
        .bind(base_gap_secs)
        .bind(project_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Forget projects deleted longer ago than `retention_secs`, and say how many were forgotten.
///
/// The bound has to be stated somewhere, and this is it: past this point a write from before the deletion
/// is no longer collected. It is a retention rather than a guess because nothing keeps a request alive
/// that long - the exporter has given up, the connection is closed, the process is gone.
pub async fn forget_deleted_projects(
    pool: &SqlitePool,
    retention_secs: i64,
) -> Result<u64, SqliteError> {
    let result = sqlx::query("DELETE FROM deleted_projects WHERE deleted_at <= unixepoch() - ?")
        .bind(retention_secs)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Claim an organization for deletion, if it exists and nobody else has claimed it.
pub async fn claim_organization_for_deletion(
    pool: &SqlitePool,
    id: &str,
    now: i64,
) -> Result<bool, SqliteError> {
    let result = sqlx::query(
        "UPDATE organizations SET deleting_at = ? WHERE id = ? AND deleting_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Organizations claimed for deletion longer ago than `older_than_secs`, so one can be resumed.
pub async fn get_stale_claimed_organizations(
    pool: &SqlitePool,
    older_than_secs: i64,
    now: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    let cutoff = now - older_than_secs;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, deleting_at FROM organizations WHERE deleting_at IS NOT NULL AND deleting_at <= ? \
         ORDER BY deleting_at LIMIT ?",
    )
    .bind(cutoff)
    .bind(sideseat_core::constants::STALE_CLEANUP_RESUME_BATCH)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// How many of an organization's projects still have rows, tombstoned or not.
///
/// An organization's row may only go when none are left: its cascade would take those rows with it, and
/// a project row is what the cleanup of that project depends on to keep running.
pub async fn count_projects_of_organization(
    pool: &SqlitePool,
    org_id: &str,
) -> Result<i64, SqliteError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE organization_id = ?")
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

/// Delete a project by ID. Returns true if a project was deleted.
pub async fn delete_project(pool: &SqlitePool, id: &str) -> Result<bool, SqliteError> {
    let result = sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Record that these traces were deleted, so a late ingest cannot resurrect them.
///
/// See `TransactionalRepository::record_deleted_traces` for the race. Written *before* the analytics
/// delete, so there is no instant at which a trace is deleted and not yet tombstoned.
mod deletion;
pub use deletion::{
    claim_deleted_sessions_for_check, claim_deleted_traces_for_check,
    claim_organization_for_deletion_journalled, claim_project_for_deletion_journalled,
    deleted_sessions_among, deleted_traces_among, reclaim_stale_organization,
    reclaim_stale_project, record_deleted_session_check, record_deleted_sessions,
    record_deleted_sessions_journalled, record_deleted_spans_journalled,
    record_deleted_trace_check, record_deleted_traces, record_deleted_traces_journalled,
    record_pressure_eviction,
};

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
