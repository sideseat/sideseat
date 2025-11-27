use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Secret storage error: {0}")]
    Secret(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("OpenTelemetry error: {0}")]
    Otel(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// Convert our Error type into an HTTP response
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Error::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Error::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Error::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Error::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Error::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Error::Storage(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Error::Secret(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Error::Auth(msg) => (StatusCode::UNAUTHORIZED, msg),
            Error::Otel(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        tracing::error!("Error response: {} - {}", status, message);

        (status, message).into_response()
    }
}

// Convert SQLx errors
impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Database(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_internal() {
        let err = Error::Internal("test error".to_string());
        assert_eq!(err.to_string(), "Internal server error: test error");
    }

    #[test]
    fn test_error_display_config() {
        let err = Error::Config("invalid config".to_string());
        assert_eq!(err.to_string(), "Configuration error: invalid config");
    }

    #[test]
    fn test_error_display_database() {
        let err = Error::Database("connection failed".to_string());
        assert_eq!(err.to_string(), "Database error: connection failed");
    }

    #[test]
    fn test_error_display_not_found() {
        let err = Error::NotFound("resource".to_string());
        assert_eq!(err.to_string(), "Not found: resource");
    }

    #[test]
    fn test_error_display_bad_request() {
        let err = Error::BadRequest("invalid input".to_string());
        assert_eq!(err.to_string(), "Bad request: invalid input");
    }

    #[test]
    fn test_error_display_storage() {
        let err = Error::Storage("disk full".to_string());
        assert_eq!(err.to_string(), "Storage error: disk full");
    }

    #[test]
    fn test_error_display_secret() {
        let err = Error::Secret("keychain error".to_string());
        assert_eq!(err.to_string(), "Secret storage error: keychain error");
    }

    #[test]
    fn test_error_display_auth() {
        let err = Error::Auth("invalid token".to_string());
        assert_eq!(err.to_string(), "Authentication error: invalid token");
    }

    #[test]
    fn test_error_display_otel() {
        let err = Error::Otel("collector error".to_string());
        assert_eq!(err.to_string(), "OpenTelemetry error: collector error");
    }

    #[test]
    fn test_error_debug() {
        let err = Error::Internal("test".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Internal"));
        assert!(debug_str.contains("test"));
    }
}
