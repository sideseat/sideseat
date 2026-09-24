//! ClickHouse error types.

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClickhouseError {
    #[error("Database error: {0}")]
    Database(#[from] clickhouse::error::Error),

    #[error("Migration {version} ({name}) failed: {error}")]
    MigrationFailed {
        version: i32,
        name: String,
        error: String,
    },

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

/// This adapter's error, as the port's error.
///
/// The conversion belongs to the adapter: an adapter knows the port it implements, while the port must not
/// depend on concrete implementations.
impl From<ClickhouseError> for DataError {
    fn from(e: ClickhouseError) -> Self {
        match e {
            // The transience verdict is made **here**, where the driver's error is still in hand. It is a
            // string search, which is a guess - the driver does not classify - but it is a guess about
            // ClickHouse, made in the ClickHouse adapter, rather than one the port makes about a driver it
            // should not know.
            ClickhouseError::Database(e) => {
                let message = e.to_string();
                let transient = message.contains("connection")
                    || message.contains("timeout")
                    || message.contains("network");
                Self::Clickhouse {
                    message,
                    transient,
                    source: Some(Box::new(e)),
                }
            }
            ClickhouseError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "clickhouse",
                version,
                name,
                error,
            },
            ClickhouseError::Connection(msg) => Self::Config(msg),
            ClickhouseError::Io(e) => Self::Io(e),
            ClickhouseError::Timeout { timeout_secs } => Self::Timeout {
                backend: "clickhouse",
                timeout_secs,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_failed_error_display() {
        let err = ClickhouseError::MigrationFailed {
            version: 2,
            name: "add_analytics_table".to_string(),
            error: "syntax error".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Migration 2 (add_analytics_table) failed: syntax error"
        );
    }

    #[test]
    fn test_connection_error_display() {
        let err = ClickhouseError::Connection("connection refused".to_string());
        assert_eq!(err.to_string(), "Connection error: connection refused");
    }

    #[test]
    fn test_timeout_error_display() {
        let err = ClickhouseError::Timeout { timeout_secs: 30 };
        assert_eq!(err.to_string(), "Query timeout after 30s");
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let ch_err: ClickhouseError = io_err.into();
        assert!(ch_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_debug() {
        let err = ClickhouseError::MigrationFailed {
            version: 1,
            name: "test".to_string(),
            error: "error".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("MigrationFailed"));
        assert!(debug_str.contains("version: 1"));
    }
}
