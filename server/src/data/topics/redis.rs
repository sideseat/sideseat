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
use std::fmt;

use super::backend::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};
use super::error::TopicError;
use super::pubsub::{ManagedSubscription, PubSubManager};

/// Stream key prefix (hash tag for Redis Cluster)
const STREAM_PREFIX: &str = "{sideseat}:stream:";

/// Pub/Sub channel prefix
const PUBSUB_PREFIX: &str = "{sideseat}:pubsub:";

/// How large an *unprocessed* backlog a stream may hold before publishing is refused.
///
/// This is a backpressure threshold, not a trimming bound. The stream used to be published with
/// `XADD ... MAXLEN ~ 100000`, which deletes the oldest entries to keep the length down - and Redis
/// trims by length, with no idea whether an entry has been read. So a consumer outage, or any backlog
/// past the bound, silently destroyed payloads that HTTP and gRPC had already answered 200. The
/// promise that a 200 means "durably queued" was broken by the queue itself.
///
/// The bound now refuses new work instead of discarding accepted work: an exporter gets 503 with
/// `Retry-After` and keeps the data, which is exactly what an OTLP exporter is built to handle.
const DEFAULT_STREAM_MAX_BACKLOG: u64 = 100_000;

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
    /// How many unprocessed entries a stream may hold before publishing is refused.
    ///
    /// Atomic so a test can lower it; production never changes it after construction.
    stream_max_backlog: std::sync::atomic::AtomicU64,
    /// Last observed length per stream key, so the common publish costs no extra round trip.
    ///
    /// The length comes back from the same pipeline as the `XADD`, so a publish that pushes the stream
    /// over the threshold succeeds and the *next* one is refused. Overshoot is bounded by the number of
    /// publishes in flight, which is what makes a threshold affordable: asking Redis for the length
    /// before every append would double the round trips on the hot ingestion path to enforce a limit
    /// that is approximate by nature.
    observed_backlog: Arc<dashmap::DashMap<String, u64>>,
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
            stream_max_backlog: std::sync::atomic::AtomicU64::new(DEFAULT_STREAM_MAX_BACKLOG),
            observed_backlog: Arc::new(dashmap::DashMap::new()),
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
            stream_max_backlog: std::sync::atomic::AtomicU64::new(DEFAULT_STREAM_MAX_BACKLOG),
            observed_backlog: Arc::new(dashmap::DashMap::new()),
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

    /// Lower the refusal threshold, so a test can reach it in a few entries instead of a hundred
    /// thousand. Test-only: the threshold is otherwise a constant, deliberately not a knob.
    #[cfg(test)]
    pub(super) fn set_max_backlog_for_test(&self, limit: u64) {
        self.stream_max_backlog
            .store(limit, std::sync::atomic::Ordering::SeqCst);
    }

    fn max_backlog(&self) -> u64 {
        self.stream_max_backlog
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Every entry currently in the stream, oldest first. Test-only: production code reads through a
    /// consumer group, and a test asserting nothing was *deleted* has to look past the group.
    #[cfg(test)]
    pub(super) async fn read_all_for_test(
        &self,
        topic: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;
        let entries: RedisValue = deadpool_redis::redis::cmd("XRANGE")
            .arg(&key)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await?;
        let RedisValue::Array(entries) = entries else {
            return Ok(Vec::new());
        };
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let RedisValue::Array(parts) = entry else {
                    return None;
                };
                let id = redis_string(parts.first()?)?;
                let RedisValue::Array(fields) = parts.get(1)? else {
                    return None;
                };
                // `payload <bytes>`, the one field `stream_publish` writes.
                let RedisValue::BulkString(bytes) = fields.get(1)? else {
                    return None;
                };
                Some((id, bytes.clone()))
            })
            .collect())
    }

    /// Create a consumer group on a stream that may not exist yet. Test-only.
    #[cfg(test)]
    pub(super) async fn ensure_group_for_test(
        &self,
        topic: &str,
        group: &str,
    ) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;
        let _: RedisResult<String> = deadpool_redis::redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&key)
            .arg(group)
            .arg("0")
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await;
        Ok(())
    }

    /// The oldest entry this group still needs, or `None` when it cannot be determined.
    ///
    /// `None` means "do not trim": an unreadable answer is not evidence that a group is finished.
    async fn oldest_needed_id(
        &self,
        conn: &mut deadpool_redis::Connection,
        key: &str,
        group: &str,
    ) -> Option<StreamId> {
        // An entry delivered but not acknowledged is still owed, whoever holds it - including a consumer
        // that has since died, whose messages `stream_claim` will hand to someone else.
        let pending: RedisValue = deadpool_redis::redis::cmd("XPENDING")
            .arg(key)
            .arg(group)
            .arg("-")
            .arg("+")
            .arg(1)
            .query_async(conn)
            .await
            .ok()?;
        if let RedisValue::Array(entries) = &pending
            && let Some(RedisValue::Array(parts)) = entries.first()
            && let Some(id) = parts.first().and_then(redis_string)
        {
            return StreamId::parse(&id);
        }

        // Nothing pending, so everything delivered is acknowledged and the next id is the boundary.
        let groups: RedisValue = deadpool_redis::redis::cmd("XINFO")
            .arg("GROUPS")
            .arg(key)
            .query_async(conn)
            .await
            .ok()?;
        let RedisValue::Array(groups) = groups else {
            return None;
        };
        let entry = groups
            .iter()
            .find(|g| group_field(g, "name").as_deref() == Some(group))?;
        let last = group_field(entry, "last-delivered-id")?;
        StreamId::parse(&last).map(StreamId::next)
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

    /// Append to the stream, refusing rather than trimming when the backlog is too large.
    ///
    /// Deliberately no `MAXLEN`: trimming is by length and blind to what has been consumed, so the old
    /// form deleted entries that a consumer had never read and an exporter had already been told were
    /// stored. Entries are instead removed by `stream_trim_consumed` once every group is past them, and
    /// a backlog that outgrows [`DEFAULT_STREAM_MAX_BACKLOG`] turns into `BufferFull` - which the OTLP
    /// routes answer with 503 and `Retry-After`, leaving the data with the exporter that still has it.
    async fn stream_publish(&self, topic: &str, payload: &[u8]) -> Result<String, TopicError> {
        let key = self.stream_key(topic);

        if let Some(observed) = self.observed_backlog.get(&key)
            && *observed >= self.max_backlog()
        {
            return Err(TopicError::BufferFull);
        }

        let mut conn = self.pool.get().await?;

        // One round trip for both, so the threshold costs nothing in the steady state.
        let mut pipe = deadpool_redis::redis::pipe();
        pipe.cmd("XADD")
            .arg(&key)
            .arg("*")
            .arg("payload")
            .arg(payload)
            .cmd("XLEN")
            .arg(&key);
        let (id, length): (String, u64) = pipe.query_async(&mut conn).await?;

        let was_over = length >= self.max_backlog();
        self.observed_backlog.insert(key.clone(), length);
        if was_over {
            tracing::warn!(
                stream = %key,
                length,
                limit = self.max_backlog(),
                "Stream backlog is at its limit; further publishes are refused until consumers catch up"
            );
        }

        Ok(id)
    }

    /// Remove entries that every consumer group has finished with.
    ///
    /// The safe boundary is the oldest entry any group still needs: its oldest *pending* entry if it has
    /// one, otherwise one past its last delivered id. `XTRIM MINID` then removes strictly older entries,
    /// so nothing unread and nothing unacknowledged is ever deleted - which is the whole difference from
    /// the `MAXLEN` this replaces.
    ///
    /// A stream with no consumer group is left alone: nobody has read it yet, so every entry is still
    /// needed. Returning 0 there rather than trimming is the difference between an idle stream and an
    /// emptied one.
    async fn stream_trim_consumed(&self, topic: &str) -> Result<u64, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;

        let groups: RedisValue = deadpool_redis::redis::cmd("XINFO")
            .arg("GROUPS")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(RedisValue::Nil);
        let RedisValue::Array(groups) = groups else {
            return Ok(0);
        };
        if groups.is_empty() {
            return Ok(0);
        }

        let mut boundary: Option<StreamId> = None;
        for group in &groups {
            let Some(name) = group_field(group, "name") else {
                // A group whose name cannot be read is a group whose progress is unknown, and trimming
                // on incomplete information is how unread entries get deleted.
                return Ok(0);
            };
            let needed = match self.oldest_needed_id(&mut conn, &key, &name).await {
                Some(id) => id,
                None => return Ok(0),
            };
            boundary = Some(match boundary {
                Some(current) if current <= needed => current,
                _ => needed,
            });
        }

        let Some(boundary) = boundary else {
            return Ok(0);
        };
        let trimmed: u64 = deadpool_redis::redis::cmd("XTRIM")
            .arg(&key)
            .arg("MINID")
            .arg(boundary.to_string())
            .query_async(&mut conn)
            .await?;

        if trimmed > 0 {
            // The refusal threshold reads this, so a trim has to update it or publishing stays refused
            // until the next append observes the shorter stream.
            if let Some(mut observed) = self.observed_backlog.get_mut(&key) {
                *observed = observed.saturating_sub(trimmed);
            }
            tracing::debug!(stream = %key, trimmed, boundary = %boundary, "Trimmed consumed stream entries");
        }
        Ok(trimmed)
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
                                let result: RedisResult<String> = deadpool_redis::redis::cmd("XGROUP")
                                    .arg("CREATE")
                                    .arg(&key)
                                    .arg(&group)
                                    .arg("0") // From beginning to consume pending
                                    .arg("MKSTREAM")
                                    .query_async(&mut conn)
                                    .await;
                                if let Err(e) = result {
                                    tracing::error!(error = %e, "Failed to recreate consumer group");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
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

    fn is_durable(&self) -> bool {
        // A Redis stream holds the message until it is acknowledged, and an unacknowledged one is
        // reclaimed by another consumer - which is what makes acknowledging before writing honest.
        true
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

/// A Redis stream id, ordered as Redis orders it rather than as a string.
///
/// Ids are `<millis>-<sequence>`, so a lexicographic comparison is wrong the moment the millisecond
/// component changes width: `"9-0"` sorts after `"10-0"` as text and before it as a stream id. The trim
/// boundary is a minimum over groups, so getting this backwards would delete entries a group still
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StreamId {
    millis: u64,
    sequence: u64,
}

impl StreamId {
    fn parse(raw: &str) -> Option<Self> {
        let (millis, sequence) = raw.split_once('-')?;
        Some(Self {
            millis: millis.parse().ok()?,
            sequence: sequence.parse().ok()?,
        })
    }

    /// The next id after this one, which is the oldest entry still needed by a group that has
    /// acknowledged everything it was delivered.
    fn next(self) -> Self {
        match self.sequence.checked_add(1) {
            Some(sequence) => Self {
                millis: self.millis,
                sequence,
            },
            None => Self {
                millis: self.millis.saturating_add(1),
                sequence: 0,
            },
        }
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.millis, self.sequence)
    }
}

/// Read a named field out of one `XINFO GROUPS` entry, which is a flat key/value array.
fn group_field(group: &RedisValue, field: &str) -> Option<String> {
    let RedisValue::Array(pairs) = group else {
        return None;
    };
    let mut iter = pairs.chunks_exact(2);
    iter.find_map(|pair| {
        let key = redis_string(&pair[0])?;
        (key == field).then(|| redis_string(&pair[1]))?
    })
}

fn redis_string(value: &RedisValue) -> Option<String> {
    match value {
        RedisValue::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::Int(i) => Some(i.to_string()),
        _ => None,
    }
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

    /// Stream ids order numerically, not lexicographically.
    ///
    /// The trim boundary is a minimum over consumer groups, and `"9-0" < "10-0"` is false as text. A
    /// string comparison would therefore pick a boundary *later* than the oldest entry a group still
    /// needs and `XTRIM MINID` would delete unread work - the very failure the trim exists to avoid.
    #[test]
    fn stream_ids_order_numerically() {
        let nine = StreamId::parse("9-0").expect("parses");
        let ten = StreamId::parse("10-0").expect("parses");
        assert!(
            nine < ten,
            "9-0 must precede 10-0, however it sorts as text"
        );
        assert!(
            "9-0" > "10-0",
            "the lexicographic order really is the wrong one"
        );

        // The sequence orders within one millisecond.
        assert!(StreamId::parse("5-2").unwrap() < StreamId::parse("5-10").unwrap());

        // Round trips, because the boundary is sent back to Redis as a string.
        assert_eq!(
            StreamId::parse("1700000000000-3").unwrap().to_string(),
            "1700000000000-3"
        );
        assert!(StreamId::parse("not-an-id").is_none());
        assert!(StreamId::parse("12345").is_none());
    }

    /// The entry after the last delivered one is the oldest a caught-up group still needs.
    #[test]
    fn the_next_id_follows_its_own_id() {
        let id = StreamId::parse("100-4").unwrap();
        assert!(id < id.next());
        assert_eq!(id.next().to_string(), "100-5");
        // A saturated sequence rolls into the next millisecond rather than wrapping backwards.
        let saturated = StreamId {
            millis: 100,
            sequence: u64::MAX,
        };
        assert!(saturated < saturated.next());
    }

    /// `XINFO GROUPS` entries are flat key/value arrays, and the fields are read by name.
    #[test]
    fn group_fields_are_read_by_name() {
        let group = RedisValue::Array(vec![
            RedisValue::BulkString(b"name".to_vec()),
            RedisValue::BulkString(b"traces".to_vec()),
            RedisValue::BulkString(b"last-delivered-id".to_vec()),
            RedisValue::BulkString(b"42-1".to_vec()),
            RedisValue::BulkString(b"pending".to_vec()),
            RedisValue::Int(3),
        ]);
        assert_eq!(group_field(&group, "name").as_deref(), Some("traces"));
        assert_eq!(
            group_field(&group, "last-delivered-id").as_deref(),
            Some("42-1")
        );
        assert_eq!(group_field(&group, "pending").as_deref(), Some("3"));
        // An absent field is absent, not the next value along.
        assert_eq!(group_field(&group, "entries-read"), None);
        assert_eq!(group_field(&RedisValue::Nil, "name"), None);
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
