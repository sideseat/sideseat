pub mod providers;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::error::CmError;
use super::types::{BranchId, StorageBackend, StorageRef};
pub use providers::{FsProvider, LocalFsProvider, MemoryFsProvider};
use providers::normalize_path;

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
// Vfs — virtual filesystem with mount table + branch-aware COW
//
// Each branch has its own logical→physical key index. When a branch is forked,
// the child inherits the parent's index (COW). A write in the child creates a
// new physical key, updating only the child's index. The parent's index is
// unchanged.
//
// Physical key layout: "{branch_id}/{relative_path_from_mount}"
// ---------------------------------------------------------------------------

struct Mount {
    prefix: String,
    provider: Arc<dyn FsProvider>,
}

type BranchIndex = HashMap<String, HashMap<String, String>>;

pub struct Vfs {
    mounts: Vec<Mount>,
    default_provider: Arc<dyn FsProvider>,
    branch_index: Arc<RwLock<BranchIndex>>,
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

    pub fn checkout_branch(&self, branch: &str) {
        // Initialize the branch index entry before activating the branch so
        // that any concurrent read sees a valid (possibly empty) index rather
        // than a missing entry that would return "not found" for every path.
        self.branch_index.write().entry(branch.to_string()).or_default();
        *self.active_branch.write() = branch.to_string();
    }

    pub fn fork_branch(&self, parent: &str, child: &str) {
        let mut index = self.branch_index.write();
        let parent_entries = index.get(parent).cloned().unwrap_or_default();
        index.insert(child.to_string(), parent_entries);
    }

    pub fn ensure_branch(&self, branch: &str) {
        self.branch_index.write().entry(branch.to_string()).or_default();
    }

    pub async fn write_on_branch(
        &self,
        branch: &str,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
        self.ensure_branch(branch);
        let vfs_path = normalize_path(path);
        let (provider, relative) = self.resolve_arc(path);
        let physical = Self::make_physical_key(branch, &relative);

        let mut meta = provider.write(&physical, data, mime_type).await?;
        meta.path = vfs_path.clone();
        Self::index_insert_on(&mut self.branch_index.write(), branch, &vfs_path, &physical);
        Ok(meta)
    }

    pub async fn read_on_branch(
        &self,
        branch: &str,
        path: &str,
    ) -> Result<Vec<u8>, CmError> {
        let vfs_path = normalize_path(path);
        let physical = Self::physical_key_in_branch(&self.branch_index.read(), branch, &vfs_path)
            .ok_or_else(|| {
                CmError::FsError(format!("File not found: {path} (branch: {branch})"))
            })?;
        let (provider, _) = self.resolve_arc(path);
        provider.read(&physical).await
    }

    pub async fn exists_on_branch(&self, branch: &str, path: &str) -> Result<bool, CmError> {
        let vfs_path = normalize_path(path);
        Ok(Self::physical_key_in_branch(&self.branch_index.read(), branch, &vfs_path).is_some())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

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

    fn make_physical_key(branch: &str, relative: &str) -> String {
        if branch.is_empty() {
            relative.to_string()
        } else {
            format!("{branch}/{relative}")
        }
    }

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

    // -----------------------------------------------------------------------
    // Public filesystem operations (active branch)
    // -----------------------------------------------------------------------

    pub async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
        let vfs_path = normalize_path(path);
        let (provider, relative) = self.resolve_arc(path);
        // Capture active branch once before the async write so that the
        // subsequent index insertion uses the same branch even if a concurrent
        // checkout races the await point.
        let branch = self.active_branch.read().clone();
        let physical = Self::make_physical_key(&branch, &relative);

        let mut meta = provider.write(&physical, data, mime_type).await?;
        meta.path = vfs_path.clone();
        Self::index_insert_on(&mut self.branch_index.write(), &branch, &vfs_path, &physical);
        Ok(meta)
    }

    pub async fn read(&self, path: &str) -> Result<Vec<u8>, CmError> {
        let vfs_path = normalize_path(path);
        let physical = self
            .physical_key_for_read(&vfs_path)
            .ok_or_else(|| CmError::FsError(format!("File not found: {path}")))?;
        let (provider, _) = self.resolve_arc(path);
        provider.read(&physical).await
    }

    pub async fn delete(&self, path: &str) -> Result<(), CmError> {
        let vfs_path = normalize_path(path);
        // Capture active branch before the async delete so that the index
        // removal operates on the same branch that produced the physical key.
        let branch = self.active_branch.read().clone();
        let Some(physical) =
            Self::physical_key_in_branch(&self.branch_index.read(), &branch, &vfs_path)
        else {
            return Ok(());
        };
        let (provider, _) = self.resolve_arc(path);
        provider.delete(&physical).await?;
        if let Some(b) = self.branch_index.write().get_mut(&branch) {
            b.remove(&vfs_path);
        }
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool, CmError> {
        let vfs_path = normalize_path(path);
        Ok(self.physical_key_for_read(&vfs_path).is_some())
    }

    pub async fn metadata(&self, path: &str) -> Result<FileMeta, CmError> {
        let vfs_path = normalize_path(path);
        let physical = self
            .physical_key_for_read(&vfs_path)
            .ok_or_else(|| CmError::FsError(format!("File not found: {path}")))?;
        let (provider, _) = self.resolve_arc(path);
        let mut meta = provider.metadata(&physical).await?;
        meta.path = vfs_path;
        Ok(meta)
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, CmError> {
        let normalized = normalize_path(prefix);
        let branch = self.active_branch.read().clone();
        let branch_entries: HashMap<String, String> =
            self.branch_index.read().get(&branch).cloned().unwrap_or_default();

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

        for mount in &self.mounts {
            let is_child = if normalized.is_empty() {
                true
            } else {
                mount.prefix.starts_with(&normalized)
                    && mount.prefix.len() > normalized.len()
                    && mount.prefix.as_bytes().get(normalized.len()) == Some(&b'/')
            };
            if is_child && !entries.iter().any(|e| e.path == mount.prefix) {
                entries.push(FileEntry {
                    path: mount.prefix.clone(),
                    is_dir: true,
                    meta: None,
                });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    pub fn active_branch_str(&self) -> String {
        self.active_branch.read().clone()
    }

    pub fn has_branch(&self, branch: &str) -> bool {
        self.branch_index.read().contains_key(branch)
    }

    pub fn serialize_branch_index(&self, branch: &str) -> Option<Vec<u8>> {
        let idx = self.branch_index.read();
        let map = idx.get(branch)?;
        serde_json::to_vec(map).ok()
    }

    /// Replace a branch's index with one deserialized from `bytes`.
    ///
    /// Returns `CmError::FsError` if `bytes` is non-empty but fails to parse.
    /// An empty `bytes` slice is treated as "no index yet" and initializes an
    /// empty map (idempotent for new branches).
    pub fn load_branch_index(&self, branch: &str, bytes: &[u8]) -> Result<(), CmError> {
        if bytes.is_empty() {
            self.branch_index.write().entry(branch.to_string()).or_default();
            return Ok(());
        }
        let map = serde_json::from_slice::<HashMap<String, String>>(bytes)
            .map_err(|e| CmError::FsError(format!("VFS index parse error for {branch}: {e}")))?;
        self.branch_index.write().insert(branch.to_string(), map);
        Ok(())
    }

    pub fn to_storage_ref(&self, path: &str, meta: &FileMeta) -> StorageRef {
        StorageRef {
            backend: StorageBackend::Vfs,
            uri: normalize_path(path),
            checksum: meta.checksum.clone(),
            size_bytes: Some(meta.size_bytes),
        }
    }

    pub async fn read_storage_ref(&self, storage_ref: &StorageRef) -> Result<Vec<u8>, CmError> {
        match &storage_ref.backend {
            StorageBackend::Inline => {
                let data_str =
                    storage_ref.uri.strip_prefix("inline:").unwrap_or(&storage_ref.uri);
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data_str)
                    .map_err(|e| CmError::FsError(format!("Base64 decode error: {e}")))
            }
            _ => self.read(&storage_ref.uri).await,
        }
    }
}

// ---------------------------------------------------------------------------
// VfsExtension — ContextExtension wrapper around Vfs
// ---------------------------------------------------------------------------

/// Extension that equips a `ContextManager` with a branch-aware COW virtual
/// filesystem. Register with `ContextManager::with_extension`.
///
/// CRDT-backed text collab on VFS files is handled via `CrdtExtension` —
/// VfsExtension does not embed its own `CrdtDoc`.
pub struct VfsExtension {
    pub(crate) vfs: Vfs,
}

impl VfsExtension {
    pub fn new() -> Self {
        Self { vfs: Vfs::new(Arc::new(MemoryFsProvider::new())) }
    }

    pub fn with_vfs(vfs: Vfs) -> Self {
        Self { vfs }
    }

    pub fn mount(&mut self, prefix: impl Into<String>, provider: Arc<dyn FsProvider>) {
        self.vfs.mount(prefix, provider);
    }

    // -----------------------------------------------------------------------
    // Filesystem operations (delegate to inner Vfs)
    // -----------------------------------------------------------------------

    pub async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
        self.vfs.write(path, data, mime_type).await
    }

    pub async fn read(&self, path: &str) -> Result<Vec<u8>, CmError> {
        self.vfs.read(path).await
    }

    pub async fn delete(&self, path: &str) -> Result<(), CmError> {
        self.vfs.delete(path).await
    }

    pub async fn exists(&self, path: &str) -> Result<bool, CmError> {
        self.vfs.exists(path).await
    }

    pub async fn metadata(&self, path: &str) -> Result<FileMeta, CmError> {
        self.vfs.metadata(path).await
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, CmError> {
        self.vfs.list(prefix).await
    }

    pub fn to_storage_ref(&self, path: &str, meta: &FileMeta) -> StorageRef {
        self.vfs.to_storage_ref(path, meta)
    }

    pub fn ensure_branch(&self, branch: &str) {
        self.vfs.ensure_branch(branch);
    }

    pub fn fork_named_branch(&self, parent: &str, child: &str) {
        self.vfs.fork_branch(parent, child);
    }

    pub async fn write_on(
        &self,
        branch: &str,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
        self.vfs.write_on_branch(branch, path, data, mime_type).await
    }

    pub async fn read_on(&self, branch: &str, path: &str) -> Result<Vec<u8>, CmError> {
        self.vfs.read_on_branch(branch, path).await
    }

    pub async fn exists_on(&self, branch: &str, path: &str) -> Result<bool, CmError> {
        self.vfs.exists_on_branch(branch, path).await
    }

    pub async fn read_storage_ref(&self, storage_ref: &StorageRef) -> Result<Vec<u8>, CmError> {
        self.vfs.read_storage_ref(storage_ref).await
    }

    pub fn serialize_active_branch_index(&self) -> Option<Vec<u8>> {
        let branch = self.vfs.active_branch_str();
        self.vfs.serialize_branch_index(&branch)
    }

    pub fn serialize_branch_index(&self, branch: &str) -> Option<Vec<u8>> {
        self.vfs.serialize_branch_index(branch)
    }

    pub fn load_branch_index(&self, branch: &str, bytes: &[u8]) -> Result<(), CmError> {
        self.vfs.load_branch_index(branch, bytes)
    }
}

impl Default for VfsExtension {
    fn default() -> Self {
        Self::new()
    }
}

// ContextExtension impl lives in mod.rs which defines the trait; here we
// just implement the branch lifecycle hooks so VFS stays consistent with the
// active branch.
impl super::ContextExtension for VfsExtension {
    fn id(&self) -> &str {
        "vfs"
    }

    fn on_branch_forked(&self, parent: &BranchId, child: &BranchId) {
        self.vfs.fork_branch(parent.as_str(), child.as_str());
    }

    fn on_branch_checked_out(&self, branch: &BranchId) {
        self.vfs.checkout_branch(branch.as_str());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextExtension;
    use std::sync::Arc;

    #[tokio::test]
    async fn vfs_mount_routing() {
        let default = Arc::new(MemoryFsProvider::new());
        let sources = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(default).with_mount("sources", sources);

        vfs.write("sources/doc.txt", b"source data", "text/plain").await.unwrap();
        vfs.write("other/file.txt", b"default data", "text/plain").await.unwrap();

        assert_eq!(vfs.read("sources/doc.txt").await.unwrap(), b"source data");
        assert_eq!(vfs.read("other/file.txt").await.unwrap(), b"default data");
        assert!(vfs.exists("sources/doc.txt").await.unwrap());
        assert!(!vfs.exists("sources/nonexistent.txt").await.unwrap());
    }

    #[tokio::test]
    async fn vfs_branch_cow() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        vfs.write("doc.txt", b"v1", "text/plain").await.unwrap();

        vfs.fork_branch("default", "child");
        vfs.checkout_branch("child");
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v1");

        vfs.write("doc.txt", b"v2", "text/plain").await.unwrap();
        assert_eq!(vfs.read("doc.txt").await.unwrap(), b"v2");

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

        vfs.checkout_branch("default");
        assert!(!vfs.exists("only-in-a.txt").await.unwrap());
    }

    #[tokio::test]
    async fn load_branch_index_roundtrip() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider.clone());

        vfs.write("a.txt", b"alpha", "text/plain").await.unwrap();
        vfs.write("b.txt", b"beta", "text/plain").await.unwrap();

        let bytes = vfs.serialize_branch_index("default").unwrap();

        let vfs2 = Vfs::new(provider);
        vfs2.load_branch_index("default", &bytes).unwrap();

        // After loading the index, files are accessible (physical keys preserved).
        assert!(vfs2.exists("a.txt").await.unwrap());
        assert!(vfs2.exists("b.txt").await.unwrap());
    }

    #[tokio::test]
    async fn load_branch_index_empty_bytes_ok() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);
        // Empty bytes should NOT error — branch is initialized with empty index.
        vfs.load_branch_index("new-branch", &[]).unwrap();
        assert!(!vfs.exists("anything").await.unwrap());
    }

    #[test]
    fn load_branch_index_bad_bytes_returns_error() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);
        let result = vfs.load_branch_index("b", b"not-valid-json");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn vfs_extension_lifecycle_hooks() {
        let ext = VfsExtension::new();
        let parent = BranchId::new();
        let child = BranchId::new();

        ext.vfs.checkout_branch(parent.as_str());
        ext.vfs.write("f.txt", b"parent data", "text/plain").await.unwrap();

        // Fork via extension hook
        ext.on_branch_forked(&parent, &child);

        // Checkout child via hook
        ext.on_branch_checked_out(&child);
        assert!(ext.exists("f.txt").await.unwrap());

        // Write in child
        ext.write("f.txt", b"child data", "text/plain").await.unwrap();
        assert_eq!(ext.read("f.txt").await.unwrap(), b"child data");

        // Parent unaffected
        ext.vfs.checkout_branch(parent.as_str());
        assert_eq!(ext.read("f.txt").await.unwrap(), b"parent data");
    }

    #[tokio::test]
    async fn vfs_to_storage_ref_roundtrip() {
        let provider = Arc::new(MemoryFsProvider::new());
        let vfs = Vfs::new(provider);

        let meta = vfs.write("test/file.bin", b"binary", "application/octet-stream").await.unwrap();
        let sref = vfs.to_storage_ref("test/file.bin", &meta);

        assert_eq!(sref.uri, "test/file.bin");
        assert_eq!(sref.size_bytes, Some(6));

        let data = vfs.read_storage_ref(&sref).await.unwrap();
        assert_eq!(data, b"binary");
    }
}
