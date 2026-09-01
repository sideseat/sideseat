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

use std::collections::HashSet;
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

/// After how many deliveries a pending entry is *reported* as chronically failing.
///
/// A threshold on a retry counter is not evidence about the payload - it is evidence about the system
/// around it. Ten failed deliveries is what a minute of analytics downtime looks like, and the entry has
/// already been answered 200. So this number decides what gets *logged*, and never what gets discarded;
/// starvation is solved by how the pending list is scanned (see [`Self::scan_pending`]), which is the
/// mechanism that actually addresses it.
const CHRONIC_DELIVERY_REPORT: i64 = 10;

/// How long a publish waits for replica acknowledgement before reporting a shortfall.
///
/// Bounded, because an unbounded wait turns a lagging replica into a stalled ingestion path - and the
/// honest answer to "cannot confirm" is a 503 the exporter retries, not a request that never returns.
const REPLICA_ACK_TIMEOUT_MS: u64 = 5_000;

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
    /// How many replicas must confirm a queued entry before the publish returns.
    ///
    /// Zero means a single-instance Redis, where there is nothing to fail over to. Above zero, each publish
    /// costs a `WAIT` round trip - which is the price of an acknowledgement that survives a promotion.
    min_replica_acks: u32,
    /// Pub/Sub manager (handles bridge lifecycle)
    pubsub_manager: Arc<PubSubManager>,
}

impl RedisTopicBackend {
    /// Create a new Redis topic backend
    /// A backend on a standalone Redis, requiring no replica acknowledgement.
    #[cfg(test)]
    pub async fn new(redis_url: &str) -> Result<Self, TopicError> {
        Self::with_replica_acks(redis_url, 0).await
    }

    /// Create a backend that requires `min_replica_acks` replicas to confirm each queued entry.
    pub async fn with_replica_acks(
        redis_url: &str,
        min_replica_acks: u32,
    ) -> Result<Self, TopicError> {
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

        // A durable queue's promise rests on the server's persistence and eviction settings, not on the
        // wire protocol - `PING` says nothing about either. The two failure modes:
        //
        //   * AOF off or `appendfsync no`: a host failure loses whatever was in the fsync window, which
        //     may include entries an exporter was already told 200 for. `everysec` bounds the loss to
        //     one second, which is the minimum a durable queue can claim.
        //   * An LRU or LFU `maxmemory-policy` on a keyspace that includes streams: memory pressure
        //     silently evicts stream entries, or the stream key itself, before any consumer has read it.
        //     The queue effectively acknowledges storage it may later delete.
        //
        // Both refused at startup rather than warned about. The shipped defaults in the local
        // docker-compose (which advertises Valkey as durable) hit both - correct in production requires
        // an explicit choice, and getting it wrong means the queue lies about durability.
        probe_redis_durability(&mut conn).await.map_err(|e| {
            TopicError::Connection(format!(
                "Redis persistence probe failed for {sanitized_url}: {e}. Set `appendonly yes` \
                 with `appendfsync everysec` (or `always`) and a `maxmemory-policy` of `noeviction` \
                 or one of the `*-with-ttl` variants."
            ))
        })?;

        if min_replica_acks == 0
            && let Ok(replicas) = connected_replicas(&mut conn).await
            && replicas > 0
        {
            tracing::warn!(
                replicas,
                "Redis has replicas but no acknowledgement is required, so a failover can promote one \
                 that never received an acknowledged export. Set database.redis.min_replica_acks to close \
                 that window, at the cost of a WAIT round trip per publish."
            );
        }

        tracing::debug!(url = %sanitized_url, "Redis topic backend connected");

        Ok(Self {
            pool,
            redis_url: redis_url.to_string(),
            stream_max_backlog: std::sync::atomic::AtomicU64::new(DEFAULT_STREAM_MAX_BACKLOG),
            observed_backlog: Arc::new(dashmap::DashMap::new()),
            min_replica_acks,
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
            min_replica_acks: 0,
            pubsub_manager: Arc::new(PubSubManager::new(DEFAULT_BROADCAST_CAPACITY)),
        }
    }

    /// The Redis key holding a group's rotating scan cursor.
    ///
    /// **In Redis, not in this process.** As a process-local map the rotation was lost on every restart, so
    /// an ephemeral instance always resumed from the oldest pending entry: with a chronically-failing prefix
    /// and instances replaced faster than `pending / count` passes, entries behind that prefix starved
    /// indefinitely. Replicas also each kept their own cursor, so there was no global rotation guarantee at
    /// all - the property the whole mechanism exists to provide.
    ///
    /// Stored under the stream's hash tag so it lives on the same Redis Cluster slot as the stream itself. No
    /// compare-and-set: a lost update between two replicas only means one pass re-scans a page it already
    /// scanned, which is idempotent - claiming is, and so is processing.
    fn scan_cursor_key(&self, topic: &str, group: &str) -> String {
        format!("{}{}:scan_cursor:{}", STREAM_PREFIX, topic, group)
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

    /// Append an entry with an arbitrary field name, so a test can produce one whose payload is unreadable.
    #[cfg(test)]
    pub(super) async fn publish_raw_field_for_test(
        &self,
        topic: &str,
        field: &str,
        value: &[u8],
    ) -> Result<String, TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;
        let id: String = deadpool_redis::redis::cmd("XADD")
            .arg(&key)
            .arg("*")
            .arg(field)
            .arg(value)
            .query_async(&mut conn)
            .await?;
        Ok(id)
    }

    /// Read a group's scan cursor, for a test driving the compare-and-set directly.
    #[cfg(test)]
    pub(super) async fn read_scan_cursor_for_test(
        &self,
        topic: &str,
        group: &str,
    ) -> Result<Option<String>, TopicError> {
        let mut conn = self.pool.get().await?;
        Ok(deadpool_redis::redis::cmd("GET")
            .arg(self.scan_cursor_key(topic, group))
            .query_async::<Option<String>>(&mut conn)
            .await?)
    }

    /// Write a group's scan cursor through the same compare-and-set the recovery pass uses.
    #[cfg(test)]
    pub(super) async fn write_scan_cursor_for_test(
        &self,
        topic: &str,
        group: &str,
        expected: Option<String>,
        next: Option<&str>,
    ) -> Result<(), TopicError> {
        let mut conn = self.pool.get().await?;
        // A test-supplied `next` is a rotation *position*; the end is set past every plausible id so the
        // rotation covers whatever the test published.
        let next = next.and_then(StreamId::parse).map(|position| Rotation {
            position: Some(position),
            end: StreamId {
                millis: u64::MAX,
                sequence: 0,
            },
        });
        self.apply_cursor(
            &mut conn,
            CursorAction {
                key: self.scan_cursor_key(topic, group),
                expected,
                next,
            },
        )
        .await;
        Ok(())
    }

    /// Write a raw value straight into the cursor key, for a test staging a malformed one.
    #[cfg(test)]
    pub(super) async fn set_raw_scan_cursor_for_test(
        &self,
        topic: &str,
        group: &str,
        raw: &str,
    ) -> Result<(), TopicError> {
        let mut conn = self.pool.get().await?;
        deadpool_redis::redis::cmd("SET")
            .arg(self.scan_cursor_key(topic, group))
            .arg(raw)
            .query_async::<RedisValue>(&mut conn)
            .await?;
        Ok(())
    }

    /// Claim one specific pending id, bypassing the rotating scan - a test's stand-in for a peer consumer.
    #[cfg(test)]
    pub(super) async fn claim_specific_for_test(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
        id: &str,
    ) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;
        deadpool_redis::redis::cmd("XCLAIM")
            .arg(&key)
            .arg(group)
            .arg(consumer)
            .arg(0)
            .arg(id)
            .query_async::<RedisValue>(&mut conn)
            .await?;
        Ok(())
    }

    /// Everything currently on a topic's dead-letter stream, one debug rendering per entry.
    #[cfg(test)]
    pub(super) async fn dead_letter_entries_for_test(
        &self,
        topic: &str,
    ) -> Result<Vec<String>, TopicError> {
        let dead_key = format!("{}:dead", self.stream_key(topic));
        let mut conn = self.pool.get().await?;
        let entries: RedisValue = deadpool_redis::redis::cmd("XRANGE")
            .arg(&dead_key)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await?;
        let RedisValue::Array(entries) = entries else {
            return Ok(vec![]);
        };
        Ok(entries.iter().map(|e| format!("{e:?}")).collect())
    }

    /// The oldest entry this group still needs, or `None` when it cannot be determined.
    ///
    /// `None` means "do not trim": an unreadable answer is not evidence that a group is finished.
    ///
    /// # Ordering matters: `last-delivered-id` is read *first*
    ///
    /// The two reads are not atomic. If pending were read first (empty) and `last-delivered-id` second, a
    /// concurrent delivery M between them made `pending` empty while `last-delivered-id` had already
    /// advanced to M - so the boundary became `M.next()` and `XTRIM MINID` deleted M while it was still
    /// pending on some consumer. If that consumer died, `stream_claim` had nothing to hand over.
    ///
    /// Reading `last-delivered-id` first bounds the answer safely. Anything delivered after that read has
    /// an id strictly greater than the snapshot's `L`, so `L.next()` cannot exceed it and the concurrent
    /// entry is preserved. If a pending entry exists it is taken as the boundary instead: it was delivered
    /// at or before `L` (otherwise it would not be visible to XPENDING here), and it is what is still owed.
    /// Choose which pending entries a recovery pass should claim.
    ///
    /// # Why a retry counter was the wrong instrument
    ///
    /// Claiming an entry resets its idle time, so an entry whose processing keeps failing becomes eligible
    /// again after `min_idle_ms` and - being among the oldest - refills a window that starts at the oldest
    /// pending entry. With a window of `count` and `count` such entries, nothing behind them is ever
    /// examined, so an abandoned entry at position `count + 1` was never recovered at all. The first answer
    /// was to acknowledge an entry after ten deliveries, which is data loss: ten failures is what a minute of
    /// analytics downtime looks like, the payload was already answered 200, and a delivery counter says
    /// nothing about the payload - only about the system that kept failing to store it.
    ///
    /// # Rotation over a *generation*, which is what makes the bound real
    ///
    /// The mechanism is a cursor that sweeps the pending list and wraps. Getting the wrap condition right is
    /// the whole difficulty, and three earlier shapes failed:
    ///
    /// * Wrapping only on a **short page** never wraps at all when the list grows at the tail faster than the
    ///   sweep advances - so entries behind the cursor were never revisited.
    /// * **Holding** the cursor at an entry that was scanned but not claimed pins rotation: a peer that
    ///   repeatedly claims and abandons that entry keeps it perpetually too fresh, and everything after it
    ///   starves. Two holds merged by earliest only moves which entry does the pinning.
    /// * Filtering the scan by `IDLE` means a too-fresh entry is *invisible* to the page, so a cursor
    ///   position can be advanced past it - which is what made a hold seem necessary in the first place.
    ///
    /// So the cursor is paired with the id that ended the pending list **when this rotation began**, and the
    /// two are stored together. A rotation examines every entry that existed at its start, exactly once,
    /// advancing unconditionally; entries that arrive during it are picked up by the next rotation. The wrap
    /// is therefore driven by a fixed endpoint rather than by the list's current shape, so tail growth cannot
    /// prevent it and no entry can pin it. Nothing is held, so nothing can starve behind a hold.
    ///
    /// The page is **not** `IDLE`-filtered - eligibility is decided locally instead, and `XCLAIM` enforces it
    /// anyway. That keeps examination and claiming separate: the cursor advances over everything it looked at
    /// (which is what bounds the rotation), while only the entries idle enough are claimed. A pass that finds
    /// nothing eligible still makes rotation progress, which is the property the earlier shapes lacked.
    ///
    /// A chronically-failing entry is therefore retried once per rotation and reported loudly, and never
    /// dropped. Returns the ids to claim and the cursor move the caller commits **after** the claim - see
    /// [`CursorAction`].
    async fn scan_pending(
        &self,
        conn: &mut deadpool_redis::Connection,
        topic: &str,
        key: &str,
        group: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<(Vec<String>, CursorAction), TopicError> {
        // The cursor lives in Redis, so rotation survives a restart and replicas share one position - see
        // `scan_cursor_key`. An unreadable cursor is treated as absent: starting a fresh rotation is always
        // safe, it just costs this pass its place.
        let cursor_key = self.scan_cursor_key(topic, group);
        let observed: Option<String> = deadpool_redis::redis::cmd("GET")
            .arg(&cursor_key)
            .query_async::<Option<String>>(conn)
            .await
            .unwrap_or(None);

        // A rotation is `<position>|<end>`: where to resume, and the id that ended the list when it began.
        let rotation = observed.as_deref().and_then(Rotation::parse);
        let rotation = match rotation {
            Some(rotation) => rotation,
            // Start one. The end is the newest id currently pending; if nothing is pending there is no
            // rotation to run.
            None => match pending_summary_max(conn, key, group).await? {
                Some(end) => Rotation {
                    position: None,
                    end,
                },
                None => {
                    return Ok((
                        vec![],
                        CursorAction {
                            key: cursor_key,
                            expected: observed,
                            next: None,
                        },
                    ));
                }
            },
        };

        let start = rotation
            .position
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        // Bounded by the rotation's end, so a pass never looks past what this rotation promised to cover.
        let scanned = parse_pending_range(
            scan_pending_page(conn, key, group, &start, &rotation.end.to_string(), count).await?,
        );

        // Where the next pass resumes, and whether this rotation is finished.
        //
        // A short page means the rotation reached its end; a full page means resume past the last entry
        // examined. Either way the advance is unconditional - that is what keeps the bound real.
        let next = advance_rotation(
            rotation,
            scanned.len(),
            count,
            scanned.last().and_then(|(id, _, _)| StreamId::parse(id)),
        );

        for (id, _, deliveries) in &scanned {
            if *deliveries >= CHRONIC_DELIVERY_REPORT {
                tracing::error!(
                    stream = %key,
                    group,
                    message_id = %id,
                    deliveries,
                    "A queued payload has failed every delivery so far and keeps being retried once per \
                     rotation; it is kept, not discarded - investigate why processing fails for it"
                );
            }
        }

        // Eligibility is decided here, from the page's own idle values, so the cursor advances over
        // everything examined while only the entries idle enough are claimed. `XCLAIM` enforces the threshold
        // again server-side, so this is a filter for efficiency and the guarantee does not rest on it.
        let to_claim: Vec<String> = scanned
            .iter()
            .filter(|(_, idle, _)| *idle >= min_idle_ms)
            .map(|(id, _, _)| id.clone())
            .collect();

        Ok((
            to_claim,
            CursorAction {
                key: cursor_key,
                expected: observed,
                next,
            },
        ))
    }

    /// Move the rotating scan cursor as [`Self::scan_pending`] computed, once the claim it enabled succeeded.
    ///
    /// **Compare-and-set**, conditioned on the value the scan read - see [`CursorAction::expected`] for the
    /// interleaving that makes a plain `SET` skip pending entries. One Lua script, so the compare and the
    /// write are one atomic step; `WATCH`/`MULTI` would need a dedicated connection to be correct, and this
    /// runs on a pooled one.
    ///
    /// A refused or failed write is logged, not propagated: the entries were claimed and are being processed,
    /// and the only cost is that the next pass re-scans this page. Losing rotation progress is recoverable;
    /// failing the claim over it would not be.
    async fn apply_cursor(&self, conn: &mut deadpool_redis::Connection, action: CursorAction) {
        // Absence is its own argument, not an empty string.
        //
        // Overloading `''` to mean "the key was absent" made a key that genuinely holds `""` unreplaceable:
        // parsing treats it as malformed and starts a fresh rotation, but the CAS then demands the key be
        // absent, which it never is - so with more entries than one page holds, every pass rescanned the first
        // page and everything behind it starved. `ARGV[1]` is now an explicit flag.
        const CAS: &str = r#"
            local current = redis.call('GET', KEYS[1])
            local matches
            if ARGV[1] == '1' then
                matches = (current == false)
            else
                matches = (current == ARGV[2])
            end
            if not matches then
                return 0
            end
            if ARGV[3] == '' then
                redis.call('DEL', KEYS[1])
            else
                redis.call('SET', KEYS[1], ARGV[3])
            end
            return 1
        "#;
        let expect_absent = action.expected.is_none();
        let expected = action.expected.clone().unwrap_or_default();
        let next = action.next.map(|r| r.to_string()).unwrap_or_default();
        // Nothing to do: no cursor was there and none is wanted.
        if expect_absent && next.is_empty() {
            return;
        }
        // `EVAL` rather than the crate's `Script` helper, which is behind a feature this build does not
        // enable. Same atomicity - the server runs the script as one step.
        let applied: RedisResult<i64> = deadpool_redis::redis::cmd("EVAL")
            .arg(CAS)
            .arg(1)
            .arg(&action.key)
            .arg(if expect_absent { "1" } else { "0" })
            .arg(expected)
            .arg(next)
            .query_async(conn)
            .await;
        match applied {
            Ok(1) => {}
            Ok(_) => tracing::debug!(
                cursor = %action.key,
                "Another instance moved the pending-scan cursor first; this pass keeps its position and the \
                 next one re-scans its page"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                cursor = %action.key,
                "Could not persist the pending-scan cursor; the next recovery pass will re-scan this page"
            ),
        }
    }

    /// Preserve an entry that cannot be processed at all on a side stream, then let the caller acknowledge it.
    ///
    /// Only for an entry whose *payload cannot be read*, which is evidence about the entry itself - unlike a
    /// delivery count, which is evidence about the system. Nothing can ever process it, and leaving it
    /// pending holds the trim boundary forever, so it has to leave the main stream. But it was answered 200,
    /// so deleting it is not an option either: the bytes move to `<stream>:dead`, where an operator can
    /// inspect or replay them.
    ///
    /// The `XADD` happens before the caller's `XACK`, so an interruption between them leaves a copy on both
    /// streams rather than none. Ingestion is idempotent by span id, so a replayed duplicate rewrites.
    ///
    /// The dead-letter stream is deliberately **not** length-capped. A cap here would delete the very
    /// evidence the stream exists to keep, which is the failure this whole path was built to remove; it
    /// stays empty unless something is genuinely broken.
    async fn dead_letter(
        &self,
        conn: &mut deadpool_redis::Connection,
        key: &str,
        group: &str,
        id: &str,
        reason: &str,
        raw: &[u8],
    ) -> Result<(), TopicError> {
        let dead_key = format!("{key}:dead");
        let mut cmd = deadpool_redis::redis::cmd("XADD");
        cmd.arg(&dead_key)
            .arg("*")
            .arg("original_id")
            .arg(id)
            .arg("group")
            .arg(group)
            .arg("reason")
            .arg(reason)
            .arg("raw")
            .arg(raw);
        cmd.query_async::<RedisValue>(conn).await?;
        Ok(())
    }

    async fn oldest_needed_id(
        &self,
        conn: &mut deadpool_redis::Connection,
        key: &str,
        group: &str,
    ) -> Option<StreamId> {
        // The snapshot bound: everything the group is currently past is <= L. Nothing delivered after this
        // read can influence the boundary we return.
        let groups: RedisValue = deadpool_redis::redis::cmd("XINFO")
            .arg("GROUPS")
            .arg(key)
            .query_async(conn)
            .await
            .ok()?;
        let RedisValue::Array(groups) = &groups else {
            return None;
        };
        let entry = groups
            .iter()
            .find(|g| group_field(g, "name").as_deref() == Some(group))?;
        let last_delivered = StreamId::parse(&group_field(entry, "last-delivered-id")?)?;
        let snapshot_boundary = last_delivered.next();

        // Now the pending list. Any oldest-pending here was delivered at or before `last_delivered`, so if
        // it exists it is the tighter (lower) bound; concurrent deliveries after our snapshot are past
        // `snapshot_boundary` and stay preserved.
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
            && let Some(oldest) = StreamId::parse(&id)
        {
            // The min of the two: a concurrent delivery cannot make the boundary loosen, only tighten.
            return Some(oldest.min(snapshot_boundary));
        }
        Some(snapshot_boundary)
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

        // Fast path: this instance's own observation says the stream is at its limit.
        //
        // On its own that would strand a replica indefinitely - another instance can trim the stream and
        // this one would never learn, so its cache stays at the limit and every publish is refused
        // against an empty backlog. So a *reachable* limit here upgrades to a fresh `XLEN` before
        // refusing, which is one round trip on the refusal path only. The steady state (below the
        // limit) still pays no extra round trip: the previous publish's pipelined XLEN is what
        // populates `observed_backlog`, and reads there decide the fast path.
        if self.observed_backlog.get(&key).map(|o| *o).unwrap_or(0) >= self.max_backlog() {
            let mut conn = self.pool.get().await?;
            let fresh: u64 = deadpool_redis::redis::cmd("XLEN")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(u64::MAX);
            self.observed_backlog.insert(key.clone(), fresh);
            if fresh >= self.max_backlog() {
                return Err(TopicError::BufferFull);
            }
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

        // Record the backlog now, from the length the XADD already returned - before the durability wait,
        // which can fail. The entry is in the stream whether or not replicas confirm it (a failed wait is a
        // 503 the exporter retries, and the retry appends again; idempotency is at the span level, on the
        // analytics write). Recording it only after a successful wait meant a wait failure left the fast
        // path reading a stale, lower backlog, so the refusal threshold undercounted exactly when the
        // system was already struggling.
        let was_over = length >= self.max_backlog();
        self.observed_backlog.insert(key.clone(), length);

        // Durability across a failover, when the deployment has replicas to lose.
        //
        // `WAITAOF`, not `WAIT`. `WAIT` blocks until N replicas have the entry *in memory*, which is not the
        // guarantee being bought here: a replica that received the write but had not yet fsynced it, then
        // promoted after a crash, serves a keyspace without the entry - and the exporter was told 200 long
        // ago. `WAITAOF numlocal numreplicas` blocks until the append-only file has been fsynced locally and
        // on that many replicas, which is the actual "survives a failover" property. It requires
        // `appendonly yes`, which the startup durability probe already enforces.
        //
        // `numlocal = 1` is confirmed too, not assumed: `appendfsync always` makes it true, but confirming
        // it costs nothing on top of the round trip already being paid and turns a silent misconfiguration
        // into a refusal. A shortfall on either count is reported so the OTLP route answers 503 and the data
        // stays with the exporter.
        if self.min_replica_acks > 0 {
            let acked: Vec<i64> = deadpool_redis::redis::cmd("WAITAOF")
                .arg(1)
                .arg(self.min_replica_acks)
                .arg(REPLICA_ACK_TIMEOUT_MS)
                .query_async(&mut conn)
                .await
                .map_err(|e| {
                    TopicError::Stream(format!("WAITAOF for durable acknowledgement failed: {e}"))
                })?;
            // `[numlocal, numreplicas]`.
            let local = acked.first().copied().unwrap_or(0);
            let replicas = acked.get(1).copied().unwrap_or(0);
            if local < 1 || (replicas as u32) < self.min_replica_acks {
                return Err(TopicError::Stream(format!(
                    "a queued trace was fsynced locally={local} and on {replicas} of {} replicas within \
                     {}ms; refusing to report it as durably stored",
                    self.min_replica_acks, REPLICA_ACK_TIMEOUT_MS
                )));
            }
        }

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

    async fn stream_dead_letter(
        &self,
        topic: &str,
        group: &str,
        id: &str,
        reason: &str,
        payload: &[u8],
    ) -> Result<(), TopicError> {
        let key = self.stream_key(topic);
        let mut conn = self.pool.get().await?;
        self.dead_letter(&mut conn, &key, group, id, reason, payload)
            .await
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

        // Which pending entries this pass will claim, chosen by `scan_pending`: a rotating, bounded scan
        // that prefers the entries with the fewest deliveries. Nothing is discarded on the strength of a
        // retry counter - see [`CHRONIC_DELIVERY_REPORT`].
        let (ids_to_claim, cursor_action) = self
            .scan_pending(&mut conn, topic, &key, group, min_idle_ms, count)
            .await?;
        if ids_to_claim.is_empty() {
            // No claim to enable, so the rotation has done its reading; commit the (Reset) cursor.
            self.apply_cursor(&mut conn, cursor_action).await;
            return Ok(vec![]);
        }

        // XCLAIM the messages
        let mut cmd = deadpool_redis::redis::cmd("XCLAIM");
        cmd.arg(&key).arg(group).arg(consumer).arg(min_idle_ms);

        for id in &ids_to_claim {
            cmd.arg(id);
        }

        let claimed: RedisValue = cmd.query_async(&mut conn).await?;

        // Parse claimed messages. An entry whose payload cannot be read is *moved to the dead-letter
        // stream* and then acknowledged - never simply deleted.
        //
        // Skipping it silently left it pending forever, and a pending entry is what holds the trim boundary,
        // so one malformed entry stopped the stream from ever being trimmed and was re-examined by every
        // sweep. It has to leave the main stream; it was also answered 200, so its bytes have to survive.
        // `<stream>:dead` is where they go. Failing to preserve it means *not* acknowledging it: an entry
        // that could be neither processed nor kept stays pending, which is the conservative end.
        let mut messages = Vec::new();
        // Which requested ids this pass actually took, so the cursor cannot step over one it did not.
        //
        // `XCLAIM` succeeding does not mean it claimed everything asked for: it silently omits an entry
        // whose idle time no longer meets `min_idle_ms`, which a peer consumer resets simply by claiming it
        // first. That peer may then crash, leaving the entry abandoned - and if this pass had advanced past
        // it, a continuously-full pending list ahead of the cursor means it is never revisited.
        let mut handled: HashSet<String> = HashSet::new();
        if let RedisValue::Array(entries) = claimed {
            for entry in entries {
                let RedisValue::Array(parts) = entry else {
                    continue;
                };
                let id = parts.first().and_then(redis_string);
                let payload = match parts.get(1) {
                    Some(RedisValue::Array(fields)) => extract_payload_from_fields(fields),
                    _ => None,
                };
                match (id, payload) {
                    (Some(id), Some(payload)) => {
                        handled.insert(id.clone());
                        messages.push(StreamMessage { id, payload });
                    }
                    (Some(id), None) => {
                        let raw = format!("{:?}", parts.get(1));
                        match self
                            .dead_letter(
                                &mut conn,
                                &key,
                                group,
                                &id,
                                "unreadable_payload",
                                raw.as_bytes(),
                            )
                            .await
                        {
                            Ok(()) => {
                                tracing::error!(
                                    stream = %key,
                                    group,
                                    message_id = %id,
                                    "Moved a queued entry with no readable payload to the dead-letter stream; \
                                     leaving it pending would hold the stream's trim boundary forever"
                                );
                                // Handled only if the ACK *succeeded*. A dead-letter `XADD` that lands while
                                // the `XACK` fails leaves the entry still pending - still holding the trim
                                // boundary - so the cursor must not pass it; the next pass retries the ack.
                                // (A duplicate on the dead-letter stream is the deliberate trade: the bytes
                                // are preserved twice rather than lost, and ingestion is idempotent.)
                                match self.stream_ack(topic, group, &id).await {
                                    Ok(()) => {
                                        handled.insert(id.clone());
                                    }
                                    Err(e) => tracing::warn!(
                                        message_id = %id,
                                        error = %e,
                                        "Could not acknowledge a dead-lettered entry; it stays pending and the \
                                         scan cursor holds at it"
                                    ),
                                }
                            }
                            Err(e) => tracing::error!(
                                stream = %key,
                                group,
                                message_id = %id,
                                error = %e,
                                "Could not preserve an unreadable entry on the dead-letter stream; leaving it \
                                 pending rather than discarding it"
                            ),
                        }
                    }
                    // No id at all: nothing to acknowledge, and nothing that can be acted on.
                    (None, _) => tracing::warn!(
                        stream = %key,
                        group,
                        "A claimed entry had no readable id"
                    ),
                }
            }
        }

        // No per-entry hold, and none is needed: an entry this pass did not take is examined again by the
        // next rotation, which is bounded by a fixed endpoint. A hold was the previous design and it pinned
        // rotation - a peer repeatedly claiming and abandoning the held entry kept it perpetually too fresh,
        // starving everything after it. See `scan_pending`.
        let action = cursor_action;
        self.apply_cursor(&mut conn, action).await;

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
        //
        // Checked, not assumed: `probe_redis_durability` requires AOF with `appendfsync always` at startup,
        // and refuses an eviction policy that could delete an unread entry. `everysec` was accepted for a
        // while on the grounds that it is what production Redis runs - but it lets a 200 precede the fsync,
        // so a host failure loses up to a second of exports already reported as stored, and no amount of
        // documenting that makes the data durable. An operator who wants that throughput has the in-memory
        // backend, which writes inside the request rather than acknowledging early.
        //
        // One window remains and is not closed here: a failover can promote a replica that had not yet
        // received the entry. Closing it needs a per-publish `WAIT`/`WAITAOF` and the latency that implies,
        // and it is not something this probe can verify from a single connection.
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

/// How many replicas the server currently has, from `INFO replication`.
///
/// Read only to *warn*: an operator whose Redis has replicas and requires no acknowledgement has a gap that
/// is invisible until a failover, and a startup line is the last moment anyone is looking.
async fn connected_replicas(conn: &mut deadpool_redis::Connection) -> Result<u32, TopicError> {
    let info: String = deadpool_redis::redis::cmd("INFO")
        .arg("replication")
        .query_async(conn)
        .await
        .map_err(|e| TopicError::Connection(format!("INFO replication failed: {e}")))?;
    for line in info.lines() {
        if let Some(value) = line.trim().strip_prefix("connected_slaves:") {
            return Ok(value.trim().parse().unwrap_or(0));
        }
    }
    Ok(0)
}

/// Persistence and eviction settings that make the queue's durability promise honest.
///
/// Returns `Ok(())` when the server is configured for at least "one-second data loss on host failure"
/// (AOF on with `everysec` or `always`), and refuses when memory pressure could evict a queue entry.
///
/// # Why these are the right knobs to check, and not others
///
/// `PING` says the server is *reachable*, not that it is durable. The two properties we depend on:
///
/// * **AOF.** RDB alone is a periodic snapshot; a crash between snapshots loses everything since. AOF
///   `everysec` bounds that to one second, which is the minimum for a queue whose 200 has to mean
///   "durably stored". `always` is stricter.
/// * **Eviction.** A stream is a key, and an LRU/LFU policy over the whole keyspace can evict it under
///   memory pressure - which is exactly when a queue is most likely to be under pressure. `noeviction`
///   refuses new writes instead, which the OTLP path already handles as backpressure. The `-with-ttl`
///   variants confine eviction to keys with an explicit TTL, and streams here have none.
async fn probe_redis_durability(conn: &mut deadpool_redis::Connection) -> Result<(), TopicError> {
    async fn read_config(
        conn: &mut deadpool_redis::Connection,
        field: &'static str,
    ) -> Result<Option<String>, TopicError> {
        let value: Vec<String> = deadpool_redis::redis::cmd("CONFIG")
            .arg("GET")
            .arg(field)
            .query_async(conn)
            .await
            .map_err(|e| TopicError::Connection(format!("CONFIG GET {field} failed: {e}")))?;
        // Ignore the fetched key and read only the value slot. When the server refuses `CONFIG` (some
        // managed Redis deployments do), we get an empty reply - the caller then decides whether that
        // is fatal, which is a per-field question.
        Ok(value.get(1).cloned())
    }

    let appendonly = read_config(conn, "appendonly").await?;
    let appendfsync = read_config(conn, "appendfsync").await?;
    let policy = read_config(conn, "maxmemory-policy").await?;

    if appendonly.is_none() && appendfsync.is_none() && policy.is_none() {
        // The server refuses `CONFIG` entirely - a managed offering, typically. We cannot verify, so we
        // decline to *claim* durability rather than pretend to have checked. The stream backend still
        // works; `is_durable()` will read this decision.
        tracing::warn!(
            "Redis CONFIG is not readable; treating the backend as non-durable. Set the persistence \
             and eviction settings out-of-band, and pin them via your managed provider's controls."
        );
        return Err(TopicError::Connection(
            "cannot verify AOF / eviction settings via CONFIG GET".to_string(),
        ));
    }

    let ao = appendonly.unwrap_or_default().to_ascii_lowercase();
    if ao != "yes" {
        return Err(TopicError::Connection(format!(
            "appendonly is {ao:?}; AOF must be enabled for a durable queue"
        )));
    }
    // `always`, not `everysec`.
    //
    // The queue's whole purpose is that a 200 means the data is stored. With `everysec` a 200 can precede
    // the next fsync, so a host failure loses up to a second of *acknowledged* exports - and documenting
    // that window makes the loss honest without making the data durable, which is not the promise. An
    // operator who wants the throughput of `everysec` has the in-memory topic backend, where the request
    // writes to the analytics store before answering and nothing is acknowledged early.
    let fs = appendfsync.unwrap_or_default().to_ascii_lowercase();
    if fs != "always" {
        return Err(TopicError::Connection(format!(
            "appendfsync is {fs:?}; a queue that acknowledges before the write needs `always`. With \
             `everysec` a 200 can precede the fsync, so a host failure loses up to a second of exports \
             this server has already reported as stored. Use the default in-memory topic backend if you \
             prefer that throughput - it writes inside the request instead of acknowledging early."
        )));
    }

    let ev = policy.unwrap_or_default().to_ascii_lowercase();
    let safe = matches!(
        ev.as_str(),
        "noeviction" | "volatile-lru" | "volatile-lfu" | "volatile-random" | "volatile-ttl"
    );
    if !safe {
        return Err(TopicError::Connection(format!(
            "maxmemory-policy is {ev:?}; a keyspace-wide LRU/LFU can evict a stream entry that has \
             been answered 200. Use `noeviction` or one of the `volatile-*` variants."
        )));
    }
    Ok(())
}

/// A Redis stream id, ordered as Redis orders it rather than as a string.
///
/// Ids are `<millis>-<sequence>`, so a lexicographic comparison is wrong the moment the millisecond
/// component changes width: `"9-0"` sorts after `"10-0"` as text and before it as a stream id. The trim
/// boundary is a minimum over groups, so getting this backwards would delete entries a group still
/// needs.
/// What a recovery pass wants done to its rotating scan cursor, deferred until the claim it enables commits.
///
/// Held separately from the scan so the cursor advances only past entries an `XCLAIM` actually claimed - a
/// claim that errors leaves the cursor put, and the next pass re-scans rather than stepping over unclaimed
/// entries and leaving them stranded under sustained backlog growth.
struct CursorAction {
    /// The Redis key holding this group's cursor.
    key: String,
    /// What the scan read there, so the write can be conditional on nothing having changed since.
    ///
    /// A plain `SET` is **not** safe here, and "a lost update is only a re-scan" was wrong. The sequence:
    /// replica A claims a tail page and stalls before writing; replica B finds the list exhausted past its
    /// own position, wraps to the front, claims there and advances the cursor to the front; A's delayed write
    /// then jumps the cursor back out to its tail position, *past* the front entries B did not claim. With
    /// sustained traffic beyond A's position the scan never wraps again, and those entries starve - the exact
    /// failure the rotation exists to prevent. Conditioning the write on the value the scan read makes A's
    /// stale update a no-op instead: A loses its own progress, which costs one re-scan, and nothing is
    /// skipped.
    expected: Option<String>,
    /// The rotation to store, or `None` to clear it so the next pass starts a fresh one.
    next: Option<Rotation>,
}

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

/// A rotation over the pending list: where to resume, and the id that ended the list when it began.
///
/// The end is what makes the sweep bounded. Wrapping on a short page alone never wraps while the list grows
/// at the tail, and holding the cursor at an unclaimed entry lets a peer that repeatedly claims and abandons
/// it pin rotation forever. A fixed endpoint is immune to both: every entry present at the rotation's start is
/// examined exactly once, and entries that arrive during it belong to the next rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rotation {
    /// `None` means "from the front of the list".
    position: Option<StreamId>,
    end: StreamId,
}

impl Rotation {
    /// Parse the stored `<position>|<end>` form. `-` is the front.
    fn parse(raw: &str) -> Option<Self> {
        let (position, end) = raw.split_once('|')?;
        Some(Self {
            position: if position == "-" {
                None
            } else {
                Some(StreamId::parse(position)?)
            },
            end: StreamId::parse(end)?,
        })
    }
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(position) => write!(f, "{}|{}", position, self.end),
            None => write!(f, "-|{}", self.end),
        }
    }
}

/// Where a rotation resumes after examining a page, or `None` when it is complete.
///
/// Complete means the next pass starts a *fresh* rotation over the list as it then is. Two conditions end one,
/// and both are needed:
///
/// * A **short page** - fewer entries than asked for - means the range held nothing more.
/// * Passing the rotation's **end** means every entry present when it began has been examined. This is the
///   condition that makes the bound real: without it, a list growing at the tail keeps returning full pages
///   and the sweep never wraps, so entries behind the cursor are never revisited.
///
/// The advance is unconditional otherwise - it does not depend on whether an entry was claimable - because a
/// cursor that waits for an entry can be pinned there by a peer that repeatedly claims and abandons it, which
/// starves everything after it.
fn advance_rotation(
    rotation: Rotation,
    scanned: usize,
    count: usize,
    last_scanned: Option<StreamId>,
) -> Option<Rotation> {
    if scanned < count {
        return None;
    }
    let last = last_scanned?;
    // Compared *before* incrementing. `StreamId::next` saturates, so the maximum id
    // (`u64::MAX-u64::MAX`) increments to `u64::MAX-0`, which is *earlier* - and a rotation ending there
    // would then rescan its final entry forever, never revisiting the earlier failures behind it.
    // Comparing the id we actually examined has no such edge.
    if last >= rotation.end {
        return None;
    }
    Some(Rotation {
        position: Some(last.next()),
        end: rotation.end,
    })
}

/// The newest pending id for a group, which starts a rotation. `None` when nothing is pending.
///
/// From the `XPENDING key group` summary form, whose third element is the highest pending id.
async fn pending_summary_max(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    group: &str,
) -> Result<Option<StreamId>, TopicError> {
    // The error is **propagated**, not swallowed. Reading a failure as "nothing is pending" made an ACL
    // denial, a `NOGROUP`, or any persistent command failure look like a healthy idle queue - so recovery
    // silently stopped running while every pass reported success. A refusal is what surfaces it.
    let summary: RedisValue = deadpool_redis::redis::cmd("XPENDING")
        .arg(key)
        .arg(group)
        .query_async(conn)
        .await?;
    let RedisValue::Array(parts) = summary else {
        return Ok(None);
    };
    // [count, min_id, max_id, consumers]; a zero count leaves the ids nil.
    if let Some(RedisValue::Int(0)) = parts.first() {
        return Ok(None);
    }
    Ok(parts
        .get(2)
        .and_then(redis_string)
        .as_deref()
        .and_then(StreamId::parse))
}

/// One page of a group's pending list, over the inclusive id range `[from, to]`.
///
/// **Not** `IDLE`-filtered, deliberately. The filter makes a too-fresh entry invisible to the page, and a
/// cursor that advances over what it saw would then step past it - which is what made a per-entry hold seem
/// necessary, and a hold is what a peer repeatedly claiming and abandoning an entry can use to pin rotation.
/// Examining every entry in the range and deciding eligibility locally keeps the sweep's bound independent of
/// idle times; `XCLAIM` enforces `min_idle_ms` server-side regardless.
async fn scan_pending_page(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    group: &str,
    from: &str,
    to: &str,
    limit: usize,
) -> RedisResult<RedisValue> {
    deadpool_redis::redis::cmd("XPENDING")
        .arg(key)
        .arg(group)
        .arg(from)
        .arg(to)
        .arg(limit)
        .query_async(conn)
        .await
}

/// Read `XPENDING` range output into `(id, idle_ms, delivery count)`, in the order Redis returned them.
///
/// The idle time is carried rather than filtered on, so the caller can advance its cursor over everything
/// examined while claiming only what is eligible.
fn parse_pending_range(pending: RedisValue) -> Vec<(String, u64, i64)> {
    let RedisValue::Array(entries) = pending else {
        return vec![];
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        // [id, consumer, idle_time, delivery_count]
        let RedisValue::Array(parts) = entry else {
            continue;
        };
        if parts.len() < 3 {
            continue;
        }
        let Some(id) = redis_string(&parts[0]) else {
            continue;
        };
        let RedisValue::Int(idle) = &parts[2] else {
            continue;
        };
        let deliveries = match parts.get(3) {
            Some(RedisValue::Int(n)) => *n,
            _ => 1,
        };
        out.push((id, (*idle).max(0) as u64, deliveries));
    }
    out
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

#[cfg(test)]
mod rotation_tests {
    use super::{Rotation, StreamId};

    fn id(millis: u64, sequence: u64) -> StreamId {
        StreamId { millis, sequence }
    }

    /// A rotation round-trips through its stored form, front position included.
    ///
    /// The stored value carries two facts - where to resume and where this rotation ends - because a cursor
    /// alone cannot express a bounded sweep: wrapping on a short page never wraps while the list grows at the
    /// tail. A parse that silently dropped the end would restore the unbounded behaviour, so the encoding is
    /// pinned here.
    #[test]
    fn a_rotation_round_trips_through_its_stored_form() {
        for rotation in [
            Rotation {
                position: Some(id(50, 3)),
                end: id(900, 0),
            },
            Rotation {
                position: None,
                end: id(900, 0),
            },
        ] {
            let encoded = rotation.to_string();
            assert_eq!(
                Rotation::parse(&encoded),
                Some(rotation),
                "round trip failed for {encoded}"
            );
        }
    }

    /// The front of the list is `-`, distinct from any real id.
    #[test]
    fn the_front_position_is_encoded_as_a_dash() {
        let rotation = Rotation {
            position: None,
            end: id(7, 0),
        };
        assert_eq!(rotation.to_string(), "-|7-0");
    }

    /// A full page whose end passes the rotation's endpoint completes the rotation.
    ///
    /// This is the condition that makes the sweep bounded. Without it a list growing at the tail keeps
    /// returning full pages, the rotation never completes, and entries behind the cursor are never revisited -
    /// the original starvation. Verified by disabling the check and watching this fail.
    #[test]
    fn passing_the_endpoint_completes_the_rotation() {
        let rotation = Rotation {
            position: Some(id(10, 0)),
            end: id(100, 0),
        };
        // Full page ending exactly at the endpoint: the next position is past it, so the rotation is done.
        assert_eq!(
            super::advance_rotation(rotation, 5, 5, Some(id(100, 0))),
            None
        );
        // Full page ending before the endpoint: resume past what was examined.
        assert_eq!(
            super::advance_rotation(rotation, 5, 5, Some(id(50, 0))),
            Some(Rotation {
                position: Some(id(50, 1)),
                end: id(100, 0),
            })
        );
    }

    /// A short page completes the rotation: the range held nothing more.
    #[test]
    fn a_short_page_completes_the_rotation() {
        let rotation = Rotation {
            position: None,
            end: id(100, 0),
        };
        assert_eq!(
            super::advance_rotation(rotation, 2, 5, Some(id(30, 0))),
            None
        );
    }

    /// The advance does not depend on whether anything was claimable.
    ///
    /// A cursor that waits for an entry can be pinned there by a peer repeatedly claiming and abandoning it,
    /// starving everything after it - which is why eligibility is not an input here at all.
    #[test]
    fn the_advance_ignores_eligibility() {
        let rotation = Rotation {
            position: None,
            end: id(100, 0),
        };
        assert_eq!(
            super::advance_rotation(rotation, 5, 5, Some(id(40, 2))),
            Some(Rotation {
                position: Some(id(40, 3)),
                end: id(100, 0),
            })
        );
    }

    /// A value that is not a rotation is refused rather than half-read.
    ///
    /// An unparseable cursor makes the pass start a fresh rotation, which is safe. Accepting a partial parse
    /// would instead resume from a position with no endpoint, which is the unbounded sweep.
    #[test]
    fn a_malformed_rotation_is_refused() {
        for raw in [
            "",
            "50-0",
            "50-0|",
            "|900-0",
            "notanid|900-0",
            "50-0|notanid",
        ] {
            assert_eq!(Rotation::parse(raw), None, "{raw} should not parse");
        }
    }
}
