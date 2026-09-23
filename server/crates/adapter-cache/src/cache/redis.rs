//! Redis-compatible cache implementation using deadpool-redis
//!
//! Feature-gated behind `redis-cache` feature.
//!
//! Supports:
//! - Redis (standard)
//! - Redis Sentinel (high availability)
//! - Valkey (open-source Redis fork, drop-in compatible)
//! - Dragonfly (high-performance Redis-compatible)
//!
//! # URL Formats
//!
//! Standard Redis/Valkey/Dragonfly:
//! ```text
//! redis://[user:password@]host:port[/db]
//! rediss://[user:password@]host:port[/db]  (TLS)
//! ```
//!
//! Redis Sentinel:
//! ```text
//! redis+sentinel://[user:password@]sentinel1:port,sentinel2:port/master_name[/db]
//! ```

use std::time::Duration;

use async_trait::async_trait;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{Config, Pool, Runtime};

use super::backend::CacheBackend;
use super::error::CacheError;

/// Redis-compatible cache implementation
///
/// Uses connection pooling via deadpool-redis for efficient connection management.
/// Compatible with Redis, Redis Sentinel, Valkey, and Dragonfly.
pub struct RedisCache {
    pool: Pool,
    backend_type: RedisBackendType,
}

/// Type of Redis-compatible backend being used
#[derive(Debug, Clone, Copy)]
enum RedisBackendType {
    /// Standard Redis or compatible (Valkey, Dragonfly)
    Redis,
    /// Redis Sentinel for high availability
    Sentinel,
}

impl RedisCache {
    /// Create a new Redis-compatible cache with the given URL
    ///
    /// # URL Formats
    ///
    /// Standard Redis/Valkey/Dragonfly:
    /// - `redis://[user:password@]host:port[/db]`
    /// - `rediss://[user:password@]host:port[/db]` (TLS)
    ///
    /// Redis Sentinel:
    /// - `redis+sentinel://[user:password@]sentinel1:port,sentinel2:port/master_name[/db]`
    pub async fn new(redis_url: &str) -> Result<Self, CacheError> {
        let sanitized_url = sanitize_redis_url(redis_url);
        let backend_type = detect_backend_type(redis_url);

        let mut config = Config::from_url(redis_url);
        // Configure pool with reasonable defaults for production
        config.pool = Some(deadpool_redis::PoolConfig {
            max_size: 32, // Allow more concurrent connections than default (16)
            timeouts: deadpool_redis::Timeouts {
                // Timeout for getting a connection from the pool
                wait: Some(Duration::from_secs(5)),
                // Timeout for creating a new connection
                create: Some(Duration::from_secs(5)),
                // Timeout for recycling connections (health check)
                recycle: Some(Duration::from_secs(5)),
            },
            ..Default::default()
        });
        let pool = config.create_pool(Some(Runtime::Tokio1)).map_err(|e| {
            let hint = match backend_type {
                RedisBackendType::Sentinel => {
                    " (Sentinel URL format: redis+sentinel://host1:port,host2:port/master_name/db)"
                }
                RedisBackendType::Redis => "",
            };
            CacheError::Connection(format!(
                "Failed to create Redis pool for {sanitized_url}: {e}{hint}"
            ))
        })?;

        // Validate connection on startup
        let mut conn = pool.get().await.map_err(|e| {
            CacheError::Connection(format!(
                "Failed to get Redis connection from pool for {sanitized_url}: {e}"
            ))
        })?;

        deadpool_redis::redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| {
                CacheError::Connection(format!("Redis PING failed for {sanitized_url}: {e}"))
            })?;

        let backend_name = match backend_type {
            RedisBackendType::Redis => "redis",
            RedisBackendType::Sentinel => "redis-sentinel",
        };
        tracing::debug!(url = %sanitized_url, backend = backend_name, "Redis cache connected");

        Ok(Self { pool, backend_type })
    }
}

/// Detect the backend type from URL scheme
fn detect_backend_type(url: &str) -> RedisBackendType {
    if url.starts_with("redis+sentinel://") || url.starts_with("rediss+sentinel://") {
        RedisBackendType::Sentinel
    } else {
        RedisBackendType::Redis
    }
}

/// Sanitize Redis URL for logging (removes password)
///
/// Handles both standard Redis and Sentinel URL formats:
/// - `redis://[user:password@]host:port/db`
/// - `redis+sentinel://[user:password@]sentinel1:port,sentinel2:port/master_name/db`
fn sanitize_redis_url(url: &str) -> String {
    // Parse URL and mask password if present
    // Use rfind('@') to handle passwords that may contain '@'
    if let Some(at_pos) = url.rfind('@') {
        // Find the protocol separator (handles redis://, rediss://, redis+sentinel://, etc.)
        let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
        // Find the colon after username (must be after scheme://)
        if let Some(colon_pos) = url[scheme_end..at_pos].find(':') {
            let abs_colon = scheme_end + colon_pos;
            let prefix = &url[..abs_colon + 1];
            let suffix = &url[at_pos..];
            return format!("{prefix}***{suffix}");
        }
    }
    url.to_string()
}

#[async_trait]
impl CacheBackend for RedisCache {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let mut conn = self.pool.get().await?;
        let result: Option<Vec<u8>> = conn.get(key).await?;
        Ok(result)
    }

    async fn set(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        let mut conn = self.pool.get().await?;
        match ttl {
            Some(ttl) => {
                // Use PSETEX for millisecond precision to avoid TTL truncation bugs
                // (as_secs() would make 999ms TTL become 0, meaning infinite)
                let ttl_ms = ttl.as_millis().try_into().unwrap_or(u64::MAX);
                // Ensure minimum 1ms TTL (0 would mean no expiry in some Redis versions)
                let ttl_ms = ttl_ms.max(1);
                let _: () = deadpool_redis::redis::cmd("PSETEX")
                    .arg(key)
                    .arg(ttl_ms)
                    .arg(value)
                    .query_async(&mut conn)
                    .await?;
            }
            None => {
                let _: () = conn.set(key, value).await?;
            }
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<bool, CacheError> {
        let mut conn = self.pool.get().await?;
        let deleted: i64 = conn.del(key).await?;
        Ok(deleted > 0)
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.exists(key).await?;
        Ok(exists)
    }

    async fn incr(&self, key: &str, ttl: Option<Duration>) -> Result<i64, CacheError> {
        let mut conn = self.pool.get().await?;

        // Lua script for atomic INCR + PEXPIRE (only sets TTL on first increment)
        // Uses PEXPIRE for millisecond precision consistency with set()
        //
        // Note: We use EVAL rather than EVALSHA because:
        // 1. Redis caches scripts by SHA internally, so repeated EVAL calls are efficient
        // 2. EVALSHA would require handling NOSCRIPT errors after Redis restart
        // 3. The script is small (~100 bytes) so network overhead is minimal
        let lua_script = r#"
            local count = redis.call('INCR', KEYS[1])
            if count == 1 and ARGV[1] then
                redis.call('PEXPIRE', KEYS[1], ARGV[1])
            end
            return count
        "#;

        // Convert to milliseconds, minimum 1ms, default 60s
        let ttl_ms = ttl
            .map(|d| d.as_millis().try_into().unwrap_or(u64::MAX).max(1))
            .unwrap_or(60_000);

        // Use EVAL command directly instead of Script (avoids needing script feature)
        let count: i64 = deadpool_redis::redis::cmd("EVAL")
            .arg(lua_script)
            .arg(1) // number of keys
            .arg(key) // KEYS[1]
            .arg(ttl_ms) // ARGV[1]
            .query_async(&mut conn)
            .await?;

        Ok(count)
    }

    async fn get_counter(&self, key: &str) -> Result<Option<i64>, CacheError> {
        let mut conn = self.pool.get().await?;

        // GET returns the string representation of the counter value
        let result: Option<String> = conn.get(key).await?;

        Ok(result.and_then(|s| s.parse::<i64>().ok()))
    }

    async fn ttl(&self, key: &str) -> Result<Option<Duration>, CacheError> {
        let mut conn = self.pool.get().await?;
        // Use PTTL for millisecond precision consistency
        let ttl_ms: i64 = deadpool_redis::redis::cmd("PTTL")
            .arg(key)
            .query_async(&mut conn)
            .await?;

        match ttl_ms {
            -2 => Ok(None), // Key doesn't exist
            -1 => Ok(None), // Key exists but has no TTL
            n if n > 0 => Ok(Some(Duration::from_millis(n as u64))),
            _ => Ok(None), // Unexpected zero or negative value, treat as no TTL
        }
    }

    async fn delete_pattern(&self, pattern: &str) -> Result<u64, CacheError> {
        let mut conn = self.pool.get().await?;
        let mut count = 0u64;
        let mut cursor: u64 = 0;

        // SCAN is O(1) per call, safe for large keyspaces
        loop {
            let (new_cursor, keys): (u64, Vec<String>) = deadpool_redis::redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;

            if !keys.is_empty() {
                let deleted: u64 = deadpool_redis::redis::cmd("DEL")
                    .arg(&keys)
                    .query_async(&mut conn)
                    .await?;
                count += deleted;
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(count)
    }

    async fn health_check(&self) -> Result<(), CacheError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| CacheError::Connection(e.to_string()))?;

        deadpool_redis::redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| CacheError::Connection(e.to_string()))?;

        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        match self.backend_type {
            RedisBackendType::Redis => "redis",
            RedisBackendType::Sentinel => "redis-sentinel",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // URL Sanitization Tests
    // ==========================================================================

    #[test]
    fn test_sanitize_redis_url_no_password() {
        let url = "redis://localhost:6379/0";
        assert_eq!(sanitize_redis_url(url), "redis://localhost:6379/0");
    }

    #[test]
    fn test_sanitize_redis_url_with_password() {
        let url = "redis://user:secretpassword@localhost:6379/0";
        assert_eq!(sanitize_redis_url(url), "redis://user:***@localhost:6379/0");
    }

    #[test]
    fn test_sanitize_redis_url_password_only() {
        let url = "redis://:password@localhost:6379";
        assert_eq!(sanitize_redis_url(url), "redis://:***@localhost:6379");
    }

    #[test]
    fn test_sanitize_redis_url_complex_password() {
        // Password contains @ character - should find the last @ as the separator
        let url = "redis://admin:p@ss:w0rd!@redis.example.com:6379/1";
        assert_eq!(
            sanitize_redis_url(url),
            "redis://admin:***@redis.example.com:6379/1"
        );
    }

    #[test]
    fn test_sanitize_redis_url_empty() {
        let url = "";
        assert_eq!(sanitize_redis_url(url), "");
    }

    #[test]
    fn test_sanitize_redis_url_tls() {
        let url = "rediss://user:secret@redis.example.com:6380/0";
        assert_eq!(
            sanitize_redis_url(url),
            "rediss://user:***@redis.example.com:6380/0"
        );
    }

    // ==========================================================================
    // Sentinel URL Tests
    // ==========================================================================

    #[test]
    fn test_sanitize_sentinel_url_no_password() {
        let url = "redis+sentinel://sentinel1:26379,sentinel2:26379/mymaster/0";
        assert_eq!(
            sanitize_redis_url(url),
            "redis+sentinel://sentinel1:26379,sentinel2:26379/mymaster/0"
        );
    }

    #[test]
    fn test_sanitize_sentinel_url_with_password() {
        let url = "redis+sentinel://user:secret@sentinel1:26379,sentinel2:26379/mymaster/0";
        assert_eq!(
            sanitize_redis_url(url),
            "redis+sentinel://user:***@sentinel1:26379,sentinel2:26379/mymaster/0"
        );
    }

    #[test]
    fn test_sanitize_sentinel_url_tls() {
        let url = "rediss+sentinel://user:pass@s1:26379,s2:26379/master/1";
        assert_eq!(
            sanitize_redis_url(url),
            "rediss+sentinel://user:***@s1:26379,s2:26379/master/1"
        );
    }

    // ==========================================================================
    // Backend Type Detection Tests
    // ==========================================================================

    #[test]
    fn test_detect_backend_type_redis() {
        assert!(matches!(
            detect_backend_type("redis://localhost:6379"),
            RedisBackendType::Redis
        ));
        assert!(matches!(
            detect_backend_type("rediss://localhost:6379"),
            RedisBackendType::Redis
        ));
    }

    #[test]
    fn test_detect_backend_type_sentinel() {
        assert!(matches!(
            detect_backend_type("redis+sentinel://s1:26379/master"),
            RedisBackendType::Sentinel
        ));
        assert!(matches!(
            detect_backend_type("rediss+sentinel://s1:26379/master"),
            RedisBackendType::Sentinel
        ));
    }

    #[test]
    fn test_detect_backend_type_valkey_dragonfly() {
        // Valkey and Dragonfly use standard redis:// URLs
        assert!(matches!(
            detect_backend_type("redis://valkey.example.com:6379"),
            RedisBackendType::Redis
        ));
        assert!(matches!(
            detect_backend_type("redis://dragonfly.example.com:6379"),
            RedisBackendType::Redis
        ));
    }
}
