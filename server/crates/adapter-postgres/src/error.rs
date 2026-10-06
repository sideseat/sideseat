//! PostgreSQL error types.

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PostgresError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    UnsupportedSchema(sideseat_core::schema_version::UnsupportedSchema),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Conflict: {0}")]
    Conflict(String),
}

/// This adapter's error, as the port's error.
///
/// The conversion belongs to the adapter: an adapter knows the port it implements, while the port must not
/// depend on concrete implementations.
impl From<PostgresError> for DataError {
    fn from(e: PostgresError) -> Self {
        match e {
            // The same `sqlx` classification as the SQLite twin, and for the same reason it lives here.
            PostgresError::Database(e) => Self::Postgres {
                transient: matches!(
                    e,
                    sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_)
                ),
                message: e.to_string(),
                source: Some(Box::new(e)),
            },
            PostgresError::UnsupportedSchema(refusal) => {
                Self::unsupported_schema("postgres", refusal)
            }
            PostgresError::Config(msg) => Self::Config(msg),
            PostgresError::Io(e) => Self::Io(e),
            PostgresError::Conflict(msg) => Self::Conflict(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_display() {
        let err = PostgresError::Config("missing URL".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing URL");
    }
}

/// Pins PostgreSQL transience classification and driver source chaining.
#[cfg(test)]
mod port_error_tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn a_pool_failure_is_transient_and_a_query_failure_is_not() {
        let pool: DataError = PostgresError::Database(sqlx::Error::PoolTimedOut).into();
        assert!(pool.is_transient(), "a pool timeout is worth retrying");
        let query: DataError = PostgresError::Database(sqlx::Error::RowNotFound).into();
        assert!(!query.is_transient(), "a missing row is not");
    }

    #[test]
    fn the_drivers_error_is_still_reachable_through_the_source_chain() {
        let err: DataError = PostgresError::Database(sqlx::Error::PoolTimedOut).into();
        assert!(
            err.source().is_some(),
            "the driver's error left the source chain"
        );
    }
}
