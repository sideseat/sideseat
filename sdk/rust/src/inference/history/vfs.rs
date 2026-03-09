use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::error::HistoryError;
use super::types::{StorageBackend, StorageRef};

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
    super::types::now_micros()
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
// Vfs — virtual filesystem with mount table
// ---------------------------------------------------------------------------

struct Mount {
    prefix: String,
    provider: std::sync::Arc<dyn FsProvider>,
}

pub struct Vfs {
    mounts: Vec<Mount>,
    default_provider: std::sync::Arc<dyn FsProvider>,
}

impl Vfs {
    pub fn new(default_provider: std::sync::Arc<dyn FsProvider>) -> Self {
        Self {
            mounts: Vec::new(),
            default_provider,
        }
    }

    pub fn with_mount(
        mut self,
        prefix: impl Into<String>,
        provider: std::sync::Arc<dyn FsProvider>,
    ) -> Self {
        self.mount(prefix, provider);
        self
    }

    pub fn mount(
        &mut self,
        prefix: impl Into<String>,
        provider: std::sync::Arc<dyn FsProvider>,
    ) {
        let prefix = normalize_path(&prefix.into());
        // Remove existing mount at same prefix
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

    fn resolve(&self, path: &str) -> (&dyn FsProvider, String) {
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
                return (mount.provider.as_ref(), relative);
            }
        }
        (self.default_provider.as_ref(), normalized)
    }

    pub async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError> {
        let (provider, relative) = self.resolve(path);
        let mut meta = provider.write(&relative, data, mime_type).await?;
        meta.path = normalize_path(path);
        Ok(meta)
    }

    pub async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError> {
        let (provider, relative) = self.resolve(path);
        provider.read(&relative).await
    }

    pub async fn delete(&self, path: &str) -> Result<(), HistoryError> {
        let (provider, relative) = self.resolve(path);
        provider.delete(&relative).await
    }

    pub async fn exists(&self, path: &str) -> Result<bool, HistoryError> {
        let (provider, relative) = self.resolve(path);
        provider.exists(&relative).await
    }

    pub async fn metadata(&self, path: &str) -> Result<FileMeta, HistoryError> {
        let (provider, relative) = self.resolve(path);
        let mut meta = provider.metadata(&relative).await?;
        meta.path = normalize_path(path);
        Ok(meta)
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, HistoryError> {
        let normalized = normalize_path(prefix);

        // Check if the prefix exactly targets a mount
        let (provider, relative) = self.resolve(prefix);
        let mut entries = provider.list(&relative).await?;

        // Remap paths back to VFS-absolute paths
        let mount_prefix = {
            let full = normalize_path(prefix);
            if relative.is_empty() {
                full
            } else {
                full[..full.len() - relative.len()].trim_end_matches('/').to_string()
            }
        };

        for entry in &mut entries {
            if !mount_prefix.is_empty() && !entry.path.starts_with(&mount_prefix) {
                entry.path = format!("{}/{}", mount_prefix, entry.path);
            }
            if let Some(meta) = &mut entry.meta {
                meta.path = entry.path.clone();
            }
        }

        // If listing root or a prefix that has child mounts, add mount prefixes as dirs
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

    pub fn to_storage_ref(&self, path: &str, meta: &FileMeta) -> StorageRef {
        StorageRef {
            backend: StorageBackend::Local,
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
                // Inline data is stored as base64 in the URI after "inline:" prefix
                let data_str = storage_ref.uri.strip_prefix("inline:").unwrap_or(&storage_ref.uri);
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data_str)
                    .map_err(|e| HistoryError::FsError(format!("Base64 decode error: {e}")))
            }
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

        let vfs = Vfs::new(default.clone())
            .with_mount("sources", sources.clone());

        // Write to /sources/doc.txt → goes to sources provider
        vfs.write("sources/doc.txt", b"source data", "text/plain").await.unwrap();
        assert!(sources.exists("doc.txt").await.unwrap());
        assert!(!default.exists("sources/doc.txt").await.unwrap());

        // Write to /other/file.txt → goes to default provider
        vfs.write("other/file.txt", b"default data", "text/plain").await.unwrap();
        assert!(default.exists("other/file.txt").await.unwrap());

        // Read back through VFS
        let data = vfs.read("sources/doc.txt").await.unwrap();
        assert_eq!(data, b"source data");

        let data = vfs.read("other/file.txt").await.unwrap();
        assert_eq!(data, b"default data");
    }

    #[tokio::test]
    async fn vfs_longest_prefix_match() {
        let default = Arc::new(MemoryFsProvider::new());
        let sources = Arc::new(MemoryFsProvider::new());
        let deep = Arc::new(MemoryFsProvider::new());

        let vfs = Vfs::new(default)
            .with_mount("sources", sources.clone())
            .with_mount("sources/special", deep.clone());

        vfs.write("sources/normal.txt", b"n", "text/plain").await.unwrap();
        vfs.write("sources/special/deep.txt", b"d", "text/plain").await.unwrap();

        // normal.txt went to sources provider
        assert!(sources.exists("normal.txt").await.unwrap());
        // deep.txt went to deep provider (longer prefix wins)
        assert!(deep.exists("deep.txt").await.unwrap());
        assert!(!sources.exists("special/deep.txt").await.unwrap());
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

        default.write("readme.txt", b"hi", "text/plain").await.unwrap();

        let vfs = Vfs::new(default).with_mount("sources", sources);

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
}
