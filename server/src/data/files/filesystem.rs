//! Filesystem-based file storage implementation
//!
//! Stores files on the local filesystem with a sharded directory structure:
//! `{base_path}/{project_id}/{hash[0:2]}/{hash[2:4]}/{hash}`

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use super::error::FileStorageError;
use super::storage::FileStorage;

/// Filesystem-based file storage
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    /// Base path for file storage
    base_path: PathBuf,
}

impl FilesystemStorage {
    /// Create a new filesystem storage with the given base path
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Get the full path for a file
    ///
    /// Returns path like: `{base}/{project}/{hash[0:2]}/{hash[2:4]}/{hash}`
    fn file_path(&self, project_id: &str, hash: &str) -> PathBuf {
        let shard1 = &hash[0..2];
        let shard2 = &hash[2..4];
        self.base_path
            .join(project_id)
            .join(shard1)
            .join(shard2)
            .join(hash)
    }

    /// Get the project directory path
    fn project_path(&self, project_id: &str) -> PathBuf {
        self.base_path.join(project_id)
    }

    /// Ensure parent directories exist for a file path
    async fn ensure_parent_dirs(&self, path: &Path) -> Result<(), FileStorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(())
    }

    /// Validate hash format (64 hex characters)
    fn validate_hash(hash: &str) -> Result<(), FileStorageError> {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FileStorageError::Backend(format!(
                "Invalid hash format: expected 64 hex chars, got {}",
                hash.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl FileStorage for FilesystemStorage {
    async fn store(
        &self,
        project_id: &str,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let path = self.file_path(project_id, hash);

        // Content-addressed: if file exists with same hash, skip write
        if path.exists() {
            tracing::trace!(
                project_id,
                hash,
                "File already exists, skipping write (content-addressed)"
            );
            return Ok(());
        }

        self.ensure_parent_dirs(&path).await?;
        fs::write(&path, data).await?;

        tracing::debug!(
            project_id,
            hash,
            size = data.len(),
            path = %path.display(),
            "File stored"
        );

        Ok(())
    }

    async fn get(&self, project_id: &str, hash: &str) -> Result<Vec<u8>, FileStorageError> {
        Self::validate_hash(hash)?;

        let path = self.file_path(project_id, hash);

        // Read directly; map ENOENT to NotFound instead of a separate exists() check
        // which would be a TOCTOU race (file could vanish between check and read).
        fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FileStorageError::NotFound {
                    project_id: project_id.to_string(),
                    hash: hash.to_string(),
                }
            } else {
                FileStorageError::Io(e)
            }
        })
    }

    async fn exists(&self, project_id: &str, hash: &str) -> Result<bool, FileStorageError> {
        Self::validate_hash(hash)?;
        let path = self.file_path(project_id, hash);
        Ok(path.exists())
    }

    async fn delete(&self, project_id: &str, hash: &str) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let path = self.file_path(project_id, hash);

        if path.exists() {
            fs::remove_file(&path).await?;
            tracing::debug!(project_id, hash, "File deleted");

            // Try to clean up empty parent directories (best effort)
            self.cleanup_empty_parents(&path).await;
        }

        Ok(())
    }

    async fn delete_project(&self, project_id: &str) -> Result<u64, FileStorageError> {
        let project_path = self.project_path(project_id);

        if !project_path.exists() {
            return Ok(0);
        }

        // Count files before deletion
        let count = self.count_files_recursive(&project_path).await;

        // Remove the entire project directory tree
        fs::remove_dir_all(&project_path).await?;

        tracing::debug!(project_id, deleted = count, "Project files deleted");

        Ok(count)
    }

    async fn finalize_temp(
        &self,
        project_id: &str,
        hash: &str,
        temp_path: &Path,
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let dest_path = self.file_path(project_id, hash);

        // Content-addressed: if file exists, just remove temp
        if dest_path.exists() {
            fs::remove_file(temp_path).await.ok();
            tracing::trace!(
                project_id,
                hash,
                "File already exists, removed temp (content-addressed)"
            );
            return Ok(());
        }

        self.ensure_parent_dirs(&dest_path).await?;

        // Try atomic rename first (works if same filesystem)
        match fs::rename(temp_path, &dest_path).await {
            Ok(_) => {
                tracing::debug!(
                    project_id,
                    hash,
                    path = %dest_path.display(),
                    "File finalized (rename)"
                );
            }
            Err(_) => {
                // Cross-filesystem: copy to a unique staging file in dest dir,
                // then atomic rename. The staging name includes PID + random suffix
                // to prevent collision between concurrent workers finalizing the
                // same content-addressed hash.
                let staging = dest_path.with_extension(format!(
                    "{}.{}.tmp",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos()
                ));
                fs::copy(temp_path, &staging).await?;
                if let Err(e) = fs::rename(&staging, &dest_path).await {
                    fs::remove_file(&staging).await.ok();
                    return Err(FileStorageError::Io(e));
                }
                fs::remove_file(temp_path).await.ok();
                tracing::debug!(
                    project_id,
                    hash,
                    path = %dest_path.display(),
                    "File finalized (copy+rename)"
                );
            }
        }

        Ok(())
    }
}

impl FilesystemStorage {
    /// Clean up empty parent directories after file deletion (best effort)
    async fn cleanup_empty_parents(&self, file_path: &Path) {
        let mut current = file_path.parent();

        // Walk up the tree, stopping at base_path
        while let Some(dir) = current {
            // Don't delete base_path or anything above it
            if dir == self.base_path || !dir.starts_with(&self.base_path) {
                break;
            }

            // Try to remove directory (will fail if not empty)
            match fs::remove_dir(dir).await {
                Ok(_) => {
                    tracing::trace!(path = %dir.display(), "Removed empty directory");
                    current = dir.parent();
                }
                Err(_) => {
                    // Directory not empty or other error, stop cleanup
                    break;
                }
            }
        }
    }

    /// Count files recursively in a directory
    async fn count_files_recursive(&self, path: &Path) -> u64 {
        let mut count = 0;

        let mut entries = match fs::read_dir(path).await {
            Ok(e) => e,
            Err(_) => return 0,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_file() {
                count += 1;
            } else if file_type.is_dir() {
                count += Box::pin(self.count_files_recursive(&entry.path())).await;
            }
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_hash() -> &'static str {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let data = b"test content";
        storage.store("project1", test_hash(), data).await.unwrap();

        let retrieved = storage.get("project1", test_hash()).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_content_addressed_dedup() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let data = b"test content";

        // Store twice with same hash
        storage.store("project1", test_hash(), data).await.unwrap();
        storage.store("project1", test_hash(), data).await.unwrap();

        // Should still work
        let retrieved = storage.get("project1", test_hash()).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        assert!(!storage.exists("project1", test_hash()).await.unwrap());

        storage
            .store("project1", test_hash(), b"data")
            .await
            .unwrap();

        assert!(storage.exists("project1", test_hash()).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        storage
            .store("project1", test_hash(), b"data")
            .await
            .unwrap();
        assert!(storage.exists("project1", test_hash()).await.unwrap());

        storage.delete("project1", test_hash()).await.unwrap();
        assert!(!storage.exists("project1", test_hash()).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Should not fail
        storage.delete("project1", test_hash()).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let result = storage.get("project1", test_hash()).await;
        assert!(matches!(result, Err(FileStorageError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_invalid_hash() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let result = storage.store("project1", "invalid", b"data").await;
        assert!(matches!(result, Err(FileStorageError::Backend(_))));
    }

    #[tokio::test]
    async fn test_file_path_sharding() {
        let storage = FilesystemStorage::new(PathBuf::from("/base"));
        let path = storage.file_path("project1", test_hash());

        // Should be sharded: /base/project1/a1/b2/full_hash
        assert!(path.to_string_lossy().contains("/project1/a1/b2/"));
        assert!(path.to_string_lossy().ends_with(test_hash()));
    }

    #[tokio::test]
    async fn test_delete_project() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Store multiple files
        let hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let hash2 = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";

        storage.store("project1", hash1, b"data1").await.unwrap();
        storage.store("project1", hash2, b"data2").await.unwrap();

        let deleted = storage.delete_project("project1").await.unwrap();
        assert_eq!(deleted, 2);

        // Verify files are gone
        assert!(!storage.exists("project1", hash1).await.unwrap());
        assert!(!storage.exists("project1", hash2).await.unwrap());
    }

    #[tokio::test]
    async fn test_finalize_temp() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Create a temp file
        let temp_file = temp_dir.path().join("temp_file");
        fs::write(&temp_file, b"temp content").await.unwrap();

        // Finalize it
        storage
            .finalize_temp("project1", test_hash(), &temp_file)
            .await
            .unwrap();

        // Temp file should be gone
        assert!(!temp_file.exists());

        // File should be in permanent storage
        let data = storage.get("project1", test_hash()).await.unwrap();
        assert_eq!(data, b"temp content");
    }

    #[tokio::test]
    async fn test_project_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Store same hash in different projects
        storage
            .store("project1", test_hash(), b"data1")
            .await
            .unwrap();
        storage
            .store("project2", test_hash(), b"data2")
            .await
            .unwrap();

        // Each project should have its own copy
        let data1 = storage.get("project1", test_hash()).await.unwrap();
        let data2 = storage.get("project2", test_hash()).await.unwrap();

        assert_eq!(data1, b"data1");
        assert_eq!(data2, b"data2");
    }
}
