//! Unified error type for data layer
//!
//! This module provides a unified error type that can represent errors from
//! all database backends (DuckDB, PostgreSQL, SQLite, ClickHouse).

use thiserror::Error;

/// Unified error type for data layer operations
///
/// This error type wraps backend-specific errors while preserving context
/// about which backend generated the error.
#[derive(Error, Debug)]
pub enum DataError {
    /// SQLite database error (transactional backend)
    #[error("SQLite error: {0}")]
    Sqlite(sqlx::Error),

    /// PostgreSQL database error (transactional backend)
    #[error("PostgreSQL error: {0}")]
    Postgres(sqlx::Error),

    /// DuckDB database error (analytics backend)
    #[error("DuckDB error: {0}")]
    Duckdb(#[from] duckdb::Error),

    /// ClickHouse database error (analytics backend)
    #[error("ClickHouse error: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),

    /// Migration failed
    #[error("Migration {version} ({name}) failed on {backend}: {error}")]
    MigrationFailed {
        backend: &'static str,
        version: i32,
        name: String,
        error: String,
    },

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Query timeout
    #[error("Query timeout after {timeout_secs}s on {backend}")]
    Timeout {
        backend: &'static str,
        timeout_secs: u64,
    },

    /// Connection pool exhausted
    #[error("Connection pool exhausted on {backend}")]
    PoolExhausted { backend: &'static str },

    /// Backend not available
    #[error("Backend {backend} is not available: {reason}")]
    BackendUnavailable {
        backend: &'static str,
        reason: String,
    },

    /// Operation not implemented for this backend
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Conflict error (e.g., limit reached, duplicate entry)
    #[error("Conflict: {0}")]
    Conflict(String),
}

impl DataError {
    /// Create a SQLite error with preserved context
    pub fn from_sqlite(e: sqlx::Error) -> Self {
        Self::Sqlite(e)
    }

    /// Create a PostgreSQL error with preserved context
    pub fn from_postgres(e: sqlx::Error) -> Self {
        Self::Postgres(e)
    }

    /// Create a migration failed error
    pub fn migration_failed(backend: &'static str, version: i32, name: &str, error: &str) -> Self {
        Self::MigrationFailed {
            backend,
            version,
            name: name.to_string(),
            error: error.to_string(),
        }
    }

    /// Create a timeout error
    pub fn timeout(backend: &'static str, timeout_secs: u64) -> Self {
        Self::Timeout {
            backend,
            timeout_secs,
        }
    }

    /// Create a pool exhausted error
    pub fn pool_exhausted(backend: &'static str) -> Self {
        Self::PoolExhausted { backend }
    }

    /// Create a backend unavailable error
    pub fn backend_unavailable(backend: &'static str, reason: impl Into<String>) -> Self {
        Self::BackendUnavailable {
            backend,
            reason: reason.into(),
        }
    }

    /// Check if this is a connection-related error that might be transient
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Timeout { .. } | Self::PoolExhausted { .. } => true,
            Self::Sqlite(e) | Self::Postgres(e) => {
                matches!(
                    e,
                    sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_)
                )
            }
            Self::Duckdb(_) => false, // DuckDB errors are typically not transient
            Self::Clickhouse(e) => {
                // Check if it's a network/connection error
                e.to_string().contains("connection")
                    || e.to_string().contains("timeout")
                    || e.to_string().contains("network")
            }
            _ => false,
        }
    }

    /// Get the backend name that generated this error
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            Self::Postgres(_) => "postgres",
            Self::Duckdb(_) => "duckdb",
            Self::Clickhouse(_) => "clickhouse",
            Self::MigrationFailed { backend, .. } => backend,
            Self::Timeout { backend, .. } => backend,
            Self::PoolExhausted { backend } => backend,
            Self::BackendUnavailable { backend, .. } => backend,
            Self::Config(_) | Self::Io(_) | Self::NotImplemented(_) | Self::Conflict(_) => {
                "unknown"
            }
        }
    }
}

/// Convert from the existing DuckdbError type
impl From<crate::data::duckdb::DuckdbError> for DataError {
    fn from(e: crate::data::duckdb::DuckdbError) -> Self {
        match e {
            crate::data::duckdb::DuckdbError::Database(e) => Self::Duckdb(e),
            crate::data::duckdb::DuckdbError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "duckdb",
                version,
                name,
                error,
            },
            crate::data::duckdb::DuckdbError::Io(e) => Self::Io(e),
            crate::data::duckdb::DuckdbError::Timeout { timeout_secs } => Self::Timeout {
                backend: "duckdb",
                timeout_secs,
            },
        }
    }
}

/// Convert from the existing SqliteError type
impl From<crate::data::sqlite::SqliteError> for DataError {
    fn from(e: crate::data::sqlite::SqliteError) -> Self {
        match e {
            crate::data::sqlite::SqliteError::Database(e) => Self::Sqlite(e),
            crate::data::sqlite::SqliteError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "sqlite",
                version,
                name,
                error,
            },
            crate::data::sqlite::SqliteError::Io(e) => Self::Io(e),
            crate::data::sqlite::SqliteError::Conflict(msg) => Self::Conflict(msg),
        }
    }
}

/// Convert from the existing PostgresError type
impl From<crate::data::postgres::PostgresError> for DataError {
    fn from(e: crate::data::postgres::PostgresError) -> Self {
        match e {
            crate::data::postgres::PostgresError::Database(e) => Self::Postgres(e),
            crate::data::postgres::PostgresError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "postgres",
                version,
                name,
                error,
            },
            crate::data::postgres::PostgresError::Config(msg) => Self::Config(msg),
            crate::data::postgres::PostgresError::Io(e) => Self::Io(e),
            crate::data::postgres::PostgresError::Conflict(msg) => Self::Conflict(msg),
        }
    }
}

/// Convert from the existing ClickhouseError type
impl From<crate::data::clickhouse::ClickhouseError> for DataError {
    fn from(e: crate::data::clickhouse::ClickhouseError) -> Self {
        match e {
            crate::data::clickhouse::ClickhouseError::Database(e) => Self::Clickhouse(e),
            crate::data::clickhouse::ClickhouseError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "clickhouse",
                version,
                name,
                error,
            },
            crate::data::clickhouse::ClickhouseError::Connection(msg) => Self::Config(msg),
            crate::data::clickhouse::ClickhouseError::Io(e) => Self::Io(e),
            crate::data::clickhouse::ClickhouseError::Timeout { timeout_secs } => Self::Timeout {
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
        let err = DataError::migration_failed("postgres", 2, "add_users_table", "syntax error");
        assert_eq!(
            err.to_string(),
            "Migration 2 (add_users_table) failed on postgres: syntax error"
        );
    }

    #[test]
    fn test_timeout_error_display() {
        let err = DataError::timeout("duckdb", 30);
        assert_eq!(err.to_string(), "Query timeout after 30s on duckdb");
    }

    #[test]
    fn test_pool_exhausted_error_display() {
        let err = DataError::pool_exhausted("postgres");
        assert_eq!(err.to_string(), "Connection pool exhausted on postgres");
    }

    #[test]
    fn test_backend_unavailable_error_display() {
        let err = DataError::backend_unavailable("clickhouse", "connection refused");
        assert_eq!(
            err.to_string(),
            "Backend clickhouse is not available: connection refused"
        );
    }

    #[test]
    fn test_backend_method() {
        assert_eq!(DataError::timeout("duckdb", 30).backend(), "duckdb");
        assert_eq!(DataError::pool_exhausted("postgres").backend(), "postgres");
        assert_eq!(
            DataError::migration_failed("sqlite", 1, "test", "error").backend(),
            "sqlite"
        );
    }

    #[test]
    fn test_is_transient() {
        assert!(DataError::timeout("duckdb", 30).is_transient());
        assert!(DataError::pool_exhausted("postgres").is_transient());
        assert!(!DataError::Config("bad config".into()).is_transient());
        assert!(!DataError::migration_failed("sqlite", 1, "test", "error").is_transient());
    }
}
