use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::super::error::CmError;
use super::{FileEntry, FileMeta};
use super::super::types::now_micros;

// ---------------------------------------------------------------------------
// normalize_path
// ---------------------------------------------------------------------------

pub(super) fn normalize_path(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

/// Reject paths that could escape the sandbox base directory.
/// Disallows absolute paths, `..` components, and null bytes.
fn validate_local_path(path: &str) -> Result<(), CmError> {
    use std::path::Component;
    if path.contains('\0') {
        return Err(CmError::FsError(format!("Invalid path (null byte): {path}")));
    }
    for component in std::path::Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(CmError::FsError(format!("Invalid path component in: {path}"))),
        }
    }
    Ok(())
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
    ) -> Result<FileMeta, CmError>;

    async fn read(&self, path: &str) -> Result<Vec<u8>, CmError>;

    async fn delete(&self, path: &str) -> Result<(), CmError>;

    async fn exists(&self, path: &str) -> Result<bool, CmError>;

    async fn metadata(&self, path: &str) -> Result<FileMeta, CmError>;

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, CmError>;

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
        Self { files: Mutex::new(HashMap::new()) }
    }
}

impl Default for MemoryFsProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FsProvider for MemoryFsProvider {
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
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

    async fn read(&self, path: &str) -> Result<Vec<u8>, CmError> {
        let key = normalize_path(path);
        self.files
            .lock()
            .get(&key)
            .map(|f| f.data.clone())
            .ok_or_else(|| CmError::FsError(format!("File not found: {path}")))
    }

    async fn delete(&self, path: &str) -> Result<(), CmError> {
        let key = normalize_path(path);
        self.files.lock().remove(&key);
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, CmError> {
        let key = normalize_path(path);
        Ok(self.files.lock().contains_key(&key))
    }

    async fn metadata(&self, path: &str) -> Result<FileMeta, CmError> {
        let key = normalize_path(path);
        let files = self.files.lock();
        let file = files
            .get(&key)
            .ok_or_else(|| CmError::FsError(format!("File not found: {path}")))?;
        Ok(FileMeta {
            path: key,
            size_bytes: file.data.len() as u64,
            mime_type: file.mime_type.clone(),
            created_at: file.created_at,
            modified_at: file.modified_at,
            checksum: None,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, CmError> {
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
                    entries.push(FileEntry { path: dir_path, is_dir: true, meta: None });
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
// LocalFsProvider
// ---------------------------------------------------------------------------

pub struct LocalFsProvider {
    base_path: PathBuf,
}

impl LocalFsProvider {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self { base_path: base_path.into() }
    }
}

#[async_trait]
impl FsProvider for LocalFsProvider {
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, CmError> {
        validate_local_path(path)?;
        let full_path = self.base_path.join(path);
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| CmError::FsError(e.to_string()))?;
        }
        tokio::fs::write(&full_path, data)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))?;

        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))?;

        let now = now_micros();
        Ok(FileMeta {
            path: path.to_string(),
            size_bytes: fs_meta.len(),
            mime_type: mime_type.to_string(),
            created_at: now,
            modified_at: now,
            checksum: None,
        })
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, CmError> {
        validate_local_path(path)?;
        let full_path = self.base_path.join(path);
        tokio::fs::read(&full_path)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))
    }

    async fn delete(&self, path: &str) -> Result<(), CmError> {
        validate_local_path(path)?;
        let full_path = self.base_path.join(path);
        tokio::fs::remove_file(&full_path)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))
    }

    async fn exists(&self, path: &str) -> Result<bool, CmError> {
        validate_local_path(path)?;
        let full_path = self.base_path.join(path);
        Ok(tokio::fs::try_exists(&full_path).await.unwrap_or(false))
    }

    async fn metadata(&self, path: &str) -> Result<FileMeta, CmError> {
        validate_local_path(path)?;
        let full_path = self.base_path.join(path);
        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))?;

        let now = now_micros();
        Ok(FileMeta {
            path: path.to_string(),
            size_bytes: fs_meta.len(),
            mime_type: "application/octet-stream".to_string(),
            created_at: now,
            modified_at: now,
            checksum: None,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, CmError> {
        validate_local_path(prefix)?;
        let dir_path = self.base_path.join(prefix);
        if !dir_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir_path)
            .await
            .map_err(|e| CmError::FsError(e.to_string()))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| CmError::FsError(e.to_string()))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| CmError::FsError(e.to_string()))?;

            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };

            if file_type.is_dir() {
                entries.push(FileEntry { path: entry_path, is_dir: true, meta: None });
            } else {
                let fs_meta = entry
                    .metadata()
                    .await
                    .map_err(|e| CmError::FsError(e.to_string()))?;
                let now = now_micros();
                entries.push(FileEntry {
                    path: entry_path.clone(),
                    is_dir: false,
                    meta: Some(FileMeta {
                        path: entry_path,
                        size_bytes: fs_meta.len(),
                        mime_type: "application/octet-stream".to_string(),
                        created_at: now,
                        modified_at: now,
                        checksum: None,
                    }),
                });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn provider_type(&self) -> &str {
        "local"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(err, CmError::FsError(_)));
    }

    #[tokio::test]
    async fn local_fs_provider_crud() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());

        let data = b"hello artifact";
        let meta = provider.write("test/file.txt", data, "text/plain").await.unwrap();
        assert_eq!(meta.size_bytes, data.len() as u64);

        assert!(provider.exists("test/file.txt").await.unwrap());

        let read_data = provider.read("test/file.txt").await.unwrap();
        assert_eq!(read_data, data);

        provider.delete("test/file.txt").await.unwrap();
        assert!(!provider.exists("test/file.txt").await.unwrap());
    }

    #[tokio::test]
    async fn local_fs_provider_list() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());

        provider.write("a/x.txt", b"x", "text/plain").await.unwrap();
        provider.write("a/y.txt", b"y", "text/plain").await.unwrap();

        let entries = provider.list("a").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !e.is_dir));
    }

    #[tokio::test]
    async fn local_fs_provider_read_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());
        assert!(provider.read("nonexistent.txt").await.is_err());
    }

    #[tokio::test]
    async fn local_fs_provider_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());

        // `..` component must be rejected.
        assert!(matches!(
            provider.write("../escape.txt", b"x", "text/plain").await,
            Err(CmError::FsError(_))
        ));
        assert!(matches!(
            provider.read("../etc/passwd").await,
            Err(CmError::FsError(_))
        ));
        assert!(matches!(
            provider.delete("../escape.txt").await,
            Err(CmError::FsError(_))
        ));

        // Null bytes must be rejected.
        assert!(matches!(
            provider.read("file\0name").await,
            Err(CmError::FsError(_))
        ));
    }
}
