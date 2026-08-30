//! Project repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::constants::{CACHE_TTL_PROJECT, CACHE_TTL_PROJECT_LIST};
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

    sqlx::query(
        "INSERT INTO projects (id, organization_id, name, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&id)
    .bind(organization_id)
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

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

/// Whether this project has stopped being live since a caller last looked.
///
/// Used to re-check after filling a cache entry: the fill is a write that follows its read, so without a
/// second look it can reinstate a project that was claimed in between.
async fn project_is_claimed_or_absent(pool: &PgPool, id: &str) -> bool {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM projects WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    !matches!(row, Some((None,)))
}

/// Get a project by ID (with optional caching)
pub async fn get_project(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<ProjectRow>, PostgresError> {
    if let Some(cache) = cache {
        let key = CacheKey::project(id);

        // Try cache first
        match cache.get::<ProjectRow>(&key).await {
            Ok(Some(project)) => {
                tracing::trace!(%id, "Project cache hit");
                return Ok(Some(project));
            }
            Err(e) => tracing::warn!(%id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = get_project_from_db(pool, id).await?;

        // Filled only while the project is live, and *checked again* after the write.
        //
        // Filling a read-through cache is a write that happens after its read, so it races the deletion
        // fence in the direction that matters: a reader misses, reads the live row, deletion claims the
        // project and invalidates the cache, and then the reader's fill puts the project back for the
        // cache's five minutes - readable, listable, and gone from the database's point of view.
        //
        // Re-reading the fence after the fill closes it. Either the claim was already visible, in which
        // case there is nothing to fill, or it lands after this check - and then its own invalidation
        // comes after the fill and removes it. There is no ordering left in which a stale entry survives.
        if let Some(ref proj) = result {
            if let Err(e) = cache
                .set(&key, proj, Some(Duration::from_secs(CACHE_TTL_PROJECT)))
                .await
            {
                tracing::warn!(%id, error = %e, "Cache set error");
            }
            if project_is_claimed_or_absent(pool, id).await
                && let Err(e) = cache.delete(&key).await
            {
                tracing::warn!(%id, error = %e, "Cache invalidation error");
            }
        }

        Ok(result)
    } else {
        get_project_from_db(pool, id).await
    }
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
pub async fn list_for_user(
    pool: &PgPool,
    cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    // Only cache first page with standard limit
    let use_cache = cache.is_some() && page == 1 && limit == 10;

    if use_cache {
        let cache = cache.unwrap();
        let key = CacheKey::projects_for_user(user_id);

        // Try cache first
        match cache.get::<(Vec<ProjectRow>, u64)>(&key).await {
            Ok(Some(result)) => {
                tracing::trace!(%user_id, "Projects for user cache hit");
                return Ok(result);
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = list_for_user_from_db(pool, user_id, page, limit).await?;

        // Store result in cache
        if let Err(e) = cache
            .set(
                &key,
                &result,
                Some(Duration::from_secs(CACHE_TTL_PROJECT_LIST)),
            )
            .await
        {
            tracing::warn!(%user_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        list_for_user_from_db(pool, user_id, page, limit).await
    }
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
pub async fn list_for_org(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), PostgresError> {
    // Only cache first page with standard limit
    let use_cache = cache.is_some() && page == 1 && limit == 10;

    if use_cache {
        let cache = cache.unwrap();
        let key = CacheKey::projects_for_org(org_id);

        // Try cache first
        match cache.get::<(Vec<ProjectRow>, u64)>(&key).await {
            Ok(Some(result)) => {
                tracing::trace!(%org_id, "Projects for org cache hit");
                return Ok(result);
            }
            Err(e) => tracing::warn!(%org_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = list_for_org_from_db(pool, org_id, page, limit).await?;

        // Store result in cache
        if let Err(e) = cache
            .set(
                &key,
                &result,
                Some(Duration::from_secs(CACHE_TTL_PROJECT_LIST)),
            )
            .await
        {
            tracing::warn!(%org_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        list_for_org_from_db(pool, org_id, page, limit).await
    }
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

/// Record what a cleanup sweep observed, and answer whether the tombstone may now go.
///
/// This is the barrier, and it is deliberately not a clock. A writer that read the fence before the
/// tombstone can commit arbitrarily later - a blocking insert that outlived its statement timeout, an
/// object store retrying, a container that was paused - so no elapsed time proves it has finished. What
/// *is* provable: while the tombstone exists no new writer passes the fence, and the sweep keeps deleting
/// whatever appears. So the row is removed on the strength of repeated observation - `required` sweeps in
/// a row that found nothing - and a sweep that finds data resets the count and starts it over.
pub async fn record_project_sweep(
    pool: &PgPool,
    id: &str,
    was_clean: bool,
    required: i64,
) -> Result<bool, PostgresError> {
    let sql = if was_clean {
        "UPDATE projects SET clean_sweeps = clean_sweeps + 1 WHERE id = $1 AND deleting_at IS NOT NULL"
    } else {
        "UPDATE projects SET clean_sweeps = 0 WHERE id = $1 AND deleting_at IS NOT NULL"
    };
    sqlx::query(sql).bind(id).execute(pool).await?;
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT clean_sweeps FROM projects WHERE id = $1 AND deleting_at IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(row, Some((n,)) if n >= required))
}

/// Claim an organization for deletion, if it exists and nobody else has claimed it.
pub async fn claim_organization_for_deletion(
    pool: &PgPool,
    id: &str,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        "UPDATE organizations SET deleting_at = $1 WHERE id = $2 AND deleting_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
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
