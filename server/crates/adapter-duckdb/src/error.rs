//! DuckDB error types.

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DuckdbError {
    #[error("Database error: {0}")]
    Database(#[from] duckdb::Error),

    #[error(transparent)]
    UnsupportedSchema(sideseat_core::schema_version::UnsupportedSchema),

    #[error(transparent)]
    LayoutMismatch(sideseat_core::schema_version::LayoutMismatch),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
}

/// This adapter's error, as the port's error.
///
/// The conversion belongs to the adapter: an adapter knows the port it implements, while the port must not
/// depend on concrete implementations.
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
            DuckdbError::UnsupportedSchema(refusal) => Self::unsupported_schema("duckdb", refusal),
            DuckdbError::LayoutMismatch(mismatch) => Self::UnsupportedSchema {
                backend: "duckdb",
                found: mismatch.version,
                supported: mismatch.version,
                detail: mismatch.to_string(),
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
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let duckdb_err: DuckdbError = io_err.into();
        assert!(duckdb_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_timeout_error_display() {
        let err = DuckdbError::Timeout { timeout_secs: 30 };
        assert_eq!(err.to_string(), "Query timeout after 30s");
    }
}
