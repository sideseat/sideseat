//! API key repository for SQLite operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::core::constants::{
    API_KEY_MAX_PER_ORG, CACHE_TTL_API_KEY_INVALID, CACHE_TTL_API_KEY_VALID,
};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::sqlite::SqliteError;
use crate::data::types::{ApiKeyRow, ApiKeyScope, ApiKeyValidation};

/// Create a new API key
#[allow(clippy::too_many_arguments)]
pub async fn create_api_key(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    scope: ApiKeyScope,
    created_by: &str,
    expires_at: Option<i64>,
) -> Result<ApiKeyRow, SqliteError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    // Use atomic INSERT with subquery to prevent TOCTOU race condition.
    // SQLite's default DEFERRED transactions don't prevent concurrent reads
    // before the first write, so a count-then-insert pattern has a race.
    // This single statement is atomic - it either inserts (if under limit) or not.
    let result = sqlx::query(
        r#"INSERT INTO api_keys (id, org_id, name, key_hash, key_prefix, scope, created_by, expires_at, created_at)
           SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
           WHERE (SELECT COUNT(*) FROM api_keys WHERE org_id = ?2) < ?10"#,
    )
    .bind(&id)
    .bind(org_id)
    .bind(name)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(scope.as_str())
    .bind(created_by)
    .bind(expires_at)
    .bind(now)
    .bind(API_KEY_MAX_PER_ORG as i64)
    .execute(pool)
    .await?;

    // If no rows were inserted, the limit was reached
    if result.rows_affected() == 0 {
        return Err(SqliteError::Conflict(format!(
            "Maximum {} API keys per organization reached",
            API_KEY_MAX_PER_ORG
        )));
    }

    // Invalidate list cache
    if let Some(cache) = cache
        && let Err(e) = cache.delete(&CacheKey::api_keys_for_org(org_id)).await
    {
        tracing::warn!(org_id, error = %e, "Cache invalidation error");
    }

    Ok(ApiKeyRow {
        id,
        org_id: org_id.to_string(),
        name: name.to_string(),
        key_prefix: key_prefix.to_string(),
        scope,
        created_by: Some(created_by.to_string()),
        last_used_at: None,
        expires_at,
        created_at: now,
    })
}

/// Get API key validation info by hash (with optional caching)
pub async fn get_by_hash(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    key_hash: &str,
) -> Result<Option<ApiKeyValidation>, SqliteError> {
    if let Some(cache) = cache {
        let key = CacheKey::api_key_by_hash(key_hash);
        let neg_key = CacheKey::api_key_negative(key_hash);

        // Try positive cache first
        match cache.get::<ApiKeyValidation>(&key).await {
            Ok(Some(validation)) => {
                tracing::trace!("API key cache hit");
                return Ok(Some(validation));
            }
            Err(e) => tracing::warn!(error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Check negative cache (known not-found)
        if cache.exists(&neg_key).await.unwrap_or(false) {
            tracing::trace!("API key negative cache hit");
            return Ok(None);
        }

        // Cache miss - query DB
        let result = get_by_hash_from_db(pool, key_hash).await?;

        // Store result in cache
        match &result {
            Some(v) => {
                if let Err(e) = cache
                    .set(&key, v, Some(Duration::from_secs(CACHE_TTL_API_KEY_VALID)))
                    .await
                {
                    tracing::warn!(error = %e, "Cache set error");
                }
            }
            None => {
                if let Err(e) = cache
                    .set_raw(
                        &neg_key,
                        vec![],
                        Some(Duration::from_secs(CACHE_TTL_API_KEY_INVALID)),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "Cache set (negative) error");
                }
            }
        }

        Ok(result)
    } else {
        get_by_hash_from_db(pool, key_hash).await
    }
}

/// Get API key validation info by hash directly from database (no caching)
async fn get_by_hash_from_db(
    pool: &SqlitePool,
    key_hash: &str,
) -> Result<Option<ApiKeyValidation>, SqliteError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
        ),
    >(
        r#"SELECT id, org_id, scope, created_by, expires_at, last_used_at
           FROM api_keys WHERE key_hash = ?"#,
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(key_id, org_id, scope_str, created_by, expires_at, last_used_at)| ApiKeyValidation {
            key_id,
            org_id,
            scope: ApiKeyScope::parse(&scope_str).unwrap_or(ApiKeyScope::Full),
            created_by,
            expires_at,
            last_used_at,
        },
    ))
}

/// List all API keys for an organization (ordered by created_at DESC)
pub async fn list_for_org(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<Vec<ApiKeyRow>, SqliteError> {
    if let Some(cache) = cache {
        let key = CacheKey::api_keys_for_org(org_id);

        // Try cache first
        match cache.get::<Vec<ApiKeyRow>>(&key).await {
            Ok(Some(keys)) => {
                tracing::trace!(org_id, "API keys list cache hit");
                return Ok(keys);
            }
            Err(e) => tracing::warn!(org_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = list_for_org_from_db(pool, org_id).await?;

        // Store in cache
        if let Err(e) = cache
            .set(
                &key,
                &result,
                Some(Duration::from_secs(CACHE_TTL_API_KEY_VALID)),
            )
            .await
        {
            tracing::warn!(org_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        list_for_org_from_db(pool, org_id).await
    }
}

/// List all API keys for an organization directly from database (no caching)
async fn list_for_org_from_db(
    pool: &SqlitePool,
    org_id: &str,
) -> Result<Vec<ApiKeyRow>, SqliteError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            i64,
        ),
    >(
        r#"SELECT id, org_id, name, key_prefix, scope, created_by, last_used_at, expires_at, created_at
           FROM api_keys WHERE org_id = ? ORDER BY created_at DESC, id DESC"#,
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                org_id,
                name,
                key_prefix,
                scope_str,
                created_by,
                last_used_at,
                expires_at,
                created_at,
            )| {
                ApiKeyRow {
                    id,
                    org_id,
                    name,
                    key_prefix,
                    scope: ApiKeyScope::parse(&scope_str).unwrap_or(ApiKeyScope::Full),
                    created_by,
                    last_used_at,
                    expires_at,
                    created_at,
                }
            },
        )
        .collect())
}

/// Delete an API key by ID
pub async fn delete_api_key(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    id: &str,
    org_id: &str,
) -> Result<bool, SqliteError> {
    // First get the key hash for cache invalidation
    let key_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM api_keys WHERE id = ? AND org_id = ?")
            .bind(id)
            .bind(org_id)
            .fetch_optional(pool)
            .await?;

    let result = sqlx::query("DELETE FROM api_keys WHERE id = ? AND org_id = ?")
        .bind(id)
        .bind(org_id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate caches
    if deleted && let Some(cache) = cache {
        // Invalidate the hash lookup cache
        if let Some((hash,)) = key_hash {
            if let Err(e) = cache.delete(&CacheKey::api_key_by_hash(&hash)).await {
                tracing::warn!(id, error = %e, "Cache invalidation error");
            }
            if let Err(e) = cache.delete(&CacheKey::api_key_negative(&hash)).await {
                tracing::warn!(id, error = %e, "Cache invalidation error");
            }
        }
        // Invalidate the list cache
        if let Err(e) = cache.delete(&CacheKey::api_keys_for_org(org_id)).await {
            tracing::warn!(org_id, error = %e, "Cache invalidation error");
        }
    }

    Ok(deleted)
}

/// Update last_used_at (debounced, only if older than threshold)
pub async fn touch_api_key(
    pool: &SqlitePool,
    id: &str,
    threshold_secs: u64,
) -> Result<bool, SqliteError> {
    let now = chrono::Utc::now().timestamp();
    let threshold = now - threshold_secs as i64;

    let result = sqlx::query(
        "UPDATE api_keys SET last_used_at = ? WHERE id = ? AND (last_used_at < ? OR last_used_at IS NULL)",
    )
    .bind(now)
    .bind(id)
    .bind(threshold)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all API keys for an organization
pub async fn delete_for_org(
    pool: &SqlitePool,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<u64, SqliteError> {
    // First get all key hashes for cache invalidation
    let hashes = get_hashes_for_org(pool, org_id).await?;

    let result = sqlx::query("DELETE FROM api_keys WHERE org_id = ?")
        .bind(org_id)
        .execute(pool)
        .await?;

    // Invalidate caches (both positive and negative)
    if let Some(cache) = cache {
        for hash in hashes {
            if let Err(e) = cache.delete(&CacheKey::api_key_by_hash(&hash)).await {
                tracing::warn!(org_id, error = %e, "Cache invalidation error");
            }
            if let Err(e) = cache.delete(&CacheKey::api_key_negative(&hash)).await {
                tracing::warn!(org_id, error = %e, "Cache invalidation error");
            }
        }
        if let Err(e) = cache.delete(&CacheKey::api_keys_for_org(org_id)).await {
            tracing::warn!(org_id, error = %e, "Cache invalidation error");
        }
    }

    Ok(result.rows_affected())
}

/// Get all key hashes for an organization (for cache invalidation)
pub async fn get_hashes_for_org(
    pool: &SqlitePool,
    org_id: &str,
) -> Result<Vec<String>, SqliteError> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT key_hash FROM api_keys WHERE org_id = ?")
        .bind(org_id)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|(hash,)| hash).collect())
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
    async fn test_create_api_key() {
        let pool = setup_test_pool().await;

        let key = create_api_key(
            &pool,
            None,
            "default",
            "Test Key",
            "hash123",
            "pk-ss-abc",
            ApiKeyScope::Full,
            "local",
            None,
        )
        .await
        .unwrap();

        assert!(!key.id.is_empty());
        assert_eq!(key.org_id, "default");
        assert_eq!(key.name, "Test Key");
        assert_eq!(key.key_prefix, "pk-ss-abc");
        assert_eq!(key.scope, ApiKeyScope::Full);
        assert_eq!(key.created_by, Some("local".to_string()));
    }

    #[tokio::test]
    async fn test_get_by_hash() {
        let pool = setup_test_pool().await;

        // Create a key
        create_api_key(
            &pool,
            None,
            "default",
            "Test Key",
            "hash123",
            "pk-ss-abc",
            ApiKeyScope::Read,
            "local",
            None,
        )
        .await
        .unwrap();

        // Get by hash
        let validation = get_by_hash(&pool, None, "hash123").await.unwrap();
        assert!(validation.is_some());
        let v = validation.unwrap();
        assert_eq!(v.org_id, "default");
        assert_eq!(v.scope, ApiKeyScope::Read);

        // Get non-existent
        let validation = get_by_hash(&pool, None, "nonexistent").await.unwrap();
        assert!(validation.is_none());
    }

    #[tokio::test]
    async fn test_list_for_org() {
        let pool = setup_test_pool().await;

        // Create two keys
        create_api_key(
            &pool,
            None,
            "default",
            "Key 1",
            "hash1",
            "pk-ss-abc",
            ApiKeyScope::Full,
            "local",
            None,
        )
        .await
        .unwrap();

        create_api_key(
            &pool,
            None,
            "default",
            "Key 2",
            "hash2",
            "pk-ss-def",
            ApiKeyScope::Read,
            "local",
            None,
        )
        .await
        .unwrap();

        let keys = list_for_org(&pool, None, "default").await.unwrap();
        assert_eq!(keys.len(), 2);
        // Verify both keys exist (order may vary for same-second timestamps)
        let names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
        assert!(names.contains(&"Key 1"));
        assert!(names.contains(&"Key 2"));
    }

    #[tokio::test]
    async fn test_delete_api_key() {
        let pool = setup_test_pool().await;

        let key = create_api_key(
            &pool,
            None,
            "default",
            "Test Key",
            "hash123",
            "pk-ss-abc",
            ApiKeyScope::Full,
            "local",
            None,
        )
        .await
        .unwrap();

        let deleted = delete_api_key(&pool, None, &key.id, "default")
            .await
            .unwrap();
        assert!(deleted);

        // Verify it's gone
        let validation = get_by_hash(&pool, None, "hash123").await.unwrap();
        assert!(validation.is_none());
    }

    #[tokio::test]
    async fn test_touch_api_key() {
        let pool = setup_test_pool().await;

        let key = create_api_key(
            &pool,
            None,
            "default",
            "Test Key",
            "hash123",
            "pk-ss-abc",
            ApiKeyScope::Full,
            "local",
            None,
        )
        .await
        .unwrap();

        // Touch should update
        let touched = touch_api_key(&pool, &key.id, 300).await.unwrap();
        assert!(touched);

        // Touching again immediately should not update (debounced)
        let touched = touch_api_key(&pool, &key.id, 300).await.unwrap();
        assert!(!touched);
    }

    #[tokio::test]
    async fn test_key_limit() {
        let pool = setup_test_pool().await;

        // Create max keys
        for i in 0..API_KEY_MAX_PER_ORG {
            create_api_key(
                &pool,
                None,
                "default",
                &format!("Key {}", i),
                &format!("hash{}", i),
                "pk-ss-abc",
                ApiKeyScope::Full,
                "local",
                None,
            )
            .await
            .unwrap();
        }

        // Next one should fail
        let result = create_api_key(
            &pool,
            None,
            "default",
            "One Too Many",
            "hash_too_many",
            "pk-ss-abc",
            ApiKeyScope::Full,
            "local",
            None,
        )
        .await;

        assert!(matches!(result, Err(SqliteError::Conflict(_))));
    }
}
