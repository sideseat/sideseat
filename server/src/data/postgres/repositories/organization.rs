//! Organization repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::constants::{CACHE_TTL_ORG, CACHE_TTL_ORG_LIST, DEFAULT_ORG_ID, RESERVED_SLUGS};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::postgres::PostgresError;
use crate::data::types::{OrgWithRole, OrganizationRow};

use super::membership::list_member_user_ids;

/// Create a new organization with a generated CUID2 ID
pub async fn create_organization(
    pool: &PgPool,
    cache: Option<&CacheService>,
    name: &str,
    slug: &str,
) -> Result<OrganizationRow, PostgresError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO organizations (id, name, slug, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    name: &str,
    slug: &str,
    owner_user_id: &str,
) -> Result<OrganizationRow, PostgresError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    // Start transaction
    let mut tx = pool.begin().await?;

    // Create organization
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
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
        "INSERT INTO organization_members (organization_id, user_id, role, created_at, updated_at) VALUES ($1, $2, 'owner', $3, $4)",
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<OrganizationRow>, PostgresError> {
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
    pool: &PgPool,
    id: &str,
) -> Result<Option<OrganizationRow>, PostgresError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT id, name, slug, created_at, updated_at FROM organizations WHERE id = $1",
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<OrgWithRole>, u64), PostgresError> {
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
    pool: &PgPool,
    user_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<OrgWithRole>, u64), PostgresError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64)>(
        r#"
        SELECT o.id, o.name, o.slug, om.role, o.created_at, o.updated_at
        FROM organizations o
        JOIN organization_members om ON o.id = om.organization_id
        WHERE om.user_id = $1
        ORDER BY o.created_at DESC
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
        FROM organizations o
        JOIN organization_members om ON o.id = om.organization_id
        WHERE om.user_id = $1
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
    name: &str,
) -> Result<Option<OrganizationRow>, PostgresError> {
    // Get old org for slug invalidation
    let old_org = get_organization_from_db(pool, id).await?;

    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query("UPDATE organizations SET name = $1, updated_at = $2 WHERE id = $3")
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, PostgresError> {
    // Get old org for slug invalidation
    let old_org = get_organization_from_db(pool, id).await?;

    // Get member user_ids BEFORE deletion for cache invalidation
    let member_user_ids = if cache.is_some() {
        list_member_user_ids(pool, id).await.unwrap_or_default()
    } else {
        vec![]
    };

    let result = sqlx::query("DELETE FROM organizations WHERE id = $1")
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
pub async fn list_project_ids(pool: &PgPool, org_id: &str) -> Result<Vec<String>, PostgresError> {
    let rows = sqlx::query_as::<_, (String,)>("SELECT id FROM projects WHERE organization_id = $1")
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
