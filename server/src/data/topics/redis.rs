//! Redis topic backend using Streams and Pub/Sub
//!
//! Feature-gated behind `redis-cache` feature.
//!
//! ## Redis Streams (Critical Topics)
//!
//! Uses Redis Streams for at-least-once delivery:
//! - `XADD` for publishing (with MAXLEN trimming)
//! - `XREADGROUP` for consuming (consumer groups)
//! - `XACK` for acknowledgment
//! - `XCLAIM` for recovery of stuck messages
//!
//! ## Redis Pub/Sub (Ephemeral Topics)
//!
//! Uses Redis Pub/Sub for broadcast delivery:
//! - `PUBLISH` for publishing (sends to Redis only)
//! - `SUBSCRIBE` for receiving (via bridge task)
//!
//! ### Bridge Architecture
//!
//! Each topic has ONE bridge task (not one per subscriber):
//! - Bridge task creates dedicated Redis connection for SUBSCRIBE
//! - Forwards messages from Redis to local broadcast channel
//! - Reference counting tracks subscribers; cleanup when zero
//! - Graceful shutdown support
//!
//! ### Message Flow (No Duplicates)
//!
//! ```text
//! publish() ──► Redis PUBLISH ──► Bridge Task ──► Local Broadcast ──► Subscribers
//! ```
//!
//! publish() does NOT send to local broadcast directly, eliminating duplicates.
//!
//! ## Key Prefixes
//!
//! - Streams: `{sideseat}:stream:{topic}` (hash tag for cluster compatibility)
//! - Pub/Sub: `{sideseat}:pubsub:{topic}`

use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use async_trait::async_trait;
use deadpool_redis::redis::{RedisResult, Value as RedisValue};
use deadpool_redis::{Config, Pool, Runtime};
use futures::StreamExt;

use super::backend::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};
use super::error::TopicError;
use super::pubsub::{ManagedSubscription, PubSubManager};

/// Stream key prefix (hash tag for Redis Cluster)
const STREAM_PREFIX: &str = "{sideseat}:stream:";

/// Pub/Sub channel prefix
const PUBSUB_PREFIX: &str = "{sideseat}:pubsub:";

/// Default MAXLEN for streams (approximate trimming)
const DEFAULT_STREAM_MAXLEN: u64 = 100_000;

/// XREADGROUP block timeout in milliseconds
const XREADGROUP_BLOCK_MS: u64 = 5000;

/// Reconnection delay for pub/sub after error
const PUBSUB_RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// Default broadcast channel capacity
const DEFAULT_BROADCAST_CAPACITY: usize = 10_000;

/// Redis topic backend
pub struct RedisTopicBackend {
    /// Connection pool for commands
    pool: Pool,
    /// Redis URL for creating dedicated pub/sub connections
    redis_url: String,
    /// Stream max length (approximate)
    stream_maxlen: u64,
    /// Pub/Sub manager (handles bridge lifecycle)
    pubsub_manager: Arc<PubSubManager>,
}

impl RedisTopicBackend {
    /// Create a new Redis topic backend
    pub async fn new(redis_url: &str) -> Result<Self, TopicError> {
        let sanitized_url = sanitize_redis_url(redis_url);

        let mut config = Config::from_url(redis_url);
        config.pool = Some(deadpool_redis::PoolConfig {
            max_size: 32,
            timeouts: deadpool_redis::Timeouts {
                wait: Some(Duration::from_secs(5)),
                create: Some(Duration::from_secs(5)),
                recycle: Some(Duration::from_secs(5)),
            },
            ..Default::default()
        });

        let pool = config.create_pool(Some(Runtime::Tokio1)).map_err(|e| {
            TopicError::Connection(format!(
                "Failed to create Redis pool for {sanitized_url}: {e}"
            ))
        })?;

        // Validate connection
        let mut conn = pool.get().await.map_err(|e| {
            TopicError::Connection(format!(
                "Failed to get Redis connection from pool for {sanitized_url}: {e}"
            ))
        })?;

        deadpool_redis::redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| {
                TopicError::Connection(format!("Redis PING failed for {sanitized_url}: {e}"))
            })?;

        tracing::debug!(url = %sanitized_url, "Redis topic backend connected");

        Ok(Self {
            pool,
            redis_url: redis_url.to_string(),
            stream_maxlen: DEFAULT_STREAM_MAXLEN,
            pubsub_manager: Arc::new(PubSubManager::new(DEFAULT_BROADCAST_CAPACITY)),
        })
    }

    /// Create using an existing connection pool
    ///
    /// Note: Requires Redis URL for dedicated pub/sub connections.
    #[allow(dead_code)]
    pub fn with_pool(pool: Pool, redis_url: &str) -> Self {
        Self {
            pool,
            redis_url: redis_url.to_string(),
            stream_maxlen: DEFAULT_STREAM_MAXLEN,
            pubsub_manager: Arc::new(PubSubManager::new(DEFAULT_BROADCAST_CAPACITY)),
        }
    }

    /// Get stream key with prefix
    fn stream_key(&self, topic: &str) -> String {
        format!("{}{}", STREAM_PREFIX, topic)
    }

    /// Get pub/sub channel with prefix
    fn pubsub_channel(&self, topic: &str) -> String {
        format!("{}{}", PUBSUB_PREFIX, topic)
    }

    /// Create consumer group if not exists
    async fn ensure_consumer_group(&self, topic: &str, group: &str) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        // Try to create group, ignore BUSYGROUP error
        let result: RedisResult<String> = deadpool_redis::redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&key)
            .arg(group)
            .arg("0") // Start from beginning to pick up messages published before consumer
            .arg("MKSTREAM") // Create stream if not exists
            .query_async(&mut conn)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("BUSYGROUP") => Ok(()), // Already exists
            Err(e) => Err(TopicError::ConsumerGroup(format!(
                "Failed to create consumer group {group}: {e}"
            ))),
        }
    }

    /// Start the bridge task for a topic
    ///
    /// Creates a dedicated Redis connection and subscribes to the channel.
    /// Forwards all messages to the local broadcast channel.
    fn start_bridge_task(&self, topic: &str) {
        let (bridge, is_new) = self.pubsub_manager.get_or_create_bridge(topic);

        if !is_new && bridge.is_task_running() {
            // Bridge already has a task running
            return;
        }

        let channel = self.pubsub_channel(topic);
        let redis_url = self.redis_url.clone();
        let bridge_clone = Arc::clone(&bridge);

        let handle = tokio::spawn(async move {
            Self::run_bridge_task(redis_url, channel, bridge_clone).await;
        });

        bridge.set_task(handle);
    }

    /// Run the bridge task that forwards Redis messages to local broadcast
    ///
    /// This task:
    /// 1. Creates a dedicated Redis connection (not from pool)
    /// 2. Subscribes to the Redis channel
    /// 3. Forwards messages to the local broadcast channel
    /// 4. Handles reconnection on errors
    /// 5. Stops on shutdown signal or when explicitly stopped
    async fn run_bridge_task(
        redis_url: String,
        channel: String,
        bridge: Arc<super::pubsub::PubSubBridge>,
    ) {
        let sanitized_url = sanitize_redis_url(&redis_url);
        tracing::debug!(channel = %channel, url = %sanitized_url, "Starting Redis pub/sub bridge");

        let mut stop_rx = bridge.stop_rx();
        let mut shutdown_rx = bridge.shutdown_rx();

        'outer: loop {
            // Check for stop/shutdown before connecting
            if *stop_rx.borrow() || *shutdown_rx.borrow() {
                break;
            }

            // Create dedicated client for pub/sub (not from pool)
            let client = match deadpool_redis::redis::Client::open(redis_url.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        channel = %channel,
                        "Failed to create Redis client for pub/sub, retrying..."
                    );
                    tokio::select! {
                        _ = stop_rx.changed() => break,
                        _ = shutdown_rx.changed() => break,
                        _ = tokio::time::sleep(PUBSUB_RECONNECT_DELAY) => continue,
                    }
                }
            };

            // Get async pub/sub connection
            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        channel = %channel,
                        "Failed to get pub/sub connection, retrying..."
                    );
                    tokio::select! {
                        _ = stop_rx.changed() => break,
                        _ = shutdown_rx.changed() => break,
                        _ = tokio::time::sleep(PUBSUB_RECONNECT_DELAY) => continue,
                    }
                }
            };

            // Subscribe to channel
            if let Err(e) = pubsub.subscribe(&channel).await {
                tracing::warn!(
                    error = %e,
                    channel = %channel,
                    "Failed to subscribe to channel, retrying..."
                );
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(PUBSUB_RECONNECT_DELAY) => continue,
                }
            }

            tracing::debug!(channel = %channel, "Redis pub/sub bridge connected");

            // Process messages
            let mut msg_stream = pubsub.on_message();
            loop {
                tokio::select! {
                    biased;

                    // Check for stop signal
                    _ = stop_rx.changed() => {
                        if *stop_rx.borrow() {
                            tracing::debug!(channel = %channel, "Bridge task stopping (explicit stop)");
                            break 'outer;
                        }
                    }

                    // Check for shutdown signal
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!(channel = %channel, "Bridge task stopping (shutdown)");
                            break 'outer;
                        }
                    }

                    // Process Redis message
                    msg_opt = msg_stream.next() => {
                        match msg_opt {
                            Some(msg) => {
                                let payload: Vec<u8> = match msg.get_payload() {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            channel = %channel,
                                            "Failed to get message payload"
                                        );
                                        continue;
                                    }
                                };

                                // Forward to local broadcast
                                // Ignore send errors (no receivers is fine for fire-and-forget)
                                let _ = bridge.send(payload);
                            }
                            None => {
                                // Stream ended (connection closed)
                                tracing::warn!(channel = %channel, "Redis pub/sub stream ended, reconnecting...");
                                break; // Break inner loop to reconnect
                            }
                        }
                    }
                }
            }

            // Reconnect after delay
            tokio::select! {
                _ = stop_rx.changed() => break,
                _ = shutdown_rx.changed() => break,
                _ = tokio::time::sleep(PUBSUB_RECONNECT_DELAY) => {}
            }
        }

        tracing::debug!(channel = %channel, "Redis pub/sub bridge stopped");
    }

    /// Graceful shutdown
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        self.pubsub_manager.shutdown().await;
    }
}

#[async_trait]
impl TopicBackend for RedisTopicBackend {
    // =========================================================================
    // Broadcast (Pub/Sub)
    // =========================================================================

    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), TopicError> {
        let channel = self.pubsub_channel(topic);
        let mut conn = self.pool.get().await?;

        // PUBLISH to Redis ONLY (not to local bridge)
        // Messages flow: Redis → Bridge Task → Local Broadcast → Subscribers
        // This eliminates duplicate messages for same-process pub/sub
        let _: i64 = deadpool_redis::redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(payload)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    async fn subscribe(&self, topic: &str) -> Result<BroadcastSubscription, TopicError> {
        // Get or create bridge
        let (bridge, is_new) = self.pubsub_manager.get_or_create_bridge(topic);

        // Start bridge task if this is a new bridge
        if is_new {
            self.start_bridge_task(topic);
        }

        // Increment subscriber count
        bridge.add_subscriber();

        // Get receiver from local broadcast
        let receiver = bridge.subscribe();

        // Create managed subscription (cleans up on drop)
        let managed = ManagedSubscription::new(
            receiver,
            Arc::clone(&bridge),
            Arc::clone(&self.pubsub_manager),
        );

        // Wrap in stream
        let stream = stream! {
            let mut managed = managed;
            loop {
                match managed.recv().await {
                    Ok(payload) => yield Ok(payload),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        yield Err(TopicError::Lagged(n));
                    }
                }
            }
        };

        Ok(BroadcastSubscription {
            receiver: Box::pin(stream),
        })
    }

    // =========================================================================
    // Stream
    // =========================================================================

    async fn stream_publish(&self, topic: &str, payload: &[u8]) -> Result<String, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        // XADD with MAXLEN trimming
        let id: String = deadpool_redis::redis::cmd("XADD")
            .arg(&key)
            .arg("MAXLEN")
            .arg("~")
            .arg(self.stream_maxlen)
            .arg("*")
            .arg("payload")
            .arg(payload)
            .query_async(&mut conn)
            .await?;

        Ok(id)
    }

    async fn stream_subscribe(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
    ) -> Result<StreamSubscription, TopicError> {
        // Ensure consumer group exists
        self.ensure_consumer_group(topic, group).await?;

        let key = self.stream_key(topic);
        let group = group.to_string();
        let consumer = consumer.to_string();
        let pool = self.pool.clone();

        let stream = stream! {
            loop {
                // Get connection from pool
                let mut conn = match pool.get().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to get Redis connection, retrying...");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };

                // XREADGROUP with block
                let result: RedisResult<RedisValue> = deadpool_redis::redis::cmd("XREADGROUP")
                    .arg("GROUP")
                    .arg(&group)
                    .arg(&consumer)
                    .arg("BLOCK")
                    .arg(XREADGROUP_BLOCK_MS)
                    .arg("COUNT")
                    .arg(256)
                    .arg("STREAMS")
                    .arg(&key)
                    .arg(">")  // Only new messages
                    .query_async(&mut conn)
                    .await;

                match result {
                    Ok(RedisValue::Nil) => {
                        // Timeout, no messages, continue
                        continue;
                    }
                    Ok(value) => {
                        // Parse response: [[stream_name, [[id, [field, value, ...]]]]]
                        if let Some(messages) = parse_xreadgroup_response(value) {
                            for msg in messages {
                                yield Ok(msg);
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("NOGROUP") {
                            // Consumer group was lost (e.g. stream key recreated).
                            // Re-create it starting from ID 0 to consume all pending.
                            tracing::warn!("Consumer group lost, recreating from start...");
                            if let Ok(mut conn) = pool.get().await {
                                let _: RedisResult<String> = deadpool_redis::redis::cmd("XGROUP")
                                    .arg("CREATE")
                                    .arg(&key)
                                    .arg(&group)
                                    .arg("0") // From beginning to consume pending
                                    .arg("MKSTREAM")
                                    .query_async(&mut conn)
                                    .await;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        } else {
                            tracing::warn!(error = %e, "XREADGROUP error, retrying...");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        };

        Ok(StreamSubscription {
            receiver: Box::pin(stream),
        })
    }

    async fn stream_ack(&self, topic: &str, group: &str, id: &str) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        let _: i64 = deadpool_redis::redis::cmd("XACK")
            .arg(&key)
            .arg(group)
            .arg(id)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    async fn stream_ack_batch(
        &self,
        topic: &str,
        group: &str,
        ids: &[String],
    ) -> Result<(), TopicError> {
        if ids.is_empty() {
            return Ok(());
        }
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        let mut cmd = deadpool_redis::redis::cmd("XACK");
        cmd.arg(&key).arg(group);
        for id in ids {
            cmd.arg(id.as_str());
        }
        let _: i64 = cmd.query_async(&mut conn).await?;

        Ok(())
    }

    async fn stream_claim(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        // First, get pending messages
        let pending: RedisValue = deadpool_redis::redis::cmd("XPENDING")
            .arg(&key)
            .arg(group)
            .arg("-")
            .arg("+")
            .arg(count)
            .query_async(&mut conn)
            .await?;

        // Parse pending to get IDs that are idle enough
        let mut ids_to_claim: Vec<String> = Vec::new();
        if let RedisValue::Array(entries) = pending {
            for entry in entries {
                // [id, consumer, idle_time, delivery_count]
                if let RedisValue::Array(parts) = entry
                    && parts.len() >= 3
                    && let (RedisValue::BulkString(id_bytes), _, RedisValue::Int(idle)) =
                        (&parts[0], &parts[1], &parts[2])
                    && *idle as u64 >= min_idle_ms
                    && let Ok(id) = String::from_utf8(id_bytes.clone())
                {
                    ids_to_claim.push(id);
                }
            }
        }

        if ids_to_claim.is_empty() {
            return Ok(vec![]);
        }

        // XCLAIM the messages
        let mut cmd = deadpool_redis::redis::cmd("XCLAIM");
        cmd.arg(&key).arg(group).arg(consumer).arg(min_idle_ms);

        for id in &ids_to_claim {
            cmd.arg(id);
        }

        let claimed: RedisValue = cmd.query_async(&mut conn).await?;

        // Parse claimed messages
        let mut messages = Vec::new();
        if let RedisValue::Array(entries) = claimed {
            for entry in entries {
                if let RedisValue::Array(parts) = entry
                    && parts.len() >= 2
                    && let (RedisValue::BulkString(id_bytes), RedisValue::Array(fields)) =
                        (&parts[0], &parts[1])
                    && let Ok(id) = String::from_utf8(id_bytes.clone())
                    && let Some(payload) = extract_payload_from_fields(fields)
                {
                    messages.push(StreamMessage { id, payload });
                }
            }
        }

        Ok(messages)
    }

    async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        // XLEN for stream length
        let length: u64 = deadpool_redis::redis::cmd("XLEN")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);

        // XPENDING summary for pending info
        let pending_info: RedisValue = deadpool_redis::redis::cmd("XPENDING")
            .arg(&key)
            .arg(group)
            .query_async(&mut conn)
            .await
            .unwrap_or(RedisValue::Nil);

        let mut pending = 0u64;
        let mut consumers = 0u64;
        let mut oldest_pending_ms = None;

        if let RedisValue::Array(parts) = pending_info
            && parts.len() >= 4
        {
            // [pending_count, smallest_id, largest_id, [[consumer, count], ...]]
            if let RedisValue::Int(p) = &parts[0] {
                pending = *p as u64;
            }
            if let RedisValue::Array(consumer_list) = &parts[3] {
                consumers = consumer_list.len() as u64;
            }
        }

        // Get oldest pending message age
        if pending > 0 {
            let pending_detail: RedisValue = deadpool_redis::redis::cmd("XPENDING")
                .arg(&key)
                .arg(group)
                .arg("-")
                .arg("+")
                .arg(1)
                .query_async(&mut conn)
                .await
                .unwrap_or(RedisValue::Nil);

            if let RedisValue::Array(entries) = pending_detail
                && let Some(RedisValue::Array(parts)) = entries.first()
                && parts.len() >= 3
                && let RedisValue::Int(idle) = &parts[2]
            {
                oldest_pending_ms = Some(*idle as u64);
            }
        }

        Ok(StreamStats {
            length,
            pending,
            consumers,
            oldest_pending_ms,
        })
    }

    // =========================================================================
    // Health
    // =========================================================================

    async fn health_check(&self) -> Result<(), TopicError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| TopicError::Connection(e.to_string()))?;

        deadpool_redis::redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| TopicError::Connection(e.to_string()))?;

        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "redis"
    }
}

/// Parse XREADGROUP response to extract messages
fn parse_xreadgroup_response(value: RedisValue) -> Option<Vec<StreamMessage>> {
    // Response format: [[stream_name, [[id, [field, value, ...]], ...]]]
    let streams = match value {
        RedisValue::Array(arr) => arr,
        _ => return None,
    };

    let mut messages = Vec::new();

    for stream_data in streams {
        let RedisValue::Array(parts) = stream_data else {
            continue;
        };
        if parts.len() < 2 {
            continue;
        }
        // parts[0] = stream name, parts[1] = messages array
        let RedisValue::Array(msg_list) = &parts[1] else {
            continue;
        };
        for msg in msg_list {
            if let RedisValue::Array(msg_parts) = msg
                && msg_parts.len() >= 2
                && let (RedisValue::BulkString(id_bytes), RedisValue::Array(fields)) =
                    (&msg_parts[0], &msg_parts[1])
                && let Ok(id) = String::from_utf8(id_bytes.clone())
                && let Some(payload) = extract_payload_from_fields(fields)
            {
                messages.push(StreamMessage { id, payload });
            }
        }
    }

    if messages.is_empty() {
        None
    } else {
        Some(messages)
    }
}

/// Extract payload field from Redis stream entry fields
fn extract_payload_from_fields(fields: &[RedisValue]) -> Option<Vec<u8>> {
    // Fields are [field1, value1, field2, value2, ...]
    let mut iter = fields.iter();
    while let Some(field) = iter.next() {
        if let RedisValue::BulkString(field_name) = field {
            if field_name == b"payload" {
                if let Some(RedisValue::BulkString(payload)) = iter.next() {
                    return Some(payload.clone());
                }
            } else {
                iter.next(); // Skip value
            }
        }
    }
    None
}

/// Sanitize Redis URL for logging (removes password)
fn sanitize_redis_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
        if let Some(colon_pos) = url[scheme_end..at_pos].find(':') {
            let abs_colon = scheme_end + colon_pos;
            let prefix = &url[..abs_colon + 1];
            let suffix = &url[at_pos..];
            return format!("{prefix}***{suffix}");
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_prefixes() {
        // Test key generation using constants directly
        let topic = "test";
        let stream_key = format!("{}{}", STREAM_PREFIX, topic);
        let pubsub_channel = format!("{}{}", PUBSUB_PREFIX, topic);

        assert_eq!(stream_key, "{sideseat}:stream:test");
        assert_eq!(pubsub_channel, "{sideseat}:pubsub:test");
    }

    #[test]
    fn test_sanitize_redis_url() {
        assert_eq!(
            sanitize_redis_url("redis://localhost:6379"),
            "redis://localhost:6379"
        );
        assert_eq!(
            sanitize_redis_url("redis://user:pass@localhost:6379"),
            "redis://user:***@localhost:6379"
        );
    }
}
