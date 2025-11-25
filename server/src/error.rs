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
