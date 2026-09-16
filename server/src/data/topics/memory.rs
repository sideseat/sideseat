//! In-memory topic backend
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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use async_stream::stream;
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::{Notify, broadcast};

use super::backend::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};
use crate::data::topics::TopicError;
use sideseat_core::core::constants::{
    STREAM_ENTRY_OVERHEAD_BYTES, STREAM_MAX_RETAINED_BYTES, STREAM_PENDING_RECORD_OVERHEAD_BYTES,
};

/// Default broadcast channel capacity
const DEFAULT_BROADCAST_CAPACITY: usize = 10_000;

/// Message stored in memory stream
#[derive(Clone)]
struct StreamEntry {
    id: u64,
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
    /// One cursor for the group, which is what a consumer group is - and what Redis's own
    /// `last-delivered-id` is. It used to be a cursor *per consumer*, and that had two consequences. It made
    /// "consumed" undefined, so nothing could decide which entries were safe to drop: a lagging consumer's
    /// cursor said entries were still owed while the group had already handed them out and been acknowledged
    /// for them. And it **delivered the same entry twice**: consumer A took entry 51 and acknowledged it,
    /// which removed the pending record, so consumer B - whose own cursor was still at 50 - found 51
    /// undelivered and processed it again. Ingestion is idempotent by span id, so that was bounded work rather
    /// than corruption, but it is not what a consumer group means.
    last_delivered_id: u64,
    /// Consumers seen in this group, and when each last took an entry. Kept for `StreamStats::consumers`.
    consumers: HashMap<String, Instant>,
    /// Pending messages: message_id -> (consumer, delivery_time)
    pending: HashMap<u64, (String, Instant)>,
}

impl ConsumerGroup {
    /// The oldest id this group still needs: its oldest pending entry, else one past what it has been handed.
    ///
    /// Everything below this has been delivered *and* acknowledged, which is the only definition of consumed
    /// that makes an entry safe to drop.
    fn oldest_needed(&self) -> u64 {
        match self.pending.keys().min() {
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

    /// Create with custom broadcast capacity
    #[allow(dead_code)]
    pub fn with_broadcast_capacity(capacity: usize) -> Self {
        Self {
            state: Arc::new(SharedState {
                broadcast_channels: RwLock::new(HashMap::new()),
                streams: RwLock::new(HashMap::new()),
                stream_notifiers: RwLock::new(HashMap::new()),
                broadcast_capacity: capacity,
                stream_max_bytes: STREAM_MAX_RETAINED_BYTES,
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
    /// **This is the only way an entry leaves the stream**, and that is the whole point. What used to be here
    /// trimmed by *length*: it popped the front until the deque was under a count bound and removed the
    /// popped entry's pending record from every group - so a message that had been delivered and not yet
    /// acknowledged was deleted, and the group's own record that it owed work went with it. Every one of those
    /// entries had already been answered 200. That is the `MAXLEN` defect this repository removed from the
    /// Redis backend, and it was still live here: a queue that discards accepted work is worse than no queue,
    /// because the loss is silent and the exporter has already moved on.
    ///
    /// The boundary is the oldest entry any group still needs - its oldest pending entry if it has one, else
    /// one past its last delivered id - which is exactly `stream_trim_consumed`'s rule on the Redis side. A
    /// stream with **no** consumer group is never trimmed: nobody has read it, so everything is still needed.
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
        // Ignored, and that is correct rather than unfinished: this backend is one process with one queue per
        // topic, so there is nothing to partition and ordering is already total. It is in the signature so every
        // caller states its key now, and a partitioned adapter needs no caller changes.
        _partition_key: &str,
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
                            let cg = stream_state.groups.entry(group.clone()).or_default();

                            // The next entry past the *group's* cursor. Read from the group rather than from a
                            // cursor local to this task, so two consumers of one group split the stream
                            // instead of both replaying whatever the other acknowledged - see
                            // `ConsumerGroup::last_delivered_id`.
                            let found = stream_state
                                .messages
                                .iter()
                                .find(|entry| entry.id > cg.last_delivered_id)
                                .map(|entry| (entry.id, entry.payload.clone()));

                            let msg = if let Some((id, payload)) = found {
                                cg.pending.insert(id, (consumer.clone(), Instant::now()));
                                cg.consumers.insert(consumer.clone(), Instant::now());
                                cg.last_delivered_id = id;
                                Some(StreamMessage {
                                    id: id.to_string(),
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

        // Find pending messages that are idle
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
                cg.consumers.insert(consumer.to_string(), Instant::now());
                claimed.push(StreamMessage {
                    id: id.to_string(),
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

/// A lagging broadcast receiver, as the queue port's error.
///
/// Beside this backend rather than beside `TopicError`, for the reason the Redis conversions moved: `tokio`'s
/// broadcast channel is *this* implementation's transport, and a port that names it depends on it.
impl From<tokio::sync::broadcast::error::RecvError> for TopicError {
    fn from(err: tokio::sync::broadcast::error::RecvError) -> Self {
        match err {
            tokio::sync::broadcast::error::RecvError::Closed => TopicError::ChannelClosed,
            tokio::sync::broadcast::error::RecvError::Lagged(n) => TopicError::Lagged(n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_broadcast_publish_subscribe() {
        let backend = MemoryTopicBackend::new();

        // Subscribe first
        let sub = backend.subscribe("test").await.unwrap();
        let mut receiver = sub.receiver;

        // Publish
        backend.publish("test", b"hello").await.unwrap();

        // Receive with timeout
        let msg = tokio::time::timeout(tokio::time::Duration::from_millis(100), receiver.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert_eq!(msg, b"hello");
    }

    #[tokio::test]
    async fn test_stream_publish_subscribe_ack() {
        let backend = MemoryTopicBackend::new();

        // Publish first
        let id = backend
            .stream_publish("stream", "test-key", b"msg1")
            .await
            .unwrap();
        assert_eq!(id, "1");

        // Subscribe
        let sub = backend
            .stream_subscribe("stream", "group1", "consumer1")
            .await
            .unwrap();
        let mut receiver = sub.receiver;

        // Receive
        let msg = tokio::time::timeout(tokio::time::Duration::from_millis(500), receiver.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert_eq!(msg.id, "1");
        assert_eq!(msg.payload, b"msg1");

        // Ack
        backend
            .stream_ack("stream", "group1", &msg.id)
            .await
            .unwrap();

        // Check stats
        let stats = backend.stream_stats("stream", "group1").await.unwrap();
        assert_eq!(stats.length, 1);
        assert_eq!(stats.pending, 0);
    }

    #[tokio::test]
    async fn test_stream_stats() {
        let backend = MemoryTopicBackend::new();

        // Publish messages
        backend
            .stream_publish("stream", "test-key", b"msg1")
            .await
            .unwrap();
        backend
            .stream_publish("stream", "test-key", b"msg2")
            .await
            .unwrap();

        let stats = backend.stream_stats("stream", "group1").await.unwrap();
        assert_eq!(stats.length, 2);
        assert_eq!(stats.pending, 0);
    }

    #[test]
    fn test_backend_name() {
        let backend = MemoryTopicBackend::new();
        assert_eq!(backend.backend_name(), "memory");
    }
}

/// The admission budget and the consumed-only trim.
///
/// These exist because the previous bound was on *length* and enforced by deletion: it popped the oldest
/// entries until the deque fit a count, and removed their pending records from every group as it went. Every
/// one of those entries had already been answered 200 by HTTP or gRPC, so the queue was discarding accepted
/// work - the `MAXLEN` defect this repository removed from the Redis backend, still live in the default one.
///
/// Each test below fails if the trim goes back to being length-driven, which is what makes them a gate rather
/// than a description.
#[cfg(test)]
mod admission_tests {
    use super::*;
    use futures::StreamExt;

    /// The ids currently retained, in order. Read from the state rather than through a subscription, because
    /// the property under test is what the queue *kept*, not what a consumer managed to see.
    fn retained_ids(backend: &MemoryTopicBackend, topic: &str) -> Vec<u64> {
        let streams = backend.state.streams.read();
        streams
            .get(topic)
            .map(|s| s.messages.iter().map(|e| e.id).collect())
            .unwrap_or_default()
    }

    fn retained_bytes(backend: &MemoryTopicBackend, topic: &str) -> u64 {
        let streams = backend.state.streams.read();
        streams.get(topic).map_or(0, |s| s.retained_bytes)
    }

    /// A full queue refuses the next publish and keeps everything it already accepted.
    ///
    /// The two halves are one assertion: refusing is only correct *because* nothing was dropped to make room,
    /// and a bound that trims would satisfy the first half while failing the second silently.
    #[tokio::test]
    async fn a_full_queue_refuses_and_loses_nothing() {
        // Room for exactly three entries of this size, so the fourth has to be refused.
        let payload = vec![b'x'; 1024];
        let budget = StreamEntry::budget_cost(payload.len()) * 3;
        let backend = MemoryTopicBackend::with_stream_budget(budget);

        let mut accepted = Vec::new();
        for _ in 0..3 {
            let id = backend
                .stream_publish("t", "k", &payload)
                .await
                .expect("within budget");
            accepted.push(id.parse::<u64>().expect("numeric id"));
        }

        let refused = backend.stream_publish("t", "k", &payload).await;
        assert!(
            matches!(refused, Err(TopicError::BufferFull)),
            "a publish past the budget must be refused, not made room for; got {refused:?}"
        );

        assert_eq!(
            retained_ids(&backend, "t"),
            accepted,
            "every accepted entry is still here - nothing was deleted to admit anything"
        );
    }

    /// A delivered-but-unacknowledged entry survives a full queue.
    ///
    /// This is the exact shape of the old defect: the entry the consumer is holding is the *oldest*, so a
    /// length-driven trim takes it first, and takes the group's record that it owed the work with it.
    #[tokio::test]
    async fn an_unacknowledged_entry_is_never_dropped_to_make_room() {
        let payload = vec![b'y'; 512];
        // Two entries plus the one pending record the delivery below creates. The pending charge is part of the
        // budget - a group holds one record per delivered-and-unacked entry, and leaving that out of the bound
        // made it multiplicative in groups - so a budget sized for entries alone would refuse the second
        // publish and this test would pass for the wrong reason.
        let budget =
            StreamEntry::budget_cost(payload.len()) * 2 + STREAM_PENDING_RECORD_OVERHEAD_BYTES;
        let backend = MemoryTopicBackend::with_stream_budget(budget);

        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("first");

        // Take it, and do not acknowledge it.
        let sub = backend
            .stream_subscribe("t", "g", "c")
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        let held = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
            .await
            .expect("delivered")
            .expect("some")
            .expect("ok");
        assert_eq!(held.id, "1");

        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("second fits");
        let refused = backend.stream_publish("t", "k", &payload).await;
        assert!(
            matches!(refused, Err(TopicError::BufferFull)),
            "the queue is full of work that is still owed, so the publish is refused"
        );

        assert!(
            retained_ids(&backend, "t").contains(&1),
            "the entry the consumer is holding must still exist"
        );
        let streams = backend.state.streams.read();
        assert!(
            streams["t"].groups["g"].pending.contains_key(&1),
            "and the group must still record that it owes the work"
        );
    }

    /// Acknowledging frees the budget, so a refusal is transient rather than terminal.
    ///
    /// Without this the refusal would be a deadlock: the queue fills once and never accepts again.
    #[tokio::test]
    async fn acknowledging_frees_the_budget() {
        let payload = vec![b'z'; 256];
        let budget = StreamEntry::budget_cost(payload.len()) * 2;
        let backend = MemoryTopicBackend::with_stream_budget(budget);

        for _ in 0..2 {
            backend
                .stream_publish("t", "k", &payload)
                .await
                .expect("fits");
        }
        assert!(matches!(
            backend.stream_publish("t", "k", &payload).await,
            Err(TopicError::BufferFull)
        ));

        let sub = backend
            .stream_subscribe("t", "g", "c")
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        for expected in ["1", "2"] {
            let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
                .await
                .expect("delivered")
                .expect("some")
                .expect("ok");
            assert_eq!(msg.id, expected);
            backend.stream_ack("t", "g", &msg.id).await.expect("ack");
        }

        // The next publish reclaims what was acknowledged and is admitted.
        let id = backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("the budget freed up once the work was acknowledged");
        assert_eq!(id, "3");
        assert_eq!(
            retained_ids(&backend, "t"),
            vec![3],
            "the two acknowledged entries were reclaimed and only the new one is retained"
        );
    }

    /// Nothing is trimmed while no consumer group exists.
    ///
    /// Nobody has read the stream, so every entry is still needed - and a publisher that outruns a consumer
    /// that has not arrived yet is told to wait rather than having its backlog quietly deleted.
    #[tokio::test]
    async fn a_stream_with_no_consumer_group_is_never_trimmed() {
        let payload = vec![b'w'; 128];
        let budget = StreamEntry::budget_cost(payload.len()) * 2;
        let backend = MemoryTopicBackend::with_stream_budget(budget);

        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("first");
        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("second");
        assert!(matches!(
            backend.stream_publish("t", "k", &payload).await,
            Err(TopicError::BufferFull)
        ));
        assert_eq!(retained_ids(&backend, "t"), vec![1, 2]);
        assert_eq!(
            backend.stream_trim_consumed("t").await.expect("trim"),
            0,
            "an unread stream has nothing consumed to reclaim"
        );
    }

    /// Two consumers of one group split the stream instead of both replaying it.
    ///
    /// With a cursor per consumer, the second consumer's cursor sat behind the first's, so an entry the first
    /// took *and acknowledged* looked undelivered to the second and was processed twice. Ingestion is
    /// idempotent by span id, so the cost was duplicated work rather than corruption - and it is still not
    /// what a consumer group means, and it is what made "consumed" undefinable.
    #[tokio::test]
    async fn two_consumers_of_one_group_split_the_stream() {
        let backend = MemoryTopicBackend::new();
        for n in 0..4u8 {
            backend
                .stream_publish("t", "k", &[n])
                .await
                .expect("publish");
        }

        let mut seen: Vec<String> = Vec::new();
        for consumer in ["c1", "c2"] {
            let sub = backend
                .stream_subscribe("t", "g", consumer)
                .await
                .expect("subscribe");
            let mut receiver = sub.receiver;
            for _ in 0..2 {
                let msg =
                    tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
                        .await
                        .expect("delivered")
                        .expect("some")
                        .expect("ok");
                backend.stream_ack("t", "g", &msg.id).await.expect("ack");
                seen.push(msg.id);
            }
        }

        seen.sort();
        assert_eq!(
            seen,
            vec!["1", "2", "3", "4"],
            "each entry goes to exactly one consumer of the group"
        );
    }

    /// Many consumer groups holding the same entries are charged for their own state.
    ///
    /// The bound counted each entry once, so it was blind to a cost that *multiplies*: every group holds a
    /// pending record per delivered-and-unacknowledged entry. Ten thousand entries against a thousand abandoned
    /// groups is ten million records the budget did not see, and the queue reported itself comfortably inside
    /// 128 MB while holding gigabytes. Retaining a group's unread entries is correct; omitting the group's own
    /// state from the bound is not.
    #[tokio::test]
    async fn group_state_is_charged_against_the_budget() {
        let payload = vec![b'g'; 64];
        // Room for two entries and nothing else, so the pending records are what tips it over.
        let budget = StreamEntry::budget_cost(payload.len()) * 2;
        let backend = MemoryTopicBackend::with_stream_budget(budget);

        backend
            .stream_publish("t", "k", &payload)
            .await
            .expect("first");

        // Several groups take it and none acknowledges. Each holds its own pending record.
        let mut receivers = Vec::new();
        for group in ["g1", "g2", "g3", "g4"] {
            let sub = backend
                .stream_subscribe("t", group, "c")
                .await
                .expect("subscribe");
            let mut receiver = sub.receiver;
            let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
                .await
                .expect("delivered")
                .expect("some")
                .expect("ok");
            assert_eq!(msg.id, "1");
            receivers.push(receiver);
        }

        let refused = backend.stream_publish("t", "k", &payload).await;
        assert!(
            matches!(refused, Err(TopicError::BufferFull)),
            "four groups each holding a pending record cost real memory the budget has to see; got {refused:?}"
        );
    }

    /// The incremental byte counter equals the entries it claims to describe.
    ///
    /// The counter is maintained in two places - incremented on publish, decremented on trim - and a counter
    /// maintained in two places is a counter that drifts. A drift downward would let the queue admit past its
    /// budget; upward, it would refuse a queue that is nearly empty.
    #[tokio::test]
    async fn retained_bytes_are_the_sum_of_the_entries() {
        let backend = MemoryTopicBackend::new();
        for size in [10usize, 5_000, 1, 900] {
            backend
                .stream_publish("t", "k", &vec![b'q'; size])
                .await
                .expect("publish");
        }

        let expected: u64 = {
            let streams = backend.state.streams.read();
            streams["t"]
                .messages
                .iter()
                .map(|e| StreamEntry::budget_cost(e.payload.len()))
                .sum()
        };
        assert_eq!(retained_bytes(&backend, "t"), expected, "after publishing");

        // Consume half, then check the counter followed the trim rather than only the publishes.
        let sub = backend
            .stream_subscribe("t", "g", "c")
            .await
            .expect("subscribe");
        let mut receiver = sub.receiver;
        for _ in 0..2 {
            let msg = tokio::time::timeout(std::time::Duration::from_millis(500), receiver.next())
                .await
                .expect("delivered")
                .expect("some")
                .expect("ok");
            backend.stream_ack("t", "g", &msg.id).await.expect("ack");
        }
        backend.stream_trim_consumed("t").await.expect("trim");

        let expected: u64 = {
            let streams = backend.state.streams.read();
            streams["t"]
                .messages
                .iter()
                .map(|e| StreamEntry::budget_cost(e.payload.len()))
                .sum()
        };
        assert_eq!(retained_bytes(&backend, "t"), expected, "after trimming");
    }

    /// A single payload larger than the whole budget is refused, not admitted and then deleted.
    ///
    /// The boundary case, and the one where "trim to fit" and "refuse" differ most: there is no set of other
    /// entries whose removal would make room, so a length-driven bound admits it and then empties the queue
    /// around it.
    #[tokio::test]
    async fn a_payload_larger_than_the_budget_is_refused() {
        let backend = MemoryTopicBackend::with_stream_budget(1024);
        let refused = backend.stream_publish("t", "k", &vec![b'!'; 4096]).await;
        assert!(
            matches!(refused, Err(TopicError::BufferFull)),
            "got {refused:?}"
        );
        assert!(
            retained_ids(&backend, "t").is_empty(),
            "and nothing was stored for it"
        );
    }
}
