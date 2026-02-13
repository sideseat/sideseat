//! SQLite error types

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
