//! DuckDB error types.

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DuckdbError {
    #[error("Database error: {0}")]
    Database(#[from] duckdb::Error),

    #[error("Migration {version} ({name}) failed: {error}")]
    MigrationFailed {
        version: i32,
        name: String,
        error: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

/// This adapter's error, as the port's error.
///
/// **Here rather than beside `DataError`.** The conversion used to live in `data::error`, which made the port's
/// error type name every adapter - the dependency exactly inverted, and enough on its own to stop `ports` being
/// a crate. An adapter knows the port it implements; the port must not know its implementations. The orphan rule
/// allows only these two homes, and this is the one that points the right way.
impl From<DuckdbError> for DataError {
    fn from(e: DuckdbError) -> Self {
        match e {
            // DuckDB is embedded and single-connection here, so there is no pool to be busy and no network
            // to drop: a database error means the statement was wrong, and retrying it will be wrong again.
            DuckdbError::Database(e) => Self::Duckdb {
                message: e.to_string(),
                transient: false,
                source: Some(Box::new(e)),
            },
            DuckdbError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "duckdb",
                version,
                name,
                error,
            },
            DuckdbError::Io(e) => Self::Io(e),
            DuckdbError::Timeout { timeout_secs } => Self::Timeout {
                backend: "duckdb",
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
        let err = DuckdbError::MigrationFailed {
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
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let duckdb_err: DuckdbError = io_err.into();
        assert!(duckdb_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_debug() {
        let err = DuckdbError::MigrationFailed {
            version: 1,
            name: "test".to_string(),
            error: "error".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("MigrationFailed"));
        assert!(debug_str.contains("version: 1"));
    }

    #[test]
    fn test_timeout_error_display() {
        let err = DuckdbError::Timeout { timeout_secs: 30 };
        assert_eq!(err.to_string(), "Query timeout after 30s");
    }
}
