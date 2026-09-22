//! Cache backend trait definition

use std::time::Duration;

use async_trait::async_trait;

use super::error::CacheError;

/// Cache backend trait
///
/// Defines the interface for cache implementations.
/// Both in-memory and Redis backends implement this trait.
///
/// # Consistency Notes
///
/// Operations on individual keys are atomic, but the return values of some
/// operations (like `delete` and `exists`) may be stale in concurrent scenarios.
/// This is acceptable for cache use cases where eventual consistency is sufficient.
#[async_trait]
pub trait CacheBackend: Send + Sync {
    /// Get a value from the cache
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError>;

    /// Set a value in the cache with optional TTL
    async fn set(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>)
    -> Result<(), CacheError>;

    /// Delete a key from the cache
    ///
    /// Returns `true` if the key existed before deletion, `false` otherwise.
    /// Note: Due to concurrent access, the return value is best-effort and
    /// may not reflect the exact state at the moment of deletion.
    async fn delete(&self, key: &str) -> Result<bool, CacheError>;

    /// Check if a key exists in the cache
    ///
    /// Note: Result may be stale due to concurrent modifications or TTL expiry.
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;

    /// Atomic increment with TTL (creates key if not exists)
    ///
    /// Used for rate limiting. Must be atomic to ensure correctness.
    async fn incr(&self, key: &str, ttl: Option<Duration>) -> Result<i64, CacheError>;

    /// Get the current counter value without incrementing
    ///
    /// Returns None if the counter doesn't exist or has expired.
    /// Used for rate limit pre-checks to avoid incrementing on blocked requests.
    async fn get_counter(&self, key: &str) -> Result<Option<i64>, CacheError>;

    /// Get the TTL remaining for a key
    async fn ttl(&self, key: &str) -> Result<Option<Duration>, CacheError>;

    /// Delete keys matching a pattern (supports glob patterns like "user:*")
    ///
    /// Performance: O(n) for memory backend, uses SCAN for Redis
    async fn delete_pattern(&self, pattern: &str) -> Result<u64, CacheError>;

    /// Health check (validates connection)
    async fn health_check(&self) -> Result<(), CacheError>;

    /// Backend name for debugging/logging
    fn backend_name(&self) -> &'static str;
}
