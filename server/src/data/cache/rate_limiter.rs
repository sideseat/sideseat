//! Rate limiter using cache backend
//!
//! Implements a fixed window counter algorithm with burst allowance.
//!
//! # Algorithm
//!
//! Uses fixed time windows (default 60 seconds) with atomic counters.
//! Each window starts when the first request arrives and resets after the
//! window duration expires.
//!
//! # Burst Allowance
//!
//! Each bucket has a configurable burst allowance that permits temporary
//! traffic spikes above the sustained rate. The total limit is
//! `requests_per_window + burst`.
//!
//! # Known Limitations
//!
//! **Window Boundary Burst**: Fixed window algorithms allow up to 2x the limit
//! at window boundaries. For example, with a 100 req/min limit:
//! - 100 requests at second 59 of window 1
//! - 100 requests at second 0 of window 2
//! - Result: 200 requests in 2 seconds
//!
//! This is acceptable for most use cases. For stricter rate limiting,
//! consider sliding window algorithms (not currently implemented).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::CacheService;
use super::key::CacheKey;
use crate::core::constants::DEFAULT_RATE_LIMIT_WINDOW_SECS;

/// Rate limit bucket configuration
#[derive(Debug, Clone)]
pub struct RateLimitBucket {
    /// Bucket name (e.g., "api", "ingestion", "auth")
    pub name: &'static str,
    /// Maximum requests per window
    pub requests_per_window: u32,
    /// Window duration in seconds
    pub window_secs: u64,
    /// Burst allowance (additional requests above limit)
    pub burst: u32,
}

impl RateLimitBucket {
    /// Create an API rate limit bucket
    pub fn api(rpm: u32) -> Self {
        Self {
            name: "api",
            requests_per_window: rpm,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            burst: rpm / 20, // 5% burst
        }
    }

    /// Create an ingestion rate limit bucket
    pub fn ingestion(rpm: u32) -> Self {
        Self {
            name: "ingestion",
            requests_per_window: rpm,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            burst: rpm / 10, // 10% burst
        }
    }

    /// Create an auth rate limit bucket
    pub fn auth(rpm: u32) -> Self {
        Self {
            name: "auth",
            requests_per_window: rpm,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            burst: rpm / 3, // 33% burst
        }
    }

    /// Create a files rate limit bucket
    pub fn files(rpm: u32) -> Self {
        Self {
            name: "files",
            requests_per_window: rpm,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            burst: rpm / 5, // 20% burst
        }
    }

    /// Create an auth failures rate limit bucket
    /// Used to track failed API key/JWT auth attempts per IP
    pub fn auth_failures(rpm: u32) -> Self {
        Self {
            name: "auth_fail",
            requests_per_window: rpm,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            burst: rpm / 10, // 10% burst
        }
    }

    /// Get the total limit (requests + burst)
    pub fn total_limit(&self) -> u32 {
        self.requests_per_window.saturating_add(self.burst)
    }
}

/// Rate limit check result
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed
    pub allowed: bool,
    /// Requests remaining in window
    pub remaining: u32,
    /// Total limit (rpm + burst)
    pub limit: u32,
    /// Unix timestamp when window resets
    pub reset_at: u64,
    /// Seconds until retry (only if blocked)
    pub retry_after: Option<u64>,
}

/// Rate limiter using cache backend
pub struct RateLimiter {
    cache: Arc<CacheService>,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(cache: Arc<CacheService>) -> Self {
        Self { cache }
    }

    /// Check rate limit for identifier in bucket
    ///
    /// Returns a result indicating whether the request is allowed
    /// and information about the current rate limit state.
    pub async fn check(&self, bucket: &RateLimitBucket, identifier: &str) -> RateLimitResult {
        let key = CacheKey::rate_limit(bucket.name, identifier);
        let window_duration = Duration::from_secs(bucket.window_secs);

        // Capture time FIRST to avoid race condition with TTL calculation
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "System clock is before UNIX epoch");
                0
            });

        // Atomic increment with TTL (creates key if not exists)
        let count = match self.cache.incr(&key, Some(window_duration)).await {
            Ok(c) => c,
            Err(e) => {
                // Log error but allow request to avoid blocking on cache failures
                tracing::error!(
                    bucket = bucket.name,
                    %identifier,
                    error = %e,
                    "Rate limit cache increment failed, allowing request"
                );
                1
            }
        };

        let limit = bucket.total_limit();
        // Use saturating arithmetic to prevent overflow
        let limit_i64 = i64::from(limit);
        let allowed = count <= limit_i64;
        let remaining = limit_i64.saturating_sub(count).try_into().unwrap_or(0u32);

        // Calculate reset time (TTL remaining + now)
        let ttl = self.cache.ttl(&key).await.ok().flatten();
        let reset_at = now.saturating_add(ttl.map(|d| d.as_secs()).unwrap_or(bucket.window_secs));

        // Log rate limit check
        tracing::trace!(
            bucket = bucket.name,
            %identifier,
            count,
            limit,
            allowed,
            "Rate limit check"
        );

        RateLimitResult {
            allowed,
            remaining,
            limit,
            reset_at,
            retry_after: if allowed {
                None
            } else {
                Some(reset_at.saturating_sub(now))
            },
        }
    }

    /// Check if identifier is blocked WITHOUT incrementing the counter.
    /// Use this for pre-validation checks where you don't want to consume rate limit budget.
    pub async fn is_blocked(&self, bucket: &RateLimitBucket, identifier: &str) -> bool {
        let key = CacheKey::rate_limit(bucket.name, identifier);

        // Read current count without incrementing
        let count = match self.cache.get_counter(&key).await {
            Ok(Some(c)) => c,
            Ok(None) => 0, // No counter yet
            Err(e) => {
                tracing::warn!(
                    bucket = bucket.name,
                    %identifier,
                    error = %e,
                    "Rate limit cache read failed, assuming not blocked"
                );
                0
            }
        };

        let limit = bucket.total_limit();
        count > i64::from(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{CacheBackendType, CacheConfig, EvictionPolicy};

    async fn test_cache() -> Arc<CacheService> {
        let config = CacheConfig {
            backend: CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        };
        Arc::new(CacheService::new(&config).await.unwrap())
    }

    #[tokio::test]
    async fn test_rate_limit_allows_under_limit() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket::api(100);

        for i in 0..50 {
            let result = limiter.check(&bucket, "192.168.1.1").await;
            assert!(result.allowed, "Request {} should be allowed", i);
            assert!(result.remaining > 0);
            assert!(result.retry_after.is_none());
        }
    }

    #[tokio::test]
    async fn test_rate_limit_blocks_over_limit() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket {
            name: "test",
            requests_per_window: 5,
            window_secs: 60,
            burst: 0,
        };

        // First 5 should be allowed
        for i in 0..5 {
            let result = limiter.check(&bucket, "192.168.1.1").await;
            assert!(result.allowed, "Request {} should be allowed", i);
        }

        // 6th should be blocked
        let result = limiter.check(&bucket, "192.168.1.1").await;
        assert!(!result.allowed, "Request 6 should be blocked");
        assert!(result.retry_after.is_some());
    }

    #[tokio::test]
    async fn test_burst_allowance() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket {
            name: "test",
            requests_per_window: 10,
            window_secs: 60,
            burst: 5, // 50% burst
        };

        // Should allow 15 requests (10 + 5 burst)
        for i in 0..15 {
            let result = limiter.check(&bucket, "192.168.1.1").await;
            assert!(result.allowed, "Request {} should be allowed", i);
        }

        // 16th should be blocked
        let result = limiter.check(&bucket, "192.168.1.1").await;
        assert!(!result.allowed, "Request 16 should be blocked");
    }

    #[tokio::test]
    async fn test_different_identifiers() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket {
            name: "test",
            requests_per_window: 5,
            window_secs: 60,
            burst: 0,
        };

        // Exhaust limit for first identifier
        for _ in 0..5 {
            limiter.check(&bucket, "192.168.1.1").await;
        }
        let result = limiter.check(&bucket, "192.168.1.1").await;
        assert!(!result.allowed);

        // Second identifier should still work
        let result = limiter.check(&bucket, "192.168.1.2").await;
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn test_bucket_constructors() {
        let api = RateLimitBucket::api(1000);
        assert_eq!(api.name, "api");
        assert_eq!(api.requests_per_window, 1000);
        assert_eq!(api.burst, 50); // 5%

        let ingestion = RateLimitBucket::ingestion(10000);
        assert_eq!(ingestion.name, "ingestion");
        assert_eq!(ingestion.requests_per_window, 10000);
        assert_eq!(ingestion.burst, 1000); // 10%

        let auth = RateLimitBucket::auth(30);
        assert_eq!(auth.name, "auth");
        assert_eq!(auth.requests_per_window, 30);
        assert_eq!(auth.burst, 10); // 33%

        let files = RateLimitBucket::files(100);
        assert_eq!(files.name, "files");
        assert_eq!(files.requests_per_window, 100);
        assert_eq!(files.burst, 20); // 20%

        let auth_failures = RateLimitBucket::auth_failures(60);
        assert_eq!(auth_failures.name, "auth_fail");
        assert_eq!(auth_failures.requests_per_window, 60);
        assert_eq!(auth_failures.burst, 6); // 10%
    }

    #[tokio::test]
    async fn test_result_fields() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket {
            name: "test",
            requests_per_window: 10,
            window_secs: 60,
            burst: 5,
        };

        let result = limiter.check(&bucket, "192.168.1.1").await;
        assert!(result.allowed);
        assert_eq!(result.limit, 15); // 10 + 5 burst
        assert_eq!(result.remaining, 14); // 15 - 1
        assert!(result.reset_at > 0);
        assert!(result.retry_after.is_none());
    }

    #[tokio::test]
    async fn test_is_blocked_without_incrementing() {
        let cache = test_cache().await;
        let limiter = RateLimiter::new(cache);
        let bucket = RateLimitBucket {
            name: "test",
            requests_per_window: 5,
            window_secs: 60,
            burst: 0,
        };

        // is_blocked should return false for new identifier
        assert!(!limiter.is_blocked(&bucket, "192.168.1.1").await);

        // Exhaust the limit with check() calls
        for _ in 0..5 {
            limiter.check(&bucket, "192.168.1.1").await;
        }

        // is_blocked should still return false (at limit, not over)
        assert!(!limiter.is_blocked(&bucket, "192.168.1.1").await);

        // One more check to exceed limit
        let result = limiter.check(&bucket, "192.168.1.1").await;
        assert!(!result.allowed);

        // Now is_blocked should return true
        assert!(limiter.is_blocked(&bucket, "192.168.1.1").await);

        // is_blocked should not increment (calling it again should still return true)
        assert!(limiter.is_blocked(&bucket, "192.168.1.1").await);
    }
}
