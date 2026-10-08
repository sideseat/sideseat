//! Unified error type for data layer
//!
//! This module provides a unified error type that can represent errors from
//! all database backends (DuckDB, PostgreSQL, SQLite, ClickHouse).

use thiserror::Error;

/// A driver's error, type-erased.
///
/// `Send + Sync` because these cross task boundaries, and `'static` because they outlive the call that made
/// them. Erased rather than concrete so this crate names no driver.
pub type BoxedSource = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Unified error type for data layer operations
///
/// This error type wraps backend-specific errors while preserving context
/// about which backend generated the error.
#[derive(Error, Debug)]
pub enum DataError {
    // The four backend variants carry the driver's **message and its error as `dyn Error`**, not the driver's
    // concrete error type.
    //
    // They used to carry `sqlx::Error`, `duckdb::Error` and `clickhouse::error::Error` directly, which made this
    // - the type every port method returns - name four drivers. Anything that called a port therefore depended
    // on all four, whatever it actually used, and no crate boundary could exist here at all.
    //
    // Nothing outside this module ever matched on the payloads. A bare `String` was the first attempt and
    // over-claimed: `#[from]` had made the driver error reachable through `Error::source()`, so a reporter walking
    // the chain got `None` where it used to get the driver's error, and that *is* observable. Keeping it as
    // `Box<dyn Error>` preserves the chain while leaving the driver unnamed here - which is the property this
    // crate exists for. Downcasting to a concrete driver error is still gone, deliberately: it is a layer
    // violation by definition, and nothing did it.
    /// SQLite database error (transactional backend)
    #[error("SQLite error: {message}")]
    Sqlite {
        message: String,
        transient: bool,
        /// The driver's own error, kept as `dyn Error` so the chain survives without naming the driver.
        #[source]
        source: Option<BoxedSource>,
    },

    /// PostgreSQL database error (transactional backend)
    #[error("PostgreSQL error: {message}")]
    Postgres {
        message: String,
        transient: bool,
        /// The driver's own error, kept as `dyn Error` so the chain survives without naming the driver.
        #[source]
        source: Option<BoxedSource>,
    },

    /// DuckDB database error (analytics backend)
    #[error("DuckDB error: {message}")]
    Duckdb {
        message: String,
        transient: bool,
        /// The driver's own error, kept as `dyn Error` so the chain survives without naming the driver.
        #[source]
        source: Option<BoxedSource>,
    },

    /// ClickHouse database error (analytics backend)
    #[error("ClickHouse error: {message}")]
    Clickhouse {
        message: String,
        transient: bool,
        /// The driver's own error, kept as `dyn Error` so the chain survives without naming the driver.
        #[source]
        source: Option<BoxedSource>,
    },

    /// The store is at a schema version this build does not read; see `sideseat_core::schema_version`.
    #[error("{backend}: {detail}")]
    UnsupportedSchema {
        backend: &'static str,
        found: i32,
        supported: i32,
        detail: String,
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

    /// A write spanning several projects committed some of them before it failed.
    ///
    /// A backend that writes projects separately - ClickHouse writes each tenant's rows in its own insert - can
    /// commit one project and then fail another. Reporting that as a plain failure made the caller undo the
    /// committed project's bookkeeping too, releasing file associations its readable rows still need. A backend
    /// that writes the batch atomically never returns this.
    #[error("Partially written ({} projects committed): {source}", committed_projects.len())]
    PartiallyWritten {
        committed_projects: Vec<String>,
        #[source]
        source: Box<DataError>,
    },

    /// A write that failed without settling: it may have been applied, or may still be.
    ///
    /// A rejection is settled - the backend ran the write to its end, and whatever it stored is stored, so a read
    /// made afterwards sees it. A lost connection, a timeout, or a wait that expired leaves the write running
    /// where the caller cannot see it, and a read made now does not prove its rows absent. A caller must not undo
    /// bookkeeping such rows could still need on the strength of a read.
    #[error("Write outcome unknown: {source}")]
    InDoubt {
        /// The one project whose write is in doubt, for a backend that writes projects separately; `None` when
        /// the whole write is.
        project: Option<String>,
        #[source]
        source: Box<DataError>,
    },
}

impl DataError {
    /// A SQLite error, with the adapter's verdict on whether it is worth retrying.
    ///
    /// The verdict is a parameter rather than something this type works out, because working it out means
    /// matching on `sqlx::Error` - and a port that matches on a driver's error variants is a port that depends on
    /// the driver. The adapter knows; this type records.
    pub fn from_sqlite(
        message: impl Into<String>,
        transient: bool,
        source: Option<BoxedSource>,
    ) -> Self {
        Self::Sqlite {
            message: message.into(),
            transient,
            source,
        }
    }

    /// A PostgreSQL error, with the adapter's verdict on whether it is worth retrying.
    pub fn from_postgres(
        message: impl Into<String>,
        transient: bool,
        source: Option<BoxedSource>,
    ) -> Self {
        Self::Postgres {
            message: message.into(),
            transient,
            source,
        }
    }

    /// A store this build refuses to open, with the operator guidance the refusal carries.
    pub fn unsupported_schema(
        backend: &'static str,
        refusal: sideseat_core::schema_version::UnsupportedSchema,
    ) -> Self {
        Self::UnsupportedSchema {
            backend,
            found: refusal.found,
            supported: refusal.supported,
            detail: refusal.to_string(),
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

    /// Whether this is worth retrying.
    ///
    /// **Read from the flag, not derived here.** Deriving it meant matching on `sqlx::Error`'s variants and
    /// string-searching a ClickHouse error's `Display` output, so this type - the one every port method returns -
    /// named the drivers, and anything calling a port depended on all four. Each adapter now decides at
    /// conversion time, which is the only place that knows what its driver's errors mean.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Timeout { .. } | Self::PoolExhausted { .. } | Self::InDoubt { .. } => true,
            Self::Sqlite { transient, .. }
            | Self::Postgres { transient, .. }
            | Self::Duckdb { transient, .. }
            | Self::Clickhouse { transient, .. } => *transient,
            _ => false,
        }
    }

    /// Whether a failed write may still have been applied where a read made now cannot see it - see
    /// [`Self::InDoubt`]. A partial write is in doubt when the project that failed it is.
    pub fn write_in_doubt(&self) -> bool {
        match self {
            Self::InDoubt { .. } => true,
            Self::PartiallyWritten { source, .. } => source.write_in_doubt(),
            _ => false,
        }
    }

    /// Which of `unwritten` - the projects a failed write did not commit - may still have rows land: the one
    /// project an in-doubt error names, or all of them when it names none; nothing when the failure is settled.
    pub fn projects_in_doubt<'a>(
        &self,
        unwritten: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        match self {
            Self::InDoubt {
                project: Some(project),
                ..
            } => vec![project.clone()],
            Self::InDoubt { project: None, .. } => {
                unwritten.into_iter().map(str::to_string).collect()
            }
            Self::PartiallyWritten { source, .. } => source.projects_in_doubt(unwritten),
            _ => Vec::new(),
        }
    }

    /// Get the backend name that generated this error
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Sqlite { .. } => "sqlite",
            Self::Postgres { .. } => "postgres",
            Self::Duckdb { .. } => "duckdb",
            Self::Clickhouse { .. } => "clickhouse",
            Self::UnsupportedSchema { backend, .. } => backend,
            Self::Timeout { backend, .. } => backend,
            Self::PoolExhausted { backend } => backend,
            Self::BackendUnavailable { backend, .. } => backend,
            Self::PartiallyWritten { source, .. } | Self::InDoubt { source, .. } => {
                source.backend()
            }
            Self::Config(_) | Self::Io(_) | Self::NotImplemented(_) | Self::Conflict(_) => {
                "unknown"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed multi-project write is in doubt only for the project whose insert might still land: the ones a
    /// backend never sent have a settled answer, and marking them in doubt kept their associations for ever.
    #[test]
    fn only_the_named_project_of_an_in_doubt_write_is_in_doubt() {
        let settled = || DataError::Conflict("rejected".to_string());
        let unwritten = ["a", "b", "c"];
        let named = DataError::InDoubt {
            project: Some("b".to_string()),
            source: Box::new(settled()),
        };
        assert_eq!(named.projects_in_doubt(unwritten), vec!["b".to_string()]);
        let whole = DataError::InDoubt {
            project: None,
            source: Box::new(settled()),
        };
        assert_eq!(whole.projects_in_doubt(unwritten).len(), 3);
        let partial = DataError::PartiallyWritten {
            committed_projects: vec!["z".to_string()],
            source: Box::new(named),
        };
        assert_eq!(partial.projects_in_doubt(unwritten), vec!["b".to_string()]);
        assert!(settled().projects_in_doubt(unwritten).is_empty());
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
            DataError::unsupported_schema(
                "sqlite",
                sideseat_core::schema_version::UnsupportedSchema {
                    found: 1,
                    supported: 2
                }
            )
            .backend(),
            "sqlite"
        );
    }

    #[test]
    fn test_is_transient() {
        assert!(DataError::timeout("duckdb", 30).is_transient());
        assert!(DataError::pool_exhausted("postgres").is_transient());
        assert!(!DataError::Config("bad config".into()).is_transient());
        assert!(
            !DataError::unsupported_schema(
                "sqlite",
                sideseat_core::schema_version::UnsupportedSchema {
                    found: 1,
                    supported: 2
                }
            )
            .is_transient()
        );
    }
}
