//! Distributed topic system
//!
//! Provides pub/sub and stream messaging with pluggable backends:
//! - In-memory (default) - local-only, for development and single-process
//! - Redis (optional) - distributed, for multi-machine deployments
//!
//! ## Topic Types
//!
//! - **Broadcast topics** (`BroadcastTopic`): Fire-and-forget, all subscribers receive.
//!   Used for ephemeral notifications like SSE events. No persistence.
//!
//! - **Stream topics** (`StreamTopic`): At-least-once delivery with acknowledgment.
//!   Used for critical data like OTLP traces. Messages persist until acknowledged.
//!
//! ## Configuration
//!
//! Topics follow cache backend configuration:
//! - `database.cache = "memory"` → in-memory topics
//! - `database.cache = "redis"` → Redis Streams + Pub/Sub

mod backend;
mod error;
mod memory;
mod pubsub;
mod redis;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use parking_lot::RwLock;
use prost::Message as ProstMessage;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

pub use backend::{
    BroadcastSubscription, StreamMessage, StreamStats, StreamSubscription, TopicBackend,
};
pub use error::TopicError;
use memory::MemoryTopicBackend;

use crate::core::config::{CacheBackendType, CacheConfig};
use crate::core::constants::{
    DEFAULT_TOPIC_BUFFER_SIZE, DEFAULT_TOPIC_CHANNEL_CAPACITY, ENV_TOPIC_BUFFER_SIZE,
    ENV_TOPIC_CHANNEL_CAPACITY,
};

// ============================================================================
// TOPIC MESSAGE TRAIT
// ============================================================================

/// Trait for messages that can be published to topics
pub trait TopicMessage: Clone + Send + Sync + 'static {
    /// Estimate message size in bytes for backpressure
    fn size_bytes(&self) -> usize;
}

// Note: TopicMessage implementations for OTLP types (ExportTraceServiceRequest,
// ExportMetricsServiceRequest, ExportLogsServiceRequest) are defined in domain/mod.rs
// with more accurate size calculations based on message structure.

// ============================================================================
// TOPIC CONFIG
// ============================================================================

/// Topic configuration
#[derive(Clone)]
pub struct TopicConfig {
    pub buffer_size: usize,
    pub channel_capacity: usize,
}

impl Default for TopicConfig {
    fn default() -> Self {
        let buffer_size = std::env::var(ENV_TOPIC_BUFFER_SIZE)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TOPIC_BUFFER_SIZE);

        let channel_capacity = std::env::var(ENV_TOPIC_CHANNEL_CAPACITY)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TOPIC_CHANNEL_CAPACITY);

        Self {
            buffer_size,
            channel_capacity,
        }
    }
}

// ============================================================================
// LOCAL PUBLISHER/SUBSCRIBER (For backward compatibility)
// ============================================================================

/// Publisher handle for local topic - clone and share across producers
#[derive(Clone, Debug)]
pub struct Publisher<T: TopicMessage> {
    tx: mpsc::Sender<T>,
    buffer_bytes: Arc<AtomicUsize>,
    max_bytes: usize,
}

impl<T: TopicMessage> Publisher<T> {
    /// Publish message (returns error if buffer full)
    pub fn publish(&self, msg: T) -> Result<(), TopicError> {
        let msg_size = msg.size_bytes();

        // Atomic CAS to reserve buffer space
        loop {
            let current = self.buffer_bytes.load(Ordering::Relaxed);
            if current + msg_size > self.max_bytes {
                return Err(TopicError::BufferFull);
            }
            if self
                .buffer_bytes
                .compare_exchange(
                    current,
                    current + msg_size,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }

        self.tx.try_send(msg).map_err(|_| {
            self.buffer_bytes.fetch_sub(msg_size, Ordering::SeqCst);
            TopicError::ChannelClosed
        })
    }
}

/// Subscriber handle for local topic
pub struct Subscriber<T: TopicMessage> {
    rx: broadcast::Receiver<T>,
}

impl<T: TopicMessage> Subscriber<T> {
    pub async fn recv(&mut self) -> Result<T, TopicError> {
        self.rx.recv().await.map_err(|e| e.into())
    }
}

// ============================================================================
// TOPIC INNER (Local implementation)
// ============================================================================

/// A single topic instance (local implementation)
struct TopicInner<T: TopicMessage> {
    broadcast_tx: broadcast::Sender<T>,
    publisher: Publisher<T>,
}

/// Type-erased topic storage
trait AnyTopic: Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: TopicMessage> AnyTopic for TopicInner<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

type TopicMap = HashMap<String, (TypeId, Arc<dyn AnyTopic>)>;

/// Dispatcher entry with shutdown control
struct DispatcherEntry {
    handle: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    /// If true, drain messages on shutdown. If false, abort immediately.
    drain_on_shutdown: bool,
}

// ============================================================================
// TOPIC SERVICE
// ============================================================================

/// Central topic service - manages all topics
///
/// Provides both local (in-process) topics for backward compatibility
/// and distributed topics via the backend trait.
pub struct TopicService {
    /// Local topics (backward compatible)
    topics: RwLock<TopicMap>,
    dispatchers: RwLock<Vec<DispatcherEntry>>,
    default_config: TopicConfig,
    /// Distributed backend (memory or Redis)
    backend: Arc<dyn TopicBackend>,
}

impl TopicService {
    /// Create a new topic service with in-memory backend
    pub fn new() -> Self {
        Self::with_config(TopicConfig::default())
    }

    /// Create with custom config (in-memory backend)
    pub fn with_config(config: TopicConfig) -> Self {
        Self {
            topics: RwLock::new(HashMap::new()),
            dispatchers: RwLock::new(Vec::new()),
            default_config: config,
            backend: Arc::new(MemoryTopicBackend::new()),
        }
    }

    /// Create from cache configuration
    pub async fn from_cache_config(cache_config: &CacheConfig) -> Result<Self, TopicError> {
        let backend: Arc<dyn TopicBackend> = match cache_config.backend {
            CacheBackendType::Memory => Arc::new(MemoryTopicBackend::new()),
            CacheBackendType::Redis => {
                let url = cache_config.redis_url.as_ref().ok_or_else(|| {
                    TopicError::Config("redis_url required for Redis backend".into())
                })?;
                Arc::new(redis::RedisTopicBackend::new(url).await?)
            }
        };

        Ok(Self {
            topics: RwLock::new(HashMap::new()),
            dispatchers: RwLock::new(Vec::new()),
            default_config: TopicConfig::default(),
            backend,
        })
    }

    /// Get the backend name
    pub fn backend_name(&self) -> &'static str {
        self.backend.backend_name()
    }

    // ========================================================================
    // LOCAL TOPIC API (Backward compatible)
    // ========================================================================

    /// Create a new critical topic (drains on shutdown) or get existing one
    ///
    /// This is the local (in-process) API for backward compatibility.
    /// For distributed topics, use `stream_topic()` or `broadcast_topic()`.
    pub fn topic<T: TopicMessage>(&self, name: &str) -> Result<Topic<T>, TopicError> {
        self.create_topic_internal(name, self.default_config.clone(), true)
    }

    /// Create an ephemeral topic (aborted on shutdown, no draining)
    /// Use for notification-only topics like SSE where data is already persisted elsewhere
    pub fn ephemeral_topic<T: TopicMessage>(&self, name: &str) -> Result<Topic<T>, TopicError> {
        self.create_topic_internal(name, self.default_config.clone(), false)
    }

    /// Create an ephemeral topic with custom config (for memory-constrained scenarios)
    pub fn ephemeral_topic_with_config<T: TopicMessage>(
        &self,
        name: &str,
        config: TopicConfig,
    ) -> Result<Topic<T>, TopicError> {
        self.create_topic_internal(name, config, false)
    }

    /// Internal topic creation with drain_on_shutdown flag
    fn create_topic_internal<T: TopicMessage>(
        &self,
        name: &str,
        config: TopicConfig,
        drain_on_shutdown: bool,
    ) -> Result<Topic<T>, TopicError> {
        let type_id = TypeId::of::<T>();

        // Hold write lock to prevent race conditions
        let mut topics = self.topics.write();

        // Check if topic exists
        if let Some((existing_type, topic)) = topics.get(name) {
            if *existing_type == type_id {
                let inner = topic.as_any().downcast_ref::<TopicInner<T>>().unwrap();
                return Ok(Topic {
                    name: name.to_string(),
                    publisher: inner.publisher.clone(),
                    broadcast_tx: inner.broadcast_tx.clone(),
                });
            }
            return Err(TopicError::TypeMismatch(name.to_string()));
        }

        // Create new topic
        let (mpsc_tx, mpsc_rx) = mpsc::channel(config.channel_capacity);
        let (broadcast_tx, _) = broadcast::channel(config.channel_capacity);
        let buffer_bytes = Arc::new(AtomicUsize::new(0));

        let publisher = Publisher {
            tx: mpsc_tx,
            buffer_bytes: buffer_bytes.clone(),
            max_bytes: config.buffer_size,
        };

        let inner = TopicInner {
            broadcast_tx: broadcast_tx.clone(),
            publisher: publisher.clone(),
        };

        // Start dispatcher with shutdown signal and track entry
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle =
            Self::start_dispatcher(mpsc_rx, broadcast_tx.clone(), buffer_bytes, shutdown_rx);
        self.dispatchers.write().push(DispatcherEntry {
            handle,
            shutdown_tx,
            drain_on_shutdown,
        });

        // Store topic
        topics.insert(name.to_string(), (type_id, Arc::new(inner)));

        Ok(Topic {
            name: name.to_string(),
            publisher,
            broadcast_tx,
        })
    }

    fn start_dispatcher<T: TopicMessage>(
        mut rx: mpsc::Receiver<T>,
        broadcast_tx: broadcast::Sender<T>,
        buffer_bytes: Arc<AtomicUsize>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    // Check for shutdown signal
                    _ = &mut shutdown_rx => {
                        // Drain remaining messages before exiting
                        while let Ok(msg) = rx.try_recv() {
                            let msg_size = msg.size_bytes();
                            let _ = broadcast_tx.send(msg);
                            buffer_bytes.fetch_sub(msg_size, Ordering::SeqCst);
                        }
                        break;
                    }
                    // Process incoming messages
                    msg = rx.recv() => {
                        match msg {
                            Some(msg) => {
                                let msg_size = msg.size_bytes();
                                let _ = broadcast_tx.send(msg);
                                buffer_bytes.fetch_sub(msg_size, Ordering::SeqCst);
                            }
                            None => break, // Channel closed
                        }
                    }
                }
            }
        })
    }

    /// Get a publisher for an existing topic (does NOT create the topic)
    /// Returns None if topic doesn't exist or type mismatch
    pub fn get_publisher<T: TopicMessage>(&self, name: &str) -> Option<Publisher<T>> {
        let type_id = TypeId::of::<T>();
        let topics = self.topics.read();

        if let Some((existing_type, topic)) = topics.get(name)
            && *existing_type == type_id
        {
            let inner = topic.as_any().downcast_ref::<TopicInner<T>>().unwrap();
            return Some(inner.publisher.clone());
        }
        None
    }

    // ========================================================================
    // DISTRIBUTED TOPIC API
    // ========================================================================

    /// Create a stream topic for at-least-once delivery
    ///
    /// Use for critical data that must not be lost (e.g., OTLP traces).
    /// Messages persist until acknowledged.
    pub fn stream_topic<T>(&self, name: &str) -> StreamTopic<T>
    where
        T: TopicMessage + ProstMessage + Default,
    {
        StreamTopic {
            name: name.to_string(),
            backend: Arc::clone(&self.backend),
            _phantom: PhantomData,
        }
    }

    /// Create a broadcast topic for fire-and-forget delivery
    ///
    /// Use for ephemeral notifications (e.g., SSE events).
    /// Messages are lost if no subscribers or subscriber lags.
    pub fn broadcast_topic<T>(&self, name: &str) -> BroadcastTopic<T>
    where
        T: TopicMessage + Serialize + DeserializeOwned,
    {
        BroadcastTopic {
            name: name.to_string(),
            backend: Arc::clone(&self.backend),
            _phantom: PhantomData,
        }
    }

    /// Get stream statistics for monitoring
    pub async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError> {
        self.backend.stream_stats(topic, group).await
    }

    /// Health check
    pub async fn health_check(&self) -> Result<(), TopicError> {
        self.backend.health_check().await
    }

    /// Gracefully shutdown all dispatcher tasks
    ///
    /// - Critical topics: Signal to drain mpsc channels, then wait
    /// - Ephemeral topics: Abort immediately (no draining needed)
    pub async fn shutdown(&self) {
        let entries: Vec<_> = {
            let mut guard = self.dispatchers.write();
            std::mem::take(&mut *guard)
        };

        let mut critical_handles = Vec::new();

        for entry in entries {
            if entry.drain_on_shutdown {
                // Signal to drain and collect handle
                let _ = entry.shutdown_tx.send(());
                critical_handles.push(entry.handle);
            } else {
                // Abort ephemeral topics immediately
                entry.handle.abort();
                let _ = entry.handle.await;
            }
        }

        // Wait for critical dispatchers to finish draining
        for handle in critical_handles {
            let _ = handle.await;
        }
    }
}

impl Default for TopicService {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// LOCAL TOPIC HANDLE (Backward compatible)
// ============================================================================

/// Handle to a specific local topic
#[derive(Clone)]
pub struct Topic<T: TopicMessage> {
    name: String,
    publisher: Publisher<T>,
    broadcast_tx: broadcast::Sender<T>,
}

impl<T: TopicMessage> Topic<T> {
    /// Get a publisher for this topic
    pub fn publisher(&self) -> Publisher<T> {
        self.publisher.clone()
    }

    /// Subscribe to this topic
    pub fn subscribe(&self) -> Subscriber<T> {
        Subscriber {
            rx: self.broadcast_tx.subscribe(),
        }
    }

    /// Publish directly via topic handle
    pub fn publish(&self, msg: T) -> Result<(), TopicError> {
        self.publisher.publish(msg)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

// ============================================================================
// STREAM TOPIC (Distributed, at-least-once)
// ============================================================================

/// Stream topic for at-least-once delivery
///
/// Uses Redis Streams when Redis backend is configured,
/// or in-memory simulation for local development.
pub struct StreamTopic<T>
where
    T: TopicMessage + ProstMessage + Default,
{
    name: String,
    backend: Arc<dyn TopicBackend>,
    _phantom: PhantomData<T>,
}

impl<T> StreamTopic<T>
where
    T: TopicMessage + ProstMessage + Default,
{
    /// Publish a message to the stream
    ///
    /// Returns the message ID for tracking.
    pub async fn publish(&self, msg: &T) -> Result<String, TopicError> {
        let payload = msg.encode_to_vec();
        self.backend.stream_publish(&self.name, &payload).await
    }

    /// Subscribe to the stream with a consumer group
    ///
    /// Messages are distributed across consumers in the group.
    /// Call `ack()` after processing each message.
    pub async fn subscribe(
        &self,
        group: &str,
        consumer: &str,
    ) -> Result<StreamTopicSubscriber<T>, TopicError> {
        let subscription = self
            .backend
            .stream_subscribe(&self.name, group, consumer)
            .await?;
        Ok(StreamTopicSubscriber {
            name: self.name.clone(),
            group: group.to_string(),
            backend: Arc::clone(&self.backend),
            subscription,
            _phantom: PhantomData,
        })
    }

    /// Get the topic name
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Acker for acknowledging stream messages (Send + Sync)
#[derive(Clone)]
pub struct StreamAcker {
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
}

impl StreamAcker {
    /// Acknowledge message processing complete
    pub async fn ack(&self, id: &str) -> Result<(), TopicError> {
        self.backend.stream_ack(&self.name, &self.group, id).await
    }

    /// Acknowledge multiple messages in a single call
    pub async fn ack_batch(&self, ids: &[String]) -> Result<(), TopicError> {
        self.backend
            .stream_ack_batch(&self.name, &self.group, ids)
            .await
    }
}

/// Claimer for claiming stuck messages from other consumers (Send + Sync)
#[derive(Clone)]
pub struct StreamClaimer {
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
}

impl StreamClaimer {
    /// Claim stuck messages from other consumers
    ///
    /// Returns (message_id, payload) pairs for messages that have been idle
    /// longer than `min_idle_ms`. The caller should decode and process these
    /// messages, then acknowledge them using `StreamAcker::ack()`.
    pub async fn claim(
        &self,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError> {
        self.backend
            .stream_claim(&self.name, &self.group, consumer, min_idle_ms, count)
            .await
    }
}

/// Subscriber to a stream topic
pub struct StreamTopicSubscriber<T>
where
    T: TopicMessage + ProstMessage + Default,
{
    name: String,
    group: String,
    backend: Arc<dyn TopicBackend>,
    subscription: StreamSubscription,
    _phantom: PhantomData<T>,
}

impl<T> StreamTopicSubscriber<T>
where
    T: TopicMessage + ProstMessage + Default,
{
    /// Receive the next message
    ///
    /// Returns (message_id, message). Call `acker().ack(message_id)` after processing.
    pub async fn recv(&mut self) -> Result<(String, T), TopicError> {
        if let Some(result) = self.subscription.receiver.next().await {
            let msg = result?;
            let decoded = T::decode(&msg.payload[..])
                .map_err(|e| TopicError::Serialization(e.to_string()))?;
            Ok((msg.id, decoded))
        } else {
            Err(TopicError::ChannelClosed)
        }
    }

    /// Get an acker for acknowledging messages (Send + Sync)
    pub fn acker(&self) -> StreamAcker {
        StreamAcker {
            name: self.name.clone(),
            group: self.group.clone(),
            backend: Arc::clone(&self.backend),
        }
    }

    /// Get a claimer for claiming stuck messages (Send + Sync)
    pub fn claimer(&self) -> StreamClaimer {
        StreamClaimer {
            name: self.name.clone(),
            group: self.group.clone(),
            backend: Arc::clone(&self.backend),
        }
    }

    /// Claim stuck messages from other consumers
    pub async fn claim(
        &self,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<(String, T)>, TopicError> {
        let messages = self
            .backend
            .stream_claim(&self.name, &self.group, consumer, min_idle_ms, count)
            .await?;

        let mut result = Vec::new();
        for msg in messages {
            let decoded = T::decode(&msg.payload[..])
                .map_err(|e| TopicError::Serialization(e.to_string()))?;
            result.push((msg.id, decoded));
        }
        Ok(result)
    }
}

// ============================================================================
// BROADCAST TOPIC (Distributed, fire-and-forget)
// ============================================================================

/// Broadcast topic for fire-and-forget delivery
///
/// Uses Redis Pub/Sub when Redis backend is configured,
/// or in-memory broadcast for local development.
pub struct BroadcastTopic<T>
where
    T: TopicMessage + Serialize + DeserializeOwned,
{
    name: String,
    backend: Arc<dyn TopicBackend>,
    _phantom: PhantomData<T>,
}

impl<T> BroadcastTopic<T>
where
    T: TopicMessage + Serialize + DeserializeOwned,
{
    /// Publish a message (fire-and-forget)
    pub async fn publish(&self, msg: &T) -> Result<(), TopicError> {
        let payload =
            rmp_serde::to_vec(msg).map_err(|e| TopicError::Serialization(e.to_string()))?;
        self.backend.publish(&self.name, &payload).await
    }

    /// Subscribe to broadcast messages
    pub async fn subscribe(&self) -> Result<BroadcastTopicSubscriber<T>, TopicError> {
        let subscription = self.backend.subscribe(&self.name).await?;
        Ok(BroadcastTopicSubscriber {
            subscription,
            _phantom: PhantomData,
        })
    }

    /// Get the topic name
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Subscriber to a broadcast topic
pub struct BroadcastTopicSubscriber<T>
where
    T: TopicMessage + Serialize + DeserializeOwned,
{
    subscription: BroadcastSubscription,
    _phantom: PhantomData<T>,
}

impl<T> BroadcastTopicSubscriber<T>
where
    T: TopicMessage + Serialize + DeserializeOwned,
{
    /// Receive the next message
    pub async fn recv(&mut self) -> Result<T, TopicError> {
        if let Some(result) = self.subscription.receiver.next().await {
            let payload = result?;
            let decoded: T = rmp_serde::from_slice(&payload)
                .map_err(|e| TopicError::Serialization(e.to_string()))?;
            Ok(decoded)
        } else {
            Err(TopicError::ChannelClosed)
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestMessage {
        data: String,
        size: usize,
    }

    impl TopicMessage for TestMessage {
        fn size_bytes(&self) -> usize {
            self.size
        }
    }

    fn msg(data: &str, size: usize) -> TestMessage {
        TestMessage {
            data: data.to_string(),
            size,
        }
    }

    #[tokio::test]
    async fn test_publisher_buffer_full() {
        let config = TopicConfig {
            buffer_size: 100,
            channel_capacity: 10,
        };
        let service = TopicService::with_config(config);
        let topic = service.topic::<TestMessage>("test").unwrap();
        let publisher = topic.publisher();

        // Fill buffer to capacity
        assert!(publisher.publish(msg("a", 50)).is_ok());
        assert!(publisher.publish(msg("b", 50)).is_ok());

        // Next message should fail
        let result = publisher.publish(msg("c", 10));
        assert!(matches!(result, Err(TopicError::BufferFull)));
    }

    #[tokio::test]
    async fn test_subscriber_receives_messages() {
        let service = TopicService::new();
        let topic = service.topic::<TestMessage>("test").unwrap();
        let publisher = topic.publisher();
        let mut subscriber = topic.subscribe();

        publisher.publish(msg("hello", 10)).unwrap();

        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.data, "hello");
    }

    #[tokio::test]
    async fn test_multiple_subscribers_receive_same_message() {
        let service = TopicService::new();
        let topic = service.topic::<TestMessage>("test").unwrap();
        let publisher = topic.publisher();
        let mut sub1 = topic.subscribe();
        let mut sub2 = topic.subscribe();

        publisher.publish(msg("broadcast", 10)).unwrap();

        let msg1 = sub1.recv().await.unwrap();
        let msg2 = sub2.recv().await.unwrap();
        assert_eq!(msg1.data, "broadcast");
        assert_eq!(msg2.data, "broadcast");
    }

    #[tokio::test]
    async fn test_topic_service_reuses_existing_topic() {
        let service = TopicService::new();
        let topic1 = service.topic::<TestMessage>("shared").unwrap();
        let topic2 = service.topic::<TestMessage>("shared").unwrap();

        assert_eq!(topic1.name(), topic2.name());
    }

    #[tokio::test]
    async fn test_topic_service_returns_error_on_type_mismatch() {
        #[derive(Clone)]
        struct OtherMessage;
        impl TopicMessage for OtherMessage {
            fn size_bytes(&self) -> usize {
                0
            }
        }

        let service = TopicService::new();
        let _topic1 = service.topic::<TestMessage>("typed").unwrap();
        let result = service.topic::<OtherMessage>("typed");
        assert!(matches!(result, Err(TopicError::TypeMismatch(_))));
    }

    #[tokio::test]
    async fn test_buffer_freed_after_dispatch() {
        let config = TopicConfig {
            buffer_size: 100,
            channel_capacity: 10,
        };
        let service = TopicService::with_config(config);
        let topic = service.topic::<TestMessage>("test").unwrap();
        let publisher = topic.publisher();
        let mut subscriber = topic.subscribe();

        // Fill buffer
        publisher.publish(msg("a", 100)).unwrap();

        // Consume message (frees buffer)
        let _ = subscriber.recv().await.unwrap();

        // Allow dispatcher to run
        tokio::task::yield_now().await;

        // Should be able to publish again
        assert!(publisher.publish(msg("b", 100)).is_ok());
    }

    #[test]
    fn test_topic_config_default() {
        let config = TopicConfig::default();
        assert_eq!(config.buffer_size, DEFAULT_TOPIC_BUFFER_SIZE);
        assert_eq!(config.channel_capacity, DEFAULT_TOPIC_CHANNEL_CAPACITY);
    }

    #[tokio::test]
    async fn test_topic_service_default() {
        let service = TopicService::default();
        let topic = service.topic::<TestMessage>("default_test").unwrap();
        assert_eq!(topic.name(), "default_test");
    }

    #[test]
    fn test_get_publisher_nonexistent_topic() {
        let service = TopicService::new();
        let result = service.get_publisher::<TestMessage>("nonexistent");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_publisher_existing_topic() {
        let service = TopicService::new();
        let _topic = service.topic::<TestMessage>("existing").unwrap();
        let publisher = service.get_publisher::<TestMessage>("existing");
        assert!(publisher.is_some());
    }

    #[tokio::test]
    async fn test_get_publisher_type_mismatch() {
        #[derive(Clone)]
        struct OtherMessage;
        impl TopicMessage for OtherMessage {
            fn size_bytes(&self) -> usize {
                0
            }
        }

        let service = TopicService::new();
        let _topic = service.topic::<TestMessage>("typed").unwrap();
        let result = service.get_publisher::<OtherMessage>("typed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_publisher_can_publish() {
        let service = TopicService::new();
        let topic = service.topic::<TestMessage>("test").unwrap();
        let mut subscriber = topic.subscribe();

        // Get publisher via get_publisher (not topic.publisher())
        let publisher = service.get_publisher::<TestMessage>("test").unwrap();

        publisher.publish(msg("via_get_publisher", 10)).unwrap();

        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.data, "via_get_publisher");
    }

    #[tokio::test]
    async fn test_backend_name() {
        let service = TopicService::new();
        assert_eq!(service.backend_name(), "memory");
    }
}
