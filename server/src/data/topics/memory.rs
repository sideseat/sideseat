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
use super::error::TopicError;

/// Default broadcast channel capacity
const DEFAULT_BROADCAST_CAPACITY: usize = 10_000;

/// Default stream max length (approximate, trimmed on publish)
const DEFAULT_STREAM_MAX_LEN: usize = 100_000;

/// Message stored in memory stream
#[derive(Clone)]
struct StreamEntry {
    id: u64,
    payload: Vec<u8>,
    timestamp: Instant,
}

/// Consumer group state for a stream
#[derive(Clone, Default)]
struct ConsumerGroup {
    /// Last delivered ID for each consumer
    last_delivered: HashMap<String, u64>,
    /// Pending messages: message_id -> (consumer, delivery_time)
    pending: HashMap<u64, (String, Instant)>,
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
    /// Maximum stream length
    max_len: usize,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            groups: HashMap::new(),
            next_id: 1,
            max_len: DEFAULT_STREAM_MAX_LEN,
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

    /// Trim stream to max length (approximately)
    fn trim_stream(stream: &mut StreamState) {
        while stream.messages.len() > stream.max_len {
            if let Some(entry) = stream.messages.pop_front() {
                // Clean up pending entries for this message
                for group in stream.groups.values_mut() {
                    group.pending.remove(&entry.id);
                }
            }
        }
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

    async fn stream_publish(&self, topic: &str, payload: &[u8]) -> Result<String, TopicError> {
        let id = {
            let mut streams = self.state.streams.write();
            let stream = streams.entry(topic.to_string()).or_default();

            let id = stream.next_id;
            stream.next_id += 1;

            stream.messages.push_back(StreamEntry {
                id,
                payload: payload.to_vec(),
                timestamp: Instant::now(),
            });

            Self::trim_stream(stream);
            id
        };

        // Wake subscriber immediately (no 100ms polling delay)
        self.get_or_create_notifier(topic).notify_one();

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
            let stream = streams.entry(topic.to_string()).or_default();
            stream.groups.entry(group.to_string()).or_default();
        }

        let topic = topic.to_string();
        let group = group.to_string();
        let consumer = consumer.to_string();
        let state = Arc::clone(&self.state);
        let notifier = self.get_or_create_notifier(&topic);

        let stream = stream! {
            let mut last_seen: u64 = 0;

            // Get initial position from consumer's last delivered
            {
                let streams = state.streams.read();
                if let Some(stream_state) = streams.get(&topic)
                    && let Some(cg) = stream_state.groups.get(&group)
                    && let Some(&last) = cg.last_delivered.get(&consumer)
                {
                    last_seen = last;
                }
            }

            loop {
                // Check for new messages - scope the lock to avoid holding across await
                let (maybe_msg, stream_exists) = {
                    let mut streams = state.streams.write();
                    match streams.get_mut(&topic) {
                        None => (None, false),
                        Some(stream_state) => {
                            let cg = stream_state.groups.entry(group.clone()).or_default();

                            // Find next undelivered message for this consumer
                            let mut found = None;
                            for entry in &stream_state.messages {
                                if entry.id > last_seen && !cg.pending.contains_key(&entry.id) {
                                    found = Some(StreamEntry {
                                        id: entry.id,
                                        payload: entry.payload.clone(),
                                        timestamp: entry.timestamp,
                                    });
                                    break;
                                }
                            }

                            let msg = if let Some(entry) = found {
                                // Mark as pending for this consumer
                                cg.pending.insert(entry.id, (consumer.clone(), Instant::now()));
                                cg.last_delivered.insert(consumer.clone(), entry.id);
                                last_seen = entry.id;
                                Some(StreamMessage {
                                    id: entry.id.to_string(),
                                    payload: entry.payload,
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
            consumers: cg.last_delivered.len() as u64,
            oldest_pending_ms,
        })
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
        let id = backend.stream_publish("stream", b"msg1").await.unwrap();
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
        backend.stream_publish("stream", b"msg1").await.unwrap();
        backend.stream_publish("stream", b"msg2").await.unwrap();

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
