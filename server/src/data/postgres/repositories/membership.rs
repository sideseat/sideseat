//! Organization membership repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::constants::{
    CACHE_TTL_MEMBERSHIP, ORG_ROLE_ADMIN, ORG_ROLE_MEMBER, ORG_ROLE_OWNER, ORG_ROLE_VIEWER,
};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::postgres::PostgresError;
use crate::data::types::{LastOwnerResult, MemberWithUser, MembershipRow};

/// Role level for hierarchy checks
fn role_level(role: &str) -> u8 {
    match role {
        ORG_ROLE_VIEWER => 1,
        ORG_ROLE_MEMBER => 2,
        ORG_ROLE_ADMIN => 3,
        ORG_ROLE_OWNER => 4,
        _ => 0,
    }
}

/// Check if a role has at least the minimum required level
pub fn has_min_role_level(user_role: &str, min_role: &str) -> bool {
    role_level(user_role) >= role_level(min_role)
}

/// Add a member to an organization (upsert: updates role if exists)
pub async fn add_member(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    role: &str,
) -> Result<MembershipRow, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT INTO organization_members (organization_id, user_id, role, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT(organization_id, user_id) DO UPDATE SET
            role = EXCLUDED.role,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(org_id)
    .bind(user_id)
    .bind(role)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    // Invalidate membership caches AFTER successful write
    if let Some(cache) = cache {
        crate::data::cache::invalidate_membership_caches(cache, org_id, user_id).await;
    }

    Ok(MembershipRow {
        organization_id: org_id.to_string(),
        user_id: user_id.to_string(),
        role: role.to_string(),
        created_at: now,
        updated_at: now,
    })
}

/// Remove a member from an organization
pub async fn remove_member(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, PostgresError> {
    let result =
        sqlx::query("DELETE FROM organization_members WHERE organization_id = $1 AND user_id = $2")
            .bind(org_id)
            .bind(user_id)
            .execute(pool)
            .await?;

    let removed = result.rows_affected() > 0;

    // Invalidate membership caches AFTER successful delete
    if removed && let Some(cache) = cache {
        crate::data::cache::invalidate_membership_caches(cache, org_id, user_id).await;
    }

    Ok(removed)
}

/// Update a member's role
pub async fn update_role(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Option<MembershipRow>, PostgresError> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "UPDATE organization_members SET role = $1, updated_at = $2 WHERE organization_id = $3 AND user_id = $4",
    )
    .bind(role)
    .bind(now)
    .bind(org_id)
    .bind(user_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    // Invalidate cache entries AFTER successful write
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::membership(org_id, user_id)).await {
            tracing::warn!(%org_id, %user_id, error = %e, "Cache invalidation error");
        }
        // Invalidate orgs_for_user since it includes role info
        if let Err(e) = cache.delete(&CacheKey::orgs_for_user(user_id)).await {
            tracing::warn!(%user_id, error = %e, "Cache invalidation error");
        }
    }

    get_membership_from_db(pool, org_id, user_id).await
}

/// Get a specific membership (with optional caching)
pub async fn get_membership(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MembershipRow>, PostgresError> {
    if let Some(cache) = cache {
        let key = CacheKey::membership(org_id, user_id);

        // Try cache first
        match cache.get::<MembershipRow>(&key).await {
            Ok(Some(membership)) => {
                tracing::trace!(%org_id, %user_id, "Membership cache hit");
                return Ok(Some(membership));
            }
            Err(e) => tracing::warn!(%org_id, %user_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = get_membership_from_db(pool, org_id, user_id).await?;

        // Store result in cache (short TTL for membership - 1 min)
        if let Some(ref m) = result
            && let Err(e) = cache
                .set(&key, m, Some(Duration::from_secs(CACHE_TTL_MEMBERSHIP)))
                .await
        {
            tracing::warn!(%org_id, %user_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        get_membership_from_db(pool, org_id, user_id).await
    }
}

/// Get a specific membership directly from database (no caching)
async fn get_membership_from_db(
    pool: &PgPool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MembershipRow>, PostgresError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT organization_id, user_id, role, created_at, updated_at
        FROM organization_members
        WHERE organization_id = $1 AND user_id = $2
        "#,
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(organization_id, user_id, role, created_at, updated_at)| MembershipRow {
            organization_id,
            user_id,
            role,
            created_at,
            updated_at,
        },
    ))
}

/// List all members of an organization with user info
pub async fn list_members(
    pool: &PgPool,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<MemberWithUser>, u64), PostgresError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, i64)>(
        r#"
        SELECT u.id, u.email, u.display_name, om.role, om.created_at
        FROM organization_members om
        JOIN users u ON om.user_id = u.id
        WHERE om.organization_id = $1
        ORDER BY om.created_at ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(org_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?;

    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM organization_members WHERE organization_id = $1")
            .bind(org_id)
            .fetch_one(pool)
            .await?;

    let members = rows
        .into_iter()
        .map(
            |(user_id, email, display_name, role, joined_at)| MemberWithUser {
                user_id,
                email,
                display_name,
                role,
                joined_at,
            },
        )
        .collect();

    Ok((members, total.0 as u64))
}

/// List all member user_ids for an organization (for cache invalidation)
pub async fn list_member_user_ids(
    pool: &PgPool,
    org_id: &str,
) -> Result<Vec<String>, PostgresError> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT user_id FROM organization_members WHERE organization_id = $1",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

/// Get a single member with user info (efficient single-row fetch)
pub async fn get_member_with_user(
    pool: &PgPool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MemberWithUser>, PostgresError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, i64)>(
        r#"
        SELECT u.id, u.email, u.display_name, om.role, om.created_at
        FROM organization_members om
        JOIN users u ON om.user_id = u.id
        WHERE om.organization_id = $1 AND om.user_id = $2
        "#,
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(user_id, email, display_name, role, joined_at)| MemberWithUser {
            user_id,
            email,
            display_name,
            role,
            joined_at,
        },
    ))
}

/// Check if user has at least the minimum role in an organization (with optional caching)
pub async fn has_min_role(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    min_role: &str,
) -> Result<bool, PostgresError> {
    let membership = get_membership(pool, cache, org_id, user_id).await?;
    Ok(membership.is_some_and(|m| has_min_role_level(&m.role, min_role)))
}

/// Count owners in an organization (for last-owner protection)
pub async fn count_owners(pool: &PgPool, org_id: &str) -> Result<u64, PostgresError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM organization_members WHERE organization_id = $1 AND role = $2",
    )
    .bind(org_id)
    .bind(ORG_ROLE_OWNER)
    .fetch_one(pool)
    .await?;

    Ok(count.0 as u64)
}

/// Check if user is the last owner of the organization (with optional caching)
pub async fn is_last_owner(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, PostgresError> {
    let membership = get_membership(pool, cache, org_id, user_id).await?;
    if membership.is_none_or(|m| m.role != ORG_ROLE_OWNER) {
        return Ok(false);
    }

    let owner_count = count_owners(pool, org_id).await?;
    Ok(owner_count == 1)
}

/// Remove a member with atomic last-owner protection (transactional)
/// Returns LastOwnerResult to indicate if operation was blocked
pub async fn remove_member_atomic(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<LastOwnerResult<()>, PostgresError> {
    let mut tx = pool.begin().await?;

    // Check if member exists and get their role
    let membership = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((role,)) = membership else {
        return Ok(LastOwnerResult::NotFound);
    };

    // If owner, check if last owner (within transaction for atomicity)
    if role == ORG_ROLE_OWNER {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM organization_members WHERE organization_id = $1 AND role = $2",
        )
        .bind(org_id)
        .bind(ORG_ROLE_OWNER)
        .fetch_one(&mut *tx)
        .await?;

        if count.0 == 1 {
            return Ok(LastOwnerResult::LastOwner);
        }
    }

    // Safe to remove
    sqlx::query("DELETE FROM organization_members WHERE organization_id = $1 AND user_id = $2")
        .bind(org_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Invalidate cache entries AFTER successful commit
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::membership(org_id, user_id)).await {
            tracing::warn!(%org_id, %user_id, error = %e, "Cache invalidation error");
        }
        if let Err(e) = cache.delete(&CacheKey::orgs_for_user(user_id)).await {
            tracing::warn!(%user_id, error = %e, "Cache invalidation error");
        }
        if let Err(e) = cache.delete(&CacheKey::projects_for_user(user_id)).await {
            tracing::warn!(%user_id, error = %e, "Cache invalidation error");
        }
    }

    Ok(LastOwnerResult::Success(()))
}

/// Update a member's role with atomic last-owner protection (transactional)
/// Prevents demoting the last owner
pub async fn update_role_atomic(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    new_role: &str,
) -> Result<LastOwnerResult<MembershipRow>, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;

    // Check if member exists and get their current role
    let membership = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((current_role,)) = membership else {
        return Ok(LastOwnerResult::NotFound);
    };

    // If demoting from owner, check if last owner (within transaction for atomicity)
    if current_role == ORG_ROLE_OWNER && new_role != ORG_ROLE_OWNER {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM organization_members WHERE organization_id = $1 AND role = $2",
        )
        .bind(org_id)
        .bind(ORG_ROLE_OWNER)
        .fetch_one(&mut *tx)
        .await?;

        if count.0 == 1 {
            return Ok(LastOwnerResult::LastOwner);
        }
    }

    // Safe to update
    sqlx::query(
        "UPDATE organization_members SET role = $1, updated_at = $2 WHERE organization_id = $3 AND user_id = $4",
    )
    .bind(new_role)
    .bind(now)
    .bind(org_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Invalidate cache entries AFTER successful commit
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::membership(org_id, user_id)).await {
            tracing::warn!(%org_id, %user_id, error = %e, "Cache invalidation error");
        }
        // Invalidate orgs_for_user since it includes role info
        if let Err(e) = cache.delete(&CacheKey::orgs_for_user(user_id)).await {
            tracing::warn!(%user_id, error = %e, "Cache invalidation error");
        }
    }

    // Fetch updated membership (bypass cache to get fresh data)
    get_membership_from_db(pool, org_id, user_id)
        .await
        .map(|opt| opt.map_or(LastOwnerResult::NotFound, LastOwnerResult::Success))
}
