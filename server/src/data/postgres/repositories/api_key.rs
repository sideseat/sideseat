//! API key repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::constants::{
    API_KEY_MAX_PER_ORG, CACHE_TTL_API_KEY_INVALID, CACHE_TTL_API_KEY_VALID,
};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::postgres::PostgresError;
use crate::data::types::{ApiKeyRow, ApiKeyScope, ApiKeyValidation};

/// Create a new API key
#[allow(clippy::too_many_arguments)]
pub async fn create_api_key(
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    scope: ApiKeyScope,
    created_by: &str,
    expires_at: Option<i64>,
) -> Result<ApiKeyRow, PostgresError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    // Use transaction with row lock for atomicity
    let mut tx = pool.begin().await?;

    // PostgreSQL: Lock the organization row, then count keys
    sqlx::query("SELECT 1 FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await?;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await?;

    if count.0 >= API_KEY_MAX_PER_ORG as i64 {
        return Err(PostgresError::Conflict(format!(
            "Maximum {} API keys per organization reached",
            API_KEY_MAX_PER_ORG
        )));
    }

    sqlx::query(
        r#"INSERT INTO api_keys (id, org_id, name, key_hash, key_prefix, scope, created_by, expires_at, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
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
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

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
    pool: &PgPool,
    cache: Option<&CacheService>,
    key_hash: &str,
) -> Result<Option<ApiKeyValidation>, PostgresError> {
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
    pool: &PgPool,
    key_hash: &str,
) -> Result<Option<ApiKeyValidation>, PostgresError> {
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
           FROM api_keys WHERE key_hash = $1"#,
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<Vec<ApiKeyRow>, PostgresError> {
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
    pool: &PgPool,
    org_id: &str,
) -> Result<Vec<ApiKeyRow>, PostgresError> {
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
           FROM api_keys WHERE org_id = $1 ORDER BY created_at DESC, id DESC"#,
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
    org_id: &str,
) -> Result<bool, PostgresError> {
    // First get the key hash for cache invalidation
    let key_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM api_keys WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(org_id)
            .fetch_optional(pool)
            .await?;

    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND org_id = $2")
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
    pool: &PgPool,
    id: &str,
    threshold_secs: u64,
) -> Result<bool, PostgresError> {
    let now = chrono::Utc::now().timestamp();
    let threshold = now - threshold_secs as i64;

    let result = sqlx::query(
        "UPDATE api_keys SET last_used_at = $1 WHERE id = $2 AND (last_used_at < $3 OR last_used_at IS NULL)",
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
    pool: &PgPool,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<u64, PostgresError> {
    // First get all key hashes for cache invalidation
    let hashes = get_hashes_for_org(pool, org_id).await?;

    let result = sqlx::query("DELETE FROM api_keys WHERE org_id = $1")
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
pub async fn get_hashes_for_org(pool: &PgPool, org_id: &str) -> Result<Vec<String>, PostgresError> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT key_hash FROM api_keys WHERE org_id = $1")
        .bind(org_id)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|(hash,)| hash).collect())
}
