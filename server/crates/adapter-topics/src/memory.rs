//! In-memory topic backend.
//!
//! Provides local-only topic functionality:
//! - Broadcast: tokio::broadcast channels (fire-and-forget)
//! - Stream: VecDeque with pending tracking (simulated consumer groups)
//!
//! ## Limitations
//!
//! This backend is suitable for local development and single-process deployments:
//! - Process crash = all messages lost (no persistence)
//! - Single consumer group per process (no cross-process coordination)
//! - XCLAIM simulation exists but is limited (single process means no
//!   "other crashed consumers" to claim from in typical scenarios)
//!
//! For production durability and multi-machine deployments, use Redis backend.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use async_stream::stream;
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::{Notify, broadcast};

use sideseat_core::constants::{
    STREAM_ENTRY_OVERHEAD_BYTES, STREAM_MAX_CONSUMER_GROUPS, STREAM_MAX_REMEMBERED_CONSUMERS,
    STREAM_MAX_RETAINED_BYTES, STREAM_PENDING_RECORD_OVERHEAD_BYTES,
};
use sideseat_ports::queue::TopicError;
use sideseat_ports::queue::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};

/// Default broadcast channel capacity
const DEFAULT_BROADCAST_CAPACITY: usize = 10_000;

/// Message stored in memory stream
#[derive(Clone)]
struct StreamEntry {
    id: u64,
    partition: u32,
    payload: Vec<u8>,
    timestamp: Instant,
}

impl StreamEntry {
    /// What this entry costs against the budget: its payload plus the fixed per-entry overhead.
    ///
    /// One figure rather than two bounds, so a queue full of tiny entries is refused for the memory it
    /// actually occupies rather than admitted for the payload bytes it barely uses.
    fn budget_cost(payload_len: usize) -> u64 {
        payload_len as u64 + STREAM_ENTRY_OVERHEAD_BYTES
    }
}

/// Consumer group state for a stream
#[derive(Clone, Default)]
struct ConsumerGroup {
    /// The highest id this **group** has handed to any of its consumers.
    ///
    /// One cursor belongs to the group, matching Redis `last-delivered-id`. Per-consumer delivery state lives
    /// only in `pending`; otherwise acknowledged entries could be handed to another consumer again and there
    /// would be no group-wide consumed boundary.
    last_delivered_id: u64,
    /// Consumers seen in this group, and when each last took an entry. Kept for `StreamStats::consumers`.
    ///
    /// Bounded - see [`Self::remember_consumer`]. Unbounded it was the one place group state still grew without
    /// limit after the group cap: a client reconnecting under a fresh name each time adds an entry per reconnect,
    /// while the group count stays at one and every entry and pending record is reclaimed.
    consumers: HashMap<String, Instant>,
    /// Pending messages: message_id -> (consumer, delivery_time).
    ///
    /// Ordered by id, so a claim walks the oldest first and stops after the entries it takes, rather than
    /// collecting and sorting the whole backlog under the streams lock.
    pending: BTreeMap<u64, (String, Instant)>,
}

impl ConsumerGroup {
    /// Note this consumer as active, evicting the least recently active name if the map is full.
    ///
    /// Eviction rather than refusal, because the map is a statistic: nothing reads it to decide anything, so
    /// refusing a subscription to keep a number tidy would trade a capability for a stat. The least recently
    /// active name is the one "how many consumers are on this group" cares about least.
    fn remember_consumer(&mut self, consumer: &str) {
        let now = Instant::now();
        if !self.consumers.contains_key(consumer)
            && self.consumers.len() >= STREAM_MAX_REMEMBERED_CONSUMERS
            && let Some(stalest) = self
                .consumers
                .iter()
                .min_by_key(|(_, seen)| **seen)
                .map(|(name, _)| name.clone())
        {
            self.consumers.remove(&stalest);
        }
        self.consumers.insert(consumer.to_string(), now);
    }

    /// The oldest id this group still needs: its oldest pending entry, else one past what it has been handed.
    ///
    /// Everything below this has been delivered *and* acknowledged, which is the only definition of consumed
    /// that makes an entry safe to drop.
    fn oldest_needed(&self) -> u64 {
        match self.pending.keys().next() {
            Some(oldest_pending) => *oldest_pending,
            None => self.last_delivered_id.saturating_add(1),
        }
    }
}

/// Stream state
#[derive(Clone)]
struct StreamState {
    /// Messages in the stream
    messages: VecDeque<StreamEntry>,
    /// Consumer groups
    groups: HashMap<String, ConsumerGroup>,
    /// Next message ID
    next_id: u64,
    /// Bytes the entries above cost against `max_bytes`, maintained incrementally.
    ///
    /// Kept rather than summed on demand because `stream_publish` consults it on every call and the deque can
    /// hold six figures of entries; `retained_bytes_are_the_sum_of_the_entries` is what keeps the increment
    /// honest, since a counter maintained in two places is a counter that drifts.
    retained_bytes: u64,
    /// Budget for unconsumed entries. See [`STREAM_MAX_RETAINED_BYTES`].
    max_bytes: u64,
}

impl StreamState {
    fn with_budget(max_bytes: u64) -> Self {
        Self {
            messages: VecDeque::new(),
            groups: HashMap::new(),
            next_id: 1,
            retained_bytes: 0,
            max_bytes,
        }
    }
}

/// Shared state for memory backend
struct SharedState {
    /// Broadcast channels by topic name
    broadcast_channels: RwLock<HashMap<String, broadcast::Sender<Vec<u8>>>>,
    /// Stream state by topic name
    streams: RwLock<HashMap<String, StreamState>>,
    /// Per-stream notifiers for immediate subscriber wakeup (avoids polling)
    stream_notifiers: RwLock<HashMap<String, Arc<Notify>>>,
    /// Channel capacity for new broadcast topics
    broadcast_capacity: usize,
    /// Byte budget applied to each stream topic created from now on. See [`STREAM_MAX_RETAINED_BYTES`].
    stream_max_bytes: u64,
}

/// In-memory topic backend
pub struct MemoryTopicBackend {
    state: Arc<SharedState>,
}

impl Clone for MemoryTopicBackend {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl Default for MemoryTopicBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTopicBackend {
    /// Create a new in-memory topic backend
    pub fn new() -> Self {
        Self {
            state: Arc::new(SharedState {
                broadcast_channels: RwLock::new(HashMap::new()),
                streams: RwLock::new(HashMap::new()),
                stream_notifiers: RwLock::new(HashMap::new()),
                broadcast_capacity: DEFAULT_BROADCAST_CAPACITY,
                stream_max_bytes: STREAM_MAX_RETAINED_BYTES,
            }),
        }
    }

    /// Create with a smaller stream byte budget, so a test can fill the queue without allocating 128 MB.
    ///
    /// A test that has to publish the real budget to reach refusal is a test nobody runs, and a refusal path
    /// nobody runs is a refusal path that does not work. `#[cfg(test)]` rather than `#[allow(dead_code)]`
    /// because it is not a production knob: the budget is a constant, and an operator who needs to change it
    /// needs a configuration key rather than a constructor.
    #[cfg(test)]
    pub fn with_stream_budget(stream_max_bytes: u64) -> Self {
        Self {
            state: Arc::new(SharedState {
                broadcast_channels: RwLock::new(HashMap::new()),
                streams: RwLock::new(HashMap::new()),
                stream_notifiers: RwLock::new(HashMap::new()),
                broadcast_capacity: DEFAULT_BROADCAST_CAPACITY,
                stream_max_bytes,
            }),
        }
    }

    /// Get or create a broadcast channel
    fn get_or_create_broadcast(&self, topic: &str) -> broadcast::Sender<Vec<u8>> {
        let channels = self.state.broadcast_channels.read();
        if let Some(sender) = channels.get(topic) {
            return sender.clone();
        }
        drop(channels);

        let mut channels = self.state.broadcast_channels.write();
        // Double-check after acquiring write lock
        if let Some(sender) = channels.get(topic) {
            return sender.clone();
        }

        let (sender, _) = broadcast::channel(self.state.broadcast_capacity);
        channels.insert(topic.to_string(), sender.clone());
        sender
    }

    /// Drop the entries no consumer group still needs, and return how many went.
    ///
    /// This is the only path that removes stream entries. The boundary is the oldest entry any group still
    /// needs: its oldest pending entry, or one past its last delivered id. A stream with no consumer group is
    /// never trimmed because none of its entries has been consumed.
    fn trim_consumed(stream: &mut StreamState) -> u64 {
        if stream.groups.is_empty() {
            return 0;
        }

        // The lowest id any group is still owed. `min` across groups, because one lagging group holds the
        // boundary for all of them - trimming to a faster group's position would delete what the slow one has
        // not read.
        let boundary = stream
            .groups
            .values()
            .map(ConsumerGroup::oldest_needed)
            .min()
            .unwrap_or(0);

        let mut removed = 0u64;
        while let Some(entry) = stream.messages.front() {
            if entry.id >= boundary {
                break;
            }
            let cost = StreamEntry::budget_cost(entry.payload.len());
            stream.retained_bytes = stream.retained_bytes.saturating_sub(cost);
            stream.messages.pop_front();
            removed += 1;
        }
        removed
    }

    /// Get or create a Notify for a stream topic (for immediate subscriber wakeup)
    fn get_or_create_notifier(&self, topic: &str) -> Arc<Notify> {
        {
            let notifiers = self.state.stream_notifiers.read();
            if let Some(n) = notifiers.get(topic) {
                return Arc::clone(n);
            }
        }
        let mut notifiers = self.state.stream_notifiers.write();
        if let Some(n) = notifiers.get(topic) {
            return Arc::clone(n);
        }
        let n = Arc::new(Notify::new());
        notifiers.insert(topic.to_string(), Arc::clone(&n));
        n
    }
}

#[async_trait]
impl TopicBackend for MemoryTopicBackend {
    // =========================================================================
    // Broadcast
    // =========================================================================

    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), TopicError> {
        let sender = self.get_or_create_broadcast(topic);
        // Ignore send errors - means no active subscribers
        let _ = sender.send(payload.to_vec());
        Ok(())
    }

    async fn subscribe(&self, topic: &str) -> Result<BroadcastSubscription, TopicError> {
        let sender = self.get_or_create_broadcast(topic);
        let mut receiver = sender.subscribe();

        let stream = stream! {
            loop {
                match receiver.recv().await {
                    Ok(payload) => yield Ok(payload),
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
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

    async fn stream_publish(
        &self,
        topic: &str,
        partition_key: &str,
        payload: &[u8],
    ) -> Result<String, TopicError> {
        let id = {
            let mut streams = self.state.streams.write();
            let stream = streams
                .entry(topic.to_string())
                .or_insert_with(|| StreamState::with_budget(self.state.stream_max_bytes));

            // Reclaim first, then decide. Consumed entries are dead weight against the budget, so refusing
            // without collecting them would refuse a queue that is not actually full - and the collection is
            // cheap, since it pops a prefix rather than scanning.
            Self::trim_consumed(stream);

            // Refusal, not trimming. The bound is on bytes because that is the resource: a count bound admits
            // a thousand 64 MiB payloads and refuses a million small ones for no reason, and this queue's
            // entries are OTLP exports whose sizes span four orders of magnitude. `BufferFull` becomes a 503
            // with `Retry-After`, which leaves the data with the exporter that still has it - the one place it
            // is guaranteed to exist.
            let cost = StreamEntry::budget_cost(payload.len());
            // The **pending records too**, because the budget was multiplicative in consumer groups while only
            // counting each entry once. Every group holds its own pending record per delivered-and-unacked
            // entry, so ten thousand entries against a thousand abandoned groups is ten million records that
            // `retained_bytes` did not see at all: the queue sat comfortably inside 128 MB while holding
            // gigabytes. Retaining a group's unread entries is correct; leaving the group's own state out of
            // the bound is not.
            //
            // Summed rather than maintained incrementally: it is O(groups), and many groups is precisely the
            // case being bounded, so paying a per-group read there is the right trade against another counter
            // that can drift.
            let pending_cost = stream
                .groups
                .values()
                .map(|group| group.pending.len() as u64)
                .sum::<u64>()
                .saturating_mul(STREAM_PENDING_RECORD_OVERHEAD_BYTES);
            if stream.retained_bytes + pending_cost + cost > stream.max_bytes {
                // Not a warning about a full buffer: this is the queue holding the line, and the number is
                // what an operator needs to size the deployment or the consumer.
                // The oldest retained entry's age is in the message because it is what separates the two
                // causes: seconds means the consumer is merely behind, minutes means it is stuck, and those
                // call for different action.
                let oldest_age_ms = stream
                    .messages
                    .front()
                    .map(|entry| entry.timestamp.elapsed().as_millis() as u64);
                tracing::warn!(
                    topic,
                    retained_bytes = stream.retained_bytes,
                    pending_bytes = pending_cost,
                    groups = stream.groups.len(),
                    max_bytes = stream.max_bytes,
                    entries = stream.messages.len(),
                    payload_bytes = payload.len(),
                    oldest_retained_ms = ?oldest_age_ms,
                    "in-process queue is at its byte budget; refusing the publish so the caller keeps the data"
                );
                return Err(TopicError::BufferFull);
            }

            let id = stream.next_id;
            stream.next_id += 1;

            stream.retained_bytes += cost;
            stream.messages.push_back(StreamEntry {
                id,
                partition: crate::virtual_partition(partition_key),
                payload: payload.to_vec(),
                timestamp: Instant::now(),
            });

            id
        };

        // Wake all waiting subscribers (supports multi-consumer groups)
        self.get_or_create_notifier(topic).notify_waiters();

        Ok(id.to_string())
    }

    async fn stream_subscribe(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
    ) -> Result<StreamSubscription, TopicError> {
        // Ensure consumer group exists
        {
            let mut streams = self.state.streams.write();
            let stream = streams
                .entry(topic.to_string())
                .or_insert_with(|| StreamState::with_budget(self.state.stream_max_bytes));

            // The group count is bounded here, because the byte budget cannot bound it.
            //
            // Group state grows at *delivery*, and delivery has no useful refusal: declining to hand an entry to
            // a subscribed consumer stalls it. So charging pending records at publish - which this does - stops
            // a backlog being admitted while group state is already large, and does nothing about one retained
            // entry delivered to unboundedly many groups. A bound on the number of groups is the part that
            // closes it, and this is the one place a new group appears.
            //
            // Refused rather than silently accepted, because a stream with dozens of groups is a mistake in the
            // calling code and the message says so.
            if !stream.groups.contains_key(group)
                && stream.groups.len() >= STREAM_MAX_CONSUMER_GROUPS
            {
                tracing::error!(
                    topic,
                    group,
                    groups = stream.groups.len(),
                    max = STREAM_MAX_CONSUMER_GROUPS,
                    "refusing a new consumer group: each holds a pending record per unacknowledged entry, so \
                     an unbounded number of groups is unbounded memory the byte budget cannot see"
                );
                return Err(TopicError::ConsumerGroup(format!(
                    "stream {topic} already has {} consumer groups (max {STREAM_MAX_CONSUMER_GROUPS})",
                    stream.groups.len()
                )));
            }
            stream.groups.entry(group.to_string()).or_default();
        }

        let topic = topic.to_string();
        let group = group.to_string();
        let consumer = consumer.to_string();
        let state = Arc::clone(&self.state);
        let notifier = self.get_or_create_notifier(&topic);

        let stream = stream! {
            loop {
                // Check for new messages - scope the lock to avoid holding across await
                let (maybe_msg, stream_exists) = {
                    let mut streams = state.streams.write();
                    match streams.get_mut(&topic) {
                        None => (None, false),
                        Some(stream_state) => {
                            // `get_mut`, not `entry(..).or_default()`: creating a group here would bypass the
                            // cap that `stream_subscribe` enforces, so the one place a group appears stays the
                            // one place it is counted. The group exists because `stream_subscribe` created it
                            // before handing out this stream; if a `stream_trim_consumed` or a future path
                            // removed it, the subscription is over rather than silently re-registered.
                            let Some(cg) = stream_state.groups.get_mut(&group) else {
                                // Unreachable as things stand - `stream_subscribe` created this group and
                                // nothing removes one - but reported rather than silently ending the
                                // subscription, because a future path that did remove a group would otherwise
                                // present as a consumer that quietly stopped receiving.
                                tracing::error!(
                                    topic = %topic,
                                    group = %group,
                                    consumer = %consumer,
                                    "consumer group vanished; ending this subscription"
                                );
                                break;
                            };

                            // The next entry past the *group's* cursor. Read from the group rather than from a
                            // cursor local to this task, so two consumers of one group split the stream
                            // instead of both replaying whatever the other acknowledged - see
                            // `ConsumerGroup::last_delivered_id`.
                            let found = stream_state
                                .messages
                                .iter()
                                .find(|entry| entry.id > cg.last_delivered_id)
                                .map(|entry| {
                                    (entry.id, entry.partition, entry.payload.clone())
                                });

                            let msg = if let Some((id, partition, payload)) = found {
                                cg.pending.insert(id, (consumer.clone(), Instant::now()));
                                cg.remember_consumer(&consumer);
                                cg.last_delivered_id = id;
                                Some(StreamMessage {
                                    id: id.to_string(),
                                    partition,
                                    payload,
                                })
                            } else {
                                None
                            };
                            (msg, true)
                        }
                    }
                };

                if !stream_exists {
                    // Stream doesn't exist yet, wait for publish to create it
                    notifier.notified().await;
                    continue;
                }

                if let Some(msg) = maybe_msg {
                    yield Ok(msg);
                } else {
                    // Wait for notification of new message (no polling delay)
                    notifier.notified().await;
                }
            }
        };

        Ok(StreamSubscription {
            receiver: Box::pin(stream),
        })
    }

    async fn stream_ack(&self, topic: &str, group: &str, id: &str) -> Result<(), TopicError> {
        let id: u64 = id
            .parse()
            .map_err(|_| TopicError::Stream(format!("invalid message id: {}", id)))?;

        let mut streams = self.state.streams.write();
        let stream = streams
            .get_mut(topic)
            .ok_or_else(|| TopicError::Stream(format!("stream not found: {}", topic)))?;

        let cg = stream.groups.get_mut(group).ok_or_else(|| {
            TopicError::ConsumerGroup(format!("consumer group not found: {}", group))
        })?;

        cg.pending.remove(&id);
        Ok(())
    }

    async fn stream_ack_batch(
        &self,
        topic: &str,
        group: &str,
        ids: &[String],
    ) -> Result<(), TopicError> {
        let mut last_err = None;
        for id in ids {
            if let Err(e) = self.stream_ack(topic, group, id).await {
                tracing::warn!(error = %e, id, "Failed to ack message in batch, continuing");
                last_err = Some(e);
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn stream_claim(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        let mut streams = self.state.streams.write();
        let stream = match streams.get_mut(topic) {
            Some(s) => s,
            None => return Ok(vec![]),
        };

        let cg = match stream.groups.get_mut(group) {
            Some(g) => g,
            None => return Ok(vec![]),
        };

        let now = Instant::now();
        let min_idle = std::time::Duration::from_millis(min_idle_ms);
        let mut claimed = Vec::new();

        // The oldest idle messages first: in a hash map's order, which messages were claimed and the order they
        // were replayed in - and so written - differed from one process to the next.
        let idle_ids: Vec<u64> = cg
            .pending
            .iter()
            .filter(|(_, (_, delivery_time))| now.duration_since(*delivery_time) >= min_idle)
            .map(|(&id, _)| id)
            .take(count)
            .collect();

        for id in idle_ids {
            // Find the message payload
            if let Some(entry) = stream.messages.iter().find(|e| e.id == id) {
                // Update pending to new consumer
                cg.pending
                    .insert(id, (consumer.to_string(), Instant::now()));
                cg.remember_consumer(consumer);
                claimed.push(StreamMessage {
                    id: id.to_string(),
                    partition: entry.partition,
                    payload: entry.payload.clone(),
                });
            }
        }

        Ok(claimed)
    }

    async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        let streams = self.state.streams.read();
        let stream = match streams.get(topic) {
            Some(s) => s,
            None => return Ok(StreamStats::default()),
        };

        let cg = match stream.groups.get(group) {
            Some(g) => g,
            None => {
                return Ok(StreamStats {
                    length: stream.messages.len() as u64,
                    ..Default::default()
                });
            }
        };

        let now = Instant::now();
        let oldest_pending_ms = cg
            .pending
            .values()
            .map(|(_, delivery_time)| now.duration_since(*delivery_time).as_millis() as u64)
            .max();

        Ok(StreamStats {
            length: stream.messages.len() as u64,
            pending: cg.pending.len() as u64,
            consumers: cg.consumers.len() as u64,
            oldest_pending_ms,
        })
    }

    /// Drop what every consumer group has acknowledged, and say how many entries went.
    ///
    /// Implemented here rather than left to the trait's `Ok(0)` default because this backend now has a real
    /// answer: the same boundary the Redis adapter uses. Exposing it lets a caller reclaim on a schedule rather
    /// than only when a publish happens to arrive - which matters exactly when the queue is at its budget and
    /// publishes are being refused.
    async fn stream_trim_consumed(&self, topic: &str) -> Result<u64, TopicError> {
        let mut streams = self.state.streams.write();
        match streams.get_mut(topic) {
            Some(stream) => Ok(Self::trim_consumed(stream)),
            None => Ok(0),
        }
    }

    // =========================================================================
    // Health
    // =========================================================================

    async fn health_check(&self) -> Result<(), TopicError> {
        // In-memory backend is always healthy
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "memory"
    }

    fn is_durable(&self) -> bool {
        // Everything lives in an `Arc<SharedState>`: a crash takes the queue with it.
        false
    }
}
#[cfg(test)]
#[path = "memory_admission_tests.rs"]
mod admission_tests;
#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
