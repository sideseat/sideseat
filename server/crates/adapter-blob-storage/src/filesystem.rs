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

use sideseat_ports::blobs::FileStorageError;
use sideseat_ports::blobs::{FileStorage, ObjectName, ObjectVisitor, StoredObject};
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

    /// The directory a hash's objects are in: `{base}/{project}/{hash[0:2]}/{hash[2:4]}`.
    fn shard_path(&self, project_id: &ProjectId, hash: &str) -> PathBuf {
        self.base_path
            .join(project_id)
            .join(&hash[0..2])
            .join(&hash[2..4])
    }

    /// An object's path: `{shard}/{hash}.{incarnation}`.
    fn file_path(&self, project_id: &ProjectId, object: ObjectName<'_>) -> PathBuf {
        self.shard_path(project_id, object.hash)
            .join(object.to_string())
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
        let name = dest
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        dest.with_file_name(format!(
            "{name}.{}.{}.tmp",
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
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(object.hash)?;
        let hash = object.hash;

        let path = self.file_path(project_id, object);
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

    async fn get(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError> {
        Self::validate_hash(object.hash)?;
        let hash = object.hash;

        let path = self.file_path(project_id, object);

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

    async fn exists(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError> {
        Self::validate_hash(object.hash)?;
        let path = self.file_path(project_id, object);
        Ok(path.exists())
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(object.hash)?;
        let hash = object.hash;

        let path = self.file_path(project_id, object);

        if path.exists() {
            fs::remove_file(&path).await?;
            tracing::debug!(project_id = %project_id, hash, "File deleted");

            // Try to clean up empty parent directories (best effort)
            self.cleanup_empty_parents(&path).await;
        }

        Ok(())
    }

    async fn incarnations(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError> {
        Self::validate_hash(hash)?;
        let mut found = Vec::new();
        let mut entries = match fs::read_dir(self.shard_path(project_id, hash)).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(object) = ObjectName::parse(&name).filter(|object| object.hash == hash) {
                found.push(object.incarnation);
            }
        }
        found.sort_unstable();
        Ok(found)
    }

    async fn scan_objects(&self, visitor: &mut dyn ObjectVisitor) -> Result<(), FileStorageError> {
        // `{base}/{project}/{aa}/{bb}/{name}`: every level read, the objects at the fourth, one shard directory a
        // batch. A level missing - nothing stored yet, or a directory removed while the walk ran - holds no object.
        // A symlinked directory is followed, as every read through it is: an object served there is an object
        // stored. The depth is fixed, so a link cycle cannot run away.
        for (project, project_dir) in subdirectories(&self.base_path).await? {
            let project_id = ProjectId::from(project.as_str());
            for (_, outer) in subdirectories(&project_dir).await? {
                for (_, shard) in subdirectories(&outer).await? {
                    let mut entries = match fs::read_dir(&shard).await {
                        Ok(entries) => entries,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error.into()),
                    };
                    let mut objects = Vec::new();
                    while let Some(entry) = entries.next_entry().await? {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if let Some(object) = ObjectName::parse(&name) {
                            objects.push(StoredObject {
                                project_id: project_id.clone(),
                                hash: object.hash.to_string(),
                                incarnation: object.incarnation,
                            });
                        }
                    }
                    if !objects.is_empty() {
                        visitor.visit(objects).await?;
                    }
                }
            }
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
        object: ObjectName<'_>,
        temp_path: &Path,
    ) -> Result<(), FileStorageError> {
        Self::validate_hash(object.hash)?;
        let hash = object.hash;

        let dest_path = self.file_path(project_id, object);

        // A directory remembered as durable can have been removed under the store - an emptied shard another
        // store's delete took - so it is forgotten and made again, as `store` does.
        if let Some(parent) = dest_path.parent()
            && !fs::try_exists(parent).await.unwrap_or(false)
        {
            self.forget_path(&dest_path);
        }
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
/// The directories in `dir`, with their names, following symlinks; none when `dir` is not there.
async fn subdirectories(dir: &Path) -> Result<Vec<(String, PathBuf)>, FileStorageError> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        match fs::metadata(&path).await {
            Ok(metadata) if metadata.is_dir() => {
                found.push((entry.file_name().to_string_lossy().into_owned(), path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(found)
}

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
mod tests;
