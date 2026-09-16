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

use async_trait::async_trait;

/// Clearing a cached value in this process.
#[async_trait]
pub trait CacheInvalidator: Send + Sync {
    /// Remove `key` from this process's cache. `Ok(false)` means it was not there.
    async fn delete_local(&self, key: &str) -> Result<bool, String>;
}
