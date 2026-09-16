//! SQLite error types

use sideseat_ports::error::DataError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SqliteError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Migration {version} ({name}) failed: {error}")]
    MigrationFailed {
        version: i32,
        name: String,
        error: String,
    },

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
impl From<SqliteError> for DataError {
    fn from(e: SqliteError) -> Self {
        match e {
            // What `sqlx` calls transient, decided where `sqlx::Error` is visible.
            SqliteError::Database(e) => Self::Sqlite {
                transient: matches!(
                    e,
                    sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_)
                ),
                message: e.to_string(),
            },
            SqliteError::MigrationFailed {
                version,
                name,
                error,
            } => Self::MigrationFailed {
                backend: "sqlite",
                version,
                name,
                error,
            },
            SqliteError::Io(e) => Self::Io(e),
            SqliteError::Conflict(msg) => Self::Conflict(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_failed_error_display() {
        let err = SqliteError::MigrationFailed {
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
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let sqlite_err: SqliteError = io_err.into();
        assert!(sqlite_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_error_debug() {
        let err = SqliteError::MigrationFailed {
            version: 1,
            name: "test".to_string(),
            error: "error".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("MigrationFailed"));
        assert!(debug_str.contains("version: 1"));
    }
}
