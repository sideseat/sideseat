//! Orphan file cleanup
//!
//! Handles cleanup of orphaned files that may result from crashes or incomplete operations:
//! - Temp files left behind after crashes
//! - Files with ref_count=0 that weren't properly deleted
//!
//! ## Cleanup Scenarios
//!
//! ### Scenario A: Temp file exists with database metadata
//! The server crashed after database insert but before permanent storage.
//! Action: Move file to permanent storage (complete the interrupted write).
//!
//! ### Scenario B: Temp file exists without database metadata
//! The server crashed before database insert.
//! Action: Delete the orphaned temp file.
//!
//! ### Scenario C: Database has ref_count=0 files
//! Decrement operation completed but file deletion failed.
//! Action: Delete the file from storage and database.

use std::path::Path;
use std::sync::Arc;

use super::error::FileServiceError;
use super::storage::FileStorage;
use crate::data::TransactionalService;

/// Run startup cleanup for orphaned temp files
///
/// Scans the temp directory and handles orphaned files:
/// - Files with database metadata: move to permanent storage
/// - Files without database metadata: delete
pub async fn cleanup_orphan_temp_files(
    temp_dir: &Path,
    storage: &Arc<dyn FileStorage>,
    database: &Arc<TransactionalService>,
) -> Result<CleanupStats, FileServiceError> {
    let mut stats = CleanupStats::default();

    if !temp_dir.exists() {
        return Ok(stats);
    }

    let mut entries = match tokio::fs::read_dir(temp_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %temp_dir.display(),
                "Failed to read temp directory for cleanup"
            );
            return Ok(stats);
        }
    };

    while let Some(entry) = entries.next_entry().await.transpose() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read temp directory entry");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Parse filename: {project_id}_{hash}
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        let (project_id, hash) = match parse_temp_filename(filename) {
            Some((p, h)) => (p, h),
            None => {
                tracing::warn!(filename, "Invalid temp filename format, deleting");
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!(error = %e, path = %path.display(), "Failed to delete invalid temp file");
                }
                stats.invalid_deleted += 1;
                continue;
            }
        };

        // Check if database has metadata for this file
        let repo = database.repository();
        let has_metadata = repo.file_exists(&project_id, &hash).await.unwrap_or(false);

        if has_metadata {
            // Scenario A: Complete the interrupted write
            let permanent_exists = storage.exists(&project_id, &hash).await.unwrap_or(false);

            if !permanent_exists {
                // Read temp and write to permanent storage
                match tokio::fs::read(&path).await {
                    Ok(data) => {
                        if let Err(e) = storage.store(&project_id, &hash, &data).await {
                            tracing::warn!(
                                error = %e,
                                project_id,
                                hash,
                                "Failed to finalize temp file to permanent storage"
                            );
                            stats.finalize_failed += 1;
                            continue; // Keep temp file for retry
                        }
                        stats.finalized += 1;
                        tracing::debug!(
                            project_id,
                            hash,
                            "Finalized orphan temp file to permanent storage"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), "Failed to read temp file for finalization");
                        stats.finalize_failed += 1;
                        continue;
                    }
                }
            } else {
                stats.already_exists += 1;
            }
        } else {
            // Scenario B: Orphaned temp file (no database record)
            stats.orphaned_deleted += 1;
            tracing::debug!(
                project_id,
                hash,
                "Deleting orphan temp file (no database record)"
            );
        }

        // Delete temp file after handling
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(error = %e, path = %path.display(), "Failed to delete temp file");
        }
    }

    if stats.total_processed() > 0 {
        tracing::debug!(
            finalized = stats.finalized,
            orphaned_deleted = stats.orphaned_deleted,
            already_exists = stats.already_exists,
            invalid_deleted = stats.invalid_deleted,
            finalize_failed = stats.finalize_failed,
            "Temp file cleanup complete"
        );
    }

    Ok(stats)
}

/// Run cleanup for files with ref_count=0 that weren't properly deleted
///
/// This handles Scenario C where decrement completed but storage deletion failed.
pub async fn cleanup_zero_ref_files(
    storage: &Arc<dyn FileStorage>,
    database: &Arc<TransactionalService>,
) -> Result<u64, FileServiceError> {
    let repo = database.repository();

    // Get files with ref_count = 0
    let orphan_files = repo.get_orphan_files().await?;

    if orphan_files.is_empty() {
        return Ok(0);
    }

    let mut deleted = 0u64;

    for (project_id, hash) in orphan_files {
        // Delete from storage
        if let Err(e) = storage.delete(&project_id, &hash).await {
            tracing::warn!(
                error = %e,
                project_id,
                hash,
                "Failed to delete orphan file from storage"
            );
            continue;
        }

        // Delete from database
        if let Err(e) = repo.delete_file(&project_id, &hash).await {
            tracing::warn!(
                error = %e,
                project_id,
                hash,
                "Failed to delete orphan file metadata from database"
            );
            continue;
        }

        deleted += 1;
        tracing::debug!(project_id, hash, "Deleted orphan file (ref_count=0)");
    }

    if deleted > 0 {
        tracing::debug!(deleted, "Orphan file cleanup complete (ref_count=0 files)");
    }

    Ok(deleted)
}

/// Parse temp filename in format: {project_id}_{hash}
fn parse_temp_filename(filename: &str) -> Option<(String, String)> {
    // Hash is 64 hex chars, so look for underscore at len - 65
    if filename.len() < 66 {
        return None; // Too short
    }

    // Find the last underscore before the 64-char hash
    let hash_start = filename.len() - 64;
    if hash_start == 0 {
        return None; // No project_id
    }

    let separator_pos = hash_start - 1;
    if filename.as_bytes()[separator_pos] != b'_' {
        return None;
    }

    let project_id = &filename[..separator_pos];
    let hash = &filename[hash_start..];

    // Validate hash is 64 hex chars
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    // Validate project_id is not empty
    if project_id.is_empty() {
        return None;
    }

    Some((project_id.to_string(), hash.to_string()))
}

/// Statistics from cleanup operations
#[derive(Debug, Default)]
pub struct CleanupStats {
    /// Files successfully finalized to permanent storage
    pub finalized: u64,
    /// Orphan temp files deleted (no SQLite record)
    pub orphaned_deleted: u64,
    /// Temp files where permanent copy already exists
    pub already_exists: u64,
    /// Invalid temp files deleted (wrong format)
    pub invalid_deleted: u64,
    /// Files that failed to finalize
    pub finalize_failed: u64,
}

impl CleanupStats {
    /// Total files processed
    pub fn total_processed(&self) -> u64 {
        self.finalized + self.orphaned_deleted + self.already_exists + self.invalid_deleted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::SqliteService;
    use crate::data::files::FilesystemStorage;
    use tempfile::TempDir;
    use tokio::fs;

    fn test_hash() -> String {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string()
    }

    async fn setup_test() -> (TempDir, Arc<TransactionalService>, Arc<dyn FileStorage>) {
        let temp_dir = TempDir::new().unwrap();

        // Create SQLite pool with schema (single connection for :memory: to ensure shared state)
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

        // Create filesystem storage
        let storage_path = temp_dir.path().join("files");
        fs::create_dir_all(&storage_path).await.unwrap();
        let storage: Arc<dyn FileStorage> = Arc::new(FilesystemStorage::new(storage_path));

        (temp_dir, database, storage)
    }

    #[test]
    fn test_parse_temp_filename_valid() {
        let hash = test_hash();
        let filename = format!("my-project_{}", hash);

        let result = parse_temp_filename(&filename);
        assert!(result.is_some());

        let (project_id, parsed_hash) = result.unwrap();
        assert_eq!(project_id, "my-project");
        assert_eq!(parsed_hash, hash);
    }

    #[test]
    fn test_parse_temp_filename_project_with_underscore() {
        let hash = test_hash();
        let filename = format!("my_project_{}", hash);

        let result = parse_temp_filename(&filename);
        assert!(result.is_some());

        let (project_id, parsed_hash) = result.unwrap();
        assert_eq!(project_id, "my_project");
        assert_eq!(parsed_hash, hash);
    }

    #[test]
    fn test_parse_temp_filename_invalid_too_short() {
        let result = parse_temp_filename("short_abc");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_temp_filename_invalid_hash() {
        // Hash with invalid chars
        let filename = "project_g1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let result = parse_temp_filename(filename);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_temp_filename_no_separator() {
        let hash = test_hash();
        let result = parse_temp_filename(&hash);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_empty_dir() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("temp");
        fs::create_dir_all(&temp_path).await.unwrap();

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.total_processed(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_dir() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("nonexistent");

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.total_processed(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_orphan_temp_no_metadata() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("temp");
        fs::create_dir_all(&temp_path).await.unwrap();

        // Create temp file without database record
        let hash = test_hash();
        let filename = format!("project1_{}", hash);
        let file_path = temp_path.join(&filename);
        fs::write(&file_path, b"test content").await.unwrap();

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.orphaned_deleted, 1);
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_cleanup_temp_with_metadata_finalize() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("temp");
        fs::create_dir_all(&temp_path).await.unwrap();

        let hash = test_hash();
        let filename = format!("project1_{}", hash);
        let file_path = temp_path.join(&filename);
        fs::write(&file_path, b"test content").await.unwrap();

        // Create database record via repository trait
        let repo = database.repository();
        repo.upsert_file("project1", &hash, Some("text/plain"), 12, "sha256")
            .await
            .unwrap();

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.finalized, 1);
        assert!(!file_path.exists());

        // Verify file was moved to permanent storage
        let data = storage.get("project1", &hash).await.unwrap();
        assert_eq!(data, b"test content");
    }

    #[tokio::test]
    async fn test_cleanup_temp_already_in_storage() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("temp");
        fs::create_dir_all(&temp_path).await.unwrap();

        let hash = test_hash();
        let filename = format!("project1_{}", hash);
        let file_path = temp_path.join(&filename);
        fs::write(&file_path, b"test content").await.unwrap();

        // Create database record and store in permanent storage
        let repo = database.repository();
        repo.upsert_file("project1", &hash, Some("text/plain"), 12, "sha256")
            .await
            .unwrap();
        storage
            .store("project1", &hash, b"test content")
            .await
            .unwrap();

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.already_exists, 1);
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_cleanup_invalid_filename() {
        let (temp_dir, database, storage) = setup_test().await;
        let temp_path = temp_dir.path().join("temp");
        fs::create_dir_all(&temp_path).await.unwrap();

        // Create temp file with invalid format
        let file_path = temp_path.join("invalid_filename.txt");
        fs::write(&file_path, b"test content").await.unwrap();

        let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
            .await
            .unwrap();

        assert_eq!(stats.invalid_deleted, 1);
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_cleanup_zero_ref_files() {
        let (temp_dir, database, storage) = setup_test().await;

        let hash = test_hash();

        // Store file
        storage
            .store("project1", &hash, b"test content")
            .await
            .unwrap();

        // Create database record with ref_count=0 via repository trait
        let repo = database.repository();
        repo.upsert_file("project1", &hash, Some("text/plain"), 12, "sha256")
            .await
            .unwrap();

        // Decrement to 0
        repo.decrement_ref_count("project1", &hash).await.unwrap();

        // Run cleanup
        let deleted = cleanup_zero_ref_files(&storage, &database).await.unwrap();

        assert_eq!(deleted, 1);

        // File should be gone
        assert!(!storage.exists("project1", &hash).await.unwrap());

        // Cleanup temp_dir to avoid unused warning
        drop(temp_dir);
    }
}
