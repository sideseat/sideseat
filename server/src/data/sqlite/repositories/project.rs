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

/// How long ago this project was claimed for deletion, in seconds, or `None` if it is not claimed.
///
/// Deletion reads this to decide whether the row may go yet: the write path checks the fence and then
/// writes, and the two are in different stores, so the row has to outlive the claim by long enough that
/// no writer can still be acting on a check taken before it.
pub async fn project_claim_age_secs(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<i64>, SqliteError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((Some(at),)) => Some((chrono::Utc::now().timestamp() - at).max(0)),
        _ => None,
    })
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
