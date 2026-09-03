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

/// Delete files nothing references, and finish deletions a crash abandoned.
///
/// Two shapes, one sweep. A zero-reference row whose bytes are still on disk is a decrement that
/// completed while the storage delete failed. A row still claimed for deletion after
/// `stale_claim_secs` is a process that died holding the fence - see the resumption block below for why
/// that cannot be left alone.
///
/// `stale_claim_secs` is a parameter rather than read from the constant here so the caller owns the
/// policy and a test can state the age it means directly.
pub async fn cleanup_zero_ref_files(
    storage: &Arc<dyn FileStorage>,
    database: &Arc<TransactionalService>,
    stale_claim_secs: i64,
) -> Result<u64, FileServiceError> {
    let repo = database.repository();

    // Get files with ref_count = 0
    let orphan_files = repo.get_orphan_files().await?;

    let mut deleted = 0u64;

    for (project_id, hash) in orphan_files {
        // The metadata row first, and only if nothing references the file *now*.
        //
        // `get_orphan_files` selects on the stored `ref_count`, which is a cached count - so a stale
        // zero, or one read before an association landed, would have deleted live content. The
        // conditional delete asks the associations directly, and deleting the row before the bytes
        // means the surviving failure is bytes without metadata rather than a row pointing at nothing.
        // Claimed first, exactly as `delete_files_for_traces` does: `get_orphan_files` selects on the
        // stored `ref_count`, which is a cached count, so a stale zero would otherwise delete live
        // content - and without the claim, ingestion could recreate and finalise between the row delete
        // and the byte delete.
        match repo.claim_file_for_deletion(&project_id, &hash).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(
                    project_id,
                    hash,
                    "Orphan file is referenced or already being deleted; keeping it"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id,
                    hash,
                    "Failed to claim orphan file for deletion"
                );
                continue;
            }
        }

        if let Err(e) = storage.delete(&project_id, &hash).await {
            if let Err(release_error) = repo.release_deletion_claim(&project_id, &hash).await {
                tracing::error!(
                    error = %e,
                    release_error = %release_error,
                    project_id,
                    hash,
                    "Could not delete orphan bytes or release the claim; the file cannot be \
                     referenced again until it is cleared"
                );
                continue;
            }
            tracing::warn!(
                error = %e,
                project_id,
                hash,
                "Could not delete orphan bytes; released the claim and left the file in place"
            );
            continue;
        }

        match repo.delete_file_if_unreferenced(&project_id, &hash).await {
            Ok(true) => {}
            // Referenced again while its bytes were being deleted. Counting this as a success was
            // wrong twice over: nothing was cleaned, and the row now points at content that is gone.
            Ok(false) => {
                tracing::error!(
                    project_id,
                    hash,
                    "Deleted a claimed file's bytes but its row is referenced again; it will read as \
                     missing content until re-ingested"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id,
                    hash,
                    "Deleted orphan bytes but could not delete the row; it will read as missing content"
                );
                continue;
            }
        }

        deleted += 1;
        tracing::debug!(project_id, hash, "Deleted orphan file (ref_count=0)");
    }

    // Claims abandoned by a crash, resumed.
    //
    // A claim is durable on purpose - that is what makes it a fence - so a process that dies between
    // claiming and finishing leaves one set. Without resuming it the file is stuck forever: this sweep
    // sees a zero-reference row, tries to claim it, is refused by the claim already there, and skips it,
    // while every ingestion naming that file keeps failing its batch on the same fence.
    //
    // Resumed only after **re-taking the claim against the value that was read**, which is what makes the
    // byte deletion safe. The scan is a snapshot: between it and the delete the row can be released (by a
    // worker whose own object delete failed) or deleted and recreated by an ingestion that then associates
    // the same content hash - and deleting the bytes on the strength of the stale reading removes content a
    // committed span references. `delete_file_if_unreferenced` re-checks afterwards, which is too late: the
    // old code logged exactly that case at error level, and logging it is not preventing it.
    //
    // A successful reclaim also refreshes the claim, so a second worker holding the same stale reading is
    // refused rather than duplicating the work.
    match repo.get_stale_claimed_files(stale_claim_secs).await {
        Ok(stale) => {
            for (project_id, hash, observed) in stale {
                match repo.reclaim_stale_file(&project_id, &hash, observed).await {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::debug!(
                            project_id,
                            hash,
                            "An abandoned claim changed under us; leaving it to whoever holds it now"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            project_id,
                            hash,
                            "Could not re-take an abandoned claim; will retry"
                        );
                        continue;
                    }
                }
                if let Err(e) = storage.delete(&project_id, &hash).await {
                    tracing::warn!(
                        error = %e,
                        project_id,
                        hash,
                        "Could not finish an abandoned deletion; will retry"
                    );
                    continue;
                }
                match repo.delete_file_if_unreferenced(&project_id, &hash).await {
                    Ok(true) => {
                        deleted += 1;
                        tracing::debug!(project_id, hash, "Finished an abandoned file deletion");
                    }
                    Ok(false) => tracing::error!(
                        project_id,
                        hash,
                        "An abandoned deletion's file was referenced again after its bytes were \
                         removed; it will read as missing content until re-ingested"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        project_id,
                        hash,
                        "Could not delete the row of an abandoned deletion; will retry"
                    ),
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not look for abandoned file deletions"),
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
    use crate::core::constants::FILE_DELETION_CLAIM_STALE_SECS;
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
        let deleted = cleanup_zero_ref_files(&storage, &database, FILE_DELETION_CLAIM_STALE_SECS)
            .await
            .unwrap();

        assert_eq!(deleted, 1);

        // File should be gone
        assert!(!storage.exists("project1", &hash).await.unwrap());

        // Cleanup temp_dir to avoid unused warning
        drop(temp_dir);
    }

    /// A deletion abandoned mid-way is finished by the next sweep, not stranded.
    ///
    /// This is the crash the fence makes possible: the claim is durable, so nothing releases it, and the
    /// ordinary zero-reference path cannot pick the file up because claiming it fails. Without resumption
    /// the bytes stay on disk forever and every ingestion naming that file keeps failing its batch on a
    /// claim nobody holds.
    #[tokio::test]
    async fn a_deletion_abandoned_by_a_crash_is_finished_by_the_next_sweep() {
        let (temp_dir, database, storage) = setup_test().await;
        let hash = test_hash();
        storage
            .store("project1", &hash, b"test content")
            .await
            .unwrap();
        let repo = database.repository();
        repo.upsert_file("project1", &hash, Some("text/plain"), 12, "sha256")
            .await
            .unwrap();
        repo.decrement_ref_count("project1", &hash).await.unwrap();

        // The crash: claimed, then nothing further.
        assert!(
            repo.claim_file_for_deletion("project1", &hash)
                .await
                .unwrap()
        );
        assert_eq!(
            cleanup_zero_ref_files(&storage, &database, FILE_DELETION_CLAIM_STALE_SECS)
                .await
                .unwrap(),
            0,
            "a fresh claim is a deletion in progress; the sweep must leave it alone"
        );
        assert!(
            storage.exists("project1", &hash).await.unwrap(),
            "and must not touch its bytes"
        );

        // Treated as abandoned, the same sweep finishes it.
        assert_eq!(
            cleanup_zero_ref_files(&storage, &database, 0)
                .await
                .unwrap(),
            1,
            "an abandoned claim is resumed"
        );
        assert!(!storage.exists("project1", &hash).await.unwrap());
        assert!(
            repo.get_stale_claimed_files(0).await.unwrap().is_empty(),
            "and nothing is left claimed"
        );

        drop(temp_dir);
    }
}
