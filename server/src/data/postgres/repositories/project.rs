//! Project repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use sqlx::PgPool;

use crate::data::cache::{CacheKey, CacheService};
use crate::data::postgres::PostgresError;
use crate::data::types::ProjectRow;

use super::membership::list_member_user_ids;

/// Create a new project with a generated CUID2 ID
pub async fn create_project(
    pool: &PgPool,
    cache: Option<&CacheService>,
    organization_id: &str,
    name: &str,
) -> Result<ProjectRow, PostgresError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    // The parent row is *locked* first, then re-read, then the child inserted - all in one transaction.
    //
    // `INSERT ... SELECT ... WHERE deleting_at IS NULL` is not enough here, and the reason is specific to
    // PostgreSQL: the statement reads under its own snapshot, and the organization claim updates only a
    // non-key column, so the row lock it takes is `FOR NO KEY UPDATE` - compatible with the key-share lock
    // a foreign-key insert wants. The insert therefore neither blocks nor sees the tombstone that
    // committed after its snapshot, and a brand-new live project appears under an organization whose
    // cleanup has already listed its projects. The caller gets 201 and then 404, and an ingest that passed
    // the project fence in between can leave data with no row to find it by.
    //
    // `FOR UPDATE` conflicts with the claim's lock, so the two serialise, and the re-read happens after the
    // lock is granted - which is when the tombstone is visible.
    let mut tx = pool.begin().await?;
    let live: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(organization_id)
            .fetch_optional(&mut *tx)
            .await?;
    if !matches!(live, Some((None,))) {
        return Err(PostgresError::Conflict(format!(
            "organization {organization_id} does not exist or is being deleted"
        )));
    }
    sqlx::query(
        "INSERT INTO projects (id, organization_id, name, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&id)
    .bind(organization_id)
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // Invalidate list caches AFTER successful insert
    if let Some(cache) = cache {
        // Invalidate org's project list cache
        if let Err(e) = cache
            .delete(&CacheKey::projects_for_org(organization_id))
            .await
        {
            tracing::warn!(%organization_id, error = %e, "Cache invalidation error");
        }

        // Invalidate projects_for_user for all org members
        if let Ok(member_user_ids) = list_member_user_ids(pool, organization_id).await {
            for user_id in &member_user_ids {
                if let Err(e) = cache.delete(&CacheKey::projects_for_user(user_id)).await {
                    tracing::warn!(%user_id, error = %e, "Cache invalidation error");
                }
            }
        }
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
/// # Why this is not cached
///
/// It was, and a cache is wrong for this question in a way a shorter TTL cannot fix. The project row is
/// the deletion fence, and with a process-local cache one instance cannot invalidate another's: instance A
/// caches a live project, instance B tombstones it and clears only B's memory, and A keeps answering from
/// its hit - a deleted project readable and listable for the cache's lifetime, on that instance only.
/// Re-reading the fence after a fill closes the *fill* race but not this one, because a hit never reaches
/// the database at all.
///
/// The cost of not caching is a primary-key lookup, which is microseconds on both backends, against a
/// question every read path asks before it trusts anything else. The parameter is kept so callers need not
/// change and so the intent is visible where they pass one.
pub async fn get_project(
    pool: &PgPool,
    _cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<ProjectRow>, PostgresError> {
    get_project_from_db(pool, id).await
}

/// Get a project by ID directly from database (no caching)
async fn get_project_from_db(pool: &PgPool, id: &str) -> Result<Option<ProjectRow>, PostgresError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects \
         WHERE id = $1 AND deleting_at IS NULL",
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

/// List all projects with pagination, ordered by created_at DESC
/// Note: This function doesn't cache as it's admin-only and pagination varies.
pub async fn list_projects(
    pool: &PgPool,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects \
         WHERE deleting_at IS NULL ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit as i64)
    .bind(offset as i64)
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

/// List projects for a user (across all their organizations) with optional caching
///
/// Note: Only caches first page with default limit for simplicity.
/// Projects a user can see.
///
/// Not cached. The first-page cache this used to have was already unreachable - every route passes `None` -
/// and it could not have been kept: a list is only correct while its projects are live, and with a
/// process-local cache one instance cannot invalidate another's, so a project another instance tombstoned
/// would keep appearing in this instance's list. The query is a join over a handful of rows.
pub async fn list_for_user(
    pool: &PgPool,
    _cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    list_for_user_from_db(pool, user_id, page, limit).await
}

/// List projects for a user directly from database (no caching)
async fn list_for_user_from_db(
    pool: &PgPool,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT p.id, p.organization_id, p.name, p.created_at, p.updated_at
        FROM projects p
        JOIN organization_members om ON p.organization_id = om.organization_id
        WHERE om.user_id = $1 AND p.deleting_at IS NULL
        ORDER BY p.created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(user_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM projects p
        JOIN organization_members om ON p.organization_id = om.organization_id
        WHERE om.user_id = $1 AND p.deleting_at IS NULL
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

/// List projects for a specific organization with optional caching
///
/// Note: Only caches first page with default limit for simplicity.
/// Projects in an organization. Not cached, for the reason [`list_for_user`] gives.
pub async fn list_for_org(
    pool: &PgPool,
    _cache: Option<&CacheService>,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    list_for_org_from_db(pool, org_id, page, limit).await
}

/// List projects for a specific organization directly from database (no caching)
async fn list_for_org_from_db(
    pool: &PgPool,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT id, organization_id, name, created_at, updated_at
        FROM projects
        WHERE organization_id = $1 AND deleting_at IS NULL
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(org_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM projects WHERE organization_id = $1 AND deleting_at IS NULL",
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
    name: &str,
) -> Result<Option<ProjectRow>, PostgresError> {
    // Get old project for org_id to invalidate list cache
    let old_project = get_project_from_db(pool, id).await?;

    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "UPDATE projects SET name = $1, updated_at = $2 WHERE id = $3 AND deleting_at IS NULL",
    )
    .bind(name)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    // Invalidate cache entries AFTER successful write
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::project(id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }

        // Invalidate org's project list cache if project existed
        if let Some(ref old) = old_project
            && let Err(e) = cache
                .delete(&CacheKey::projects_for_org(&old.organization_id))
                .await
        {
            tracing::warn!(org_id = %old.organization_id, error = %e, "Cache invalidation error");
        }
    }

    get_project_from_db(pool, id).await
}

/// Drop every cached fact about a project.
///
/// One list, because the caches expire in five minutes and a project that has just stopped being live
/// must not stay readable for that long. `project_org` is the one that was never invalidated anywhere:
/// it is written by the auth path and read on every request, so a deleted project's organization
/// mapping outlived the project itself and the fence could not be seen at all through it.
async fn invalidate_project_caches(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
    organization_id: Option<&str>,
) {
    let Some(cache) = cache else {
        return;
    };
    for key in [CacheKey::project(id), CacheKey::project_org(id)] {
        if let Err(e) = cache.delete(&key).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }
    }
    let Some(org_id) = organization_id else {
        return;
    };
    if let Err(e) = cache.delete(&CacheKey::projects_for_org(org_id)).await {
        tracing::warn!(%org_id, error = %e, "Cache invalidation error");
    }
    if let Ok(member_user_ids) = list_member_user_ids(pool, org_id).await {
        for user_id in &member_user_ids {
            if let Err(e) = cache.delete(&CacheKey::projects_for_user(user_id)).await {
                tracing::warn!(%user_id, error = %e, "Cache invalidation error");
            }
        }
    }
}

/// Claim a project for deletion, if it exists and nobody else has claimed it.
///
/// The compare-and-set is the whole mechanism: two admins deleting the same project concurrently, or a
/// sweep racing a request, must not both run the cleanup - and every other path (reads, ingestion,
/// rename) consults the claim, so setting it is what makes the project stop being live.
///
/// Returns false when the project does not exist or is already claimed.
pub async fn claim_project_for_deletion(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE projects SET deleting_at = $1 WHERE id = $2 AND deleting_at IS NULL")
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
    let claimed = result.rows_affected() > 0;
    if claimed {
        // Immediately, not when the row goes. From here the project is not live, and every cached
        // answer that says otherwise is wrong - `get_project` reads its cache before the fence.
        let org = org_of_project_ignoring_fence(pool, id).await.ok().flatten();
        invalidate_project_caches(pool, cache, id, org.as_deref()).await;
    }
    Ok(claimed)
}

/// Whether this project currently accepts writes: a row exists and nothing has claimed it.
///
/// Both halves matter, and the second is the one that closes the orphan-span hole. A claimed project is
/// going away; a *missing* project is already gone, or was never created, and in both cases a span
/// written for it is unreachable - every read path finds data through the project row, so those rows
/// are invisible, uncounted against any quota, and inherited by the next project to take the id.
/// Refusing them is what makes "no data outlives its project" true rather than merely likely.
pub async fn project_accepts_writes(pool: &PgPool, id: &str) -> Result<bool, PostgresError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM projects WHERE id = $1")
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
    pool: &PgPool,
    older_than_secs: i64,
) -> Result<Vec<String>, PostgresError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM projects WHERE deleting_at IS NOT NULL AND deleting_at <= $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The organization a project belongs to, whether or not it is claimed for deletion.
async fn org_of_project_ignoring_fence(
    pool: &PgPool,
    id: &str,
) -> Result<Option<String>, PostgresError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT organization_id FROM projects WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(org,)| org))
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
    pool: &PgPool,
    id: &str,
    was_clean: bool,
    required: i64,
    min_gap_secs: i64,
) -> Result<bool, PostgresError> {
    if was_clean {
        sqlx::query(
            "UPDATE projects SET clean_sweeps = clean_sweeps + 1, last_sweep_at = extract(epoch from now())::bigint \
             WHERE id = $1 AND deleting_at IS NOT NULL \
               AND (last_sweep_at IS NULL OR last_sweep_at <= extract(epoch from now())::bigint - $2)",
        )
        .bind(id)
        .bind(min_gap_secs)
        .execute(pool)
        .await?;
    } else {
        // A late writer's spans reset the evidence, and unconditionally: the safe direction is never
        // gated on a window.
        sqlx::query(
            "UPDATE projects SET clean_sweeps = 0, last_sweep_at = extract(epoch from now())::bigint \
             WHERE id = $1 AND deleting_at IS NOT NULL",
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
         WHERE id = $1 AND deleting_at IS NOT NULL AND clean_sweeps >= $2",
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
             VALUES ($1, extract(epoch from now())::bigint, extract(epoch from now())::bigint) \
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
/// **Claimed exclusively.** `FOR UPDATE SKIP LOCKED` on the inner select, because `WHERE project_id IN (SELECT ...)` is \
/// not enough: the subquery is evaluated against the statement's snapshot, so a replica whose outer update \
/// blocks on a row another replica is updating resumes with a subquery result that still lists it, and the \
/// outer condition only compares `project_id` - which has not changed. Both replicas then return the same \
/// id and both do the storage work. It is the same stale-subquery mechanism as the file claim's.
///
/// **Backed off.** `next_check_at` is materialised from `quiet_checks` on every report, so a project
/// deleted long ago is not re-checked at the same rate as one deleted a minute ago. Without it, a hundred
/// thousand historical deletions meant a hundred thousand storage listings every sweep, forever.
///
/// **Bounded per sweep**, so one pass cannot outlive its window - and the *search* is bounded too, because
/// the index is on the due time rather than on an input to it.
pub async fn claim_deleted_projects_for_check(
    pool: &PgPool,
    lease_secs: i64,
    limit: i64,
) -> Result<Vec<(String, i64)>, PostgresError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "UPDATE deleted_projects \
         SET last_checked_at = extract(epoch from now())::bigint, next_check_at = extract(epoch from now())::bigint + $1, \
             claim_token = claim_token + 1 \
         WHERE project_id IN ( \
             SELECT project_id FROM deleted_projects \
             WHERE next_check_at <= extract(epoch from now())::bigint \
             ORDER BY next_check_at \
             LIMIT $2 FOR UPDATE SKIP LOCKED \
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
    pool: &PgPool,
    project_id: &str,
    claim_token: i64,
    was_quiet: bool,
    base_gap_secs: i64,
    max_gap_secs: i64,
) -> Result<(), PostgresError> {
    // Matched on the token, so a worker whose lease expired part way through its batch updates nothing: the
    // id has been claimed again since, the token has moved on, and overwriting the new holder's schedule -
    // or its result - would let a third worker claim an id that is still being processed.
    if was_quiet {
        sqlx::query(
            "UPDATE deleted_projects \
             SET quiet_checks = quiet_checks + 1, \
                 next_check_at = extract(epoch from now())::bigint + LEAST($1 * (2::bigint ^ LEAST(quiet_checks, 20))::bigint, $2) \
             WHERE project_id = $3 AND claim_token = $4",
        )
        .bind(base_gap_secs)
        .bind(max_gap_secs)
        .bind(project_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE deleted_projects SET quiet_checks = 0, next_check_at = extract(epoch from now())::bigint + $1 \
             WHERE project_id = $2 AND claim_token = $3",
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
    pool: &PgPool,
    retention_secs: i64,
) -> Result<u64, PostgresError> {
    let result = sqlx::query(
        "DELETE FROM deleted_projects WHERE deleted_at <= extract(epoch from now())::bigint - $1",
    )
    .bind(retention_secs)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Claim an organization for deletion, if it exists and nobody else has claimed it.
pub async fn claim_organization_for_deletion(
    pool: &PgPool,
    id: &str,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    // `FOR UPDATE` before the update, so this serialises with `create_project`.
    //
    // The bare UPDATE takes a `FOR NO KEY UPDATE` lock - it changes no key column - which is *compatible*
    // with the key-share lock a foreign-key insert takes on this row. So a project creation could commit
    // under an organization this claim had already tombstoned. Taking the stronger lock first makes the
    // two conflict, which is what forces one to see the other's outcome.
    let mut tx = pool.begin().await?;
    let _: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let result = sqlx::query(
        "UPDATE organizations SET deleting_at = $1 WHERE id = $2 AND deleting_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Organizations claimed for deletion longer ago than `older_than_secs`, so one can be resumed.
pub async fn get_stale_claimed_organizations(
    pool: &PgPool,
    older_than_secs: i64,
) -> Result<Vec<String>, PostgresError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM organizations WHERE deleting_at IS NOT NULL AND deleting_at <= $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// How many of an organization's projects still have rows, tombstoned or not.
///
/// An organization's row may only go when none are left: its cascade would take those rows with it, and
/// a project row is what the cleanup of that project depends on to keep running.
pub async fn count_projects_of_organization(
    pool: &PgPool,
    org_id: &str,
) -> Result<i64, PostgresError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE organization_id = $1")
        .bind(org_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

/// Delete a project by ID. Returns true if a project was deleted.
pub async fn delete_project(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, PostgresError> {
    // Read through the fence: by the time deletion removes the row the project is claimed, so every
    // ordinary read reports it absent - and the org id is still needed to invalidate that org's list.
    let old_org = org_of_project_ignoring_fence(pool, id).await?;

    let result = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate cache entries AFTER successful delete
    if deleted {
        invalidate_project_caches(pool, cache, id, old_org.as_deref()).await;
    }

    Ok(deleted)
}

/// Record that these traces were deleted, so a late ingest cannot resurrect them.
///
/// See `TransactionalRepository::record_deleted_traces` for the race. Written *before* the analytics
/// delete, so there is no instant at which a trace is deleted and not yet tombstoned.
pub async fn record_deleted_traces(
    pool: &PgPool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<(), PostgresError> {
    if trace_ids.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    // `UNNEST` rather than a loop: one statement for the whole batch, and no placeholder count that
    // grows with it.
    sqlx::query(
        "INSERT INTO deleted_traces (project_id, trace_id, deleted_at)
         SELECT $1, t, $3 FROM UNNEST($2::text[]) AS t
         ON CONFLICT (project_id, trace_id) DO NOTHING",
    )
    .bind(project_id)
    .bind(trace_ids)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Which of these traces are tombstoned.
pub async fn deleted_traces_among(
    pool: &PgPool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<std::collections::HashSet<String>, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT trace_id FROM deleted_traces WHERE project_id = $1 AND trace_id = ANY($2::text[])",
    )
    .bind(project_id)
    .bind(trace_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}
