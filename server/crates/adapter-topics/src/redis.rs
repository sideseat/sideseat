//! Redis topic backend using Streams and Pub/Sub.
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

use deadpool_redis::redis::{RedisResult, Value as RedisValue};
use deadpool_redis::{Config, Pool, Runtime};
use futures::StreamExt;

use super::pubsub::{ManagedSubscription, PubSubManager};
use sideseat_ports::queue::TopicError;
use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};

mod backend;
mod configuration;
mod cursor;
mod protocol;

use configuration::{connected_replicas, probe_redis_durability, sanitize_redis_url};
use cursor::{
    CursorAction, Rotation, StreamId, advance_rotation, group_field, parse_pending_range,
    pending_summary_max, redis_string, scan_pending_page,
};
#[cfg(test)]
use protocol::extract_message_fields;

fn pool_error(error: deadpool_redis::PoolError) -> TopicError {
    TopicError::Connection(error.to_string())
}

fn redis_error(error: deadpool_redis::redis::RedisError) -> TopicError {
    TopicError::Stream(error.to_string())
}

/// Stream key prefix (hash tag for Redis Cluster)
const STREAM_PREFIX: &str = "{sideseat}:stream:";

/// Pub/Sub channel prefix
const PUBSUB_PREFIX: &str = "{sideseat}:pubsub:";

/// How large an *unprocessed* backlog a stream may hold before publishing is refused.
///
/// This is a backpressure threshold, not a trimming bound. Work beyond it is refused with
/// `BufferFull`; accepted entries remain until every consumer group has passed them.
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

    /// The Redis key holding a group's rotating scan cursor.
    ///
    /// Stored in Redis so restarts and multiple replicas share one rotation rather than repeatedly beginning
    /// at the oldest pending entry.
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;

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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        let entries: RedisValue = deadpool_redis::redis::cmd("XRANGE")
            .arg(&key)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        let id: String = deadpool_redis::redis::cmd("XADD")
            .arg(&key)
            .arg("*")
            .arg(field)
            .arg(value)
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;
        Ok(id)
    }

    /// Read a group's scan cursor, for a test driving the compare-and-set directly.
    #[cfg(test)]
    pub(super) async fn read_scan_cursor_for_test(
        &self,
        topic: &str,
        group: &str,
    ) -> Result<Option<String>, TopicError> {
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        deadpool_redis::redis::cmd("GET")
            .arg(self.scan_cursor_key(topic, group))
            .query_async::<Option<String>>(&mut conn)
            .await
            .map_err(redis_error)
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        deadpool_redis::redis::cmd("SET")
            .arg(self.scan_cursor_key(topic, group))
            .arg(raw)
            .query_async::<RedisValue>(&mut conn)
            .await
            .map_err(redis_error)?;
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
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        deadpool_redis::redis::cmd("XCLAIM")
            .arg(&key)
            .arg(group)
            .arg(consumer)
            .arg(0)
            .arg(id)
            .query_async::<RedisValue>(&mut conn)
            .await
            .map_err(redis_error)?;
        Ok(())
    }

    /// Everything currently on a topic's dead-letter stream, one debug rendering per entry.
    #[cfg(test)]
    pub(super) async fn dead_letter_entries_for_test(
        &self,
        topic: &str,
    ) -> Result<Vec<String>, TopicError> {
        let dead_key = format!("{}:dead", self.stream_key(topic));
        let mut conn = self.pool.get().await.map_err(pool_error)?;
        let entries: RedisValue = deadpool_redis::redis::cmd("XRANGE")
            .arg(&dead_key)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .map_err(redis_error)?;
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
    /// # Retry policy
    ///
    /// Claiming an entry resets its idle time, so an entry whose processing keeps failing becomes eligible
    /// again after `min_idle_ms` and - being among the oldest - refills a window that starts at the oldest
    /// pending entry. With a window of `count` and `count` such entries, nothing behind them is ever
    /// examined. A delivery count is diagnostic rather than a deletion condition: repeated failures can
    /// describe downstream downtime, and the payload has already been acknowledged to the exporter.
    ///
    /// # Rotation over a *generation*, which is what makes the bound real
    ///
    /// The cursor is paired with the id that ended the pending list **when this rotation began**, and the
    /// two are stored together. A rotation examines every entry that existed at its start, exactly once,
    /// advancing unconditionally; entries that arrive during it are picked up by the next rotation. The wrap
    /// is driven by a fixed endpoint rather than the list's current shape, so tail growth cannot prevent it and
    /// no entry can pin it.
    ///
    /// The page is **not** `IDLE`-filtered - eligibility is decided locally instead, and `XCLAIM` enforces it
    /// anyway. That keeps examination and claiming separate: the cursor advances over everything it looked at
    /// (which is what bounds the rotation), while only the entries idle enough are claimed. A pass that finds
    /// nothing eligible still makes rotation progress.
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
            scan_pending_page(conn, key, group, &start, &rotation.end.to_string(), count)
                .await
                .map_err(redis_error)?,
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
        // Absence is an explicit argument rather than an empty-string sentinel, because an existing key may
        // legitimately contain an empty value.
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
        cmd.query_async::<RedisValue>(conn)
            .await
            .map_err(redis_error)?;
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
                    changed = stop_rx.changed() => {
                        if changed.is_err() || *stop_rx.borrow() {
                            tracing::debug!(channel = %channel, "Bridge task stopping (explicit stop)");
                            break 'outer;
                        }
                    }

                    // Check for shutdown signal
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
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
    pub async fn shutdown(&self) {
        self.pubsub_manager.shutdown().await;
    }
}

#[cfg(test)]
#[path = "redis_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "redis/rotation_tests.rs"]
mod rotation_tests;
