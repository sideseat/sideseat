//! User repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::core::constants::{CACHE_TTL_NEGATIVE, CACHE_TTL_USER, DEFAULT_USER_ID};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::sqlite::SqliteError;
use crate::data::types::UserRow;

/// Create a new user with a generated CUID2 ID
pub async fn create_user(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    email: Option<&str>,
    display_name: Option<&str>,
) -> Result<UserRow, SqliteError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO users (id, email, display_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(email)
    .bind(display_name)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    // Invalidate negative caches for new user
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::user_negative(&id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }
        // Invalidate email negative cache if email was provided
        if let Some(email) = email
            && let Err(e) = cache.delete(&CacheKey::user_by_email_negative(email)).await
        {
            tracing::warn!(%email, error = %e, "Cache invalidation error");
        }
    }

    Ok(UserRow {
        id,
        email: email.map(String::from),
        display_name: display_name.map(String::from),
        created_at: now,
        updated_at: now,
    })
}

/// Get a user by ID (with optional caching)
pub async fn get_user(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<Option<UserRow>, SqliteError> {
    if let Some(cache) = cache {
        let key = CacheKey::user(id);
        let neg_key = CacheKey::user_negative(id);

        // Try cache first
        match cache.get::<UserRow>(&key).await {
            Ok(Some(user)) => {
                tracing::trace!(%id, "User cache hit");
                return Ok(Some(user));
            }
            Err(e) => tracing::warn!(%id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Check negative cache (known not-found)
        if cache.exists(&neg_key).await.unwrap_or(false) {
            tracing::trace!(%id, "User negative cache hit");
            return Ok(None);
        }

        // Cache miss - query DB
        let result = get_user_from_db(pool, id).await?;

        // Store result in cache
        match &result {
            Some(u) => {
                if let Err(e) = cache
                    .set(&key, u, Some(Duration::from_secs(CACHE_TTL_USER)))
                    .await
                {
                    tracing::warn!(%id, error = %e, "Cache set error");
                }
            }
            None => {
                if let Err(e) = cache
                    .set_raw(
                        &neg_key,
                        vec![],
                        Some(Duration::from_secs(CACHE_TTL_NEGATIVE)),
                    )
                    .await
                {
                    tracing::warn!(%id, error = %e, "Cache set (negative) error");
                }
            }
        }

        Ok(result)
    } else {
        get_user_from_db(pool, id).await
    }
}

/// Get a user by ID directly from database (no caching)
async fn get_user_from_db(pool: &SqlitePool, id: &str) -> Result<Option<UserRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, i64, i64)>(
        "SELECT id, email, display_name, created_at, updated_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, email, display_name, created_at, updated_at)| UserRow {
            id,
            email,
            display_name,
            created_at,
            updated_at,
        },
    ))
}

/// Get a user by email (with optional caching)
pub async fn get_by_email(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    email: &str,
) -> Result<Option<UserRow>, SqliteError> {
    if let Some(cache) = cache {
        let key = CacheKey::user_by_email(email);
        let neg_key = CacheKey::user_by_email_negative(email);

        // Try cache first
        match cache.get::<UserRow>(&key).await {
            Ok(Some(user)) => {
                tracing::trace!(%email, "User by email cache hit");
                return Ok(Some(user));
            }
            Err(e) => tracing::warn!(%email, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Check negative cache (known not-found)
        if cache.exists(&neg_key).await.unwrap_or(false) {
            tracing::trace!(%email, "User by email negative cache hit");
            return Ok(None);
        }

        // Cache miss - query DB
        let result = get_by_email_from_db(pool, email).await?;

        // Store result in cache
        match &result {
            Some(u) => {
                if let Err(e) = cache
                    .set(&key, u, Some(Duration::from_secs(CACHE_TTL_USER)))
                    .await
                {
                    tracing::warn!(%email, error = %e, "Cache set error");
                }
            }
            None => {
                // Negative cache with shorter TTL
                if let Err(e) = cache
                    .set_raw(
                        &neg_key,
                        vec![],
                        Some(Duration::from_secs(CACHE_TTL_NEGATIVE)),
                    )
                    .await
                {
                    tracing::warn!(%email, error = %e, "Cache set (negative) error");
                }
            }
        }

        Ok(result)
    } else {
        get_by_email_from_db(pool, email).await
    }
}

/// Get a user by email directly from database (no caching)
async fn get_by_email_from_db(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<UserRow>, SqliteError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, i64, i64)>(
        "SELECT id, email, display_name, created_at, updated_at FROM users WHERE email = ?",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, email, display_name, created_at, updated_at)| UserRow {
            id,
            email,
            display_name,
            created_at,
            updated_at,
        },
    ))
}

/// Update a user's display name
pub async fn update_user(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
    display_name: Option<&str>,
) -> Result<Option<UserRow>, SqliteError> {
    // Get old user for email invalidation
    let old_user = get_user_from_db(pool, id).await?;

    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query("UPDATE users SET display_name = ?, updated_at = ? WHERE id = ?")
        .bind(display_name)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Ok(None);
    }

    // Invalidate cache entries AFTER successful write
    if let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::user(id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }

        // Invalidate email lookup if user had email
        if let Some(ref old) = old_user
            && let Some(ref email) = old.email
            && let Err(e) = cache.delete(&CacheKey::user_by_email(email)).await
        {
            tracing::warn!(%email, error = %e, "Cache invalidation error");
        }
    }

    get_user_from_db(pool, id).await
}

/// Delete a user by ID
pub async fn delete_user(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, SqliteError> {
    // Get old user for email invalidation
    let old_user = get_user_from_db(pool, id).await?;

    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate cache entries AFTER successful write
    if deleted && let Some(cache) = cache {
        if let Err(e) = cache.delete(&CacheKey::user(id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }
        if let Err(e) = cache.delete(&CacheKey::user_negative(id)).await {
            tracing::warn!(%id, error = %e, "Cache invalidation error");
        }

        // Invalidate email lookup if user had email
        if let Some(ref old) = old_user
            && let Some(ref email) = old.email
            && let Err(e) = cache.delete(&CacheKey::user_by_email(email)).await
        {
            tracing::warn!(%email, error = %e, "Cache invalidation error");
        }
    }

    Ok(deleted)
}

/// Check if user is the last owner of any organization
pub async fn is_last_owner_of_any_org(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<bool, SqliteError> {
    // Find orgs where this user is the only owner
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM organization_members om1
        WHERE om1.user_id = ? AND om1.role = 'owner'
        AND NOT EXISTS (
            SELECT 1 FROM organization_members om2
            WHERE om2.organization_id = om1.organization_id
            AND om2.role = 'owner'
            AND om2.user_id != ?
        )
        "#,
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    Ok(count.0 > 0)
}

/// Check if user is the default user (cannot be deleted)
pub fn is_default_user(id: &str) -> bool {
    id == DEFAULT_USER_ID
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
    async fn test_create_user() {
        let pool = setup_test_pool().await;
        let user = create_user(&pool, None, Some("test@example.com"), Some("Test User"))
            .await
            .unwrap();

        assert!(!user.id.is_empty());
        assert_eq!(user.email, Some("test@example.com".to_string()));
        assert_eq!(user.display_name, Some("Test User".to_string()));
    }

    #[tokio::test]
    async fn test_create_user_minimal() {
        let pool = setup_test_pool().await;
        let user = create_user(&pool, None, None, None).await.unwrap();

        assert!(!user.id.is_empty());
        assert!(user.email.is_none());
        assert!(user.display_name.is_none());
    }

    #[tokio::test]
    async fn test_get_user() {
        let pool = setup_test_pool().await;
        let created = create_user(&pool, None, Some("test@example.com"), Some("Test User"))
            .await
            .unwrap();

        let fetched = get_user(&pool, None, &created.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.email, Some("test@example.com".to_string()));
    }

    #[tokio::test]
    async fn test_get_by_email() {
        let pool = setup_test_pool().await;
        create_user(&pool, None, Some("test@example.com"), Some("Test User"))
            .await
            .unwrap();

        let fetched = get_by_email(&pool, None, "test@example.com").await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().display_name, Some("Test User".to_string()));
    }

    #[tokio::test]
    async fn test_update_user() {
        let pool = setup_test_pool().await;
        let user = create_user(&pool, None, Some("test@example.com"), Some("Test User"))
            .await
            .unwrap();

        let updated = update_user(&pool, None, &user.id, Some("Updated Name"))
            .await
            .unwrap();
        assert!(updated.is_some());
        assert_eq!(
            updated.unwrap().display_name,
            Some("Updated Name".to_string())
        );
    }

    #[tokio::test]
    async fn test_delete_user() {
        let pool = setup_test_pool().await;
        let user = create_user(&pool, None, None, None).await.unwrap();

        let deleted = delete_user(&pool, None, &user.id).await.unwrap();
        assert!(deleted);

        let fetched = get_user(&pool, None, &user.id).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_default_user_exists() {
        let pool = setup_test_pool().await;
        let user = get_user(&pool, None, "local").await.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().display_name, Some("Local User".to_string()));
    }

    #[tokio::test]
    async fn test_is_last_owner_of_any_org() {
        let pool = setup_test_pool().await;

        // Local user is owner of default org and only owner
        let is_last = is_last_owner_of_any_org(&pool, "local").await.unwrap();
        assert!(is_last);

        // Non-existent user is not owner of anything
        let is_last = is_last_owner_of_any_org(&pool, "nonexistent")
            .await
            .unwrap();
        assert!(!is_last);
    }

    #[tokio::test]
    async fn test_is_default_user() {
        assert!(is_default_user("local"));
        assert!(!is_default_user("other"));
    }
}
