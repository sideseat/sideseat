//! Where a file's bytes live: the blob store, as a port.
//!
//! **A port, not a service.** `FileStorage` was a trait in the data layer, so the domain reached into an adapter
//! module to name the abstraction it depends on - and `FileStorageError` with it. Neither has any dependency of
//! its own, which is what made this a move rather than a redesign: the trait was already the tightest seam in the
//! codebase, and the plan calls it the model for the rest.
//!
//! The filesystem and S3 implementations stay where they are. What moved is the statement of what they must do.

use async_trait::async_trait;
use thiserror::Error;

use crate::traits::SurvivorReferences;
use crate::types::ProjectId;

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

/// File content with metadata
#[derive(Debug)]
pub struct FileContent {
    /// Raw file bytes
    pub data: Vec<u8>,
    /// MIME type if known
    pub media_type: Option<String>,
}

/// Trait for file storage backends
///
/// All implementations must be thread-safe (Send + Sync) for use in async contexts.
/// Files are organized per-project with content-addressed storage using content hashes.
#[async_trait]
pub trait FileStorage: Send + Sync {
    /// Store a file
    ///
    /// # Arguments
    /// * `project_id` - Project identifier for isolation
    /// * `hash` - Content hash of the file (64 hex chars)
    /// * `data` - File bytes to store
    ///
    /// # Notes
    /// If a file with the same hash already exists, this is a no-op (content-addressed).
    async fn store(
        &self,
        project_id: &ProjectId,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError>;

    /// Retrieve a file
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash of the file
    ///
    /// # Returns
    /// File bytes or NotFound error
    async fn get(&self, project_id: &ProjectId, hash: &str) -> Result<Vec<u8>, FileStorageError>;

    /// Check if a file exists
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash of the file
    async fn exists(&self, project_id: &ProjectId, hash: &str) -> Result<bool, FileStorageError>;

    /// Delete a file
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash of the file
    ///
    /// # Notes
    /// Does not fail if file doesn't exist.
    async fn delete(&self, project_id: &ProjectId, hash: &str) -> Result<(), FileStorageError>;

    /// Delete all files for a project
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    ///
    /// # Returns
    /// Number of files deleted
    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError>;

    /// Move a file from temp storage to permanent storage
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash (used as filename)
    /// * `temp_path` - Path to the temporary file
    ///
    /// # Notes
    /// This is an atomic operation on local filesystem (rename).
    /// For S3, this uploads from temp and deletes temp.
    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        hash: &str,
        temp_path: &std::path::Path,
    ) -> Result<(), FileStorageError>;
}

/// File-side retention work required by an analytics adapter after span expiry.
#[async_trait]
pub trait RetentionFileReconciler: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn reconcile_body_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn SurvivorReferences,
    ) -> Result<(), String>;

    async fn reconcile_trace_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn SurvivorReferences,
    ) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_content_debug() {
        let content = FileContent {
            data: vec![1, 2, 3],
            media_type: Some("image/png".to_string()),
        };
        let debug = format!("{:?}", content);
        assert!(debug.contains("FileContent"));
        assert!(debug.contains("image/png"));
    }
}
