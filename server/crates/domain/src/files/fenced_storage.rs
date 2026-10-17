//! The blob store as this process may change it: no object is deleted while its store lease is lost.
//!
//! Every deletion of stored bytes - a collector's, a project's, a staged payload's release - goes through here, so
//! one closed [`CollectionFence`] stops all of them, including any added later, and a delete already under way is
//! abandoned at the fence's deadline rather than finishing after a restore repair started.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use sideseat_core::utils::collection_fence::CollectionFence;
use sideseat_ports::blobs::{FileStorage, FileStorageError, ObjectName, ObjectVisitor};
use sideseat_ports::types::ProjectId;

pub struct FencedStorage {
    inner: Arc<dyn FileStorage>,
    fence: CollectionFence,
}

impl FencedStorage {
    pub fn new(inner: Arc<dyn FileStorage>, fence: CollectionFence) -> Self {
        Self { inner, fence }
    }

    /// Refused before it started when the fence is closed; abandoned, its outcome unknown, at the deadline.
    async fn deleting<T>(
        &self,
        delete: impl std::future::Future<Output = Result<T, FileStorageError>>,
    ) -> Result<T, FileStorageError> {
        if !self.fence.is_open() {
            return Err(FileStorageError::Fenced);
        }
        self.fence
            .bounded(delete)
            .await
            .unwrap_or(Err(FileStorageError::Abandoned))
    }
}

#[async_trait]
impl FileStorage for FencedStorage {
    async fn store(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        self.inner.store(project_id, object, data).await
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError> {
        self.inner.get(project_id, object).await
    }

    async fn exists(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError> {
        self.inner.exists(project_id, object).await
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError> {
        self.deleting(self.inner.delete(project_id, object)).await
    }

    async fn incarnations(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError> {
        self.inner.incarnations(project_id, hash).await
    }

    async fn scan_objects(&self, visitor: &mut dyn ObjectVisitor) -> Result<(), FileStorageError> {
        self.inner.scan_objects(visitor).await
    }

    async fn max_incarnation(&self) -> Result<i64, FileStorageError> {
        self.inner.max_incarnation().await
    }

    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        self.deleting(self.inner.delete_project(project_id)).await
    }

    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        temp_path: &Path,
        expected: sideseat_ports::blobs::ExpectedContent,
    ) -> Result<(), FileStorageError> {
        self.inner
            .finalize_temp(project_id, object, temp_path, expected)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use sideseat_adapter_blob_storage::FilesystemStorage;
    use tokio::sync::watch;
    use tokio::time::Instant;

    const HASH: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    /// Closed, the fence refuses every deletion and nothing else; open, they go through.
    #[tokio::test]
    async fn a_closed_fence_refuses_deletes_and_nothing_else() {
        let root = tempfile::TempDir::new().unwrap();
        let project = ProjectId::from("project");
        let (held, receiver) = watch::channel(None);
        let storage = FencedStorage::new(
            Arc::new(FilesystemStorage::new(root.path().to_path_buf())),
            CollectionFence::following(receiver),
        );
        let name = ObjectName::new(HASH, 1);
        storage.store(&project, name, b"bytes").await.unwrap();
        assert_eq!(storage.get(&project, name).await.unwrap(), b"bytes");

        assert!(matches!(
            storage.delete(&project, name).await,
            Err(FileStorageError::Fenced)
        ));
        assert!(matches!(
            storage.delete_project(&project).await,
            Err(FileStorageError::Fenced)
        ));
        assert!(storage.exists(&project, name).await.unwrap());

        held.send(Some(
            sideseat_core::utils::collection_fence::HeldUntil::new(
                Instant::now() + Duration::from_secs(60),
                0,
            ),
        ))
        .unwrap();
        storage.delete(&project, name).await.unwrap();
        assert!(!storage.exists(&project, name).await.unwrap());
    }

    /// A trace's cleanup is refused while the lease is lost; one whose release landed before the fence closed
    /// still counts the file as unreferenced, so the zero-reference sweep, once the lease is held again, finds and
    /// collects it - left counted as referenced, nothing would ever find it again.
    #[tokio::test]
    async fn a_fenced_cleanup_counts_what_it_leaves_for_the_sweep() {
        use crate::files::FileService;
        use crate::files::cleanup::cleanup_zero_ref_files_governed;
        use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
        use sideseat_core::config::{
            CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, StorageBackend,
        };
        use sideseat_core::storage::AppStorage;
        use sideseat_core::utils::collection_fence::HeldUntil;
        use sideseat_ports::traits::TransactionalRepository;

        #[derive(Debug)]
        struct Now;
        impl sideseat_ports::clock::Clock for Now {
            fn now(&self) -> chrono::DateTime<chrono::Utc> {
                chrono::Utc::now()
            }
        }

        let root = tempfile::TempDir::new().unwrap();
        let app_storage = AppStorage::init_for_test(root.path().to_path_buf());
        let database: Arc<dyn TransactionalRepository + Send + Sync> =
            Arc::new(SqliteRepository(Arc::new(
                SqliteService::init(&app_storage, Arc::new(Now))
                    .await
                    .unwrap(),
            )));
        let cache = Arc::new(
            sideseat_adapter_cache::CacheService::new(&CacheConfig {
                backend: CacheBackendType::Memory,
                max_entries: 10,
                eviction_policy: EvictionPolicy::TinyLfu,
                redis_url: None,
            })
            .await
            .unwrap(),
        );
        let (held, receiver) = watch::channel(None);
        let fence = CollectionFence::following(receiver);
        let service = FileService::new_inner(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 1 << 20,
                filesystem_path: None,
                s3: None,
            },
            root.path().join("files_temp"),
            Arc::new(FilesystemStorage::new(root.path().join("files"))),
            Arc::clone(&database),
            cache,
            None,
            fence.clone(),
        );
        let project = ProjectId::from("project");
        database
            .upsert_file(&project, HASH, None, 5, "sha256")
            .await
            .unwrap();
        let row = database.get_file(&project, HASH).await.unwrap().unwrap();
        let name = ObjectName::new(HASH, row.id);
        service
            .storage()
            .store(&project, name, b"bytes")
            .await
            .unwrap();
        database
            .insert_trace_file("trace", &project, HASH)
            .await
            .unwrap();

        // Refused whole while fenced: the association stays with its count.
        assert!(
            service
                .cleanup_traces(&project, &["trace".to_string()])
                .await
                .is_err()
        );
        assert_eq!(
            database
                .get_file(&project, HASH)
                .await
                .unwrap()
                .unwrap()
                .ref_count,
            1
        );
        // The fence closing between a cleanup's release and its reclaim: the association is gone, and the
        // reclaim, refused its collection, still counts what is left.
        database
            .delete_trace_files(&project, &["trace".to_string()])
            .await
            .unwrap();
        service
            .reclaim_unreferenced(&project, vec![HASH.to_string()])
            .await
            .unwrap();
        let row = database.get_file(&project, HASH).await.unwrap().unwrap();
        assert_eq!(
            row.ref_count, 0,
            "left for the sweep, counted as referenced"
        );
        assert!(service.storage().exists(&project, name).await.unwrap());

        held.send(Some(HeldUntil::new(
            Instant::now() + Duration::from_secs(60),
            1,
        )))
        .unwrap();
        let collected = cleanup_zero_ref_files_governed(
            service.storage(),
            service.database(),
            sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS,
            None,
            &fence,
        )
        .await
        .unwrap();
        assert_eq!(collected, 1);
        assert!(!service.storage().exists(&project, name).await.unwrap());
    }
}
