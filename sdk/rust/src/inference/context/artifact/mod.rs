use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::ContextBackend;
use super::error::CmError;
use super::types::{
    ArtifactSetId, ConversationId, NodeId, StorageBackend, StorageRef, UserId, now_micros,
};
use super::vfs::VfsExtension;

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

/// Stateless extension for versioned artifact management.
/// Metadata stored in KV (`artifact:{set_id}` namespace).
/// File content stored in the VFS via branch-per-version COW.
pub struct ArtifactExtension;

impl super::ContextExtension for ArtifactExtension {
    fn id(&self) -> &str {
        "artifact"
    }
}

impl ArtifactExtension {
    // -----------------------------------------------------------------------
    // KV helpers
    // -----------------------------------------------------------------------

    fn ns(set_id: &ArtifactSetId) -> String {
        format!("artifact:{}", set_id.as_str())
    }

    fn version_key(version: u32) -> String {
        format!("v{:020}", version)
    }

    // -----------------------------------------------------------------------
    // Artifact set CRUD
    // -----------------------------------------------------------------------

    pub async fn create<B: ContextBackend>(
        &self,
        backend: &B,
        conversation_id: ConversationId,
        name: impl Into<String>,
        kind: ArtifactKind,
    ) -> Result<ArtifactSetId, CmError> {
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
            metadata: HashMap::new(),
        };
        let id = set.id.clone();
        let bytes = serde_json::to_vec(&set)?;
        backend.kv_put(&Self::ns(&id), "meta", &bytes).await?;
        Ok(id)
    }

    pub async fn get<B: ContextBackend>(
        &self,
        backend: &B,
        id: &ArtifactSetId,
    ) -> Result<Option<ArtifactSet>, CmError> {
        match backend.kv_get(&Self::ns(id), "meta").await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Version CRUD
    // -----------------------------------------------------------------------

    /// Persist a new version and bump `latest_version` on the set.
    ///
    /// # Concurrency note
    ///
    /// The read-modify-write on `latest_version` is not atomic for distributed
    /// backends. For `InMemoryContextBackend` (single `Mutex`) the operation is
    /// safe. For production KV stores, callers must serialize `push_version`
    /// calls for a given artifact set (e.g., hold an application-level lock or
    /// route all writes for a set through a single actor). The condition
    /// `version.version > set.latest_version` prevents regression but cannot
    /// prevent a concurrent higher-version write from being obscured by a
    /// lower-version write that races ahead of it.
    pub async fn push_version<B: ContextBackend>(
        &self,
        backend: &B,
        version: &ArtifactVersion,
    ) -> Result<(), CmError> {
        // Bump latest_version on the set.
        if let Some(mut set) = self.get(backend, &version.artifact_set_id).await?
            && version.version > set.latest_version
        {
            set.latest_version = version.version;
            set.updated_at = now_micros();
            let bytes = serde_json::to_vec(&set)?;
            backend
                .kv_put(&Self::ns(&version.artifact_set_id), "meta", &bytes)
                .await?;
        }

        let key = Self::version_key(version.version);
        let bytes = serde_json::to_vec(version)?;
        backend
            .kv_put(&Self::ns(&version.artifact_set_id), &key, &bytes)
            .await
    }

    pub async fn get_version<B: ContextBackend>(
        &self,
        backend: &B,
        set_id: &ArtifactSetId,
        version: u32,
    ) -> Result<Option<ArtifactVersion>, CmError> {
        let key = Self::version_key(version);
        match backend.kv_get(&Self::ns(set_id), &key).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Return all versions in ascending order (KV list is lexicographic;
    /// zero-padded keys ensure lex == numeric order).
    pub async fn list_versions<B: ContextBackend>(
        &self,
        backend: &B,
        set_id: &ArtifactSetId,
    ) -> Result<Vec<ArtifactVersion>, CmError> {
        let ns = Self::ns(set_id);
        let keys = backend.kv_list(&ns, "v").await?;
        let mut versions = Vec::new();
        for key in keys {
            if let Some(bytes) = backend.kv_get(&ns, &key).await? {
                let v: ArtifactVersion = serde_json::from_slice(&bytes)?;
                versions.push(v);
            }
        }
        Ok(versions)
    }

    pub async fn latest_version<B: ContextBackend>(
        &self,
        backend: &B,
        set_id: &ArtifactSetId,
    ) -> Result<Option<ArtifactVersion>, CmError> {
        let set = match self.get(backend, set_id).await? {
            Some(s) => s,
            None => return Ok(None),
        };
        if set.latest_version == 0 {
            return Ok(None);
        }
        self.get_version(backend, set_id, set.latest_version).await
    }

    // -----------------------------------------------------------------------
    // VFS file operations (COW branch per version)
    //
    // Branch name: `art/{set_id}/v{version}`
    // StorageRef.uri: `art/{set_id}/v{version}/{file_path}`
    // -----------------------------------------------------------------------

    fn vfs_branch(set_id: &ArtifactSetId, version: u32) -> String {
        format!("art/{}/v{}", set_id.as_str(), version)
    }

    fn parse_artifact_uri(uri: &str) -> Result<(String, String), CmError> {
        let mut parts = uri.splitn(4, '/');
        let p0 = parts.next().unwrap_or("");
        let p1 = parts.next().unwrap_or("");
        let p2 = parts.next().unwrap_or("");
        let p3 = parts.next().unwrap_or("");

        if p0 != "art" || p1.is_empty() || !p2.starts_with('v') || p3.is_empty() {
            return Err(CmError::FsError(format!(
                "Malformed artifact VFS URI: {uri}"
            )));
        }
        Ok((format!("{p0}/{p1}/{p2}"), p3.to_string()))
    }

    /// COW-fork the VFS branch of `base_version` into `new_version`.
    pub fn fork_version(
        &self,
        vfs: &VfsExtension,
        set_id: &ArtifactSetId,
        base_version: u32,
        new_version: u32,
    ) {
        let parent = Self::vfs_branch(set_id, base_version);
        let child = Self::vfs_branch(set_id, new_version);
        vfs.fork_named_branch(&parent, &child);
    }

    /// Write a file for a specific artifact version into its VFS branch.
    pub async fn write_file(
        &self,
        vfs: &VfsExtension,
        set_id: &ArtifactSetId,
        version: u32,
        file_path: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<StorageRef, CmError> {
        let branch = Self::vfs_branch(set_id, version);
        let meta = vfs.write_on(&branch, file_path, data, mime_type).await?;
        Ok(StorageRef {
            backend: StorageBackend::Vfs,
            uri: format!("{branch}/{file_path}"),
            checksum: meta.checksum,
            size_bytes: Some(meta.size_bytes),
        })
    }

    /// Read a file from a `StorageRef` produced by [`write_file`].
    pub async fn read_file(
        &self,
        vfs: &VfsExtension,
        storage_ref: &StorageRef,
    ) -> Result<Vec<u8>, CmError> {
        match &storage_ref.backend {
            StorageBackend::Vfs => {
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
                    .map_err(|e| CmError::FsError(format!("Base64 decode: {e}")))
            }
            other => Err(CmError::FsError(format!(
                "Cannot read {other:?} backend via ArtifactExtension"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::backend::InMemoryContextBackend;
    use crate::context::vfs::{MemoryFsProvider, Vfs, VfsExtension};
    use std::sync::Arc;

    fn make_backend() -> InMemoryContextBackend {
        InMemoryContextBackend::new()
    }

    fn make_vfs() -> VfsExtension {
        VfsExtension::with_vfs(Vfs::new(Arc::new(MemoryFsProvider::new())))
    }

    #[tokio::test]
    async fn create_and_get() {
        let backend = make_backend();
        let ext = ArtifactExtension;
        let conv_id = ConversationId::new();

        let id = ext
            .create(&backend, conv_id, "my-artifact", ArtifactKind::SingleFile)
            .await
            .unwrap();

        let set = ext.get(&backend, &id).await.unwrap().unwrap();
        assert_eq!(set.name, "my-artifact");
        assert_eq!(set.latest_version, 0);
    }

    #[tokio::test]
    async fn push_version_bumps_latest() {
        let backend = make_backend();
        let ext = ArtifactExtension;
        let conv_id = ConversationId::new();

        let id = ext
            .create(&backend, conv_id, "art", ArtifactKind::Generated)
            .await
            .unwrap();

        let v1 = ArtifactVersion {
            artifact_set_id: id.clone(),
            version: 1,
            created_at: now_micros(),
            created_by: None,
            node_id: None,
            files: Vec::new(),
            changes: Vec::new(),
            base_version: None,
            changelog: Some("Initial".into()),
        };
        ext.push_version(&backend, &v1).await.unwrap();

        let set = ext.get(&backend, &id).await.unwrap().unwrap();
        assert_eq!(set.latest_version, 1);

        let fetched = ext.get_version(&backend, &id, 1).await.unwrap().unwrap();
        assert_eq!(fetched.changelog, Some("Initial".into()));
    }

    #[tokio::test]
    async fn list_versions_lex_order() {
        let backend = make_backend();
        let ext = ArtifactExtension;
        let conv_id = ConversationId::new();

        let id = ext
            .create(&backend, conv_id, "art", ArtifactKind::FileSet)
            .await
            .unwrap();

        for ver in [3u32, 1, 2] {
            let v = ArtifactVersion {
                artifact_set_id: id.clone(),
                version: ver,
                created_at: now_micros(),
                created_by: None,
                node_id: None,
                files: Vec::new(),
                changes: Vec::new(),
                base_version: None,
                changelog: None,
            };
            ext.push_version(&backend, &v).await.unwrap();
        }

        let versions = ext.list_versions(&backend, &id).await.unwrap();
        assert_eq!(versions.len(), 3);
        // Zero-padded keys guarantee ascending order.
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[2].version, 3);
    }

    #[tokio::test]
    async fn write_and_read_file() {
        let vfs = make_vfs();
        let ext = ArtifactExtension;
        let set_id = ArtifactSetId::new();

        let sref = ext
            .write_file(&vfs, &set_id, 1, "main.rs", b"fn main() {}", "text/x-rust")
            .await
            .unwrap();

        let data = ext.read_file(&vfs, &sref).await.unwrap();
        assert_eq!(data, b"fn main() {}");
    }

    #[tokio::test]
    async fn fork_version_cow() {
        let vfs = make_vfs();
        let ext = ArtifactExtension;
        let set_id = ArtifactSetId::new();

        // Write v1 files.
        ext.write_file(&vfs, &set_id, 1, "a.txt", b"v1-a", "text/plain")
            .await
            .unwrap();
        ext.write_file(&vfs, &set_id, 1, "b.txt", b"v1-b", "text/plain")
            .await
            .unwrap();

        // Fork v1 → v2 and overwrite only a.txt.
        ext.fork_version(&vfs, &set_id, 1, 2);
        let sref_a2 = ext
            .write_file(&vfs, &set_id, 2, "a.txt", b"v2-a", "text/plain")
            .await
            .unwrap();

        // b.txt in v2 is inherited from v1 (COW).
        let branch_v2 = format!("art/{}/v2", set_id.as_str());
        let b_data = vfs.read_on(&branch_v2, "b.txt").await.unwrap();
        assert_eq!(b_data, b"v1-b");

        // a.txt in v2 is the new version.
        let a2_data = ext.read_file(&vfs, &sref_a2).await.unwrap();
        assert_eq!(a2_data, b"v2-a");
    }

    #[test]
    fn version_key_zero_padded() {
        let k1 = ArtifactExtension::version_key(1);
        let k10 = ArtifactExtension::version_key(10);
        let k1000 = ArtifactExtension::version_key(1000);
        // Lex order must equal numeric order.
        assert!(k1 < k10);
        assert!(k10 < k1000);
    }
}
