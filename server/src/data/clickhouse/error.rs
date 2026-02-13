//! ClickHouse error types

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
