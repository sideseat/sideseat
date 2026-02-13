//! Organization membership repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::core::constants::{
    CACHE_TTL_MEMBERSHIP, ORG_ROLE_ADMIN, ORG_ROLE_MEMBER, ORG_ROLE_OWNER, ORG_ROLE_VIEWER,
};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::sqlite::SqliteError;
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    role: &str,
) -> Result<MembershipRow, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT INTO organization_members (organization_id, user_id, role, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(organization_id, user_id) DO UPDATE SET
            role = excluded.role,
            updated_at = excluded.updated_at
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, SqliteError> {
    let result =
        sqlx::query("DELETE FROM organization_members WHERE organization_id = ? AND user_id = ?")
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Option<MembershipRow>, SqliteError> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "UPDATE organization_members SET role = ?, updated_at = ? WHERE organization_id = ? AND user_id = ?",
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MembershipRow>, SqliteError> {
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
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MembershipRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        r#"
        SELECT organization_id, user_id, role, created_at, updated_at
        FROM organization_members
        WHERE organization_id = ? AND user_id = ?
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
    pool: &SqlitePool,
    org_id: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<MemberWithUser>, u64), SqliteError> {
    let offset = (page.saturating_sub(1)) * limit;

    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, i64)>(
        r#"
        SELECT u.id, u.email, u.display_name, om.role, om.created_at
        FROM organization_members om
        JOIN users u ON om.user_id = u.id
        WHERE om.organization_id = ?
        ORDER BY om.created_at ASC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM organization_members WHERE organization_id = ?")
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
    pool: &SqlitePool,
    org_id: &str,
) -> Result<Vec<String>, SqliteError> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT user_id FROM organization_members WHERE organization_id = ?",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

/// Get a single member with user info (efficient single-row fetch)
pub async fn get_member_with_user(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MemberWithUser>, SqliteError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, i64)>(
        r#"
        SELECT u.id, u.email, u.display_name, om.role, om.created_at
        FROM organization_members om
        JOIN users u ON om.user_id = u.id
        WHERE om.organization_id = ? AND om.user_id = ?
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    min_role: &str,
) -> Result<bool, SqliteError> {
    let membership = get_membership(pool, cache, org_id, user_id).await?;
    Ok(membership.is_some_and(|m| has_min_role_level(&m.role, min_role)))
}

/// Count owners in an organization (for last-owner protection)
pub async fn count_owners(pool: &SqlitePool, org_id: &str) -> Result<u64, SqliteError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND role = ?",
    )
    .bind(org_id)
    .bind(ORG_ROLE_OWNER)
    .fetch_one(pool)
    .await?;

    Ok(count.0 as u64)
}

/// Check if user is the last owner of the organization (with optional caching)
pub async fn is_last_owner(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, SqliteError> {
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
) -> Result<LastOwnerResult<()>, SqliteError> {
    let mut tx = pool.begin().await?;

    // Check if member exists and get their role
    let membership = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?",
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
            "SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND role = ?",
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
    sqlx::query("DELETE FROM organization_members WHERE organization_id = ? AND user_id = ?")
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
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    user_id: &str,
    new_role: &str,
) -> Result<LastOwnerResult<MembershipRow>, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;

    // Check if member exists and get their current role
    let membership = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?",
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
            "SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND role = ?",
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
        "UPDATE organization_members SET role = ?, updated_at = ? WHERE organization_id = ? AND user_id = ?",
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
    async fn test_add_member() {
        let pool = setup_test_pool().await;

        // Create a new user first
        sqlx::query("INSERT INTO users (id, created_at, updated_at) VALUES ('user1', 0, 0)")
            .execute(&pool)
            .await
            .unwrap();

        let membership = add_member(&pool, None, "default", "user1", "member")
            .await
            .unwrap();

        assert_eq!(membership.organization_id, "default");
        assert_eq!(membership.user_id, "user1");
        assert_eq!(membership.role, "member");
    }

    #[tokio::test]
    async fn test_add_member_upsert() {
        let pool = setup_test_pool().await;

        // Local user is already an owner of default org
        let membership = add_member(&pool, None, "default", "local", "admin")
            .await
            .unwrap();

        assert_eq!(membership.role, "admin");

        // Verify it was updated
        let fetched = get_membership(&pool, None, "default", "local")
            .await
            .unwrap();
        assert_eq!(fetched.unwrap().role, "admin");
    }

    #[tokio::test]
    async fn test_remove_member() {
        let pool = setup_test_pool().await;

        // Create a new user and add to org
        sqlx::query("INSERT INTO users (id, created_at, updated_at) VALUES ('user1', 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        add_member(&pool, None, "default", "user1", "member")
            .await
            .unwrap();

        let removed = remove_member(&pool, None, "default", "user1")
            .await
            .unwrap();
        assert!(removed);

        let fetched = get_membership(&pool, None, "default", "user1")
            .await
            .unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_update_role() {
        let pool = setup_test_pool().await;

        // Create a new user and add to org
        sqlx::query("INSERT INTO users (id, created_at, updated_at) VALUES ('user1', 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        add_member(&pool, None, "default", "user1", "member")
            .await
            .unwrap();

        let updated = update_role(&pool, None, "default", "user1", "admin")
            .await
            .unwrap();
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().role, "admin");
    }

    #[tokio::test]
    async fn test_list_members() {
        let pool = setup_test_pool().await;

        let (members, total) = list_members(&pool, "default", 1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, "local");
        assert_eq!(members[0].role, "owner");
    }

    #[tokio::test]
    async fn test_has_min_role() {
        let pool = setup_test_pool().await;

        // Local is owner of default
        assert!(
            has_min_role(&pool, None, "default", "local", "viewer")
                .await
                .unwrap()
        );
        assert!(
            has_min_role(&pool, None, "default", "local", "member")
                .await
                .unwrap()
        );
        assert!(
            has_min_role(&pool, None, "default", "local", "admin")
                .await
                .unwrap()
        );
        assert!(
            has_min_role(&pool, None, "default", "local", "owner")
                .await
                .unwrap()
        );

        // Non-member has no role
        assert!(
            !has_min_role(&pool, None, "default", "nonexistent", "viewer")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_count_owners() {
        let pool = setup_test_pool().await;

        let count = count_owners(&pool, "default").await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_is_last_owner() {
        let pool = setup_test_pool().await;

        // Local is the only owner
        assert!(
            is_last_owner(&pool, None, "default", "local")
                .await
                .unwrap()
        );

        // Add another owner
        sqlx::query("INSERT INTO users (id, created_at, updated_at) VALUES ('user1', 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        add_member(&pool, None, "default", "user1", "owner")
            .await
            .unwrap();

        // Now local is not the last owner
        assert!(
            !is_last_owner(&pool, None, "default", "local")
                .await
                .unwrap()
        );
    }

    #[test]
    fn test_has_min_role_level() {
        assert!(has_min_role_level("owner", "owner"));
        assert!(has_min_role_level("owner", "admin"));
        assert!(has_min_role_level("owner", "member"));
        assert!(has_min_role_level("owner", "viewer"));

        assert!(!has_min_role_level("admin", "owner"));
        assert!(has_min_role_level("admin", "admin"));
        assert!(has_min_role_level("admin", "member"));
        assert!(has_min_role_level("admin", "viewer"));

        assert!(!has_min_role_level("member", "admin"));
        assert!(has_min_role_level("member", "member"));

        assert!(!has_min_role_level("viewer", "member"));
        assert!(has_min_role_level("viewer", "viewer"));
    }
}
