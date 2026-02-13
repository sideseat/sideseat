//! PostgreSQL error types

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
