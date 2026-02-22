//! File storage trait definition
//!
//! Defines the interface for file storage backends (filesystem, S3, etc.)

use async_trait::async_trait;

use super::error::FileStorageError;

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
        project_id: &str,
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
    async fn get(&self, project_id: &str, hash: &str) -> Result<Vec<u8>, FileStorageError>;

    /// Check if a file exists
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash of the file
    async fn exists(&self, project_id: &str, hash: &str) -> Result<bool, FileStorageError>;

    /// Delete a file
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    /// * `hash` - Content hash of the file
    ///
    /// # Notes
    /// Does not fail if file doesn't exist.
    async fn delete(&self, project_id: &str, hash: &str) -> Result<(), FileStorageError>;

    /// Delete all files for a project
    ///
    /// # Arguments
    /// * `project_id` - Project identifier
    ///
    /// # Returns
    /// Number of files deleted
    async fn delete_project(&self, project_id: &str) -> Result<u64, FileStorageError>;

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
        project_id: &str,
        hash: &str,
        temp_path: &std::path::Path,
    ) -> Result<(), FileStorageError>;
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
