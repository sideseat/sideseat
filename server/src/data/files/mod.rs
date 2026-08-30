//! File storage layer
//!
//! Provides binary file storage with deduplication for the application.
//! Files are stored outside DuckDB with hash-based content addressing.
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
//! let file_service = FileService::new(config, storage, database, cache).await?;
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
use std::time::Duration;

use crate::core::config::FilesConfig;
use crate::core::constants::CACHE_TTL_FILE_QUOTA;
use crate::core::storage::{AppStorage, DataSubdir};
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};

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
    /// Shared cache service (Redis in SaaS, in-memory in local)
    cache: Arc<CacheService>,
}

impl FileService {
    /// Create a new file service
    ///
    /// This function is async because S3 storage initialization requires loading AWS config.
    pub async fn new(
        config: FilesConfig,
        app_storage: &AppStorage,
        database: Arc<TransactionalService>,
        cache: Arc<CacheService>,
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
            cache,
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
            && let Err(e) = cleanup::cleanup_zero_ref_files(
                &service.storage,
                &service.database,
                crate::core::constants::FILE_DELETION_CLAIM_STALE_SECS,
            )
            .await
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

        // How many references these traces hold per file, not just which files.
        //
        // `get_file_hashes_for_traces` returns each hash once, and the loop decremented once per hash -
        // so deleting three traces that all referenced one file removed three associations and one
        // reference, leaving the file permanently unreachable and uncollectable.
        let references = repo
            .get_file_reference_counts_for_traces(project_id, trace_ids)
            .await?;

        // Delete trace-file associations
        repo.delete_trace_files(project_id, trace_ids).await?;

        // Recompute each count from the associations that remain, and delete when none do.
        //
        // Subtracting a previously-read number is not safe against a concurrent cleanup: both would read
        // three, both subtract three, and a file four traces referenced would reach zero and be deleted
        // under the fourth. Recomputing cannot do that - whatever else happened, the count becomes the
        // truth.
        for (hash, _) in references {
            // The count is kept accurate for display, but it is not what authorises the deletion.
            repo.sync_ref_count(project_id, &hash).await?;

            // The metadata row goes first, and only if nothing references the file *at that instant*.
            //
            // Deciding from a count read earlier races with ingestion and loses: cleanup recomputes
            // zero, a new trace associates, and the delete fires anyway - taking the bytes from under a
            // span that was just committed. With the condition inside the statement, the association
            // makes the delete match nothing and the file stays.
            //
            // Deleting the row before the bytes also means the surviving failure mode is bytes with no
            // metadata - a leak - rather than a row pointing at bytes that are gone, which is what a
            // reader would see as corruption.
            // Claim first, then the bytes, then the row.
            //
            // The claim is what closes the window a conditional delete alone leaves open: with the row
            // deleted, ingestion could recreate it, associate and finalise the bytes, and this loop
            // would then delete content a committed row references. `associate_file` refuses through a
            // claim, so that ingestion fails its batch and retries - by which time the file is either
            // gone, and the retry writes it again with the bytes in hand, or the claim was released.
            if !repo.claim_file_for_deletion(project_id, &hash).await? {
                tracing::debug!(
                    project_id,
                    hash,
                    "File is referenced or already being deleted; leaving it"
                );
                continue;
            }

            if let Err(e) = self.storage.delete(project_id, &hash).await {
                // The row is still there, holding the claim, so nothing has been lost - but the claim
                // has to go or the file becomes permanently unassociable.
                if let Err(release_error) = repo.release_deletion_claim(project_id, &hash).await {
                    tracing::error!(
                        project_id,
                        hash,
                        error = %e,
                        release_error = %release_error,
                        "Could not delete file bytes and could not release the deletion claim; the \
                         file cannot be referenced again until it is cleared"
                    );
                    continue;
                }
                tracing::warn!(
                    project_id,
                    hash,
                    error = %e,
                    "Could not delete file bytes; released the claim and left the file in place"
                );
                continue;
            }

            // Bytes gone, so the row must go too - it is the only thing that could still be found.
            if !repo.delete_file_if_unreferenced(project_id, &hash).await? {
                tracing::error!(
                    project_id,
                    hash,
                    "Deleted a claimed file's bytes but its row is now referenced; it will read as \
                     missing content until re-ingested"
                );
                continue;
            }

            tracing::debug!(project_id, hash, "Deleted orphaned file");
        }

        self.invalidate_quota_cache(&[project_id]).await;

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

        self.invalidate_quota_cache(&[project_id]).await;

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

    /// Check if project has quota for additional bytes. Returns true if within quota.
    /// Uses CacheService (Redis/memory) with TTL to avoid hitting the DB on every batch.
    pub async fn check_quota(
        &self,
        project_id: &str,
        additional_bytes: i64,
    ) -> Result<bool, FileServiceError> {
        if !self.config.enabled {
            return Ok(true);
        }
        let key = CacheKey::file_quota(project_id);
        let current = match self.cache.get::<i64>(&key).await.unwrap_or(None) {
            Some(cached) => cached,
            None => {
                let bytes = self.get_storage_bytes(project_id).await?;
                let _ = self
                    .cache
                    .set(
                        &key,
                        &bytes,
                        Some(Duration::from_secs(CACHE_TTL_FILE_QUOTA)),
                    )
                    .await;
                bytes
            }
        };
        let quota = i64::try_from(self.config.quota_bytes).unwrap_or(i64::MAX);
        Ok((current + additional_bytes) <= quota)
    }

    /// Invalidate cached quota for projects after storage changes (writes or deletes).
    /// Called by persist layer after file writes and by cleanup after deletions.
    pub async fn invalidate_quota_cache(&self, project_ids: &[&str]) {
        for project_id in project_ids {
            self.cache
                .invalidate_key(&CacheKey::file_quota(project_id))
                .await;
        }
    }

    /// Get total file storage used by all projects in an organization
    pub async fn get_org_storage_bytes(&self, org_id: &str) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }
        let repo = self.database.repository();
        Ok(repo.get_org_file_storage_bytes(org_id).await?)
    }

    /// Get total file storage used across all orgs a user belongs to
    pub async fn get_user_storage_bytes(&self, user_id: &str) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }
        let repo = self.database.repository();
        Ok(repo.get_user_file_storage_bytes(user_id).await?)
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

    async fn setup_test() -> (TempDir, Arc<TransactionalService>, Arc<CacheService>) {
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

        let cache_config = crate::core::config::CacheConfig {
            backend: crate::core::config::CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: crate::core::config::EvictionPolicy::TinyLfu,
            redis_url: None,
        };
        let cache = Arc::new(CacheService::new(&cache_config).await.unwrap());

        (temp_dir, database, cache)
    }

    #[tokio::test]
    async fn test_file_service_disabled() {
        let (temp_dir, database, cache) = setup_test().await;

        let config = FilesConfig {
            enabled: false,
            storage: crate::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = FileService::new(config, &app_storage, database, cache)
            .await
            .unwrap();

        assert!(!service.is_enabled());

        let result = service.get_file("default", &test_hash()).await;
        assert!(matches!(result, Err(FileServiceError::Disabled)));
    }

    #[tokio::test]
    async fn test_file_service_get_file() {
        let (temp_dir, database, cache) = setup_test().await;

        let config = FilesConfig {
            enabled: true,
            storage: crate::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = FileService::new(config, &app_storage, database.clone(), cache)
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
        repo.upsert_file("default", &test_hash(), Some("text/plain"), 12, "sha256")
            .await
            .unwrap();

        // Get file through service
        let content = service.get_file("default", &test_hash()).await.unwrap();
        assert_eq!(content.data, b"test content");
        assert_eq!(content.media_type, Some("text/plain".to_string()));
    }

    #[tokio::test]
    async fn test_file_service_cleanup_traces() {
        let (temp_dir, database, cache) = setup_test().await;

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
        let service = FileService::new(config, &app_storage, database.clone(), cache)
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
        repo.upsert_file("default", &test_hash(), None, 12, "sha256")
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
