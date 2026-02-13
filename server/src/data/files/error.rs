//! File storage error types

use thiserror::Error;

use crate::data::error::DataError;

/// Errors from low-level file storage operations (filesystem/S3)
#[derive(Error, Debug)]
pub enum FileStorageError {
    #[error("File not found: {project_id}/{hash}")]
    NotFound { project_id: String, hash: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Storage backend error: {0}")]
    Backend(String),
}

/// Errors from the high-level file service
#[derive(Error, Debug)]
pub enum FileServiceError {
    #[error("File not found: {project_id}/{hash}")]
    NotFound { project_id: String, hash: String },

    #[error("Storage error: {0}")]
    Storage(#[from] FileStorageError),

    #[error("Database error: {0}")]
    Database(#[from] DataError),

    #[error("Invalid hash format: expected 64 hex characters")]
    InvalidHash,

    #[error("File too large: {size} bytes (max: {max})")]
    TooLarge { size: usize, max: usize },

    #[error("File storage is disabled")]
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_storage_not_found_display() {
        let err = FileStorageError::NotFound {
            project_id: "default".to_string(),
            hash: "abc123".to_string(),
        };
        assert_eq!(err.to_string(), "File not found: default/abc123");
    }

    #[test]
    fn test_file_service_too_large_display() {
        let err = FileServiceError::TooLarge {
            size: 100_000_000,
            max: 50_000_000,
        };
        assert_eq!(
            err.to_string(),
            "File too large: 100000000 bytes (max: 50000000)"
        );
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let storage_err: FileStorageError = io_err.into();
        assert!(storage_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_storage_error_from() {
        let storage_err = FileStorageError::NotFound {
            project_id: "test".to_string(),
            hash: "hash123".to_string(),
        };
        let service_err: FileServiceError = storage_err.into();
        assert!(matches!(service_err, FileServiceError::Storage(_)));
    }
}
