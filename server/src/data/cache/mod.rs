//! Cache module
//!
//! Provides caching infrastructure with pluggable backends:
//! - In-memory (default) - uses moka + dashmap
//! - Redis (optional) - uses deadpool-redis
//!
//! ## Security model
//!
//! `CacheService` maintains two independent caches:
//!
//! - **Primary backend** — the configured backend (Redis or in-process memory).
//!   Used for general caching: sessions, org metadata, project lists, etc.
//!
//! - **Process-local cache** — always an in-process `InMemoryCache`, regardless of
//!   the primary backend configuration. Sensitive data (credential secrets, credential
//!   metadata) is routed here via [`CacheService::get_local`] / [`CacheService::set_local`]
//!   so it never leaves the process or touches Redis.
//!
//! Call sites choose which store to use explicitly:
//! - `get` / `set` / `delete` — primary backend (may be Redis)
//! - `get_local` / `set_local` / `delete_local` — always in-process memory
//!
//! Also provides rate limiting using the primary cache backend.

mod backend;
mod error;
mod key;
mod memory;
pub mod rate_limiter;
mod redis;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use backend::CacheBackend;
pub use error::CacheError;
pub use key::CacheKey;
pub use rate_limiter::{RateLimitBucket, RateLimitResult, RateLimiter};

/// Invalidate all caches related to a user's membership in an organization.
///
/// Call this when membership is added, removed, or when an organization is deleted.
/// This ensures auth checks and user lists stay in sync.
pub async fn invalidate_membership_caches(cache: &CacheService, org_id: &str, user_id: &str) {
    cache
        .invalidate_key(&CacheKey::membership(org_id, user_id))
        .await;
    cache
        .invalidate_key(&CacheKey::user_org_member(user_id, org_id))
        .await;
    cache
        .invalidate_key(&CacheKey::orgs_for_user(user_id))
        .await;
    cache
        .invalidate_key(&CacheKey::projects_for_user(user_id))
        .await;
}

use memory::InMemoryCache;

use crate::core::config::{CacheBackendType, CacheConfig};
use crate::core::constants::LOCAL_CACHE_MAX_ENTRIES;

/// Cache service providing typed access to a primary cache backend plus a
/// process-local in-memory cache for sensitive data.
///
/// ## Two-tier design
///
/// ```text
///   caller
///     │
///     ├── get / set / delete          → primary backend (Redis or in-process)
///     │
///     └── get_local / set_local /     → always in-process InMemoryCache
///         delete_local                  (never touches Redis)
/// ```
///
/// Use the `*_local` methods for any data that must not leave the process
/// (credential secrets, credential metadata, session tokens, etc.).
pub struct CacheService {
    /// Primary backend — either Redis or in-process memory per config.
    backend: Arc<dyn CacheBackend>,
    /// Process-local cache — always in-process, never Redis.
    ///
    /// Holds sensitive data that must never be serialized to an external store.
    /// Independent of `backend`; present even when `backend` is also in-process.
    local: InMemoryCache,
}

impl std::fmt::Debug for CacheService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheService")
            .field("backend", &self.backend.backend_name())
            .field("local", &"in-process")
            .finish()
    }
}

impl CacheService {
    /// Create a new cache service from configuration.
    ///
    /// Always creates a process-local `InMemoryCache` in addition to the
    /// configured primary backend.
    pub async fn new(config: &CacheConfig) -> Result<Self, CacheError> {
        let backend: Arc<dyn CacheBackend> = match config.backend {
            CacheBackendType::Memory => {
                tracing::debug!(
                    max_entries = config.max_entries,
                    eviction_policy = ?config.eviction_policy,
                    "Initializing in-memory cache"
                );
                Arc::new(InMemoryCache::new(config))
            }
            CacheBackendType::Redis => {
                let url = config.redis_url.as_ref().ok_or_else(|| {
                    CacheError::Config("redis_url required for Redis backend".into())
                })?;
                // Note: RedisCache::new logs sanitized URL internally
                Arc::new(redis::RedisCache::new(url).await?)
            }
        };

        let local = InMemoryCache::with_capacity(LOCAL_CACHE_MAX_ENTRIES);

        Ok(Self { backend, local })
    }

    /// Get the backend name
    pub fn backend_name(&self) -> &'static str {
        self.backend.backend_name()
    }

    // =========================================================================
    // Raw bytes API
    // =========================================================================

    /// Get raw bytes from cache
    pub async fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.backend.get(key).await
    }

    /// Set raw bytes in cache
    pub async fn set_raw(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        self.backend.set(key, value, ttl).await
    }

    // =========================================================================
    // Typed API (serde)
    // =========================================================================

    /// Get a typed value from cache
    ///
    /// Uses MessagePack for compact, fast deserialization.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.get_raw(key).await? {
            Some(bytes) => {
                let value = rmp_serde::from_slice(&bytes)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Set a typed value in cache
    ///
    /// Uses MessagePack for compact, fast serialization.
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        let bytes =
            rmp_serde::to_vec(value).map_err(|e| CacheError::Serialization(e.to_string()))?;
        self.set_raw(key, bytes, ttl).await
    }

    // =========================================================================
    // Process-local API — always in-process memory, never Redis
    // =========================================================================

    /// Get raw bytes from the process-local cache.
    ///
    /// Use for sensitive data that must never leave the process.
    pub async fn get_local_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.local.get(key).await
    }

    /// Set raw bytes in the process-local cache.
    ///
    /// Use for sensitive data that must never leave the process.
    pub async fn set_local_raw(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        self.local.set(key, value, ttl).await
    }

    /// Get a typed value from the process-local cache (MessagePack).
    ///
    /// Use for sensitive data that must never leave the process.
    pub async fn get_local<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.get_local_raw(key).await? {
            Some(bytes) => {
                let value = rmp_serde::from_slice(&bytes)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Set a typed value in the process-local cache (MessagePack).
    ///
    /// Use for sensitive data that must never leave the process.
    pub async fn set_local<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        let bytes =
            rmp_serde::to_vec(value).map_err(|e| CacheError::Serialization(e.to_string()))?;
        self.set_local_raw(key, bytes, ttl).await
    }

    /// Delete a key from the process-local cache.
    pub async fn delete_local(&self, key: &str) -> Result<bool, CacheError> {
        self.local.delete(key).await
    }

    // =========================================================================
    // Other operations
    // =========================================================================

    /// Delete a key from cache
    pub async fn delete(&self, key: &str) -> Result<bool, CacheError> {
        self.backend.delete(key).await
    }

    /// Delete a key from cache with automatic error logging.
    ///
    /// This is a convenience method for cache invalidation where errors
    /// should be logged but not propagated (cache misses are acceptable).
    pub async fn invalidate_key(&self, key: &str) {
        if let Err(e) = self.backend.delete(key).await {
            tracing::warn!(key = %key, error = %e, "Cache invalidation failed");
        }
    }

    /// Check if a key exists
    pub async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        self.backend.exists(key).await
    }

    /// Invalidate keys matching a pattern
    pub async fn invalidate(&self, pattern: &str) -> Result<u64, CacheError> {
        self.backend.delete_pattern(pattern).await
    }

    /// Atomic increment (for rate limiting)
    pub async fn incr(&self, key: &str, ttl: Option<Duration>) -> Result<i64, CacheError> {
        self.backend.incr(key, ttl).await
    }

    /// Get current counter value without incrementing (for rate limit pre-checks)
    pub async fn get_counter(&self, key: &str) -> Result<Option<i64>, CacheError> {
        self.backend.get_counter(key).await
    }

    /// Get TTL remaining for a key
    pub async fn ttl(&self, key: &str) -> Result<Option<Duration>, CacheError> {
        self.backend.ttl(key).await
    }

    /// Health check
    pub async fn health_check(&self) -> Result<(), CacheError> {
        self.backend.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::EvictionPolicy;

    fn test_config() -> CacheConfig {
        CacheConfig {
            backend: CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        }
    }

    #[tokio::test]
    async fn test_cache_service_backend_name() {
        let service = CacheService::new(&test_config()).await.unwrap();
        assert_eq!(service.backend_name(), "memory");
    }

    #[tokio::test]
    async fn test_typed_get_set() {
        let service = CacheService::new(&test_config()).await.unwrap();

        #[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
        struct User {
            id: String,
            name: String,
        }

        let user = User {
            id: "u1".to_string(),
            name: "Test User".to_string(),
        };

        service.set("user:1", &user, None).await.unwrap();
        let fetched: Option<User> = service.get("user:1").await.unwrap();
        assert_eq!(fetched, Some(user));
    }

    #[tokio::test]
    async fn test_health_check() {
        let service = CacheService::new(&test_config()).await.unwrap();
        assert!(service.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_local_cache_independent_of_primary() {
        let service = CacheService::new(&test_config()).await.unwrap();

        // Write to local, not to primary
        service
            .set_local("secret:key", &"my-api-key".to_string(), None)
            .await
            .unwrap();

        // Should be readable from local
        let v: Option<String> = service.get_local("secret:key").await.unwrap();
        assert_eq!(v.as_deref(), Some("my-api-key"));

        // Should NOT be present in primary backend
        let raw = service.get_raw("secret:key").await.unwrap();
        assert!(
            raw.is_none(),
            "sensitive data must not reach primary backend"
        );
    }

    #[tokio::test]
    async fn test_local_delete() {
        let service = CacheService::new(&test_config()).await.unwrap();

        service.set_local("k", &42u32, None).await.unwrap();

        let deleted = service.delete_local("k").await.unwrap();
        assert!(deleted);

        let v: Option<u32> = service.get_local("k").await.unwrap();
        assert!(v.is_none());
    }

    #[tokio::test]
    async fn test_local_and_primary_namespaces_are_separate() {
        let service = CacheService::new(&test_config()).await.unwrap();

        // Write the same key to both stores with different values
        service
            .set("shared:key", &"primary-value".to_string(), None)
            .await
            .unwrap();
        service
            .set_local("shared:key", &"local-value".to_string(), None)
            .await
            .unwrap();

        let from_primary: Option<String> = service.get("shared:key").await.unwrap();
        let from_local: Option<String> = service.get_local("shared:key").await.unwrap();

        assert_eq!(from_primary.as_deref(), Some("primary-value"));
        assert_eq!(from_local.as_deref(), Some("local-value"));
    }

    #[tokio::test]
    async fn test_invalidate() {
        let service = CacheService::new(&test_config()).await.unwrap();

        service
            .set_raw("user:1", b"a".to_vec(), None)
            .await
            .unwrap();
        service
            .set_raw("user:2", b"b".to_vec(), None)
            .await
            .unwrap();
        service.set_raw("org:1", b"c".to_vec(), None).await.unwrap();

        let deleted = service.invalidate("user:*").await.unwrap();
        assert_eq!(deleted, 2);

        assert!(!service.exists("user:1").await.unwrap());
        assert!(service.exists("org:1").await.unwrap());
    }
}
