//! Cache module
//!
//! Provides caching infrastructure with pluggable backends:
//! - In-memory (default) - uses moka + dashmap
//! - Redis (optional) - uses deadpool-redis
//!
//! Also provides rate limiting using the cache backend.

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

/// Cache service providing typed access to cache backend
///
/// Wraps the underlying cache backend and provides:
/// - Raw bytes API for flexibility
/// - Typed API using MessagePack serialization
pub struct CacheService {
    backend: Arc<dyn CacheBackend>,
}

impl std::fmt::Debug for CacheService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheService")
            .field("backend", &self.backend.backend_name())
            .finish()
    }
}

impl CacheService {
    /// Create a new cache service from configuration
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

        Ok(Self { backend })
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
