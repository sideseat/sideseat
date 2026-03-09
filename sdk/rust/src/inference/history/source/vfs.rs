use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::super::error::HistoryError;
use super::super::types::{StorageBackend, StorageRef};

// ---------------------------------------------------------------------------
// FileMeta / FileEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: String,
    pub size_bytes: u64,
    pub mime_type: String,
    pub created_at: i64,
    pub modified_at: i64,
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub is_dir: bool,
    pub meta: Option<FileMeta>,
}

// ---------------------------------------------------------------------------
// FsProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait FsProvider: Send + Sync {
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError>;

    async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError>;

    async fn delete(&self, path: &str) -> Result<(), HistoryError>;

    async fn exists(&self, path: &str) -> Result<bool, HistoryError>;

    async fn metadata(&self, path: &str) -> Result<FileMeta, HistoryError>;

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, HistoryError>;

    fn provider_type(&self) -> &str;
}

// ---------------------------------------------------------------------------
// MemoryFsProvider
// ---------------------------------------------------------------------------

struct StoredFile {
    data: Vec<u8>,
    mime_type: String,
    created_at: i64,
    modified_at: i64,
}

pub struct MemoryFsProvider {
    files: Mutex<HashMap<String, StoredFile>>,
}

impl MemoryFsProvider {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryFsProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn now_micros() -> i64 {
    super::super::types::now_micros()
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    trimmed.to_string()
}

#[async_trait]
impl FsProvider for MemoryFsProvider {
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError> {
        let key = normalize_path(path);
        let now = now_micros();
        let size = data.len() as u64;

        let mut files = self.files.lock();
        let created_at = files.get(&key).map(|f| f.created_at).unwrap_or(now);

        files.insert(
            key.clone(),
            StoredFile {
                data: data.to_vec(),
                mime_type: mime_type.to_string(),
                created_at,
                modified_at: now,
            },
        );

        Ok(FileMeta {
            path: key,
            size_bytes: size,
            mime_type: mime_type.to_string(),
            created_at,
            modified_at: now,
            checksum: None,
        })
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError> {
        let key = normalize_path(path);
        self.files
            .lock()
            .get(&key)
            .map(|f| f.data.clone())
            .ok_or_else(|| HistoryError::FsError(format!("File not found: {path}")))
    }

    async fn delete(&self, path: &str) -> Result<(), HistoryError> {
        let key = normalize_path(path);
        self.files.lock().remove(&key);
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, HistoryError> {
        let key = normalize_path(path);
        Ok(self.files.lock().contains_key(&key))
    }

    async fn metadata(&self, path: &str) -> Result<FileMeta, HistoryError> {
        let key = normalize_path(path);
        let files = self.files.lock();
        let file = files
            .get(&key)
            .ok_or_else(|| HistoryError::FsError(format!("File not found: {path}")))?;
        Ok(FileMeta {
            path: key,
            size_bytes: file.data.len() as u64,
            mime_type: file.mime_type.clone(),
            created_at: file.created_at,
            modified_at: file.modified_at,
            checksum: None,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, HistoryError> {
        let normalized = normalize_path(prefix);
        let files = self.files.lock();

        let mut entries = Vec::new();
        let mut seen_dirs = std::collections::HashSet::new();

        for (key, file) in files.iter() {
            let matches = if normalized.is_empty() {
                true
            } else {
                key.starts_with(&normalized)
                    && (key.len() == normalized.len()
                        || key.as_bytes().get(normalized.len()) == Some(&b'/'))
            };

            if !matches {
                continue;
            }

            // Check if there's a subdirectory between prefix and this file
            let remainder = if normalized.is_empty() {
                key.as_str()
            } else if key.len() > normalized.len() {
                &key[normalized.len() + 1..]
            } else {
                ""
            };

            if let Some(slash_pos) = remainder.find('/') {
                let dir_name = &remainder[..slash_pos];
                let dir_path = if normalized.is_empty() {
                    dir_name.to_string()
                } else {
                    format!("{normalized}/{dir_name}")
                };
                if seen_dirs.insert(dir_path.clone()) {
                    entries.push(FileEntry {
                        path: dir_path,
                        is_dir: true,
                        meta: None,
                    });
                }
            } else if !remainder.is_empty() {
                entries.push(FileEntry {
                    path: key.clone(),
                    is_dir: false,
                    meta: Some(FileMeta {
                        path: key.clone(),
                        size_bytes: file.data.len() as u64,
                        mime_type: file.mime_type.clone(),
                        created_at: file.created_at,
                        modified_at: file.modified_at,
                        checksum: None,
                    }),
                });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn provider_type(&self) -> &str {
        "memory"
    }
}

// ---------------------------------------------------------------------------
// Vfs — virtual filesystem with mount table + branch-aware COW
//
// Each branch has its own logical→physical key index. When a branch is forked,
// the child inherits the parent's index (logical paths point to the same
// physical keys = copy-on-write). A write in the child creates a new physical
// key under the child branch, updating only the child's index entry. The
// parent's index is unchanged.
//
// Physical key layout in providers: "{branch_id}/{relative_path_from_mount}"
// Branch index layout: branch_key → { vfs_logical_path → physical_key }
// ---------------------------------------------------------------------------

struct Mount {
    prefix: String,
    provider: Arc<dyn FsProvider>,
}

/// Type alias for the branch index: branch → (logical_path → physical_key).
type BranchIndex = HashMap<String, HashMap<String, String>>;

pub struct Vfs {
    mounts: Vec<Mount>,
    default_provider: Arc<dyn FsProvider>,
    /// COW branch index shared between clones of the same Vfs.
    branch_index: Arc<RwLock<BranchIndex>>,
    /// Currently active branch name.
    active_branch: Arc<RwLock<String>>,
}

impl Vfs {
    pub fn new(default_provider: Arc<dyn FsProvider>) -> Self {
        let initial_branch = "default".to_string();
        let mut index: BranchIndex = HashMap::new();
        index.insert(initial_branch.clone(), HashMap::new());
        Self {
            mounts: Vec::new(),
            default_provider,
            branch_index: Arc::new(RwLock::new(index)),
            active_branch: Arc::new(RwLock::new(initial_branch)),
        }
    }

    pub fn with_mount(mut self, prefix: impl Into<String>, provider: Arc<dyn FsProvider>) -> Self {
        self.mount(prefix, provider);
        self
    }

    pub fn mount(&mut self, prefix: impl Into<String>, provider: Arc<dyn FsProvider>) {
        let prefix = normalize_path(&prefix.into());
        self.mounts.retain(|m| m.prefix != prefix);
        self.mounts.push(Mount { prefix, provider });
        // Sort by prefix length descending for longest-prefix match
        self.mounts.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
    }

    pub fn unmount(&mut self, prefix: &str) -> bool {
        let normalized = normalize_path(prefix);
        let before = self.mounts.len();
        self.mounts.retain(|m| m.prefix != normalized);
        self.mounts.len() < before
    }

    pub fn mounts(&self) -> Vec<(&str, &str)> {
        self.mounts
            .iter()
            .map(|m| (m.prefix.as_str(), m.provider.provider_type()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Branch management
    // -----------------------------------------------------------------------

    /// Switch the active branch. Creates an empty index entry if this branch
    /// has never been seen before.
    pub fn checkout_branch(&self, branch: &str) {
        *self.active_branch.write() = branch.to_string();
        self.branch_index
            .write()
            .entry(branch.to_string())
            .or_default();
    }

    /// Fork `parent` → `child`: child inherits all of parent's logical→physical
    /// mappings (COW snapshot). The active branch is NOT changed automatically.
    pub fn fork_branch(&self, parent: &str, child: &str) {
        let parent_entries = self
            .branch_index
            .read()
            .get(parent)
            .cloned()
            .unwrap_or_default();
        self.branch_index
            .write()
            .insert(child.to_string(), parent_entries);
    }

    /// Ensure a named branch exists in the index without changing the active
    /// branch. Idempotent — safe to call multiple times.
    pub fn ensure_branch(&self, branch: &str) {
        self.branch_index
            .write()
            .entry(branch.to_string())
            .or_default();
    }

    /// Write a file to a specific named branch **without** changing the
    /// active branch. The physical key is `"{branch}/{relative_from_mount}"`.
    pub async fn write_on_branch(
        &self,
        branch: &str,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError> {
        self.ensure_branch(branch);
        let vfs_path = normalize_path(path);
        let (provider, relative) = self.resolve_arc(path);
        let physical = Self::make_physical_key(branch, &relative);

        let mut meta = provider.write(&physical, data, mime_type).await?;
        meta.path = vfs_path.clone();
        Self::index_insert_on(&mut self.branch_index.write(), branch, &vfs_path, &physical);
        Ok(meta)
    }

    /// Read a file from a specific named branch (follows the COW index for
    /// that branch, so inherited entries from a forked parent are resolved).
    pub async fn read_on_branch(
        &self,
        branch: &str,
        path: &str,
    ) -> Result<Vec<u8>, HistoryError> {
        let vfs_path = normalize_path(path);
        let physical = Self::physical_key_in_branch(
            &self.branch_index.read(),
            branch,
            &vfs_path,
        )
        .ok_or_else(|| HistoryError::FsError(format!("File not found: {path} (branch: {branch})")))?;
        let (provider, _) = self.resolve_arc(path);
        provider.read(&physical).await
    }

    /// Check whether a file exists in a specific named branch.
    pub async fn exists_on_branch(&self, branch: &str, path: &str) -> Result<bool, HistoryError> {
        let vfs_path = normalize_path(path);
        Ok(
            Self::physical_key_in_branch(&self.branch_index.read(), branch, &vfs_path)
                .is_some(),
        )
    }

    // -----------------------------------------------------------------------
    // Internal: mount resolution + physical-key helpers
    // -----------------------------------------------------------------------

    /// Resolve a VFS path to (provider Arc, path-relative-to-mount).
    fn resolve_arc(&self, path: &str) -> (Arc<dyn FsProvider>, String) {
        let normalized = normalize_path(path);
        for mount in &self.mounts {
            if normalized.starts_with(&mount.prefix)
                && (normalized.len() == mount.prefix.len()
                    || normalized.as_bytes().get(mount.prefix.len()) == Some(&b'/'))
            {
                let relative = if normalized.len() > mount.prefix.len() {
                    normalized[mount.prefix.len() + 1..].to_string()
                } else {
                    String::new()
                };
                return (Arc::clone(&mount.provider), relative);
            }
        }
        (Arc::clone(&self.default_provider), normalized)
    }

    /// Build the physical key for a new write in the active branch.
    /// Format: "{branch}/{relative_from_mount}"
    fn physical_key_for_write(&self, relative: &str) -> String {
        let branch = self.active_branch.read().clone();
        Self::make_physical_key(&branch, relative)
    }

    fn make_physical_key(branch: &str, relative: &str) -> String {
        if branch.is_empty() {
            relative.to_string()
        } else {
            format!("{branch}/{relative}")
        }
    }

    /// Look up the physical key for `vfs_path` in the active branch's index.
    fn physical_key_for_read(&self, vfs_path: &str) -> Option<String> {
        let branch = self.active_branch.read().clone();
        Self::physical_key_in_branch(&self.branch_index.read(), &branch, vfs_path)
    }

    fn physical_key_in_branch(
        index: &HashMap<String, HashMap<String, String>>,
        branch: &str,
        vfs_path: &str,
    ) -> Option<String> {
        index.get(branch).and_then(|b| b.get(vfs_path)).cloned()
    }

    /// Update the active branch's index: logical vfs_path → physical_key.
    fn index_insert(&self, vfs_path: &str, physical_key: &str) {
        let branch = self.active_branch.read().clone();
        Self::index_insert_on(&mut self.branch_index.write(), &branch, vfs_path, physical_key);
    }

    fn index_insert_on(
        index: &mut HashMap<String, HashMap<String, String>>,
        branch: &str,
        vfs_path: &str,
        physical_key: &str,
    ) {
        index
            .entry(branch.to_string())
            .or_default()
            .insert(vfs_path.to_string(), physical_key.to_string());
    }

    /// Remove a logical path from the active branch's index.
    fn index_remove(&self, vfs_path: &str) {
        let branch = self.active_branch.read().clone();
        if let Some(b) = self.branch_index.write().get_mut(&branch) {
            b.remove(vfs_path);
        }
    }

    // -----------------------------------------------------------------------
    // Public filesystem operations
    // -----------------------------------------------------------------------

    pub async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError> {
        let vfs_path = normalize_path(path);
        let (provider, relative) = self.resolve_arc(path);
        let physical = self.physical_key_for_write(&relative);

        let mut meta = provider.write(&physical, data, mime_type).await?;
        meta.path = vfs_path.clone();
        self.index_insert(&vfs_path, &physical);
        Ok(meta)
    }

    pub async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError> {
        let vfs_path = normalize_path(path);
        let physical = self
            .physical_key_for_read(&vfs_path)
            .ok_or_else(|| HistoryError::FsError(format!("File not found: {path}")))?;
        let (provider, _) = self.resolve_arc(path);
        provider.read(&physical).await
    }

    pub async fn delete(&self, path: &str) -> Result<(), HistoryError> {
        let vfs_path = normalize_path(path);
        let Some(physical) = self.physical_key_for_read(&vfs_path) else {
            return Ok(()); // idempotent
        };
        let (provider, _) = self.resolve_arc(path);
        provider.delete(&physical).await?;
        self.index_remove(&vfs_path);
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool, HistoryError> {
        let vfs_path = normalize_path(path);
        Ok(self.physical_key_for_read(&vfs_path).is_some())
    }

    pub async fn metadata(&self, path: &str) -> Result<FileMeta, HistoryError> {
        let vfs_path = normalize_path(path);
        let physical = self
            .physical_key_for_read(&vfs_path)
            .ok_or_else(|| HistoryError::FsError(format!("File not found: {path}")))?;
        let (provider, _) = self.resolve_arc(path);
        let mut meta = provider.metadata(&physical).await?;
        meta.path = vfs_path;
        Ok(meta)
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, HistoryError> {
        let normalized = normalize_path(prefix);
        let branch = self.active_branch.read().clone();
        let branch_entries: HashMap<String, String> = self
            .branch_index
            .read()
            .get(&branch)
            .cloned()
            .unwrap_or_default();

        let mut entries: Vec<FileEntry> = Vec::new();
        let mut seen_dirs = std::collections::HashSet::new();

        for (vfs_path, physical_key) in &branch_entries {
            let matches = if normalized.is_empty() {
                true
            } else {
                vfs_path.starts_with(&normalized)
                    && (vfs_path.len() == normalized.len()
                        || vfs_path.as_bytes().get(normalized.len()) == Some(&b'/'))
            };
            if !matches {
                continue;
            }

            let remainder = if normalized.is_empty() {
                vfs_path.as_str()
            } else if vfs_path.len() > normalized.len() {
                &vfs_path[normalized.len() + 1..]
            } else {
                ""
            };

            if let Some(slash_pos) = remainder.find('/') {
                let dir_name = &remainder[..slash_pos];
                let dir_path = if normalized.is_empty() {
                    dir_name.to_string()
                } else {
                    format!("{normalized}/{dir_name}")
                };
                if seen_dirs.insert(dir_path.clone()) {
                    entries.push(FileEntry { path: dir_path, is_dir: true, meta: None });
                }
            } else if !remainder.is_empty() {
                let (provider, _) = self.resolve_arc(vfs_path);
                let file_meta = provider.metadata(physical_key).await.ok().map(|mut m| {
                    m.path = vfs_path.clone();
                    m
                });
                entries.push(FileEntry {
                    path: vfs_path.clone(),
                    is_dir: false,
                    meta: file_meta,
                });
            }
        }

        // Add mount prefixes as virtual directories when listing root or parent prefix
        for mount in &self.mounts {
            let is_child = if normalized.is_empty() {
                true
            } else {
                mount.prefix.starts_with(&normalized)
                    && mount.prefix.len() > normalized.len()
                    && mount.prefix.as_bytes().get(normalized.len()) == Some(&b'/')
            };
            if is_child && !entries.iter().any(|e| e.path == mount.prefix) {
                entries.push(FileEntry { path: mount.prefix.clone(), is_dir: true, meta: None });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// Return the name of the currently active branch.
    pub fn active_branch_str(&self) -> String {
        self.active_branch.read().clone()
    }

    /// Return `true` if the named branch has an index entry.
    pub fn has_branch(&self, branch: &str) -> bool {
        self.branch_index.read().contains_key(branch)
    }

    /// Serialize the logical→physical index for `branch` to JSON bytes.
    /// Returns `None` if the branch has no index entry yet.
    pub fn serialize_branch_index(&self, branch: &str) -> Option<Vec<u8>> {
        let idx = self.branch_index.read();
        let map = idx.get(branch)?;
        serde_json::to_vec(map).ok()
    }

    /// Replace a branch's index with one deserialized from JSON bytes.
    /// Silently ignored on parse failure so the caller can still proceed.
    pub fn load_branch_index(&self, branch: &str, bytes: &[u8]) {
        if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes) {
            self.branch_index.write().insert(branch.to_string(), map);
        }
    }

    pub fn to_storage_ref(&self, path: &str, meta: &FileMeta) -> StorageRef {
        StorageRef {
            backend: StorageBackend::Vfs,
            uri: normalize_path(path),
            checksum: meta.checksum.clone(),
            size_bytes: Some(meta.size_bytes),
        }
    }

    pub async fn read_storage_ref(
        &self,
        storage_ref: &StorageRef,
    ) -> Result<Vec<u8>, HistoryError> {
        match &storage_ref.backend {
            StorageBackend::Inline => {
                let data_str =
                    storage_ref.uri.strip_prefix("inline:").unwrap_or(&storage_ref.uri);
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data_str)
                    .map_err(|e| HistoryError::FsError(format!("Base64 decode error: {e}")))
            }
            // Both Vfs and Local are read via the VFS path lookup
            _ => self.read(&storage_ref.uri).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn memory_provider_write_read_delete() {
        let provider = MemoryFsProvider::new();

        let meta = provider.write("test/file.txt", b"hello", "text/plain").await.unwrap();
        assert_eq!(meta.size_bytes, 5);
        assert_eq!(meta.mime_type, "text/plain");

        assert!(provider.exists("test/file.txt").await.unwrap());

        let data = provider.read("test/file.txt").await.unwrap();
        assert_eq!(data, b"hello");

        provider.delete("test/file.txt").await.unwrap();
        assert!(!provider.exists("test/file.txt").await.unwrap());
    }

    #[tokio::test]
    async fn memory_provider_metadata() {
        let provider = MemoryFsProvider::new();
        provider.write("doc.pdf", b"pdf data", "application/pdf").await.unwrap();

        let meta = provider.metadata("doc.pdf").await.unwrap();
        assert_eq!(meta.size_bytes, 8);
        assert_eq!(meta.mime_type, "application/pdf");
    }

    #[tokio::test]
    async fn memory_provider_list() {
        let provider = MemoryFsProvider::new();
        provider.write("a/1.txt", b"1", "text/plain").await.unwrap();
        provider.write("a/2.txt", b"2", "text/plain").await.unwrap();
        provider.write("b/3.txt", b"3", "text/plain").await.unwrap();

        let root = provider.list("").await.unwrap();
        assert_eq!(root.len(), 2); // dirs: a, b
        assert!(root.iter().all(|e| e.is_dir));

        let a_entries = provider.list("a").await.unwrap();
        assert_eq!(a_entries.len(), 2);
        assert!(a_entries.iter().all(|e| !e.is_dir));
    }

    #[tokio::test]
    async fn memory_provider_read_not_found() {
        let provider = MemoryFsProvider::new();
        let err = provider.read("nonexistent").await.unwrap_err();
        assert!(matches!(err, HistoryError::FsError(_)));
    }

    #[tokio::test]
    async fn vfs_mount_routing() {
        let default = Arc::new(MemoryFsProvider::new());
        let sources = Arc::new(MemoryFsProvider::new());

        let vfs = Vfs::new(default).with_mount("sources", sources);

        // Write to /sources/doc.txt → goes to sources provider
        vfs.write("sources/doc.txt", b"source data", "text/plain").await.unwrap();
        // Write to /other/file.txt → goes to default provider
        vfs.write("other/file.txt", b"default data", "text/plain").await.unwrap();

        // Read back through VFS (branch-aware: check via VFS interface)
        let data = vfs.read("sources/doc.txt").await.unwrap();
        assert_eq!(data, b"source data");

        let data = vfs.read("other/file.txt").await.unwrap();
        assert_eq!(data, b"default data");

        assert!(vfs.exists("sources/doc.txt").await.unwrap());
        assert!(!vfs.exists("sources/nonexistent.txt").await.unwrap());
    }

    #[tokio::test]
    async fn vfs_longest_prefix_match() {
        let default = Arc::new(MemoryFsProvider::new());

        let vfs = Vfs::new(default)
            .with_mount("sources", Arc::new(MemoryFsProvider::new()))
            .with_mount("sources/special", Arc::new(MemoryFsProvider::new()));

        vfs.write("sources/normal.txt", b"n", "text/plain").await.unwrap();
        vfs.write("sources/special/deep.txt", b"d", "text/plain").await.unwrap();

        // Both readable via VFS; longer prefix wins for routing
        assert_eq!(vfs.read("sources/normal.txt").await.unwrap(), b"n");
        assert_eq!(vfs.read("sources/special/deep.txt").await.unwrap(), b"d");
    }

    #[tokio::test]
    async fn vfs_unmount() {
        let default = Arc::new(MemoryFsProvider::new());
        let extra = Arc::new(MemoryFsProvider::new());

        let mut vfs = Vfs::new(default);
        vfs.mount("extra", extra);

        assert_eq!(vfs.mounts().len(), 1);
        assert!(vfs.unmount("extra"));
        assert_eq!(vfs.mounts().len(), 0);
        assert!(!vfs.unmount("extra"));
    }

    #[tokio::test]
    async fn vfs_list_aggregates_mounts() {
        let default = Arc::new(MemoryFsProvider::new());
        let sources = Arc::new(MemoryFsProvider::new());

        // Write through VFS so the branch index is populated
        let vfs = Vfs::new(default).with_mount("sources", sources);
        vfs.write("readme.txt", b"hi", "text/plain").await.unwrap();

        let entries = vfs.list("").await.unwrap();
        // Should include the file from default + the "sources" mount dir
        let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(names.contains(&"readme.txt"));
        assert!(names.contains(&"sources"));
    }

    #[tokio::test]
    async fn vfs_to_storage_ref_and_back() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        let meta = vfs.write("test/file.bin", b"binary", "application/octet-stream").await.unwrap();
        let sref = vfs.to_storage_ref("test/file.bin", &meta);

        assert_eq!(sref.uri, "test/file.bin");
        assert_eq!(sref.size_bytes, Some(6));

        let data = vfs.read_storage_ref(&sref).await.unwrap();
        assert_eq!(data, b"binary");
    }

    #[tokio::test]
    async fn vfs_read_inline_storage_ref() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"inline data");
        let sref = StorageRef {
            backend: StorageBackend::Inline,
            uri: format!("inline:{encoded}"),
            checksum: None,
            size_bytes: None,
        };

        let data = vfs.read_storage_ref(&sref).await.unwrap();
        assert_eq!(data, b"inline data");
    }

    #[tokio::test]
    async fn vfs_metadata_remaps_path() {
        let default = Arc::new(MemoryFsProvider::new());
        let sources = Arc::new(MemoryFsProvider::new());

        let vfs = Vfs::new(default).with_mount("sources", sources);
        vfs.write("sources/doc.txt", b"data", "text/plain").await.unwrap();

        let meta = vfs.metadata("sources/doc.txt").await.unwrap();
        assert_eq!(meta.path, "sources/doc.txt");
    }

    #[tokio::test]
    async fn vfs_branch_cow() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        // Write in default branch
        vfs.write("doc.txt", b"v1", "text/plain").await.unwrap();
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v1");

        // Fork to child branch — child inherits parent's files
        vfs.fork_branch("default", "child");
        vfs.checkout_branch("child");
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v1");

        // Write in child branch — does not affect default
        vfs.write("doc.txt", b"v2", "text/plain").await.unwrap();
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v2");

        // Switch back to default — still sees v1
        vfs.checkout_branch("default");
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v1");
    }

    #[tokio::test]
    async fn vfs_branch_new_file_in_child() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        vfs.fork_branch("default", "branch-a");
        vfs.checkout_branch("branch-a");
        vfs.write("only-in-a.txt", b"branch-a", "text/plain").await.unwrap();

        assert!(vfs.exists("only-in-a.txt").await.unwrap());

        // Switch back to default — file doesn't exist there
        vfs.checkout_branch("default");
        assert!(!vfs.exists("only-in-a.txt").await.unwrap());
    }
}
