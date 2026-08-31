//! Project repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use sqlx::SqlitePool;

use crate::data::cache::{CacheKey, CacheService};
use crate::data::sqlite::SqliteError;
use crate::data::types::ProjectRow;

use super::membership::list_member_user_ids;

/// Create a new project with a generated CUID2 ID
pub async fn create_project(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    organization_id: &str,
    name: &str,
) -> Result<ProjectRow, SqliteError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

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
    pool: &SqlitePool,
    _cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
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

/// List all projects with pagination, ordered by created_at DESC
/// Note: This function doesn't cache as it's admin-only and pagination varies.
pub async fn list_projects(
    pool: &SqlitePool,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects \
         WHERE deleting_at IS NULL ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
    pool: &SqlitePool,
    _cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    list_for_user_from_db(pool, user_id, page, limit).await
}

/// List projects for a user directly from database (no caching)
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
        ORDER BY p.created_at DESC
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

/// List projects for a specific organization with optional caching
///
/// Note: Only caches first page with default limit for simplicity.
/// Projects in an organization. Not cached, for the reason [`list_for_user`] gives.
pub async fn list_for_org(
    pool: &SqlitePool,
    _cache: Option<&CacheService>,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
    list_for_org_from_db(pool, org_id, page, limit).await
}

/// List projects for a specific organization directly from database (no caching)
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
        ORDER BY created_at DESC
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
    cache: Option<&CacheService>,
    id: &str,
    name: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
    // Get old project for org_id to invalidate list cache
    let old_project = get_project_from_db(pool, id).await?;

    let now = chrono::Utc::now().timestamp();

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
    pool: &SqlitePool,
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let result =
        sqlx::query("UPDATE projects SET deleting_at = ? WHERE id = ? AND deleting_at IS NULL")
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
) -> Result<Vec<String>, SqliteError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM projects WHERE deleting_at IS NOT NULL AND deleting_at <= ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The organization a project belongs to, whether or not it is claimed for deletion.
async fn org_of_project_ignoring_fence(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<String>, SqliteError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT organization_id FROM projects WHERE id = ?")
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
            "INSERT INTO deleted_projects (project_id, deleted_at) VALUES (?, unixepoch()) \
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
/// Three things make this affordable for records that are kept forever.
///
/// **Claimed, not listed.** Moving `last_checked_at` forward in the statement that returns the ids means
/// concurrent instances race per id and one wins per window, so the work does not grow with the instance
/// count.
///
/// **Backed off.** `quiet_checks` counts consecutive checks that found nothing, and the next check is due
/// `base * 2^quiet_checks` later, capped. Without it a hundred thousand historical deletions meant a
/// hundred thousand storage listings every window, forever - correct, and unbounded lifetime work.
///
/// **Bounded per sweep.** At most `limit` ids per call, so one sweep cannot run longer than its own window
/// and overlap the next.
///
/// Times are the database's: with per-instance clocks, skew would decide which ids are due.
pub async fn claim_deleted_projects_for_check(
    pool: &SqlitePool,
    base_gap_secs: i64,
    max_gap_secs: i64,
    limit: i64,
) -> Result<Vec<String>, SqliteError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "UPDATE deleted_projects SET last_checked_at = unixepoch() \
         WHERE project_id IN ( \
             SELECT project_id FROM deleted_projects \
             WHERE last_checked_at IS NULL \
                OR last_checked_at <= unixepoch() - MIN(? * (1 << MIN(quiet_checks, 20)), ?) \
             ORDER BY last_checked_at NULLS FIRST \
             LIMIT ? \
         ) \
         RETURNING project_id",
    )
    .bind(base_gap_secs)
    .bind(max_gap_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Record what a deleted project's check found: quiet moves the next check further out, anything found
/// brings it back to the base interval.
pub async fn record_deleted_project_check(
    pool: &SqlitePool,
    project_id: &str,
    was_quiet: bool,
) -> Result<(), SqliteError> {
    let sql = if was_quiet {
        "UPDATE deleted_projects SET quiet_checks = quiet_checks + 1 WHERE project_id = ?"
    } else {
        // Something arrived for a project that no longer exists, so check it often again.
        "UPDATE deleted_projects SET quiet_checks = 0 WHERE project_id = ?"
    };
    sqlx::query(sql).bind(project_id).execute(pool).await?;
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
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();
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
) -> Result<Vec<String>, SqliteError> {
    let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM organizations WHERE deleting_at IS NOT NULL AND deleting_at <= ?",
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
pub async fn delete_project(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, SqliteError> {
    // Read through the fence: by the time deletion removes the row the project is claimed, so every
    // ordinary read reports it absent - and the org id is still needed to invalidate that org's list.
    let old_org = org_of_project_ignoring_fence(pool, id).await?;

    let result = sqlx::query("DELETE FROM projects WHERE id = ?")
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(crate::data::sqlite::schema::SCHEMA)
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn test_create_project() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Test Project")
            .await
            .unwrap();

        assert!(!project.id.is_empty());
        assert_eq!(project.organization_id, "default");
        assert_eq!(project.name, "Test Project");
        assert!(project.created_at > 0);
        assert_eq!(project.created_at, project.updated_at);
    }

    #[tokio::test]
    async fn test_get_project() {
        let pool = setup_test_pool().await;
        let created = create_project(&pool, None, "default", "Test Project")
            .await
            .unwrap();

        let fetched = get_project(&pool, None, &created.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.organization_id, "default");
        assert_eq!(fetched.name, "Test Project");
    }

    #[tokio::test]
    async fn test_get_project_not_found() {
        let pool = setup_test_pool().await;
        let result = get_project(&pool, None, "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_projects() {
        let pool = setup_test_pool().await;

        // Default project should exist
        let (projects, total) = list_projects(&pool, 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "default");
        assert_eq!(projects[0].organization_id, "default");

        // Create more projects
        create_project(&pool, None, "default", "Project 1")
            .await
            .unwrap();
        create_project(&pool, None, "default", "Project 2")
            .await
            .unwrap();

        let (projects, total) = list_projects(&pool, 1, 10).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(projects.len(), 3);
    }

    #[tokio::test]
    async fn test_list_for_user() {
        let pool = setup_test_pool().await;

        // Local user should see default project
        let (projects, total) = list_for_user(&pool, None, "local", 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "default");

        // Non-member should see nothing
        let (projects, total) = list_for_user(&pool, None, "nonexistent", 1, 10)
            .await
            .unwrap();
        assert_eq!(total, 0);
        assert_eq!(projects.len(), 0);
    }

    #[tokio::test]
    async fn test_list_for_org() {
        let pool = setup_test_pool().await;

        // Default org has default project
        let (projects, total) = list_for_org(&pool, None, "default", 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(projects.len(), 1);

        // Create another project in default org
        create_project(&pool, None, "default", "Project 1")
            .await
            .unwrap();

        let (projects, total) = list_for_org(&pool, None, "default", 1, 10).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(projects.len(), 2);

        // Non-existent org has no projects
        let (projects, total) = list_for_org(&pool, None, "nonexistent", 1, 10)
            .await
            .unwrap();
        assert_eq!(total, 0);
        assert_eq!(projects.len(), 0);
    }

    #[tokio::test]
    async fn test_list_projects_pagination() {
        let pool = setup_test_pool().await;

        for i in 1..=5 {
            create_project(&pool, None, "default", &format!("Project {}", i))
                .await
                .unwrap();
        }

        // Page 1 with limit 2
        let (projects, total) = list_projects(&pool, 1, 2).await.unwrap();
        assert_eq!(total, 6); // 5 + default
        assert_eq!(projects.len(), 2);

        // Page 2 with limit 2
        let (projects, _) = list_projects(&pool, 2, 2).await.unwrap();
        assert_eq!(projects.len(), 2);

        // Page 3 with limit 2
        let (projects, _) = list_projects(&pool, 3, 2).await.unwrap();
        assert_eq!(projects.len(), 2);

        // Page 4 with limit 2 (no more results)
        let (projects, _) = list_projects(&pool, 4, 2).await.unwrap();
        assert_eq!(projects.len(), 0);
    }

    #[tokio::test]
    async fn test_update_project() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Original Name")
            .await
            .unwrap();

        let updated = update_project(&pool, None, &project.id, "Updated Name")
            .await
            .unwrap();
        assert!(updated.is_some());
        let updated = updated.unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.organization_id, "default"); // org unchanged
    }

    #[tokio::test]
    async fn test_delete_project() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "To Delete")
            .await
            .unwrap();

        let deleted = delete_project(&pool, None, &project.id).await.unwrap();
        assert!(deleted);

        let fetched = get_project(&pool, None, &project.id).await.unwrap();
        assert!(fetched.is_none());
    }

    /// A claimed project is not a project any more, as far as every read is concerned.
    ///
    /// This is what makes the fence work at all: deletion spans four stores with no transaction over
    /// them, so the claim is the only interval in which "this project is going away" is a fact anyone
    /// can observe. If reads still returned it, a user could open a project whose spans were already
    /// deleted and whose files were already gone.
    #[tokio::test]
    async fn a_project_claimed_for_deletion_reads_as_absent() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Going Away")
            .await
            .unwrap();

        assert!(
            claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap()
        );

        assert!(
            get_project(&pool, None, &project.id)
                .await
                .unwrap()
                .is_none(),
            "a claimed project is reported absent"
        );
        let (listed, total) = list_projects(&pool, 1, 100).await.unwrap();
        assert!(
            !listed.iter().any(|p| p.id == project.id),
            "and is not listed"
        );
        assert_eq!(
            total as usize,
            listed.len(),
            "the total must count what the page can show, or pagination reports a project nothing returns"
        );
        let (for_org, org_total) = list_for_org(&pool, None, "default", 1, 100).await.unwrap();
        assert!(!for_org.iter().any(|p| p.id == project.id));
        assert_eq!(org_total as usize, for_org.len());

        // Renaming it is refused for the same reason: it is not there to rename.
        assert!(
            update_project(&pool, None, &project.id, "New Name")
                .await
                .unwrap()
                .is_none()
        );

        // But deletion itself still reaches the row - it is the one thing that must.
        assert!(delete_project(&pool, None, &project.id).await.unwrap());
    }

    /// Two deletions of one project: exactly one owns it.
    #[tokio::test]
    async fn only_one_claim_on_a_project_can_win() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Contested")
            .await
            .unwrap();

        assert!(
            claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap()
        );
        assert!(
            !claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap(),
            "the second caller must learn it does not own this deletion"
        );
        assert!(
            !claim_project_for_deletion(&pool, None, "no-such-project")
                .await
                .unwrap(),
            "and a project that does not exist cannot be claimed"
        );
        assert!(
            !project_accepts_writes(&pool, &project.id).await.unwrap(),
            "and a claimed project accepts no writes"
        );
    }

    /// An abandoned project deletion is findable by age, so it can be finished.
    ///
    /// Nothing releases a claim - that is what lets it survive a restart - so without this a project
    /// whose cleanup died is fenced forever: hidden from every read while its data is still on disk.
    #[tokio::test]
    async fn an_abandoned_project_claim_is_found_by_age() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Half Deleted")
            .await
            .unwrap();
        assert!(
            claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap()
        );

        assert!(
            get_stale_claimed_projects(&pool, 60)
                .await
                .unwrap()
                .is_empty(),
            "a claim taken a moment ago is a deletion in progress"
        );

        sqlx::query("UPDATE projects SET deleting_at = deleting_at - 1000 WHERE id = ?")
            .bind(&project.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            get_stale_claimed_projects(&pool, 60).await.unwrap(),
            vec![project.id.clone()]
        );

        delete_project(&pool, None, &project.id).await.unwrap();
        assert!(
            get_stale_claimed_projects(&pool, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A tombstone is removed on repeated evidence, and a late write starts that evidence over.
    ///
    /// This is the barrier, expressed as the thing it has to do. A writer that read the fence before the
    /// tombstone can commit arbitrarily later, so a sweep that finds data must not be treated as a
    /// failure - it is the case the tombstone exists for. It deletes what appeared and the count restarts.
    #[tokio::test]
    async fn a_tombstone_is_removed_by_repeated_evidence_and_a_late_write_resets_it() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Going Away")
            .await
            .unwrap();
        assert!(
            claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap()
        );

        // Four quiet sweeps are not enough when five are required.
        for pass in 1..=4 {
            assert!(
                !record_project_sweep(&pool, &project.id, true, 5, 0)
                    .await
                    .unwrap(),
                "pass {pass} must not be enough on its own"
            );
        }
        // A late writer's spans show up. The count starts over - that is the whole point.
        assert!(
            !record_project_sweep(&pool, &project.id, false, 5, 0)
                .await
                .unwrap(),
            "a sweep that found data cannot also authorise removing the row"
        );
        for pass in 1..=4 {
            assert!(
                !record_project_sweep(&pool, &project.id, true, 5, 0)
                    .await
                    .unwrap(),
                "the count restarted, so pass {pass} is not enough again"
            );
        }
        // Through all of it the project accepted no writes.
        assert!(!project_accepts_writes(&pool, &project.id).await.unwrap());

        // The fifth removes the row, in the same statement that decides it may go - so no other sweep can
        // reset the count in between and leave this one acting on a decision that is no longer true.
        assert!(
            record_project_sweep(&pool, &project.id, true, 5, 0)
                .await
                .unwrap(),
            "five consecutive quiet sweeps is the evidence the row waits for"
        );
        assert!(
            get_project(&pool, None, &project.id)
                .await
                .unwrap()
                .is_none(),
            "and the row is gone with it"
        );
        assert!(
            !record_project_sweep(&pool, &project.id, true, 5, 0)
                .await
                .unwrap(),
            "a project with no row has no tombstone to advance"
        );
    }

    /// A removed tombstone leaves a record, so a write that arrives afterwards is still collected.
    ///
    /// This is what makes the guarantee hold for an *arbitrarily* delayed writer rather than one that
    /// finishes within a few sweeps. The tombstone goes on finite evidence; a writer that read the fence
    /// before it can commit after the row is gone, and without a record nothing would know the project had
    /// existed. The record is written in the same transaction as the removal, because a crash in between is
    /// the one moment it matters.
    #[tokio::test]
    async fn removing_a_tombstone_records_that_the_project_existed() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Remembered")
            .await
            .unwrap();
        claim_project_for_deletion(&pool, None, &project.id)
            .await
            .unwrap();

        assert!(
            claim_deleted_projects_for_check(&pool, 0, 0, 100)
                .await
                .unwrap()
                .is_empty(),
            "nothing is remembered while the tombstone is still there"
        );
        assert!(
            record_project_sweep(&pool, &project.id, true, 1, 0)
                .await
                .unwrap(),
            "one clean sweep is enough when one is required"
        );
        assert_eq!(
            claim_deleted_projects_for_check(&pool, 0, 0, 100)
                .await
                .unwrap(),
            vec![project.id.clone()],
            "and the id is remembered, so the sweep keeps collecting for it"
        );

        // Still refused for writes: an absent project is refused as firmly as a claimed one.
        assert!(!project_accepts_writes(&pool, &project.id).await.unwrap());

        // Forgotten only past the retention, which is where the residual is stated.
        assert_eq!(
            forget_deleted_projects(&pool, 3600).await.unwrap(),
            0,
            "a deletion from a moment ago is inside any retention"
        );
        assert_eq!(
            forget_deleted_projects(&pool, 0).await.unwrap(),
            1,
            "and past it the id is forgotten"
        );
        assert!(
            claim_deleted_projects_for_check(&pool, 0, 0, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A deleted id is checked once per window, however many instances are sweeping.
    ///
    /// The records are permanent - any retention would be a bound on how late a stalled writer may commit
    /// and still be collected - so a bare list would have every instance re-check every deletion ever made
    /// on every sweep: work proportional to instances times lifetime deletions. Claiming the check in the
    /// statement that returns it makes concurrent instances race for each id.
    #[tokio::test]
    async fn a_deleted_project_is_claimed_for_checking_once_per_window() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Remembered")
            .await
            .unwrap();
        claim_project_for_deletion(&pool, None, &project.id)
            .await
            .unwrap();
        record_project_sweep(&pool, &project.id, true, 1, 0)
            .await
            .unwrap();

        // Five instances, one window: exactly one gets the id.
        let mut claimed = 0;
        for _ in 0..5 {
            claimed += claim_deleted_projects_for_check(&pool, 600, 600, 100)
                .await
                .unwrap()
                .len();
        }
        assert_eq!(
            claimed, 1,
            "an id claimed inside a window must not be handed out again, or the work grows with the \
             instance count"
        );

        // And with no window it is available again - which is what makes the next window's check happen.
        assert_eq!(
            claim_deleted_projects_for_check(&pool, 0, 0, 100)
                .await
                .unwrap()
                .len(),
            1,
            "the next window claims it again"
        );
    }

    /// Discovery is bounded per sweep and backs off per quiet check.
    ///
    /// The records are permanent - any retention would bound how late a stalled writer may commit and still
    /// be collected - so what has to be bounded is the *rate*. Without the backoff a hundred thousand
    /// historical deletions meant a hundred thousand storage listings every sweep, forever; without the
    /// batch cap one sweep could outlive its own window and overlap the next.
    #[tokio::test]
    async fn deleted_project_checks_are_batched_and_back_off() {
        let pool = setup_test_pool().await;
        for n in 0..5 {
            let project = create_project(&pool, None, "default", &format!("Gone {n}"))
                .await
                .unwrap();
            claim_project_for_deletion(&pool, None, &project.id)
                .await
                .unwrap();
            record_project_sweep(&pool, &project.id, true, 1, 0)
                .await
                .unwrap();
        }

        // The batch caps the work regardless of how many are due.
        assert_eq!(
            claim_deleted_projects_for_check(&pool, 0, 0, 2)
                .await
                .unwrap()
                .len(),
            2,
            "a sweep must not take on more than its batch, or it can outlive its own window"
        );

        // A quiet check pushes the next one out; a check that found something brings it back.
        let ids = claim_deleted_projects_for_check(&pool, 0, 0, 10)
            .await
            .unwrap();
        let subject = ids.first().expect("some id is due").clone();
        record_deleted_project_check(&pool, &subject, true)
            .await
            .unwrap();
        record_deleted_project_check(&pool, &subject, true)
            .await
            .unwrap();
        let quiet: (i64,) =
            sqlx::query_as("SELECT quiet_checks FROM deleted_projects WHERE project_id = ?")
                .bind(&subject)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(quiet.0, 2, "consecutive quiet checks accumulate");

        // With a base interval of 60s and two quiet checks, this id is not due for 240s.
        let due = claim_deleted_projects_for_check(&pool, 60, 86_400, 10)
            .await
            .unwrap();
        assert!(
            !due.contains(&subject),
            "a project checked twice with nothing found must not be re-checked at the base rate"
        );

        record_deleted_project_check(&pool, &subject, false)
            .await
            .unwrap();
        let quiet: (i64,) =
            sqlx::query_as("SELECT quiet_checks FROM deleted_projects WHERE project_id = ?")
                .bind(&subject)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            quiet.0, 0,
            "finding something brings it back to the base interval"
        );
    }

    /// Concurrent instances cannot inflate the count: one window, one increment.
    ///
    /// Every instance of a horizontally scaled deployment runs the sweep. With a bare increment, five
    /// instances reached five "consecutive clean sweeps" inside a single interval - the barrier getting
    /// *weaker* the more instances you run, which is the opposite of what scaling out should do. The
    /// increment is gated on a window having passed, and it is one atomic UPDATE, so the instances race
    /// for the row and one wins.
    #[tokio::test]
    async fn concurrent_sweeps_within_one_window_count_once() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Swept By Many")
            .await
            .unwrap();
        claim_project_for_deletion(&pool, None, &project.id)
            .await
            .unwrap();

        // Five instances, one window: the count must advance by one, so five is never reached.
        for _ in 0..5 {
            assert!(
                !record_project_sweep(&pool, &project.id, true, 5, 600)
                    .await
                    .unwrap(),
                "sweeps inside one window must not stack up into the evidence the row waits for"
            );
        }
        let count: (i64,) = sqlx::query_as("SELECT clean_sweeps FROM projects WHERE id = ?")
            .bind(&project.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count.0, 1,
            "five concurrent sweeps in one window are one observation"
        );

        // A finding of data resets unconditionally - the safe direction, and not gated on the window.
        record_project_sweep(&pool, &project.id, false, 5, 600)
            .await
            .unwrap();
        let count: (i64,) = sqlx::query_as("SELECT clean_sweeps FROM projects WHERE id = ?")
            .bind(&project.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count.0, 0,
            "a late writer's spans reset the evidence whatever the window"
        );
    }

    /// An organization is tombstoned too, because its row is what its projects' tombstones hang from.
    #[tokio::test]
    async fn an_organization_tombstone_hides_it_and_its_row_waits_for_its_projects() {
        let pool = setup_test_pool().await;
        let project = create_project(&pool, None, "default", "Owned")
            .await
            .unwrap();

        assert!(
            claim_organization_for_deletion(&pool, "default")
                .await
                .unwrap()
        );
        assert!(
            !claim_organization_for_deletion(&pool, "default")
                .await
                .unwrap(),
            "one caller owns the deletion"
        );
        assert!(
            get_stale_claimed_organizations(&pool, 0)
                .await
                .unwrap()
                .contains(&"default".to_string())
        );
        assert!(
            count_projects_of_organization(&pool, "default")
                .await
                .unwrap()
                >= 1,
            "its row may not go while a project row still hangs from it"
        );

        // Tombstoned projects still count: their rows are what their own cleanups depend on.
        claim_project_for_deletion(&pool, None, &project.id)
            .await
            .unwrap();
        let before = count_projects_of_organization(&pool, "default")
            .await
            .unwrap();
        delete_project(&pool, None, &project.id).await.unwrap();
        assert_eq!(
            count_projects_of_organization(&pool, "default")
                .await
                .unwrap(),
            before - 1,
            "and it stops counting only when the row is really gone"
        );
    }

    #[tokio::test]
    async fn test_delete_project_not_found() {
        let pool = setup_test_pool().await;
        let deleted = delete_project(&pool, None, "nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_default_project_exists() {
        let pool = setup_test_pool().await;
        let project = get_project(&pool, None, "default").await.unwrap();
        assert!(project.is_some());
        let project = project.unwrap();
        assert_eq!(project.name, "Default Project");
        assert_eq!(project.organization_id, "default");
    }
}
