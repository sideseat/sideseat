use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::HistoryError;
use super::storage::{HistoryStorage, ListParams};
use super::types::{
    ArtifactSetId, ConversationId, NodeId, StorageBackend, StorageRef, UserId, now_micros,
};
use super::source::vfs::{FileEntry, FileMeta, FsProvider};
use super::{ExtensionSet, HistoryExtension};
use super::source::VfsExtension;

// ---------------------------------------------------------------------------
// ArtifactSet
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSet {
    pub id: ArtifactSetId,
    pub conversation_id: ConversationId,
    pub name: String,
    pub artifact_kind: ArtifactKind,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub latest_version: u32,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    SingleFile,
    FileSet,
    Generated,
    Uploaded,
    Sandbox,
    Custom(String),
}

// ---------------------------------------------------------------------------
// ArtifactVersion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub artifact_set_id: ArtifactSetId,
    pub version: u32,
    pub created_at: i64,
    pub created_by: Option<UserId>,
    pub node_id: Option<NodeId>,
    pub files: Vec<ArtifactFile>,
    pub changes: Vec<FileChange>,
    pub base_version: Option<u32>,
    pub changelog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactFile {
    pub path: String,
    pub file_type: FileType,
    pub mime_type: String,
    pub size_bytes: u64,
    pub storage_ref: StorageRef,
    pub checksum: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub diff: Option<String>,
    pub old_checksum: Option<String>,
    pub new_checksum: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Copied { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    Text,
    Binary,
    Directory,
}

// ---------------------------------------------------------------------------
// ArtifactExtension
// ---------------------------------------------------------------------------

/// Stateless extension that adds versioned artifact management to a `History`.
/// Register with `History::with_extension(Arc::new(ArtifactExtension))`.
pub struct ArtifactExtension;

impl HistoryExtension for ArtifactExtension {
    fn id(&self) -> &str {
        "artifact"
    }
}

impl ArtifactExtension {
    /// Create a new artifact set linked to this conversation.
    pub async fn create(
        &self,
        storage: &impl HistoryStorage,
        conversation_id: ConversationId,
        name: impl Into<String>,
        kind: ArtifactKind,
    ) -> Result<ArtifactSetId, HistoryError> {
        let now = now_micros();
        let set = ArtifactSet {
            id: ArtifactSetId::new(),
            conversation_id,
            name: name.into(),
            artifact_kind: kind,
            description: None,
            created_at: now,
            updated_at: now,
            latest_version: 0,
            metadata: std::collections::HashMap::new(),
        };
        let id = set.id.clone();
        storage.save_artifact_set(&set).await?;
        Ok(id)
    }

    pub async fn get(
        &self,
        storage: &impl HistoryStorage,
        id: &ArtifactSetId,
    ) -> Result<Option<ArtifactSet>, HistoryError> {
        storage.get_artifact_set(id).await
    }

    /// Push a new version. `version` should be `set.latest_version + 1`.
    pub async fn push_version(
        &self,
        storage: &impl HistoryStorage,
        version: &ArtifactVersion,
    ) -> Result<(), HistoryError> {
        // Bump latest_version on the set
        if let Some(mut set) = storage.get_artifact_set(&version.artifact_set_id).await?
            && version.version > set.latest_version
        {
            set.latest_version = version.version;
            set.updated_at = now_micros();
            storage.save_artifact_set(&set).await?;
        }
        storage.save_artifact_version(version).await
    }

    pub async fn list_versions(
        &self,
        storage: &impl HistoryStorage,
        set_id: &ArtifactSetId,
    ) -> Result<Vec<ArtifactVersion>, HistoryError> {
        storage.list_artifact_versions(set_id, &ListParams::default()).await
    }

    pub async fn get_version(
        &self,
        storage: &impl HistoryStorage,
        set_id: &ArtifactSetId,
        version: u32,
    ) -> Result<Option<ArtifactVersion>, HistoryError> {
        storage.get_artifact_version(set_id, version).await
    }

    // -----------------------------------------------------------------------
    // VFS integration (depends on VfsExtension via ExtensionSet)
    // -----------------------------------------------------------------------
    //
    // Each artifact version lives in its own **named VFS branch**:
    //
    //   branch name : `art/{set_id}/v{version}`
    //   logical path: `{file_path}` within that branch
    //   StorageRef.uri: `art/{set_id}/v{version}/{file_path}`
    //
    // When a new version is created from an existing one, `fork_version` COW-
    // clones the parent branch's index into the child. Unchanged files are
    // never re-written; they read through the inherited physical key.

    fn branch_name(set_id: &ArtifactSetId, version: u32) -> String {
        format!("art/{set_id}/v{version}")
    }

    /// Parse a `Vfs`-backend artifact `StorageRef.uri` into `(branch, file_path)`.
    ///
    /// URI format: `art/{set_id}/v{version}/{file_path}` — the branch is the
    /// first three path components; everything after is the file path.
    fn parse_artifact_uri(uri: &str) -> Result<(String, String), HistoryError> {
        // Split into at most 4 parts: ["art", "{set_id}", "v{n}", "{file_path…}"]
        let mut parts = uri.splitn(4, '/');
        let p0 = parts.next().unwrap_or("");
        let p1 = parts.next().unwrap_or("");
        let p2 = parts.next().unwrap_or("");
        let p3 = parts.next().unwrap_or("");

        if p0 != "art" || p1.is_empty() || !p2.starts_with('v') || p3.is_empty() {
            return Err(HistoryError::FsError(format!(
                "Malformed artifact VFS URI: {uri}"
            )));
        }
        Ok((format!("{p0}/{p1}/{p2}"), p3.to_string()))
    }

    /// COW-fork the VFS branch of `base_version` into `new_version`.
    ///
    /// Call this **before** writing any files for `new_version`. Unchanged
    /// files from `base_version` will be inherited without re-writing.
    pub fn fork_version(
        &self,
        extensions: &ExtensionSet,
        set_id: &ArtifactSetId,
        base_version: u32,
        new_version: u32,
    ) -> Result<(), HistoryError> {
        let vfs = extensions
            .extension::<VfsExtension>()
            .ok_or_else(|| HistoryError::FsError("VfsExtension not registered".into()))?;
        let parent = Self::branch_name(set_id, base_version);
        let child = Self::branch_name(set_id, new_version);
        vfs.fork_named_branch(&parent, &child);
        Ok(())
    }

    /// Write a file for a specific artifact version into its VFS branch.
    ///
    /// If this version was created via [`fork_version`], only the files
    /// written here are physically stored; all others are inherited from the
    /// parent version's branch via the COW index.
    ///
    /// Returns a `StorageRef` whose `uri` encodes the branch and file path.
    pub async fn write_file(
        &self,
        extensions: &ExtensionSet,
        set_id: &ArtifactSetId,
        version: u32,
        file_path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<StorageRef, HistoryError> {
        let vfs = extensions
            .extension::<VfsExtension>()
            .ok_or_else(|| HistoryError::FsError("VfsExtension not registered".into()))?;

        let branch = Self::branch_name(set_id, version);
        let meta = vfs.write_on(&branch, file_path, data, mime_type).await?;

        Ok(StorageRef {
            backend: StorageBackend::Vfs,
            uri: format!("{branch}/{file_path}"),
            checksum: meta.checksum,
            size_bytes: Some(meta.size_bytes),
        })
    }

    /// Read a file from a `StorageRef` produced by [`write_file`].
    ///
    /// Follows the COW branch index, so files inherited from a parent version
    /// are transparently resolved to the parent's physical key.
    pub async fn read_file(
        &self,
        extensions: &ExtensionSet,
        storage_ref: &StorageRef,
    ) -> Result<Vec<u8>, HistoryError> {
        match &storage_ref.backend {
            StorageBackend::Vfs => {
                let vfs = extensions
                    .extension::<VfsExtension>()
                    .ok_or_else(|| HistoryError::FsError("VfsExtension not registered".into()))?;
                let (branch, path) = Self::parse_artifact_uri(&storage_ref.uri)?;
                vfs.read_on(&branch, &path).await
            }
            StorageBackend::Inline => {
                let data_str = storage_ref
                    .uri
                    .strip_prefix("inline:")
                    .unwrap_or(&storage_ref.uri);
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data_str)
                    .map_err(|e| HistoryError::FsError(format!("Base64 decode: {e}")))
            }
            other => Err(HistoryError::FsError(format!(
                "Cannot read {other:?} backend via ArtifactExtension"
            ))),
        }
    }

    // -----------------------------------------------------------------------

    /// Convenience: fetch the latest version for a set.
    pub async fn latest_version(
        &self,
        storage: &impl HistoryStorage,
        set_id: &ArtifactSetId,
    ) -> Result<Option<ArtifactVersion>, HistoryError> {
        let set = match storage.get_artifact_set(set_id).await? {
            Some(s) => s,
            None => return Ok(None),
        };
        if set.latest_version == 0 {
            return Ok(None);
        }
        storage.get_artifact_version(set_id, set.latest_version).await
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
        Self {
            base_path: base_path.into(),
        }
    }
}

#[async_trait]
impl FsProvider for LocalFsProvider {
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<FileMeta, HistoryError> {
        let full_path = self.base_path.join(path);
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| HistoryError::FsError(e.to_string()))?;
        }
        tokio::fs::write(&full_path, data)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))?;

        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))?;

        let now = super::types::now_micros();
        Ok(FileMeta {
            path: path.to_string(),
            size_bytes: fs_meta.len(),
            mime_type: mime_type.to_string(),
            created_at: now,
            modified_at: now,
            checksum: None,
        })
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, HistoryError> {
        let full_path = self.base_path.join(path);
        tokio::fs::read(&full_path)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))
    }

    async fn delete(&self, path: &str) -> Result<(), HistoryError> {
        let full_path = self.base_path.join(path);
        tokio::fs::remove_file(&full_path)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))
    }

    async fn exists(&self, path: &str) -> Result<bool, HistoryError> {
        let full_path = self.base_path.join(path);
        Ok(tokio::fs::try_exists(&full_path).await.unwrap_or(false))
    }

    async fn metadata(&self, path: &str) -> Result<FileMeta, HistoryError> {
        let full_path = self.base_path.join(path);
        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))?;

        let now = super::types::now_micros();
        Ok(FileMeta {
            path: path.to_string(),
            size_bytes: fs_meta.len(),
            // Can't determine mime from fs metadata; use generic
            mime_type: "application/octet-stream".to_string(),
            created_at: now,
            modified_at: now,
            checksum: None,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<FileEntry>, HistoryError> {
        let dir_path = self.base_path.join(prefix);
        if !dir_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir_path)
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| HistoryError::FsError(e.to_string()))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| HistoryError::FsError(e.to_string()))?;

            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };

            if file_type.is_dir() {
                entries.push(FileEntry {
                    path: entry_path,
                    is_dir: true,
                    meta: None,
                });
            } else {
                let fs_meta = entry
                    .metadata()
                    .await
                    .map_err(|e| HistoryError::FsError(e.to_string()))?;
                let now = super::types::now_micros();
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
    use super::super::types::{now_micros, StorageBackend};

    #[tokio::test]
    async fn local_fs_provider_crud() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());

        let data = b"hello artifact";
        let meta = provider
            .write("test/file.txt", data, "text/plain")
            .await
            .unwrap();

        assert_eq!(meta.size_bytes, data.len() as u64);
        assert!(provider.exists("test/file.txt").await.unwrap());

        let read_data = provider.read("test/file.txt").await.unwrap();
        assert_eq!(read_data, data);

        let file_meta = provider.metadata("test/file.txt").await.unwrap();
        assert_eq!(file_meta.size_bytes, data.len() as u64);

        provider.delete("test/file.txt").await.unwrap();
        assert!(!provider.exists("test/file.txt").await.unwrap());
    }

    #[tokio::test]
    async fn local_fs_provider_list() {
        let dir = tempfile::tempdir().unwrap();
        let provider = LocalFsProvider::new(dir.path());

        provider.write("docs/a.txt", b"a", "text/plain").await.unwrap();
        provider.write("docs/b.txt", b"bb", "text/plain").await.unwrap();

        let entries = provider.list("docs").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !e.is_dir));
    }

    #[test]
    fn version_chain() {
        let set_id = ArtifactSetId::new();

        let v1 = ArtifactVersion {
            artifact_set_id: set_id.clone(),
            version: 1,
            created_at: now_micros(),
            created_by: None,
            node_id: None,
            files: vec![ArtifactFile {
                path: "main.rs".into(),
                file_type: FileType::Text,
                mime_type: "text/x-rust".into(),
                size_bytes: 100,
                storage_ref: StorageRef {
                    backend: StorageBackend::Inline,
                    uri: "inline:base64data".into(),
                    checksum: Some("abc123".into()),
                    size_bytes: Some(100),
                },
                checksum: Some("abc123".into()),
                language: Some("rust".into()),
            }],
            changes: vec![FileChange {
                path: "main.rs".into(),
                change_type: ChangeType::Added,
                diff: None,
                old_checksum: None,
                new_checksum: Some("abc123".into()),
            }],
            base_version: None,
            changelog: Some("Initial version".into()),
        };

        let v2 = ArtifactVersion {
            version: 2,
            base_version: Some(1),
            changes: vec![FileChange {
                path: "main.rs".into(),
                change_type: ChangeType::Modified,
                diff: Some("@@ -1 +1 @@\n-old\n+new".into()),
                old_checksum: Some("abc123".into()),
                new_checksum: Some("def456".into()),
            }],
            changelog: Some("Bug fix".into()),
            ..v1.clone()
        };

        assert!(v2.base_version.is_some());
        assert_eq!(v2.version, 2);
    }

    #[tokio::test]
    async fn artifact_write_read_via_vfs() {
        use std::sync::Arc;
        use super::super::{ExtensionSet, source::VfsExtension};

        let extensions = ExtensionSet::new();
        extensions.register(Arc::new(VfsExtension::new()));

        let ext = ArtifactExtension;
        let set_id = ArtifactSetId::new();

        let code = b"fn main() { println!(\"hello\"); }";
        let sref = ext
            .write_file(&extensions, &set_id, 1, "src/main.rs", code, "text/x-rust")
            .await
            .unwrap();

        assert_eq!(sref.backend, StorageBackend::Vfs);
        // URI encodes the versioned branch: art/{set_id}/v1/src/main.rs
        assert!(sref.uri.starts_with("art/"), "uri: {}", sref.uri);
        assert!(sref.uri.contains("/v1/"), "uri: {}", sref.uri);
        assert_eq!(sref.size_bytes, Some(code.len() as u64));

        let read_back = ext.read_file(&extensions, &sref).await.unwrap();
        assert_eq!(read_back, code);
    }

    #[tokio::test]
    async fn artifact_cow_fork_version() {
        use std::sync::Arc;
        use super::super::{ExtensionSet, source::VfsExtension};

        let extensions = ExtensionSet::new();
        extensions.register(Arc::new(VfsExtension::new()));

        let ext = ArtifactExtension;
        let set_id = ArtifactSetId::new();

        // Version 1: write two files
        let main_v1 = b"fn main() {}";
        let readme_v1 = b"# My Project";

        let sref_main_v1 = ext
            .write_file(&extensions, &set_id, 1, "src/main.rs", main_v1, "text/x-rust")
            .await
            .unwrap();
        let sref_readme_v1 = ext
            .write_file(&extensions, &set_id, 1, "README.md", readme_v1, "text/markdown")
            .await
            .unwrap();

        // Version 2: COW-fork from v1, only change main.rs
        ext.fork_version(&extensions, &set_id, 1, 2).unwrap();

        let main_v2 = b"fn main() { println!(\"v2\"); }";
        let sref_main_v2 = ext
            .write_file(&extensions, &set_id, 2, "src/main.rs", main_v2, "text/x-rust")
            .await
            .unwrap();

        // README.md in v2 inherits from v1 via COW — no re-write
        // We reconstruct the v2 StorageRef for README by replacing the version in the URI
        let sref_readme_v2 = StorageRef {
            uri: sref_readme_v1.uri.replacen("/v1/", "/v2/", 1),
            ..sref_readme_v1.clone()
        };

        // v2 main.rs is the new content
        let read_main_v2 = ext.read_file(&extensions, &sref_main_v2).await.unwrap();
        assert_eq!(read_main_v2, main_v2);

        // v2 README is inherited from v1 (same physical storage, different branch index)
        let read_readme_v2 = ext.read_file(&extensions, &sref_readme_v2).await.unwrap();
        assert_eq!(read_readme_v2, readme_v1, "README should be inherited from v1");

        // v1 main.rs is unchanged
        let read_main_v1 = ext.read_file(&extensions, &sref_main_v1).await.unwrap();
        assert_eq!(read_main_v1, main_v1);

        // v1 README is still readable
        let read_readme_v1 = ext.read_file(&extensions, &sref_readme_v1).await.unwrap();
        assert_eq!(read_readme_v1, readme_v1);
    }

    #[tokio::test]
    async fn artifact_write_file_requires_vfs() {
        let extensions = ExtensionSet::new(); // no VFS registered
        let ext = ArtifactExtension;
        let set_id = ArtifactSetId::new();

        let result = ext
            .write_file(&extensions, &set_id, 1, "file.txt", b"data", "text/plain")
            .await;

        assert!(result.is_err());
    }
}
