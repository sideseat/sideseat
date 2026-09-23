//! Queue and pub/sub ports.
//!
//! Defines the interface implemented by in-memory, Redis, and Redpanda adapters.
//! Supports two delivery semantics:
//! - Broadcast (Pub/Sub): Fire-and-forget, all subscribers receive
//! - Stream: At-least-once, one consumer per message, acknowledgment required

use std::pin::Pin;
use std::{error::Error, fmt};

use async_trait::async_trait;
use futures::Stream;

/// Error shared by queue consumers and adapters.
///
/// Driver errors are converted to strings inside the adapter, so this port never names Redis,
/// Kafka, Tokio, or any other implementation.
#[derive(Debug)]
pub enum TopicError {
    ChannelClosed,
    BufferFull,
    Lagged(u64),
    Connection(String),
    Serialization(String),
    Stream(String),
    ConsumerGroup(String),
    Undecodable {
        id: String,
        detail: String,
        raw: Vec<u8>,
    },
    Config(String),
}

impl Error for TopicError {}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelClosed => write!(f, "channel closed"),
            Self::BufferFull => write!(f, "buffer full"),
            Self::Lagged(n) => write!(f, "receiver lagged by {n} messages"),
            Self::Connection(message) => write!(f, "connection error: {message}"),
            Self::Serialization(message) => write!(f, "serialization error: {message}"),
            Self::Stream(message) => write!(f, "stream error: {message}"),
            Self::ConsumerGroup(message) => write!(f, "consumer group error: {message}"),
            Self::Undecodable { id, detail, .. } => {
                write!(f, "undecodable payload at {id}: {detail}")
            }
            Self::Config(message) => write!(f, "configuration error: {message}"),
        }
    }
}

/// Message received from a stream with its ID for acknowledgment
#[derive(Debug, Clone)]
pub struct StreamMessage {
    /// Unique message ID (Redis stream ID or memory sequence)
    pub id: String,
    /// Broker partition, or a stable virtual partition for single-log backends.
    ///
    /// Consumers use this for bounded weighted draining. It is delivery metadata rather than part of the
    /// payload: callers must not infer identity or ordering across partitions from it.
    pub partition: u32,
    /// Message payload
    pub payload: Vec<u8>,
}

/// Subscription to a broadcast topic (Pub/Sub semantics)
pub struct BroadcastSubscription {
    /// Stream of received messages
    pub receiver: Pin<Box<dyn Stream<Item = Result<Vec<u8>, TopicError>> + Send>>,
}

/// Subscription to a stream topic (at-least-once semantics)
pub struct StreamSubscription {
    /// Stream of received messages with IDs
    pub receiver: Pin<Box<dyn Stream<Item = Result<StreamMessage, TopicError>> + Send>>,
}

/// Topic backend trait
///
/// Defines the interface for topic implementations.
/// Both in-memory and Redis backends implement this trait.
///
/// # Topic Types
///
/// - **Broadcast topics** (Pub/Sub): Use `publish` and `subscribe`. Best-effort delivery,
///   all active subscribers receive each message. No persistence - if no subscribers,
///   messages are lost. Ideal for SSE notifications.
///
/// - **Stream topics**: Use `stream_publish`, `stream_subscribe`, and `stream_ack`.
///   At-least-once delivery with acknowledgment. Messages persist until acknowledged.
///   Ideal for critical data like OTLP traces.
#[async_trait]
pub trait TopicBackend: Send + Sync {
    /// Publish message to broadcast topic (fire-and-forget)
    ///
    /// All active subscribers receive the message. If no subscribers exist,
    /// the message is silently dropped.
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), TopicError>;

    /// Subscribe to broadcast topic
    ///
    /// Returns a stream of messages. Lagging subscribers may miss messages
    /// (bounded buffer overflow).
    async fn subscribe(&self, topic: &str) -> Result<BroadcastSubscription, TopicError>;

    /// Publish message to stream topic, under a **partition key**.
    ///
    /// Returns the message ID. Messages persist until acknowledged.
    ///
    /// The key is part of the delivery contract. Partitioned brokers preserve order only within one
    /// partition, so records that require relative ordering must use the same stable key.
    ///
    /// The caller selects a domain-specific key:
    ///
    /// | Signal | Key |
    /// | --- | --- |
    /// | Spans | the trace id, always |
    /// | Metrics | `(project, instrument)` |
    /// | Logs | the trace id when present, else `(project, resource, scope)` |
    ///
    /// A trace id is immutable and available on every span, unlike a session id that may appear only on
    /// the root. The in-process backend has one partition and may ignore the key.
    async fn stream_publish(
        &self,
        topic: &str,
        partition_key: &str,
        payload: &[u8],
    ) -> Result<String, TopicError>;

    /// Subscribe to stream topic with consumer group
    ///
    /// Messages are distributed across consumers in the group.
    /// Each message is delivered to exactly one consumer until acknowledged.
    ///
    /// # Arguments
    /// - `topic`: Stream name
    /// - `group`: Consumer group name (e.g., "trace_pipeline")
    /// - `consumer`: Unique consumer name (e.g., "{uuid}:{pid}")
    async fn stream_subscribe(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
    ) -> Result<StreamSubscription, TopicError>;

    /// Acknowledge message processing complete
    ///
    /// Removes the message from the pending list. Must be called after
    /// successful processing to prevent re-delivery.
    ///
    /// Brokers with entry IDs may acknowledge that exact entry. Offset-based brokers must not commit past
    /// an earlier unfinished record: they track completed offsets and advance only the highest contiguous
    /// prefix. Otherwise a later success can prevent an earlier failure from being redelivered.
    async fn stream_ack(&self, topic: &str, group: &str, id: &str) -> Result<(), TopicError>;

    /// Acknowledge multiple messages in a single call
    async fn stream_ack_batch(
        &self,
        topic: &str,
        group: &str,
        ids: &[String],
    ) -> Result<(), TopicError>;

    /// Claim pending messages that have been idle too long
    ///
    /// Used for recovery when consumers crash without acknowledging.
    /// Returns IDs of messages claimed by this consumer.
    ///
    /// # Arguments
    /// - `topic`: Stream name
    /// - `group`: Consumer group name
    /// - `consumer`: Consumer claiming the messages
    /// - `min_idle_ms`: Minimum idle time before claiming (e.g., 60000 for 1 min)
    /// - `count`: Maximum messages to claim
    async fn stream_claim(
        &self,
        topic: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<StreamMessage>, TopicError>;

    /// Get stream statistics for monitoring
    async fn stream_stats(&self, topic: &str, group: &str) -> Result<StreamStats, TopicError>;

    /// Remove stream entries that every consumer group has finished with. Returns how many went.
    ///
    /// A durable stream has to be bounded by *progress*, not by length: trimming to a maximum length
    /// deletes the oldest entries whether or not anyone has read them, and those entries were already
    /// answered 200. Backends with no persistent stream have nothing to trim.
    async fn stream_trim_consumed(&self, _topic: &str) -> Result<u64, TopicError> {
        Ok(0)
    }

    /// Preserve a payload on a side stream before the caller acknowledges it away.
    ///
    /// For an entry that decoded as a stream structure but not as the message it should be. It was answered
    /// 200 when queued, so acking it away discards accepted bytes; this keeps them for inspection or replay.
    /// The default is a no-op: a backend with no durable stream (the in-memory one) has nowhere to put it,
    /// and there the queue was skipped entirely - the request wrote inline, so an undecodable payload never
    /// reached a queue. Returning `Ok` there means "nothing to preserve", and the caller still acks.
    async fn stream_dead_letter(
        &self,
        _topic: &str,
        _group: &str,
        _id: &str,
        _reason: &str,
        _payload: &[u8],
    ) -> Result<(), TopicError> {
        Ok(())
    }

    /// Health check (validates connection)
    async fn health_check(&self) -> Result<(), TopicError>;

    /// Stop background resources owned by the backend.
    async fn shutdown(&self) {}

    /// Backend name for debugging/logging
    fn backend_name(&self) -> &'static str;

    /// Whether a published message survives this process dying.
    ///
    /// Read by the ingest path to decide whether it may acknowledge before writing. A queue that only
    /// exists in memory cannot make that promise, so with such a backend the request writes first and
    /// answers second: nothing is acknowledged that could be lost.
    fn is_durable(&self) -> bool;
}

/// Stream statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct StreamStats {
    /// Total messages in the stream
    pub length: u64,
    /// Messages pending acknowledgment
    pub pending: u64,
    /// Number of consumers in the group
    pub consumers: u64,
    /// Oldest pending message age in milliseconds
    pub oldest_pending_ms: Option<u64>,
}
