//! Filesystem-backed implementation of the blob-storage port.
//!
//! Stores files on the local filesystem with a sharded directory structure:
//! `{base_path}/{project_id}/{hash[0:2]}/{hash[2:4]}/{hash}`
//!
//! **Publication is atomic and durable.** An object is written to a unique temporary file beside its final
//! path, fsynced, renamed into place, and the directory is fsynced, so the final path never names a partial
//! object and a successful `store` survives a crash or power loss. The callers acknowledge telemetry after
//! `store` returns; writing straight to the final path without a sync let a crash leave a truncated object
//! that the existence check then accepted as complete, and lost recently acknowledged content outright.
//!
//! **A path is durable only when every directory on it is.** A directory's entry survives a power loss once the
//! directory holding it has been synced, so a store makes every directory between the base and its object
//! durable before it returns - whoever created them. Syncing only the levels a call created itself left a race:
//! one writer created a project directory and had not yet synced its holder when a second saw it exist, stored
//! beneath it and was acknowledged, and a power loss could take the directory and the acknowledged object with
//! it. Each directory is synced once per process: [`FilesystemStorage`] remembers the directories it has made
//! durable - only after their sync returns, so a directory another writer is still creating is synced again
//! rather than trusted - and forgets the ones it removes. An object that already exists is treated the same way:
//! its rename may be another writer's whose directory sync has not returned, so the directory is synced before
//! the store answers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::fs;

use sideseat_ports::blobs::FileStorage;
use sideseat_ports::blobs::FileStorageError;
use sideseat_ports::types::ProjectId;

/// Directories remembered as durable, at most: past it the set starts again, which costs a sync per directory
/// and nothing in correctness. A project's shards are 65,536 directories.
const DURABLE_DIRECTORIES_MAX: usize = 1 << 16;

/// Filesystem-based file storage
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    /// Base path for file storage
    base_path: PathBuf,
    /// The directories under the base this process has made durable: created, and their holder synced since.
    durable: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FilesystemStorage {
    /// Create a new filesystem storage with the given base path
    pub fn new(base_path: PathBuf) -> Self {
        Self {
            base_path,
            durable: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn is_durable(&self, dir: &Path) -> bool {
        self.durable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(dir)
    }

    fn remember_durable(&self, dir: &Path) {
        let mut durable = self
            .durable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if durable.len() >= DURABLE_DIRECTORIES_MAX {
            durable.clear();
        }
        durable.insert(dir.to_path_buf());
    }

    /// Forget every directory on `path`: one of them is gone, and which is not known.
    fn forget_path(&self, path: &Path) {
        let mut durable = self
            .durable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for dir in path.ancestors() {
            durable.remove(dir);
        }
    }

    /// Forget the directories at and under `dir`: removed, so the next store creates and syncs them again.
    fn forget_durable(&self, dir: &Path) {
        self.durable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|known| !known.starts_with(dir));
    }

    /// Get the full path for a file
    ///
    /// Returns path like: `{base}/{project}/{hash[0:2]}/{hash[2:4]}/{hash}`
    fn file_path(&self, project_id: &ProjectId, hash: &str) -> PathBuf {
        let shard1 = &hash[0..2];
        let shard2 = &hash[2..4];
        self.base_path
            .join(project_id)
            .join(shard1)
            .join(shard2)
            .join(hash)
    }

    /// Get the project directory path
    fn project_path(&self, project_id: &ProjectId) -> PathBuf {
        self.base_path.join(project_id)
    }

    /// Make every directory from the base down to `path`'s directory exist and be durable.
    ///
    /// Top down, so a directory's holder is durable before its own entry is synced into it. The base is created
    /// with whatever it lacks above it, each level it creates synced into its holder, as a store's first call
    /// finds it.
    async fn ensure_parent_dirs(&self, path: &Path) -> Result<(), FileStorageError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let mut chain: Vec<&Path> = parent
            .ancestors()
            .take_while(|dir| dir.starts_with(&self.base_path))
            .collect();
        chain.reverse();
        for dir in chain {
            if self.is_durable(dir) {
                continue;
            }
            if dir == self.base_path {
                create_dir_all_durably(dir).await?;
            } else {
                match fs::create_dir(dir).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(FileStorageError::Io(error)),
                }
            }
            // Whoever created it: its entry is durable once its holder is synced after it exists.
            if let Some(holder) = dir.parent() {
                sync_dir(holder).await?;
            }
            self.remember_durable(dir);
        }
        Ok(())
    }

    /// A unique sibling of `dest` to write into before the atomic rename.
    ///
    /// Unique per call, so concurrent writers of the same content-addressed hash never share a file.
    fn temp_sibling(dest: &Path) -> PathBuf {
        dest.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    /// Rename a fully written, fsynced `temp` onto `dest` and make the rename durable.
    async fn publish(temp: &Path, dest: &Path) -> Result<(), FileStorageError> {
        if let Err(error) = fs::rename(temp, dest).await {
            fs::remove_file(temp).await.ok();
            return Err(FileStorageError::Io(error));
        }
        if let Some(parent) = dest.parent() {
            sync_dir(parent).await?;
        }
        Ok(())
    }

    /// Store `data` at `path` durably, or make an existing object there durable.
    async fn store_at(&self, path: &Path, data: &[u8]) -> Result<(), FileStorageError> {
        self.ensure_parent_dirs(path).await?;
        // Content-addressed: an object already there is this content, but its rename may be another writer's
        // whose directory sync has not returned yet.
        if fs::try_exists(path).await.unwrap_or(false) {
            if let Some(parent) = path.parent() {
                sync_dir(parent).await?;
            }
            tracing::trace!(path = %path.display(), "File already exists (content-addressed)");
            return Ok(());
        }
        let temp = Self::temp_sibling(path);
        if let Err(error) = write_synced(&temp, data).await {
            fs::remove_file(&temp).await.ok();
            return Err(error);
        }
        Self::publish(&temp, path).await
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
        project_id: &ProjectId,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let path = self.file_path(project_id, hash);
        match self.store_at(&path, data).await {
            // A directory remembered as durable was removed under the store - an emptied shard a delete took -
            // so it is forgotten and the store made once more, creating it again.
            Err(FileStorageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                self.forget_path(&path);
                self.store_at(&path, data).await?;
            }
            other => other?,
        }

        tracing::debug!(
            project_id = %project_id,
            hash,
            size = data.len(),
            path = %path.display(),
            "File stored"
        );

        Ok(())
    }

    async fn get(&self, project_id: &ProjectId, hash: &str) -> Result<Vec<u8>, FileStorageError> {
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

    async fn exists(&self, project_id: &ProjectId, hash: &str) -> Result<bool, FileStorageError> {
        Self::validate_hash(hash)?;
        let path = self.file_path(project_id, hash);
        Ok(path.exists())
    }

    async fn delete(&self, project_id: &ProjectId, hash: &str) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let path = self.file_path(project_id, hash);

        if path.exists() {
            fs::remove_file(&path).await?;
            tracing::debug!(project_id = %project_id, hash, "File deleted");

            // Try to clean up empty parent directories (best effort)
            self.cleanup_empty_parents(&path).await;
        }

        Ok(())
    }

    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        let project_path = self.project_path(project_id);

        if !project_path.exists() {
            return Ok(0);
        }

        // Count files before deletion
        let count = self.count_files_recursive(&project_path).await;

        // Remove the entire project directory tree, and forget it was durable.
        self.forget_durable(&project_path);
        fs::remove_dir_all(&project_path).await?;

        tracing::debug!(project_id = %project_id, deleted = count, "Project files deleted");

        Ok(count)
    }

    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        hash: &str,
        temp_path: &Path,
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(hash)?;

        let dest_path = self.file_path(project_id, hash);

        self.ensure_parent_dirs(&dest_path).await?;
        // Content-addressed: an object already there is this content; made durable as `store` makes it.
        if fs::try_exists(&dest_path).await.unwrap_or(false) {
            fs::remove_file(temp_path).await.ok();
            if let Some(parent) = dest_path.parent() {
                sync_dir(parent).await?;
            }
            tracing::trace!(
                project_id = %project_id,
                hash,
                "File already exists, removed temp (content-addressed)"
            );
            return Ok(());
        }

        // The temp file is the caller's; its bytes must be on disk before the rename publishes them.
        sync_file(temp_path).await?;
        // Try atomic rename first (works if same filesystem)
        match fs::rename(temp_path, &dest_path).await {
            Ok(_) => {
                if let Some(parent) = dest_path.parent() {
                    sync_dir(parent).await?;
                }
                tracing::debug!(
                    project_id = %project_id,
                    hash,
                    path = %dest_path.display(),
                    "File finalized (rename)"
                );
            }
            Err(_) => {
                // Cross-filesystem: copy to a unique staging file in dest dir, sync it, then atomic rename.
                let staging = Self::temp_sibling(&dest_path);
                fs::copy(temp_path, &staging).await?;
                if let Err(error) = sync_file(&staging).await {
                    fs::remove_file(&staging).await.ok();
                    return Err(error);
                }
                Self::publish(&staging, &dest_path).await?;
                fs::remove_file(temp_path).await.ok();
                tracing::debug!(
                    project_id = %project_id,
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
                    self.forget_durable(dir);
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

/// Write `data` to a new file at `path` and fsync it.
async fn write_synced(path: &Path, data: &[u8]) -> Result<(), FileStorageError> {
    use tokio::io::AsyncWriteExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(data).await?;
    file.flush().await?;
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || sync_handle(&file))
        .await
        .map_err(|error| FileStorageError::Backend(error.to_string()))??;
    Ok(())
}

/// Fsync an existing file's contents.
async fn sync_file(path: &Path) -> Result<(), FileStorageError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        sync_handle(&std::fs::OpenOptions::new().write(true).open(path)?)
    })
    .await
    .map_err(|error| FileStorageError::Backend(error.to_string()))??;
    Ok(())
}

/// Create `dir` with whatever it lacks above it, syncing each created level into its holder, deepest last.
async fn create_dir_all_durably(dir: &Path) -> Result<(), FileStorageError> {
    let mut missing = Vec::new();
    let mut cursor = Some(dir);
    while let Some(level) = cursor {
        if fs::try_exists(level).await.unwrap_or(false) {
            break;
        }
        missing.push(level.to_path_buf());
        cursor = level.parent();
    }
    fs::create_dir_all(dir).await?;
    for level in missing.iter().rev() {
        if let Some(holder) = level.parent() {
            sync_dir(holder).await?;
        }
    }
    Ok(())
}

/// The directories synced, in order, for the tests to read: the only way to see a sync from inside the process.
#[cfg(test)]
static SYNCED_DIRECTORIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Fsync a directory, making the entries created or renamed in it durable.
///
/// POSIX only: Windows cannot open a directory as a file, and NTFS journals the rename itself.
async fn sync_dir(path: &Path) -> Result<(), FileStorageError> {
    #[cfg(unix)]
    {
        let path = path.to_path_buf();
        #[cfg(test)]
        let recorded = path.clone();
        tokio::task::spawn_blocking(move || sync_handle(&std::fs::File::open(path)?))
            .await
            .map_err(|error| FileStorageError::Backend(error.to_string()))??;
        #[cfg(test)]
        SYNCED_DIRECTORIES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(recorded);
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Make a file's contents, or a directory's entries, durable.
///
/// std's `sync_all` is `F_FULLFSYNC` on Apple platforms, where plain `fsync` leaves the bytes in the drive's
/// write cache and a power failure can lose them; elsewhere it is `fsync` (Linux) or `FlushFileBuffers`
/// (Windows). The rows that reference these objects are made durable the same way on every platform, so an
/// object is never less durable than the row naming it.
fn sync_handle(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_all()
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
        storage
            .store(&ProjectId::from("project1"), test_hash(), data)
            .await
            .unwrap();

        let retrieved = storage
            .get(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
        assert_eq!(retrieved, data);
    }

    /// Publication goes through a temporary sibling; none may be left behind, and two writers racing on one
    /// content-addressed hash must both succeed with one complete object.
    #[tokio::test]
    async fn store_publishes_atomically_and_leaves_no_temporary_files() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
        let project = ProjectId::from("project1");
        let data = vec![7u8; 256 * 1024];
        let (a, b) = tokio::join!(
            storage.store(&project, test_hash(), &data),
            storage.store(&project, test_hash(), &data)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(storage.get(&project, test_hash()).await.unwrap(), data);
        let dir = storage
            .file_path(&project, test_hash())
            .parent()
            .unwrap()
            .to_path_buf();
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![test_hash().to_string()]);
    }

    /// A reader that sees the object must see all of it: the existence check is what content addressing
    /// trusts, so a partially written object at the final path would be accepted as complete.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_object_is_never_visible_partially_written() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
        let project = ProjectId::from("project1");
        let data = vec![9u8; 64 * 1024 * 1024];
        let path = storage.file_path(&project, test_hash());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // A plain thread polling the final path as fast as it can: every size it observes must be complete.
        let poller = {
            let stop = stop.clone();
            let expected = data.len() as u64;
            std::thread::spawn(move || {
                let mut partial = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() != expected) {
                        partial += 1;
                    }
                }
                partial
            })
        };
        storage.store(&project, test_hash(), &data).await.unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            poller.join().unwrap(),
            0,
            "a reader saw a partially written object"
        );
    }

    #[tokio::test]
    async fn test_content_addressed_dedup() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let data = b"test content";

        // Store twice with same hash
        storage
            .store(&ProjectId::from("project1"), test_hash(), data)
            .await
            .unwrap();
        storage
            .store(&ProjectId::from("project1"), test_hash(), data)
            .await
            .unwrap();

        // Should still work
        let retrieved = storage
            .get(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        assert!(
            !storage
                .exists(&ProjectId::from("project1"), test_hash())
                .await
                .unwrap()
        );

        storage
            .store(&ProjectId::from("project1"), test_hash(), b"data")
            .await
            .unwrap();

        assert!(
            storage
                .exists(&ProjectId::from("project1"), test_hash())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        storage
            .store(&ProjectId::from("project1"), test_hash(), b"data")
            .await
            .unwrap();
        assert!(
            storage
                .exists(&ProjectId::from("project1"), test_hash())
                .await
                .unwrap()
        );

        storage
            .delete(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
        assert!(
            !storage
                .exists(&ProjectId::from("project1"), test_hash())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Should not fail
        storage
            .delete(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let result = storage.get(&ProjectId::from("project1"), test_hash()).await;
        assert!(matches!(result, Err(FileStorageError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_invalid_hash() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        let result = storage
            .store(&ProjectId::from("project1"), "invalid", b"data")
            .await;
        assert!(matches!(result, Err(FileStorageError::Backend(_))));
    }

    #[tokio::test]
    async fn test_file_path_sharding() {
        let storage = FilesystemStorage::new(PathBuf::from("/base"));
        let path = storage.file_path(&ProjectId::from("project1"), test_hash());

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

        storage
            .store(&ProjectId::from("project1"), hash1, b"data1")
            .await
            .unwrap();
        storage
            .store(&ProjectId::from("project1"), hash2, b"data2")
            .await
            .unwrap();

        let deleted = storage
            .delete_project(&ProjectId::from("project1"))
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        // Verify files are gone
        assert!(
            !storage
                .exists(&ProjectId::from("project1"), hash1)
                .await
                .unwrap()
        );
        assert!(
            !storage
                .exists(&ProjectId::from("project1"), hash2)
                .await
                .unwrap()
        );
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
            .finalize_temp(&ProjectId::from("project1"), test_hash(), &temp_file)
            .await
            .unwrap();

        // Temp file should be gone
        assert!(!temp_file.exists());

        // File should be in permanent storage
        let data = storage
            .get(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
        assert_eq!(data, b"temp content");
    }

    #[tokio::test]
    async fn test_project_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());

        // Store same hash in different projects
        storage
            .store(&ProjectId::from("project1"), test_hash(), b"data1")
            .await
            .unwrap();
        storage
            .store(&ProjectId::from("project2"), test_hash(), b"data2")
            .await
            .unwrap();

        // Each project should have its own copy
        let data1 = storage
            .get(&ProjectId::from("project1"), test_hash())
            .await
            .unwrap();
        let data2 = storage
            .get(&ProjectId::from("project2"), test_hash())
            .await
            .unwrap();

        assert_eq!(data1, b"data1");
        assert_eq!(data2, b"data2");
    }

    /// The directories synced under `root` since the test began, in order.
    fn synced_under(root: &Path, from: usize) -> Vec<PathBuf> {
        SYNCED_DIRECTORIES.lock().unwrap()[from..]
            .iter()
            .filter(|dir| dir.starts_with(root))
            .cloned()
            .collect()
    }

    fn synced_so_far() -> usize {
        SYNCED_DIRECTORIES.lock().unwrap().len()
    }

    /// A project directory another writer has created but not yet made durable is synced into its holder by a
    /// store beneath it, before the store returns: trusted because it existed, a power loss could take it and
    /// the acknowledged object with it.
    #[tokio::test]
    async fn a_directory_another_writer_created_is_made_durable_before_the_store_returns() {
        let temp_dir = TempDir::new().unwrap();
        let base = temp_dir.path().canonicalize().unwrap();
        let storage = FilesystemStorage::new(base.clone());
        // Another writer's directory, created and not synced.
        std::fs::create_dir_all(base.join("project1")).unwrap();
        let from = synced_so_far();
        storage
            .store(&ProjectId::from("project1"), test_hash(), b"bytes")
            .await
            .unwrap();
        let synced = synced_under(&base, from);
        assert!(
            synced.contains(&base),
            "the project directory's entry was never synced into the base: {synced:?}"
        );
        let shard = base.join("project1").join("a1");
        assert!(synced.contains(&base.join("project1")), "{synced:?}");
        assert!(synced.contains(&shard), "{synced:?}");
        assert!(
            synced.contains(&shard.join("b2")),
            "the object's own entry: {synced:?}"
        );

        // Once durable, a directory is not synced again for the next object beneath it.
        let from = synced_so_far();
        let other = "a1b2ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        storage
            .store(&ProjectId::from("project1"), other, b"more")
            .await
            .unwrap();
        assert_eq!(synced_under(&base, from), vec![shard.join("b2")]);
    }

    /// An object already in place - another writer's rename whose directory sync has not returned - is made
    /// durable before a store of the same content answers.
    #[tokio::test]
    async fn an_object_another_writer_renamed_is_made_durable_before_the_store_returns() {
        let temp_dir = TempDir::new().unwrap();
        let base = temp_dir.path().canonicalize().unwrap();
        let storage = FilesystemStorage::new(base.clone());
        let directory = base.join("project1").join("a1").join("b2");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(test_hash()), b"bytes").unwrap();
        let from = synced_so_far();
        storage
            .store(&ProjectId::from("project1"), test_hash(), b"bytes")
            .await
            .unwrap();
        assert!(
            synced_under(&base, from).contains(&directory),
            "the existing object's entry was not synced"
        );
    }

    /// A directory remembered as durable and then removed outside a delete - the tree gone under the store - is
    /// created again, not trusted into a failure on every later store.
    #[tokio::test]
    async fn a_directory_removed_under_the_store_is_created_again() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf());
        let project = ProjectId::from("project1");
        storage
            .store(&project, test_hash(), b"first")
            .await
            .unwrap();
        std::fs::remove_dir_all(temp_dir.path().join("project1")).unwrap();
        let other = "a1b2ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        storage.store(&project, other, b"second").await.unwrap();
        assert_eq!(storage.get(&project, other).await.unwrap(), b"second");
    }
}
