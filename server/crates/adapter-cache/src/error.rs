//! Cache error types

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache configuration error: {0}")]
    Config(String),

    #[error("Cache connection error: {0}")]
    Connection(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Cache operation failed: {0}")]
    Operation(String),

    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),

    #[error("Redis pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_display() {
        let err = CacheError::Config("redis_url required".to_string());
        assert_eq!(
            err.to_string(),
            "Cache configuration error: redis_url required"
        );
    }

    #[test]
    fn test_connection_error_display() {
        let err = CacheError::Connection("connection refused".to_string());
        assert_eq!(
            err.to_string(),
            "Cache connection error: connection refused"
        );
    }

    #[test]
    fn test_serialization_error_display() {
        let err = CacheError::Serialization("invalid msgpack".to_string());
        assert_eq!(err.to_string(), "Serialization error: invalid msgpack");
    }

    #[test]
    fn test_operation_error_display() {
        let err = CacheError::Operation("key too long".to_string());
        assert_eq!(err.to_string(), "Cache operation failed: key too long");
    }

    #[test]
    fn test_error_debug() {
        let err = CacheError::Config("test".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Config"));
    }
}
