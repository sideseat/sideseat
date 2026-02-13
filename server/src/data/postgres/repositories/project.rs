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

        // Store result in cache
        if let Some(ref proj) = result
            && let Err(e) = cache
                .set(&key, proj, Some(Duration::from_secs(CACHE_TTL_PROJECT)))
                .await
        {
            tracing::warn!(%id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        get_project_from_db(pool, id).await
    }
}

/// Get a project by ID directly from database (no caching)
async fn get_project_from_db(pool: &PgPool, id: &str) -> Result<Option<ProjectRow>, PostgresError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects WHERE id = $1",
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
        "SELECT id, organization_id, name, created_at, updated_at FROM projects ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects")
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
        WHERE om.user_id = $1
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
        WHERE om.user_id = $1
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
        WHERE organization_id = $1
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(org_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE organization_id = $1")
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

    let result = sqlx::query("UPDATE projects SET name = $1, updated_at = $2 WHERE id = $3")
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

/// Delete a project by ID. Returns true if a project was deleted.
pub async fn delete_project(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, PostgresError> {
    // Get old project for org_id to invalidate list cache
    let old_project = get_project_from_db(pool, id).await?;

    let result = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate cache entries AFTER successful delete
    if deleted && let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::project(id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }

        // Invalidate org's project list cache if project existed
        if let Some(ref old) = old_project {
            if let Err(e) = cache
                .delete(&CacheKey::projects_for_org(&old.organization_id))
                .await
            {
                tracing::warn!(org_id = %old.organization_id, error = %e, "Cache invalidation error");
            }

            // Invalidate projects_for_user for all org members
            if let Ok(member_user_ids) = list_member_user_ids(pool, &old.organization_id).await {
                for user_id in &member_user_ids {
                    if let Err(e) = cache.delete(&CacheKey::projects_for_user(user_id)).await {
                        tracing::warn!(%user_id, error = %e, "Cache invalidation error");
                    }
                }
            }
        }
    }

    Ok(deleted)
}
