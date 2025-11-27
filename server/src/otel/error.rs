//! OTel-specific error types

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// OTel-specific error type
#[derive(Debug, thiserror::Error)]
pub enum OtelError {
    #[error("Unsupported content type")]
    UnsupportedContentType(String),

    #[error("Invalid request format")]
    InvalidProtobuf(#[from] prost::DecodeError),

    #[error("Invalid request format")]
    InvalidJson(#[from] serde_json::Error),

    #[error("Server temporarily unavailable")]
    BufferFull,

    #[error("Server temporarily unavailable")]
    DiskSpaceCritical(u8),

    #[error("Invalid request: {0}")]
    ValidationError(String),

    #[error("Trace not found")]
    TraceNotFound(String),

    #[error("Too many connections")]
    TooManyConnections,

    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("Database migration failed")]
    MigrationFailed { version: i32, name: String, error: String },

    #[error("Schema validation failed")]
    SchemaValidationFailed(String),

    #[error("Database error")]
    Database(#[from] sqlx::Error),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query error")]
    Query(#[from] datafusion::error::DataFusionError),

    #[error("Server error")]
    ChannelSend,
}

/// Result type alias for OTel operations
pub type OtelResult<T> = std::result::Result<T, OtelError>;

impl IntoResponse for OtelError {
    fn into_response(self) -> Response {
        // Log internal details but return sanitized messages to clients
        let (status, message) = match &self {
            Self::UnsupportedContentType(ct) => {
                tracing::debug!("Unsupported content type: {}", ct);
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "Unsupported content type".to_string())
            }
            Self::InvalidProtobuf(e) => {
                tracing::debug!("Invalid protobuf: {}", e);
                (StatusCode::BAD_REQUEST, "Invalid request format".to_string())
            }
            Self::InvalidJson(e) => {
                tracing::debug!("Invalid JSON: {}", e);
                (StatusCode::BAD_REQUEST, "Invalid request format".to_string())
            }
            Self::BufferFull | Self::ChannelSend => {
                tracing::warn!("Server overloaded: {:?}", self);
                (StatusCode::SERVICE_UNAVAILABLE, "Server temporarily unavailable".to_string())
            }
            Self::DiskSpaceCritical(pct) => {
                tracing::error!("Disk space critical: {}%", pct);
                (StatusCode::SERVICE_UNAVAILABLE, "Server temporarily unavailable".to_string())
            }
            Self::ValidationError(msg) => {
                // Validation errors are safe to return (user input issues)
                (StatusCode::BAD_REQUEST, format!("Invalid request: {}", msg))
            }
            Self::TraceNotFound(id) => {
                tracing::debug!("Trace not found: {}", id);
                (StatusCode::NOT_FOUND, "Trace not found".to_string())
            }
            Self::TooManyConnections => {
                (StatusCode::TOO_MANY_REQUESTS, "Too many connections".to_string())
            }
            Self::ConnectionTimeout => {
                (StatusCode::REQUEST_TIMEOUT, "Connection timeout".to_string())
            }
            Self::MigrationFailed { version, name, error } => {
                tracing::error!("Migration {} ({}) failed: {}", version, name, error);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::SchemaValidationFailed(msg) => {
                tracing::error!("Schema validation failed: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::Database(e) => {
                tracing::error!("Database error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::StorageError(msg) => {
                tracing::error!("Storage error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::ParseError(msg) => {
                tracing::error!("Parse error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::Parquet(e) => {
                tracing::error!("Parquet error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::Arrow(e) => {
                tracing::error!("Arrow error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::Io(e) => {
                tracing::error!("IO error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            Self::Query(e) => {
                tracing::error!("Query error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
        };

        let body = serde_json::json!({
            "error": message,
            "code": status.as_u16(),
        });

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otel_error_display_unsupported_content_type() {
        let err = OtelError::UnsupportedContentType("text/plain".to_string());
        assert_eq!(err.to_string(), "Unsupported content type");
    }

    #[test]
    fn test_otel_error_display_buffer_full() {
        let err = OtelError::BufferFull;
        assert_eq!(err.to_string(), "Server temporarily unavailable");
    }

    #[test]
    fn test_otel_error_display_disk_space_critical() {
        let err = OtelError::DiskSpaceCritical(95);
        assert_eq!(err.to_string(), "Server temporarily unavailable");
    }

    #[test]
    fn test_otel_error_display_validation_error() {
        let err = OtelError::ValidationError("invalid span".to_string());
        assert_eq!(err.to_string(), "Invalid request: invalid span");
    }

    #[test]
    fn test_otel_error_display_trace_not_found() {
        let err = OtelError::TraceNotFound("abc123".to_string());
        assert_eq!(err.to_string(), "Trace not found");
    }

    #[test]
    fn test_otel_error_display_too_many_connections() {
        let err = OtelError::TooManyConnections;
        assert_eq!(err.to_string(), "Too many connections");
    }

    #[test]
    fn test_otel_error_display_connection_timeout() {
        let err = OtelError::ConnectionTimeout;
        assert_eq!(err.to_string(), "Connection timeout");
    }

    #[test]
    fn test_otel_error_display_migration_failed() {
        let err = OtelError::MigrationFailed {
            version: 1,
            name: "initial".to_string(),
            error: "syntax error".to_string(),
        };
        assert_eq!(err.to_string(), "Database migration failed");
    }

    #[test]
    fn test_otel_error_display_schema_validation_failed() {
        let err = OtelError::SchemaValidationFailed("missing column".to_string());
        assert_eq!(err.to_string(), "Schema validation failed");
    }

    #[test]
    fn test_otel_error_display_storage_error() {
        let err = OtelError::StorageError("disk full".to_string());
        assert_eq!(err.to_string(), "Storage error: disk full");
    }

    #[test]
    fn test_otel_error_display_parse_error() {
        let err = OtelError::ParseError("invalid json".to_string());
        assert_eq!(err.to_string(), "Parse error: invalid json");
    }

    #[test]
    fn test_otel_error_display_channel_send() {
        let err = OtelError::ChannelSend;
        assert_eq!(err.to_string(), "Server error");
    }

    #[test]
    fn test_otel_error_debug() {
        let err = OtelError::BufferFull;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("BufferFull"));
    }
}
