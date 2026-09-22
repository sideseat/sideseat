//! Queue backend adapters.
//!
//! The transport-neutral queue contract lives in `sideseat-ports`; typed protobuf/MessagePack
//! wrappers live in `sideseat-domain`. This crate owns only the in-process and Redis implementations.

pub mod ack_window;
pub mod memory;
pub mod pubsub;
pub mod redis;
#[cfg(test)]
mod redis_stream_tests;

use std::sync::Arc;

use sideseat_core::core::config::{CacheBackendType, CacheConfig};
pub use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend, TopicError,
};

pub use memory::MemoryTopicBackend;
pub use redis::RedisTopicBackend;

/// Build the configured queue backend without coupling queue choice to its typed domain wrapper.
pub async fn backend_from_cache_config(
    cache_config: &CacheConfig,
) -> Result<Arc<dyn TopicBackend>, TopicError> {
    match cache_config.backend {
        CacheBackendType::Memory => Ok(Arc::new(MemoryTopicBackend::new())),
        CacheBackendType::Redis => {
            let url = cache_config
                .redis_url
                .as_ref()
                .ok_or_else(|| TopicError::Config("redis_url required for Redis backend".into()))?;
            Ok(Arc::new(
                RedisTopicBackend::with_replica_acks(url, cache_config.redis_min_replica_acks)
                    .await?,
            ))
        }
    }
}

#[must_use]
pub fn memory_backend() -> Arc<dyn TopicBackend> {
    Arc::new(MemoryTopicBackend::new())
}
