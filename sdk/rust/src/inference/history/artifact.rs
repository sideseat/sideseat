use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::HistoryError;
use super::types::{
    ArtifactSetId, ConversationId, NodeId, StorageRef, UserId,
};
use super::vfs::{FileEntry, FileMeta, FsProvider};

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
}
