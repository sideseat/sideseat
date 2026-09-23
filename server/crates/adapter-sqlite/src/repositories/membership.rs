//! Organization membership repository for SQLite operations.
//!
use sqlx::SqlitePool;

use crate::SqliteError;
use sideseat_core::constants::ORG_ROLE_OWNER;
use sideseat_ports::types::{LastOwnerResult, MemberWithUser, MembershipRow};

/// Add a member to an organization (upsert: updates role if exists)
/// Whether this organization is live - exists and is not being deleted - asked *inside* the caller's
/// transaction.
///
/// Taking it on a pooled connection of its own was a read-then-write: the mutation saw a live
/// organization, the deletion tombstoned it and committed, and the mutation then wrote into something no
/// read can see and the cascade is about to remove - reporting success. Asked on the transaction's own
/// connection (SQLite has one writer, so the transaction that reads then writes fails busy if another wrote in between), the check and the write cannot be separated.
async fn organization_is_live(
    conn: &mut sqlx::SqliteConnection,
    org_id: &str,
) -> Result<bool, SqliteError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT deleting_at FROM organizations WHERE id = ?")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(matches!(row, Some((None,))))
}

pub async fn add_member(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
    role: &str,
    now: i64,
) -> Result<MembershipRow, SqliteError> {
    // The liveness check and the write are one transaction: separately they are a read-then-write that a
    // deletion committing in between defeats.
    let mut tx = pool.begin().await?;
    if !organization_is_live(&mut tx, org_id).await? {
        return Err(SqliteError::Conflict(format!(
            "organization {org_id} does not exist or is being deleted"
        )));
    }

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
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(MembershipRow {
        organization_id: org_id.to_string(),
        user_id: user_id.to_string(),
        role: role.to_string(),
        created_at: now,
        updated_at: now,
    })
}

/// Get a specific membership.
pub async fn get_membership(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<Option<MembershipRow>, SqliteError> {
    get_membership_from_db(pool, org_id, user_id).await
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

/// Remove a member with atomic last-owner protection (transactional)
/// Returns LastOwnerResult to indicate if operation was blocked
pub async fn remove_member_atomic(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
) -> Result<LastOwnerResult<()>, SqliteError> {
    let mut tx = pool.begin().await?;
    // Inside the transaction that writes, not before it: a deletion committing in between would otherwise
    // let this succeed against an organization no read can see.
    if !organization_is_live(&mut tx, org_id).await? {
        return Ok(LastOwnerResult::NotFound);
    }

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

    Ok(LastOwnerResult::Success(()))
}

/// Update a member's role with atomic last-owner protection (transactional)
/// Prevents demoting the last owner
pub async fn update_role_atomic(
    pool: &SqlitePool,
    org_id: &str,
    user_id: &str,
    new_role: &str,
    now: i64,
) -> Result<LastOwnerResult<MembershipRow>, SqliteError> {
    let mut tx = pool.begin().await?;
    // Inside the transaction that writes, not before it: a deletion committing in between would otherwise
    // let this succeed against an organization no read can see.
    if !organization_is_live(&mut tx, org_id).await? {
        return Ok(LastOwnerResult::NotFound);
    }

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

    // Fetch the updated membership.
    get_membership_from_db(pool, org_id, user_id)
        .await
        .map(|opt| opt.map_or(LastOwnerResult::NotFound, LastOwnerResult::Success))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_NOW: i64 = 1_700_000_000;

    async fn add_member(
        pool: &SqlitePool,
        org_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<MembershipRow, SqliteError> {
        super::add_member(pool, org_id, user_id, role, TEST_NOW).await
    }

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(crate::schema::SCHEMA)
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

        let membership = add_member(&pool, "default", "user1", "member")
            .await
            .unwrap();

        assert_eq!(membership.organization_id, "default");
        assert_eq!(membership.user_id, "user1");
        assert_eq!(membership.role, "member");
        assert_eq!(membership.created_at, TEST_NOW);
        assert_eq!(membership.updated_at, TEST_NOW);
    }

    #[tokio::test]
    async fn test_add_member_upsert() {
        let pool = setup_test_pool().await;

        // Local user is already an owner of default org
        let membership = add_member(&pool, "default", "local", "admin")
            .await
            .unwrap();

        assert_eq!(membership.role, "admin");

        // Verify it was updated
        let fetched = get_membership(&pool, "default", "local").await.unwrap();
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
        add_member(&pool, "default", "user1", "member")
            .await
            .unwrap();

        let removed = remove_member_atomic(&pool, "default", "user1")
            .await
            .unwrap();
        assert!(matches!(removed, LastOwnerResult::Success(())));

        let fetched = get_membership(&pool, "default", "user1").await.unwrap();
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
        add_member(&pool, "default", "user1", "member")
            .await
            .unwrap();

        let updated = update_role_atomic(&pool, "default", "user1", "admin", TEST_NOW + 1)
            .await
            .unwrap();
        let LastOwnerResult::Success(updated) = updated else {
            panic!("expected role update to succeed");
        };
        assert_eq!(updated.role, "admin");
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
}
