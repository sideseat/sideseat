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
pub mod redpanda;
#[cfg(test)]
mod redpanda_tests;

use std::sync::Arc;

use sideseat_core::core::config::{QueueBackendType, QueueConfig};
pub use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend, TopicError,
};

pub use memory::MemoryTopicBackend;
pub use redis::RedisTopicBackend;
pub use redpanda::RedpandaTopicBackend;

/// Number of logical partitions exposed by single-log queue backends.
///
/// RedPanda reports its real broker partition. Memory and Redis have one physical log, but preserving a stable
/// virtual partition still lets the domain scheduler keep a hot key from monopolising every selected batch.
const VIRTUAL_QUEUE_PARTITIONS: u32 = 32;

/// A stable FNV-1a partition for adapters that do not have broker partition metadata.
fn virtual_partition(partition_key: &str) -> u32 {
    let hash = partition_key
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    (hash % u64::from(VIRTUAL_QUEUE_PARTITIONS)) as u32
}

/// Build the configured queue backend without coupling queue choice to its typed domain wrapper.
pub async fn backend_from_queue_config(
    queue_config: &QueueConfig,
) -> Result<Arc<dyn TopicBackend>, TopicError> {
    match queue_config.backend {
        QueueBackendType::Memory => Ok(Arc::new(MemoryTopicBackend::new())),
        QueueBackendType::Redis => {
            let url = queue_config
                .redis_url
                .as_ref()
                .ok_or_else(|| TopicError::Config("redis_url required for Redis backend".into()))?;
            Ok(Arc::new(
                RedisTopicBackend::with_replica_acks(url, queue_config.redis_min_replica_acks)
                    .await?,
            ))
        }
        QueueBackendType::Redpanda => {
            let config = queue_config.redpanda.as_ref().ok_or_else(|| {
                TopicError::Config("redpanda configuration required for RedPanda backend".into())
            })?;
            Ok(Arc::new(RedpandaTopicBackend::new(config).await?))
        }
    }
}

#[must_use]
pub fn memory_backend() -> Arc<dyn TopicBackend> {
    Arc::new(MemoryTopicBackend::new())
}

#[cfg(test)]
mod partition_tests {
    use super::virtual_partition;

    #[test]
    fn virtual_partitions_are_stable_and_use_more_than_one_lane() {
        assert_eq!(virtual_partition("trace-a"), virtual_partition("trace-a"));
        let lanes = (0..64)
            .map(|index| virtual_partition(&format!("trace-{index}")))
            .collect::<std::collections::HashSet<_>>();
        assert!(lanes.len() > 1);
    }
}
