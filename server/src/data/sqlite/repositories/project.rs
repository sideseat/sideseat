//! Project repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::core::constants::{CACHE_TTL_PROJECT, CACHE_TTL_PROJECT_LIST};
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

    sqlx::query(
        "INSERT INTO projects (id, organization_id, name, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
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
async fn get_project_from_db(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ProjectRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, organization_id, name, created_at, updated_at FROM projects WHERE id = ?",
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
        "SELECT id, organization_id, name, created_at, updated_at FROM projects ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
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
        WHERE om.user_id = ?
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
        WHERE om.user_id = ?
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<ProjectRow>, u64), SqliteError> {
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
        WHERE organization_id = ?
        ORDER BY created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE organization_id = ?")
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

    let result = sqlx::query("UPDATE projects SET name = ?, updated_at = ? WHERE id = ?")
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, SqliteError> {
    // Get old project for org_id to invalidate list cache
    let old_project = get_project_from_db(pool, id).await?;

    let result = sqlx::query("DELETE FROM projects WHERE id = ?")
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
