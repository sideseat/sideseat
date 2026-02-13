//! Topic error types

use std::fmt;

/// Error type for topic operations
#[derive(Debug)]
pub enum TopicError {
    /// Channel or connection closed
    ChannelClosed,
    /// Buffer full (backpressure)
    BufferFull,
    /// Receiver lagged behind
    Lagged(u64),
    /// Topic exists with different type
    TypeMismatch(String),
    /// Connection error (Redis)
    Connection(String),
    /// Serialization/deserialization error
    Serialization(String),
    /// Stream operation error
    Stream(String),
    /// Consumer group error
    ConsumerGroup(String),
    /// Configuration error
    Config(String),
}

impl std::error::Error for TopicError {}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopicError::ChannelClosed => write!(f, "channel closed"),
            TopicError::BufferFull => write!(f, "buffer full"),
            TopicError::Lagged(n) => write!(f, "receiver lagged by {} messages", n),
            TopicError::TypeMismatch(name) => {
                write!(f, "topic '{}' already exists with different type", name)
            }
            TopicError::Connection(msg) => write!(f, "connection error: {}", msg),
            TopicError::Serialization(msg) => write!(f, "serialization error: {}", msg),
            TopicError::Stream(msg) => write!(f, "stream error: {}", msg),
            TopicError::ConsumerGroup(msg) => write!(f, "consumer group error: {}", msg),
            TopicError::Config(msg) => write!(f, "configuration error: {}", msg),
        }
    }
}

// Conversion from broadcast errors
impl From<tokio::sync::broadcast::error::RecvError> for TopicError {
    fn from(err: tokio::sync::broadcast::error::RecvError) -> Self {
        match err {
            tokio::sync::broadcast::error::RecvError::Closed => TopicError::ChannelClosed,
            tokio::sync::broadcast::error::RecvError::Lagged(n) => TopicError::Lagged(n),
        }
    }
}

impl From<deadpool_redis::PoolError> for TopicError {
    fn from(err: deadpool_redis::PoolError) -> Self {
        TopicError::Connection(err.to_string())
    }
}

impl From<deadpool_redis::redis::RedisError> for TopicError {
    fn from(err: deadpool_redis::redis::RedisError) -> Self {
        TopicError::Stream(err.to_string())
    }
}
