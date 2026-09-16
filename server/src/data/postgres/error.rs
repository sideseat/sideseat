//! PostgreSQL error types

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PostgresError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Migration {version} ({name}) failed: {error}")]
    MigrationFailed {
        version: i32,
        name: String,
        error: String,
    },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Conflict: {0}")]
    Conflict(String),
}

/// This adapter's error, as the port's error.
///
/// **Here rather than beside `DataError`.** The conversion used to live in `data::error`, which made the port's
/// error type name every adapter - the dependency exactly inverted, and enough on its own to stop `ports` being
/// a crate. An adapter knows the port it implements; the port must not know its implementations. The orphan rule
/// allows only these two homes, and this is the one that points the right way.
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
            },
            PostgresError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "postgres",
                version,
                name,
                error,
            },
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
    fn test_migration_failed_error_display() {
        let err = PostgresError::MigrationFailed {
            version: 2,
            name: "add_users_table".to_string(),
            error: "syntax error".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Migration 2 (add_users_table) failed: syntax error"
        );
    }

    #[test]
    fn test_config_error_display() {
        let err = PostgresError::Config("missing URL".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing URL");
    }
}
