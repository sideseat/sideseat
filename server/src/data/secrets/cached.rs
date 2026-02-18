use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;

use super::error::SecretError;
use super::provider::SecretProvider;
use super::types::{Secret, SecretKey, SecretScope};

const CACHE_TTL_SECS: u64 = 300;
const CACHE_MAX_CAPACITY: u64 = 10_000;

#[derive(Debug)]
pub struct CachedProvider {
    inner: Arc<dyn SecretProvider>,
    cache: Cache<String, Option<Secret>>,
}

impl CachedProvider {
    pub fn new(inner: Arc<dyn SecretProvider>) -> Self {
        let cache = Cache::builder()
            .max_capacity(CACHE_MAX_CAPACITY)
            .time_to_live(Duration::from_secs(CACHE_TTL_SECS))
            .build();
        Self { inner, cache }
    }
}

#[async_trait]
impl SecretProvider for CachedProvider {
    async fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        let cache_key = key.to_string();
        if let Some(cached) = self.cache.get(&cache_key).await {
            return Ok(cached);
        }
        let result = self.inner.get(key).await?;
        self.cache.insert(cache_key, result.clone()).await;
        Ok(result)
    }

    async fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretError> {
        self.inner.set(key, secret).await?;
        self.cache
            .insert(key.to_string(), Some(secret.clone()))
            .await;
        Ok(())
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.inner.delete(key).await?;
        self.cache.invalidate(&key.to_string()).await;
        Ok(())
    }

    async fn list(&self, scope: &SecretScope) -> Result<Vec<SecretKey>, SecretError> {
        self.inner.list(scope).await
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }

    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }

    async fn health_check(&self) -> Result<(), SecretError> {
        self.inner.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::secrets::file::FileProvider;

    async fn make_cached(dir: &std::path::Path) -> CachedProvider {
        let inner = FileProvider::init(dir).await.unwrap();
        CachedProvider::new(Arc::new(inner))
    }

    #[tokio::test]
    async fn test_cache_hit_after_get() {
        let dir = tempfile::tempdir().unwrap();
        let cached = make_cached(dir.path()).await;

        let key = SecretKey::global("test_key");
        let secret = Secret::new("val");
        cached.inner.set(&key, &secret).await.unwrap();

        // First get populates cache
        let r1 = cached.get(&key).await.unwrap().unwrap();
        assert_eq!(r1.value, "val");

        // Delete from inner directly (bypass cache)
        cached.inner.delete(&key).await.unwrap();

        // Cache still returns the value
        let r2 = cached.get(&key).await.unwrap().unwrap();
        assert_eq!(r2.value, "val");
    }

    #[tokio::test]
    async fn test_write_through_on_set() {
        let dir = tempfile::tempdir().unwrap();
        let cached = make_cached(dir.path()).await;

        let key = SecretKey::global("wt_key");
        cached.set(&key, &Secret::new("v1")).await.unwrap();

        // Cache returns new value
        let r = cached.get(&key).await.unwrap().unwrap();
        assert_eq!(r.value, "v1");

        // Inner also has the value
        let inner_r = cached.inner.get(&key).await.unwrap().unwrap();
        assert_eq!(inner_r.value, "v1");
    }

    #[tokio::test]
    async fn test_invalidation_on_delete() {
        let dir = tempfile::tempdir().unwrap();
        let cached = make_cached(dir.path()).await;

        let key = SecretKey::global("del_key");
        cached.set(&key, &Secret::new("val")).await.unwrap();

        // Populate cache
        assert!(cached.get(&key).await.unwrap().is_some());

        // Delete through cache
        cached.delete(&key).await.unwrap();

        // Cache no longer returns it (fetches from inner, which also deleted it)
        assert!(cached.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_negative_caching() {
        let dir = tempfile::tempdir().unwrap();
        let cached = make_cached(dir.path()).await;

        let key = SecretKey::global("missing");
        // First get: miss from inner, caches None
        assert!(cached.get(&key).await.unwrap().is_none());

        // Write directly to inner (bypassing cache)
        cached
            .inner
            .set(&key, &Secret::new("now_exists"))
            .await
            .unwrap();

        // Cache still returns None (negative cached)
        assert!(cached.get(&key).await.unwrap().is_none());
    }
}
