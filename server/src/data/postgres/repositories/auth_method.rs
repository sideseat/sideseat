//! Auth method repository for PostgreSQL operations
//!
//! All read operations support optional caching. Pass `Some(cache)` to enable caching,
//! or `None` to bypass cache. Mutations automatically invalidate relevant cache keys.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::constants::{AUTH_METHOD_BOOTSTRAP, AUTH_METHOD_OAUTH, CACHE_TTL_AUTH_METHOD};
use crate::data::cache::{CacheKey, CacheService};
use crate::data::postgres::PostgresError;
use crate::data::types::AuthMethodRow;

/// Create a new auth method with a generated CUID2 ID
#[allow(clippy::too_many_arguments)]
pub async fn create_auth_method(
    pool: &PgPool,
    cache: Option<&CacheService>,
    user_id: &str,
    method_type: &str,
    provider: Option<&str>,
    provider_id: Option<&str>,
    credential_hash: Option<&str>,
    metadata: Option<&str>,
) -> Result<AuthMethodRow, PostgresError> {
    let id = cuid2::create_id();
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT INTO auth_methods (id, user_id, method_type, provider, provider_id, credential_hash, metadata, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(&id)
    .bind(user_id)
    .bind(method_type)
    .bind(provider)
    .bind(provider_id)
    .bind(credential_hash)
    .bind(metadata)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    // Invalidate cache entries AFTER successful insert
    if let Some(cache) = cache {
        // Invalidate user's auth methods list cache
        if let Err(e) = cache
            .delete(&CacheKey::auth_methods_for_user(user_id))
            .await
        {
            tracing::warn!(%user_id, error = %e, "Cache invalidation error");
        }
        // Invalidate OAuth lookup if this is an OAuth method
        if let (Some(prov), Some(prov_id)) = (provider, provider_id)
            && let Err(e) = cache.delete(&CacheKey::auth_oauth(prov, prov_id)).await
        {
            tracing::warn!(%prov, %prov_id, error = %e, "Cache invalidation error");
        }
    }

    Ok(AuthMethodRow {
        id,
        user_id: user_id.to_string(),
        method_type: method_type.to_string(),
        provider: provider.map(String::from),
        provider_id: provider_id.map(String::from),
        credential_hash: credential_hash.map(String::from),
        metadata: metadata.map(String::from),
        created_at: now,
        updated_at: now,
    })
}

/// Get an auth method by ID
pub async fn get_auth_method(
    pool: &PgPool,
    id: &str,
) -> Result<Option<AuthMethodRow>, PostgresError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"
        SELECT id, user_id, method_type, provider, provider_id, credential_hash, metadata, created_at, updated_at
        FROM auth_methods
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            user_id,
            method_type,
            provider,
            provider_id,
            credential_hash,
            metadata,
            created_at,
            updated_at,
        )| {
            AuthMethodRow {
                id,
                user_id,
                method_type,
                provider,
                provider_id,
                credential_hash,
                metadata,
                created_at,
                updated_at,
            }
        },
    ))
}

/// Find an auth method by OAuth provider and provider ID (with optional caching)
pub async fn find_by_oauth(
    pool: &PgPool,
    cache: Option<&CacheService>,
    provider: &str,
    provider_id: &str,
) -> Result<Option<AuthMethodRow>, PostgresError> {
    if let Some(cache) = cache {
        let key = CacheKey::auth_oauth(provider, provider_id);

        // Try cache first
        match cache.get::<AuthMethodRow>(&key).await {
            Ok(Some(method)) => {
                tracing::trace!(%provider, %provider_id, "OAuth auth method cache hit");
                return Ok(Some(method));
            }
            Err(e) => tracing::warn!(%provider, %provider_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = find_by_oauth_from_db(pool, provider, provider_id).await?;

        // Store result in cache
        if let Some(ref method) = result
            && let Err(e) = cache
                .set(
                    &key,
                    method,
                    Some(Duration::from_secs(CACHE_TTL_AUTH_METHOD)),
                )
                .await
        {
            tracing::warn!(%provider, %provider_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        find_by_oauth_from_db(pool, provider, provider_id).await
    }
}

/// Find an auth method by OAuth provider and provider ID directly from database (no caching)
async fn find_by_oauth_from_db(
    pool: &PgPool,
    provider: &str,
    provider_id: &str,
) -> Result<Option<AuthMethodRow>, PostgresError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"
        SELECT id, user_id, method_type, provider, provider_id, credential_hash, metadata, created_at, updated_at
        FROM auth_methods
        WHERE method_type = $1 AND provider = $2 AND provider_id = $3
        "#,
    )
    .bind(AUTH_METHOD_OAUTH)
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            user_id,
            method_type,
            provider,
            provider_id,
            credential_hash,
            metadata,
            created_at,
            updated_at,
        )| {
            AuthMethodRow {
                id,
                user_id,
                method_type,
                provider,
                provider_id,
                credential_hash,
                metadata,
                created_at,
                updated_at,
            }
        },
    ))
}

/// List all auth methods for a user (with optional caching)
pub async fn list_for_user(
    pool: &PgPool,
    cache: Option<&CacheService>,
    user_id: &str,
) -> Result<Vec<AuthMethodRow>, PostgresError> {
    if let Some(cache) = cache {
        let key = CacheKey::auth_methods_for_user(user_id);

        // Try cache first
        match cache.get::<Vec<AuthMethodRow>>(&key).await {
            Ok(Some(methods)) => {
                tracing::trace!(%user_id, "Auth methods for user cache hit");
                return Ok(methods);
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "Cache get error"),
            Ok(None) => {}
        }

        // Cache miss - query DB
        let result = list_for_user_from_db(pool, user_id).await?;

        // Store result in cache
        if let Err(e) = cache
            .set(
                &key,
                &result,
                Some(Duration::from_secs(CACHE_TTL_AUTH_METHOD)),
            )
            .await
        {
            tracing::warn!(%user_id, error = %e, "Cache set error");
        }

        Ok(result)
    } else {
        list_for_user_from_db(pool, user_id).await
    }
}

/// List all auth methods for a user directly from database (no caching)
async fn list_for_user_from_db(
    pool: &PgPool,
    user_id: &str,
) -> Result<Vec<AuthMethodRow>, PostgresError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"
        SELECT id, user_id, method_type, provider, provider_id, credential_hash, metadata, created_at, updated_at
        FROM auth_methods
        WHERE user_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                user_id,
                method_type,
                provider,
                provider_id,
                credential_hash,
                metadata,
                created_at,
                updated_at,
            )| {
                AuthMethodRow {
                    id,
                    user_id,
                    method_type,
                    provider,
                    provider_id,
                    credential_hash,
                    metadata,
                    created_at,
                    updated_at,
                }
            },
        )
        .collect())
}

/// Delete an auth method by ID
pub async fn delete_auth_method(
    pool: &PgPool,
    cache: Option<&CacheService>,
    id: &str,
) -> Result<bool, PostgresError> {
    // Get old method for cache invalidation
    let old_method = get_auth_method(pool, id).await?;

    let result = sqlx::query("DELETE FROM auth_methods WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;

    let deleted = result.rows_affected() > 0;

    // Invalidate cache entries AFTER successful delete
    if deleted
        && let Some(cache) = cache
        && let Some(ref old) = old_method
    {
        // Invalidate user's auth methods list cache
        if let Err(e) = cache
            .delete(&CacheKey::auth_methods_for_user(&old.user_id))
            .await
        {
            tracing::warn!(user_id = %old.user_id, error = %e, "Cache invalidation error");
        }
        // Invalidate OAuth lookup if this was an OAuth method
        if let (Some(prov), Some(prov_id)) = (&old.provider, &old.provider_id)
            && let Err(e) = cache.delete(&CacheKey::auth_oauth(prov, prov_id)).await
        {
            tracing::warn!(%prov, %prov_id, error = %e, "Cache invalidation error");
        }
    }

    Ok(deleted)
}

/// Get the bootstrap auth method for a user (if any)
pub async fn get_bootstrap_method(
    pool: &PgPool,
    user_id: &str,
) -> Result<Option<AuthMethodRow>, PostgresError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    >(
        r#"
        SELECT id, user_id, method_type, provider, provider_id, credential_hash, metadata, created_at, updated_at
        FROM auth_methods
        WHERE user_id = $1 AND method_type = $2
        "#,
    )
    .bind(user_id)
    .bind(AUTH_METHOD_BOOTSTRAP)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            user_id,
            method_type,
            provider,
            provider_id,
            credential_hash,
            metadata,
            created_at,
            updated_at,
        )| {
            AuthMethodRow {
                id,
                user_id,
                method_type,
                provider,
                provider_id,
                credential_hash,
                metadata,
                created_at,
                updated_at,
            }
        },
    ))
}
