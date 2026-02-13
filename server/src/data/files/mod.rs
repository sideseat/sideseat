//! File storage layer
//!
//! Provides binary file storage with deduplication for the application.
//! Files are stored outside DuckDB with SHA-256 hash-based content addressing.
//!
//! ## Architecture
//!
//! - `storage` - Trait definition for file storage backends
//! - `filesystem` - Local filesystem implementation
//! - `error` - Error types for file operations
//!
//! ## Storage Layout
//!
//! Files are organized per-project with sharded directories:
//! ```text
//! {base_path}/
//! └── {project_id}/
//!     └── {hash[0:2]}/
//!         └── {hash[2:4]}/
//!             └── {hash}
//! ```
//!
//! ## Usage
//!
//! ```text
//! let file_service = FileService::new(config, storage, database).await?;
//!
//! // Get file content
//! let content = file_service.get_file(project_id, hash).await?;
//!
//! // Cleanup after trace deletion
//! file_service.cleanup_traces(project_id, &trace_ids).await?;
//! ```

pub mod cleanup;
pub mod error;
pub mod filesystem;
pub mod s3;
pub mod storage;

use std::path::PathBuf;
use std::sync::Arc;

use crate::core::config::FilesConfig;
use crate::core::storage::{AppStorage, DataSubdir};
use crate::data::TransactionalService;

pub use error::{FileServiceError, FileStorageError};
pub use filesystem::FilesystemStorage;
pub use s3::S3Storage;
pub use storage::{FileContent, FileStorage};

/// File metadata without content
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// File size in bytes
    pub size_bytes: i64,
    /// MIME type (e.g., "image/png")
    pub media_type: Option<String>,
}

/// Main file service coordinating storage, metadata, and cleanup
pub struct FileService {
    /// Storage backend (filesystem or S3)
    storage: Arc<dyn FileStorage>,
    /// Transactional database for metadata operations
    database: Arc<TransactionalService>,
    /// Configuration
    config: FilesConfig,
    /// Path to temp directory
    temp_dir: PathBuf,
}

impl FileService {
    /// Create a new file service
    ///
    /// This function is async because S3 storage initialization requires loading AWS config.
    pub async fn new(
        config: FilesConfig,
        app_storage: &AppStorage,
        database: Arc<TransactionalService>,
    ) -> Result<Self, FileServiceError> {
        let temp_dir = app_storage.subdir(DataSubdir::FilesTemp);

        // Create storage backend based on config
        let storage: Arc<dyn FileStorage> = match config.storage {
            crate::core::config::StorageBackend::S3 => {
                let s3_config = config.s3.as_ref().ok_or_else(|| {
                    FileServiceError::Storage(FileStorageError::Backend(
                        "S3 storage configured but no s3 config provided (missing bucket)"
                            .to_string(),
                    ))
                })?;

                let s3_storage = s3::S3Storage::new(
                    s3_config.bucket.clone(),
                    s3_config.prefix.clone(),
                    s3_config.region.clone(),
                    s3_config.endpoint.clone(),
                )
                .await?;

                Arc::new(s3_storage)
            }
            crate::core::config::StorageBackend::Filesystem => {
                let files_path = config
                    .filesystem_path
                    .as_ref()
                    .map(|p| crate::utils::file::expand_path(p))
                    .unwrap_or_else(|| app_storage.subdir(DataSubdir::Files));

                Arc::new(FilesystemStorage::new(files_path))
            }
        };

        tracing::debug!(
            enabled = config.enabled,
            storage = %config.storage,
            quota_bytes = config.quota_bytes,
            "File service initialized"
        );

        let service = Self {
            storage,
            database,
            config,
            temp_dir,
        };

        // Run startup cleanup for orphan temp files
        if service.config.enabled
            && let Err(e) = cleanup::cleanup_orphan_temp_files(
                &service.temp_dir,
                &service.storage,
                &service.database,
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to cleanup orphan temp files on startup");
        }

        // Run startup cleanup for files with ref_count=0 (failed storage deletions)
        if service.config.enabled
            && let Err(e) =
                cleanup::cleanup_zero_ref_files(&service.storage, &service.database).await
        {
            tracing::warn!(error = %e, "Failed to cleanup zero-ref files on startup");
        }

        Ok(service)
    }

    /// Check if file storage is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the temp directory path for writing temp files
    pub fn temp_dir(&self) -> &PathBuf {
        &self.temp_dir
    }

    /// Get file content
    pub async fn get_file(
        &self,
        project_id: &str,
        hash: &str,
    ) -> Result<FileContent, FileServiceError> {
        if !self.config.enabled {
            return Err(FileServiceError::Disabled);
        }

        // Get metadata for media_type via repository trait
        let repo = self.database.repository();
        let media_type = repo
            .get_file(project_id, hash)
            .await?
            .and_then(|f| f.media_type);

        // Get data from storage
        let data = self
            .storage
            .get(project_id, hash)
            .await
            .map_err(|e| match e {
                FileStorageError::NotFound { .. } => FileServiceError::NotFound {
                    project_id: project_id.to_string(),
                    hash: hash.to_string(),
                },
                e => FileServiceError::Storage(e),
            })?;

        Ok(FileContent { data, media_type })
    }

    /// Check if a file exists
    pub async fn file_exists(
        &self,
        project_id: &str,
        hash: &str,
    ) -> Result<bool, FileServiceError> {
        if !self.config.enabled {
            return Ok(false);
        }

        Ok(self.storage.exists(project_id, hash).await?)
    }

    /// Get file metadata without loading content
    ///
    /// Returns size and media_type from database metadata.
    pub async fn get_file_metadata(
        &self,
        project_id: &str,
        hash: &str,
    ) -> Result<FileMetadata, FileServiceError> {
        if !self.config.enabled {
            return Err(FileServiceError::Disabled);
        }

        // Get metadata via repository trait
        let repo = self.database.repository();
        let file_row =
            repo.get_file(project_id, hash)
                .await?
                .ok_or_else(|| FileServiceError::NotFound {
                    project_id: project_id.to_string(),
                    hash: hash.to_string(),
                })?;

        // Verify file exists in storage
        if !self.storage.exists(project_id, hash).await? {
            return Err(FileServiceError::NotFound {
                project_id: project_id.to_string(),
                hash: hash.to_string(),
            });
        }

        Ok(FileMetadata {
            size_bytes: file_row.size_bytes,
            media_type: file_row.media_type,
        })
    }

    /// Cleanup files for deleted traces
    ///
    /// Decrements ref_count for each file associated with the traces.
    /// Deletes files when ref_count reaches 0.
    ///
    /// If storage deletion fails, the database metadata is preserved (with ref_count=0)
    /// so the startup cleanup job can retry later.
    pub async fn cleanup_traces(
        &self,
        project_id: &str,
        trace_ids: &[String],
    ) -> Result<(), FileServiceError> {
        if !self.config.enabled || trace_ids.is_empty() {
            return Ok(());
        }

        let repo = self.database.repository();

        // Get file hashes for these traces
        let file_hashes = repo
            .get_file_hashes_for_traces(project_id, trace_ids)
            .await?;

        // Delete trace-file associations
        repo.delete_trace_files(project_id, trace_ids).await?;

        // Decrement ref_count for each file, delete if zero
        for hash in file_hashes {
            let new_ref_count = repo.decrement_ref_count(project_id, &hash).await?;

            if new_ref_count == Some(0) {
                // Delete from storage first
                if let Err(e) = self.storage.delete(project_id, &hash).await {
                    tracing::warn!(
                        project_id,
                        hash,
                        error = %e,
                        "Failed to delete file from storage, keeping metadata for retry"
                    );
                    continue;
                }

                // Only delete metadata after successful storage deletion
                repo.delete_file(project_id, &hash).await?;

                tracing::debug!(project_id, hash, "Deleted orphaned file");
            }
        }

        Ok(())
    }

    /// Delete all files for a project
    pub async fn delete_project(&self, project_id: &str) -> Result<u64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }

        // Delete from storage
        let deleted = self.storage.delete_project(project_id).await?;

        // Delete metadata (CASCADE will handle trace_files)
        let repo = self.database.repository();
        repo.delete_project_files(project_id).await?;

        tracing::debug!(project_id, deleted, "Deleted all project files");

        Ok(deleted)
    }

    /// Get storage usage for a project
    pub async fn get_storage_bytes(&self, project_id: &str) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }

        let repo = self.database.repository();
        Ok(repo.get_project_storage_bytes(project_id).await?)
    }

    /// Get the storage backend
    pub fn storage(&self) -> &Arc<dyn FileStorage> {
        &self.storage
    }

    /// Get the transactional database
    pub fn database(&self) -> &Arc<TransactionalService> {
        &self.database
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::SqliteService;
    use tempfile::TempDir;
    use tokio::fs;

    fn test_hash() -> String {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string()
    }

    async fn setup_test() -> (TempDir, Arc<TransactionalService>) {
        let temp_dir = TempDir::new().unwrap();

        // Create SQLite pool with full schema (single connection for :memory: to ensure shared state)
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        for statement in crate::data::sqlite::schema::SCHEMA
            .split(';')
            .filter(|s| !s.trim().is_empty())
        {
            sqlx::query(statement.trim()).execute(&pool).await.unwrap();
        }

        // Create a TransactionalService wrapping the SQLite service
        let sqlite_service = SqliteService::from_pool(pool);
        let database = Arc::new(TransactionalService::Sqlite(Arc::new(sqlite_service)));

        (temp_dir, database)
    }

    #[tokio::test]
    async fn test_file_service_disabled() {
        let (temp_dir, database) = setup_test().await;

        let config = FilesConfig {
            enabled: false,
            storage: crate::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = FileService::new(config, &app_storage, database)
            .await
            .unwrap();

        assert!(!service.is_enabled());

        let result = service.get_file("default", &test_hash()).await;
        assert!(matches!(result, Err(FileServiceError::Disabled)));
    }

    #[tokio::test]
    async fn test_file_service_get_file() {
        let (temp_dir, database) = setup_test().await;

        let config = FilesConfig {
            enabled: true,
            storage: crate::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = FileService::new(config, &app_storage, database.clone())
            .await
            .unwrap();

        // Store a file directly
        service
            .storage
            .store("default", &test_hash(), b"test content")
            .await
            .unwrap();

        // Insert metadata via repository trait
        let repo = database.repository();
        repo.upsert_file("default", &test_hash(), Some("text/plain"), 12)
            .await
            .unwrap();

        // Get file through service
        let content = service.get_file("default", &test_hash()).await.unwrap();
        assert_eq!(content.data, b"test content");
        assert_eq!(content.media_type, Some("text/plain".to_string()));
    }

    #[tokio::test]
    async fn test_file_service_cleanup_traces() {
        let (temp_dir, database) = setup_test().await;

        // Create directories
        fs::create_dir_all(temp_dir.path().join("files"))
            .await
            .unwrap();
        fs::create_dir_all(temp_dir.path().join("files_temp"))
            .await
            .unwrap();

        let config = FilesConfig {
            enabled: true,
            storage: crate::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = FileService::new(config, &app_storage, database.clone())
            .await
            .unwrap();

        // Store a file
        service
            .storage
            .store("default", &test_hash(), b"test content")
            .await
            .unwrap();

        // Insert metadata with ref_count = 1 via repository trait
        let repo = database.repository();
        repo.upsert_file("default", &test_hash(), None, 12)
            .await
            .unwrap();

        // Associate with trace
        repo.insert_trace_file("trace1", "default", &test_hash())
            .await
            .unwrap();

        // Cleanup the trace
        service
            .cleanup_traces("default", &["trace1".to_string()])
            .await
            .unwrap();

        // File should be deleted (ref_count was 1, now 0)
        assert!(!service.file_exists("default", &test_hash()).await.unwrap());

        // Metadata should be gone
        let file = repo.get_file("default", &test_hash()).await.unwrap();
        assert!(file.is_none());
    }
}
