//! Invalidating a cached value, as a port.
//!
//! **Narrow on purpose: one method.** The domain's only need is to *invalidate* - a provider whose credential
//! changed must not be served from a cache - and a port scoped to that says so, where `&CacheService` said "this
//! code may do anything a cache can". Caching itself is a decorator over a port, never a parameter to one, which
//! is why there is no `get` here: nothing in the domain reads through a cache.
//!
//! `delete_local` rather than `delete`, and the name is the honest one: a process-local cache cannot be
//! invalidated across instances, so this clears *this* process. That limit is why project rows are not cached at
//! all, and naming it here stops a caller assuming otherwise.

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use crate::cache_key::CacheKey;

/// Object-safe cache operations used across crate boundaries.
///
/// Values cross this port as bytes. [`TypedCache`] adds MessagePack
/// convenience without making this trait non-object-safe.
#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, String>;

    async fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>)
    -> Result<(), String>;

    async fn delete(&self, key: &str) -> Result<bool, String>;

    async fn invalidate_key(&self, key: &str);

    async fn exists(&self, key: &str) -> Result<bool, String>;

    async fn delete_pattern(&self, pattern: &str) -> Result<u64, String>;

    async fn incr(&self, key: &str, ttl: Option<Duration>) -> Result<i64, String>;

    async fn get_counter(&self, key: &str) -> Result<Option<i64>, String>;

    async fn ttl(&self, key: &str) -> Result<Option<Duration>, String>;

    async fn health_check(&self) -> Result<(), String>;

    fn backend_name(&self) -> &'static str;
}

/// Typed MessagePack access layered over the object-safe byte port.
#[async_trait]
pub trait TypedCache: CacheStore {
    async fn get<T>(&self, key: &str) -> Result<Option<T>, String>
    where
        T: DeserializeOwned + Send,
    {
        self.get_raw(key).await?.map_or(Ok(None), |bytes| {
            rmp_serde::from_slice(&bytes)
                .map(Some)
                .map_err(|error| error.to_string())
        })
    }

    async fn set<T>(&self, key: &str, value: &T, ttl: Option<Duration>) -> Result<(), String>
    where
        T: Serialize + Sync,
    {
        let bytes = rmp_serde::to_vec(value).map_err(|error| error.to_string())?;
        self.set_raw(key, bytes, ttl).await
    }
}

impl<T> TypedCache for T where T: CacheStore + ?Sized {}

pub async fn invalidate_user_org_lists(cache: &dyn CacheStore, user_id: &str) {
    cache
        .invalidate_key(&CacheKey::orgs_for_user(user_id))
        .await;
    cache
        .invalidate_key(&CacheKey::projects_for_user(user_id))
        .await;
}

/// Clearing a cached value in this process.
#[async_trait]
pub trait CacheInvalidator: Send + Sync {
    /// Remove `key` from this process's cache. `Ok(false)` means it was not there.
    async fn delete_local(&self, key: &str) -> Result<bool, String>;
}

/// Process-local cache for values that must never be replicated to Redis.
#[async_trait]
pub trait LocalCacheStore: Send + Sync {
    async fn get_local_raw(&self, key: &str) -> Result<Option<Vec<u8>>, String>;

    async fn set_local_raw(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), String>;

    async fn delete_local(&self, key: &str) -> Result<bool, String>;
}

#[async_trait]
pub trait TypedLocalCache: LocalCacheStore {
    async fn get_local<T>(&self, key: &str) -> Result<Option<T>, String>
    where
        T: DeserializeOwned + Send,
    {
        self.get_local_raw(key).await?.map_or(Ok(None), |bytes| {
            rmp_serde::from_slice(&bytes)
                .map(Some)
                .map_err(|error| error.to_string())
        })
    }

    async fn set_local<T>(&self, key: &str, value: &T, ttl: Option<Duration>) -> Result<(), String>
    where
        T: Serialize + Sync,
    {
        let bytes = rmp_serde::to_vec(value).map_err(|error| error.to_string())?;
        self.set_local_raw(key, bytes, ttl).await
    }
}

impl<T> TypedLocalCache for T where T: LocalCacheStore + ?Sized {}
