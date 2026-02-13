//! Organization repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::core::constants::{CACHE_TTL_ORG, CACHE_TTL_ORG_LIST, DEFAULT_ORG_ID, RESERVED_SLUGS};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::sqlite::SqliteError;
use crate::data::types::{OrgWithRole, OrganizationRow};

use super::membership::list_member_user_ids;

/// Create a new organization with a generated CUID2 ID
pub async fn create_organization(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    name: &str,
    slug: &str,
) -> Result<OrganizationRow, SqliteError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO organizations (id, name, slug, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(slug)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    // Invalidate slug lookup cache (new slug now exists)
    if let Some(cache) = cache {
        cache.invalidate_key(&CacheKey::org_by_slug(slug)).await;
    }

    Ok(OrganizationRow {
        id,
        name: name.to_string(),
        slug: slug.to_string(),
        created_at: now,
        updated_at: now,
    })
}

/// Create a new organization with owner membership atomically
/// This ensures no orphan orgs if the membership insert fails
pub async fn create_organization_with_owner(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    name: &str,
    slug: &str,
    owner_user_id: &str,
) -> Result<OrganizationRow, SqliteError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    // Start transaction
    let mut tx = pool.begin().await?;

    // Create organization
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(slug)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // Add owner membership
    sqlx::query(
        "INSERT INTO organization_members (organization_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)",
    )
    .bind(&id)
    .bind(owner_user_id)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // Commit transaction
    tx.commit().await?;

    // Invalidate caches AFTER successful commit
    if let Some(cache) = cache {
        cache.invalidate_key(&CacheKey::org_by_slug(slug)).await;
        crate::data::cache::invalidate_membership_caches(cache, &id, owner_user_id).await;
    }

    Ok(OrganizationRow {
        id,
        name: name.to_string(),
        slug: slug.to_string(),
        created_at: now,
        updated_at: now,
    })
}

/// Get an organization by ID (with optional caching)
pub async fn get_organization(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<OrganizationRow>, SqliteError> {
    if let Some(cache) = cache {
        let key = CacheKey::organization(id);

        // Try cache first
        match cache.get::<OrganizationRow>(&key).await {
            Ok(Some(org)) => {
                tracing::trace!(%id, "Organization cache hit");
                return Ok(Some(org));
            }
            Err(e) => tracing::warn!(%id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = get_organization_from_db(pool, id).await?;

        // Store result in cache
        if let Some(ref org) = result
            && let Err(e) = cache
                .set(&key, org, Some(Duration::from_secs(CACHE_TTL_ORG)))
                .await
        {
            tracing::warn!(%id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        get_organization_from_db(pool, id).await
    }
}

/// Get an organization by ID directly from database (no caching)
async fn get_organization_from_db(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<OrganizationRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, name, slug, created_at, updated_at FROM organizations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(
        row.map(|(id, name, slug, created_at, updated_at)| OrganizationRow {
            id,
            name,
            slug,
            created_at,
            updated_at,
        }),
    )
}

/// List organizations for a user with their role (with optional caching)
///
/// Note: Only caches first page with default limit for simplicity.
/// Other pagination parameters bypass cache.
pub async fn list_for_user(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<OrgWithRole>, u64), SqliteError> {
    // Only cache first page with standard limit
    let use_cache = cache.is_some() && page == 1 && limit == 10;

    if use_cache {
        let cache = cache.unwrap();
        let key = CacheKey::orgs_for_user(user_id);

        // Try cache first
        match cache.get::<(Vec<OrgWithRole>, u64)>(&key).await {
            Ok(Some(result)) => {
                tracing::trace!(%user_id, "Orgs for user cache hit");
                return Ok(result);
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = list_for_user_from_db(pool, user_id, page, limit).await?;

        // Store result in cache
        if let Err(e) = cache
            .set(&key, &result, Some(Duration::from_secs(CACHE_TTL_ORG_LIST)))
            .await
        {
            tracing::warn!(%user_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        list_for_user_from_db(pool, user_id, page, limit).await
    }
}

/// List organizations for a user directly from database (no caching)
async fn list_for_user_from_db(
    pool: &SqlitePool,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<OrgWithRole>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64)>(
        r#"
        SELECT o.id, o.name, o.slug, om.role, o.created_at, o.updated_at
        FROM organizations o
        JOIN organization_members om ON o.id = om.organization_id
        WHERE om.user_id = ?
        ORDER BY o.created_at DESC
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
        FROM organizations o
        JOIN organization_members om ON o.id = om.organization_id
        WHERE om.user_id = ?
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let orgs = rows
        .into_iter()
        .map(
            |(id, name, slug, role, created_at, updated_at)| OrgWithRole {
                id,
                name,
                slug,
                role,
                created_at,
                updated_at,
            },
        )
        .collect();

    Ok((orgs, total.0 as u64))
}

/// Update an organization's name by ID (slug is immutable)
pub async fn update_organization(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
    name: &str,
) -> Result<Option<OrganizationRow>, SqliteError> {
    // Get old org for slug invalidation
    let old_org = get_organization_from_db(pool, id).await?;

    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query("UPDATE organizations SET name = ?, updated_at = ? WHERE id = ?")
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
        cache.invalidate_key(&CacheKey::organization(id)).await;
        if let Some(ref old) = old_org {
            cache
                .invalidate_key(&CacheKey::org_by_slug(&old.slug))
                .await;
        }
    }

    get_organization_from_db(pool, id).await
}

/// Delete an organization by ID (transactional cascade only - caller must handle analytics/files)
pub async fn delete_organization(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, SqliteError> {
    // Get old org for slug invalidation
    let old_org = get_organization_from_db(pool, id).await?;

    // Get member user_ids BEFORE deletion for cache invalidation
    let member_user_ids = if cache.is_some() {
        list_member_user_ids(pool, id).await.unwrap_or_default()
    } else {
        vec![]
    };

    let result = sqlx::query("DELETE FROM organizations WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate cache entries AFTER successful delete
    if deleted && let Some(cache) = cache {
        cache.invalidate_key(&CacheKey::organization(id)).await;
        if let Some(ref old) = old_org {
            cache
                .invalidate_key(&CacheKey::org_by_slug(&old.slug))
                .await;
        }

        // Invalidate caches for all affected members
        for user_id in &member_user_ids {
            crate::data::cache::invalidate_membership_caches(cache, id, user_id).await;
        }
    }

    Ok(deleted)
}

/// List all project IDs for an organization (for cleanup before delete)
pub async fn list_project_ids(pool: &SqlitePool, org_id: &str) -> Result<Vec<String>, SqliteError> {
    let rows = sqlx::query_as::<_, (String,)>("SELECT id FROM projects WHERE organization_id = ?")
        .bind(org_id)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Check if a slug is reserved
pub fn is_reserved_slug(slug: &str) -> bool {
    RESERVED_SLUGS.contains(&slug)
}

/// Check if organization is the default (cannot be deleted)
pub fn is_default_org(id: &str) -> bool {
    id == DEFAULT_ORG_ID
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
    async fn test_create_organization() {
        let pool = setup_test_pool().await;
        let org = create_organization(&pool, None, "Test Org", "test-org")
            .await
            .unwrap();

        assert!(!org.id.is_empty());
        assert_eq!(org.name, "Test Org");
        assert_eq!(org.slug, "test-org");
        assert!(org.created_at > 0);
        assert_eq!(org.created_at, org.updated_at);
    }

    #[tokio::test]
    async fn test_get_organization() {
        let pool = setup_test_pool().await;
        let created = create_organization(&pool, None, "Test Org", "test-org")
            .await
            .unwrap();

        let fetched = get_organization(&pool, None, &created.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "Test Org");
        assert_eq!(fetched.slug, "test-org");
    }

    #[tokio::test]
    async fn test_list_for_user() {
        let pool = setup_test_pool().await;

        // Default user should see default org with owner role
        let (orgs, total) = list_for_user(&pool, None, "local", 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(orgs.len(), 1);
        assert_eq!(orgs[0].id, "default");
        assert_eq!(orgs[0].role, "owner");
    }

    #[tokio::test]
    async fn test_update_organization() {
        let pool = setup_test_pool().await;
        let org = create_organization(&pool, None, "Test Org", "test-org")
            .await
            .unwrap();

        let updated = update_organization(&pool, None, &org.id, "Updated Name")
            .await
            .unwrap();
        assert!(updated.is_some());
        let updated = updated.unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.slug, "test-org"); // slug unchanged
    }

    #[tokio::test]
    async fn test_delete_organization() {
        let pool = setup_test_pool().await;
        let org = create_organization(&pool, None, "Test Org", "test-org")
            .await
            .unwrap();

        let deleted = delete_organization(&pool, None, &org.id).await.unwrap();
        assert!(deleted);

        let fetched = get_organization(&pool, None, &org.id).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_default_org_exists() {
        let pool = setup_test_pool().await;
        let org = get_organization(&pool, None, "default").await.unwrap();
        assert!(org.is_some());
        assert_eq!(org.unwrap().name, "Default Organization");
    }

    #[tokio::test]
    async fn test_is_reserved_slug() {
        assert!(is_reserved_slug("default"));
        assert!(is_reserved_slug("api"));
        assert!(is_reserved_slug("admin"));
        assert!(!is_reserved_slug("my-org"));
    }

    #[tokio::test]
    async fn test_is_default_org() {
        assert!(is_default_org("default"));
        assert!(!is_default_org("other"));
    }

    #[tokio::test]
    async fn test_create_organization_with_owner() {
        let pool = setup_test_pool().await;

        // Create org with owner atomically
        let org = create_organization_with_owner(&pool, None, "New Org", "new-org", "local")
            .await
            .unwrap();

        assert!(!org.id.is_empty());
        assert_eq!(org.name, "New Org");
        assert_eq!(org.slug, "new-org");

        // Verify membership was created
        let (orgs, total) = list_for_user(&pool, None, "local", 1, 10).await.unwrap();
        assert_eq!(total, 2); // default + new-org
        assert!(
            orgs.iter()
                .any(|o| o.slug == "new-org" && o.role == "owner")
        );
    }

    #[tokio::test]
    async fn test_create_organization_with_owner_rollback_on_duplicate() {
        let pool = setup_test_pool().await;

        // Create org
        create_organization_with_owner(&pool, None, "First Org", "first", "local")
            .await
            .unwrap();

        // Try to create with same slug - should fail
        let result =
            create_organization_with_owner(&pool, None, "Second Org", "first", "local").await;
        assert!(result.is_err());

        // Verify only one org with that slug
        let (orgs, _) = list_for_user(&pool, None, "local", 1, 10).await.unwrap();
        assert_eq!(orgs.iter().filter(|o| o.slug == "first").count(), 1);
    }
}
