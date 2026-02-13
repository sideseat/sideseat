//! In-memory cache implementation using moka + dashmap
//!
//! Uses moka for the main cache with TinyLFU eviction and dashmap
//! for atomic counters (rate limiting).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use moka::Expiry;
use moka::future::Cache;

use super::backend::CacheBackend;
use super::error::CacheError;
use crate::core::config::{CacheConfig, EvictionPolicy};

/// Cache entry with data and metadata
#[derive(Clone)]
struct CacheEntry {
    data: Vec<u8>,
    ttl: Option<Duration>,
    created_at: Instant,
}

/// Per-entry expiry tracking for variable TTLs
struct VariableTtlExpiry;

impl Expiry<String, CacheEntry> for VariableTtlExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &CacheEntry,
        _created_at: Instant,
    ) -> Option<Duration> {
        value.ttl
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &CacheEntry,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        value.ttl
    }

    fn expire_after_read(
        &self,
        _key: &String,
        _value: &CacheEntry,
        _read_at: Instant,
        duration_until_expiry: Option<Duration>,
        _last_modified_at: Instant,
    ) -> Option<Duration> {
        duration_until_expiry
    }
}

/// Counter entry for rate limiting
struct CounterEntry {
    count: AtomicI64,
    expires_at: Instant,
}

/// In-memory cache implementation
///
/// Uses:
/// - `moka::Cache` - General cache with TinyLFU eviction, automatic cleanup
/// - `DashMap<CounterEntry>` - Atomic counters for rate limiting
/// - `cleanup_ops` - Tracks operations to trigger periodic counter cleanup
pub struct InMemoryCache {
    cache: Cache<String, CacheEntry>,
    counters: DashMap<String, CounterEntry>,
    /// Counter for cleanup scheduling (increments on every incr operation)
    cleanup_ops: AtomicU64,
}

impl InMemoryCache {
    /// Create a new in-memory cache with the given configuration
    ///
    /// Note: moka uses TinyLFU eviction regardless of the eviction_policy setting.
    /// The LRU option exists for API compatibility but has the same behavior as TinyLFU.
    pub fn new(config: &CacheConfig) -> Self {
        let builder = Cache::builder()
            .max_capacity(config.max_entries)
            // Set initial capacity to reduce rehashing during warmup
            .initial_capacity((config.max_entries as usize / 4).min(10_000));

        // Note: moka always uses TinyLFU internally. The eviction_policy config
        // is kept for API compatibility but doesn't change behavior.
        if config.eviction_policy == EvictionPolicy::Lru {
            tracing::debug!(
                "LRU eviction policy selected but moka uses TinyLFU internally. \
                 TinyLFU provides similar recency-based eviction with better hit rates."
            );
        }

        let cache = builder.expire_after(VariableTtlExpiry).build();

        Self {
            cache,
            counters: DashMap::new(),
            cleanup_ops: AtomicU64::new(0),
        }
    }

    /// Clean up expired counters (called periodically)
    fn cleanup_expired_counters(&self) {
        let now = Instant::now();
        self.counters.retain(|_, entry| now < entry.expires_at);
    }
}

#[async_trait]
impl CacheBackend for InMemoryCache {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        Ok(self.cache.get(key).await.map(|entry| entry.data.clone()))
    }

    async fn set(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        let entry = CacheEntry {
            data: value,
            ttl,
            created_at: Instant::now(),
        };
        self.cache.insert(key.to_string(), entry).await;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<bool, CacheError> {
        let existed = self.cache.contains_key(key);
        self.cache.invalidate(key).await;
        Ok(existed)
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        Ok(self.cache.contains_key(key))
    }

    async fn incr(&self, key: &str, ttl: Option<Duration>) -> Result<i64, CacheError> {
        use dashmap::mapref::entry::Entry;

        let now = Instant::now();
        let ttl_duration = ttl.unwrap_or(Duration::from_secs(60));
        let expires_at = now + ttl_duration;

        // Use entry API for atomic access - prevents TOCTOU race condition
        let count = match self.counters.entry(key.to_string()) {
            Entry::Occupied(mut occupied) => {
                let counter = occupied.get_mut();
                if now >= counter.expires_at {
                    // Expired - reset atomically while holding exclusive access
                    counter.count.store(1, Ordering::SeqCst);
                    counter.expires_at = expires_at;
                    1
                } else {
                    counter.count.fetch_add(1, Ordering::SeqCst) + 1
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(CounterEntry {
                    count: AtomicI64::new(1),
                    expires_at,
                });
                1
            }
        };

        // Periodically clean up expired counters
        // Uses dedicated cleanup counter (not counter value) for reliable scheduling
        // Cleanup runs every 256 operations regardless of counter map size
        // This prevents memory leaks in small deployments
        let ops = self.cleanup_ops.fetch_add(1, Ordering::Relaxed);
        if ops.is_multiple_of(256) {
            self.cleanup_expired_counters();
        }

        Ok(count)
    }

    async fn get_counter(&self, key: &str) -> Result<Option<i64>, CacheError> {
        let now = Instant::now();

        if let Some(entry) = self.counters.get(key) {
            // Check if counter has expired
            if now < entry.expires_at {
                return Ok(Some(entry.count.load(Ordering::SeqCst)));
            }
        }

        Ok(None)
    }

    async fn ttl(&self, key: &str) -> Result<Option<Duration>, CacheError> {
        // Check counters first (for rate limiting)
        if let Some(entry) = self.counters.get(key) {
            let now = Instant::now();
            // Use saturating_duration_since to avoid panic on clock issues
            let remaining = entry.expires_at.saturating_duration_since(now);
            if remaining > Duration::ZERO {
                return Ok(Some(remaining));
            }
            return Ok(None);
        }

        // For regular cache entries, calculate remaining TTL from stored values
        if let Some(entry) = self.cache.get(key).await {
            if let Some(ttl) = entry.ttl {
                let elapsed = entry.created_at.elapsed();
                // Use checked_sub to safely handle edge cases
                if let Some(remaining) = ttl.checked_sub(elapsed)
                    && remaining > Duration::ZERO
                {
                    return Ok(Some(remaining));
                }
                // Entry is expired but not yet evicted
                return Ok(None);
            }
            // Entry exists but has no TTL (infinite)
            return Ok(None);
        }

        Ok(None)
    }

    async fn delete_pattern(&self, pattern: &str) -> Result<u64, CacheError> {
        // Convert glob pattern to prefix (simple implementation)
        let prefix = pattern.trim_end_matches('*');
        let mut count = 0u64;

        // Collect keys to delete (avoid holding lock during deletion)
        // Note: moka iter returns Arc<String> for keys, so we dereference
        let keys_to_delete: Vec<String> = self
            .cache
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| (*k).clone())
            .collect();

        for key in keys_to_delete {
            self.cache.invalidate(&key).await;
            count += 1;
        }

        // Also clean up counters matching the pattern
        self.counters.retain(|k, _| {
            if k.starts_with(prefix) {
                count += 1;
                false
            } else {
                true
            }
        });

        Ok(count)
    }

    async fn health_check(&self) -> Result<(), CacheError> {
        // In-memory is always healthy
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CacheConfig {
        CacheConfig {
            backend: crate::core::config::CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        }
    }

    #[tokio::test]
    async fn test_set_get_roundtrip() {
        let cache = InMemoryCache::new(&test_config());

        cache.set("key1", b"value1".to_vec(), None).await.unwrap();
        let result = cache.get("key1").await.unwrap();
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let cache = InMemoryCache::new(&test_config());

        let result = cache.get("nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_delete() {
        let cache = InMemoryCache::new(&test_config());

        cache.set("key1", b"value1".to_vec(), None).await.unwrap();
        let deleted = cache.delete("key1").await.unwrap();
        assert!(deleted);

        let result = cache.get("key1").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let cache = InMemoryCache::new(&test_config());

        let deleted = cache.delete("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_exists() {
        let cache = InMemoryCache::new(&test_config());

        assert!(!cache.exists("key1").await.unwrap());

        cache.set("key1", b"value1".to_vec(), None).await.unwrap();
        assert!(cache.exists("key1").await.unwrap());
    }

    #[tokio::test]
    async fn test_incr_atomic() {
        let cache = InMemoryCache::new(&test_config());
        let ttl = Some(Duration::from_secs(60));

        let count1 = cache.incr("counter", ttl).await.unwrap();
        assert_eq!(count1, 1);

        let count2 = cache.incr("counter", ttl).await.unwrap();
        assert_eq!(count2, 2);

        let count3 = cache.incr("counter", ttl).await.unwrap();
        assert_eq!(count3, 3);
    }

    #[tokio::test]
    async fn test_incr_expired_resets() {
        let cache = InMemoryCache::new(&test_config());

        // Set with very short TTL
        let count1 = cache
            .incr("counter", Some(Duration::from_millis(1)))
            .await
            .unwrap();
        assert_eq!(count1, 1);

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Should reset to 1
        let count2 = cache
            .incr("counter", Some(Duration::from_secs(60)))
            .await
            .unwrap();
        assert_eq!(count2, 1);
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let cache = InMemoryCache::new(&test_config());

        // Set with very short TTL
        cache
            .set("key1", b"value1".to_vec(), Some(Duration::from_millis(50)))
            .await
            .unwrap();

        // Should exist immediately
        assert!(cache.exists("key1").await.unwrap());

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Force cache cleanup by running sync
        cache.cache.run_pending_tasks().await;

        // Should be gone
        let result = cache.get("key1").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_delete_pattern() {
        let cache = InMemoryCache::new(&test_config());

        cache.set("user:1", b"a".to_vec(), None).await.unwrap();
        cache.set("user:2", b"b".to_vec(), None).await.unwrap();
        cache.set("org:1", b"c".to_vec(), None).await.unwrap();

        let deleted = cache.delete_pattern("user:*").await.unwrap();
        assert_eq!(deleted, 2);

        assert!(!cache.exists("user:1").await.unwrap());
        assert!(!cache.exists("user:2").await.unwrap());
        assert!(cache.exists("org:1").await.unwrap());
    }

    #[tokio::test]
    async fn test_health_check() {
        let cache = InMemoryCache::new(&test_config());
        assert!(cache.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_backend_name() {
        let cache = InMemoryCache::new(&test_config());
        assert_eq!(cache.backend_name(), "memory");
    }

    #[tokio::test]
    async fn test_ttl_for_counter() {
        let cache = InMemoryCache::new(&test_config());

        cache
            .incr("counter", Some(Duration::from_secs(60)))
            .await
            .unwrap();
        let ttl = cache.ttl("counter").await.unwrap();
        assert!(ttl.is_some());
        assert!(ttl.unwrap() > Duration::from_secs(50));
    }

    #[tokio::test]
    async fn test_ttl_for_cache_entry() {
        let cache = InMemoryCache::new(&test_config());

        // Set with known TTL
        cache
            .set("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .await
            .unwrap();

        let ttl = cache.ttl("key1").await.unwrap();
        assert!(ttl.is_some());
        // TTL should be close to 60 seconds (allowing for test execution time)
        let ttl_secs = ttl.unwrap().as_secs();
        assert!((58..=60).contains(&ttl_secs));
    }

    #[tokio::test]
    async fn test_ttl_for_nonexistent_key() {
        let cache = InMemoryCache::new(&test_config());

        let ttl = cache.ttl("nonexistent").await.unwrap();
        assert!(ttl.is_none());
    }

    #[tokio::test]
    async fn test_ttl_for_infinite_entry() {
        let cache = InMemoryCache::new(&test_config());

        // Set without TTL (infinite)
        cache.set("key1", b"value1".to_vec(), None).await.unwrap();

        let ttl = cache.ttl("key1").await.unwrap();
        // Infinite entries return None for TTL
        assert!(ttl.is_none());
    }
}
