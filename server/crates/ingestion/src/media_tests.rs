use super::*;

use async_trait::async_trait;
use futures::future::BoxFuture;
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::TransactionalRepository;
use tokio::sync::Mutex;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("instant")
    }
}

type Interleaving = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// The filesystem store, with something run right after the first existence check answers.
struct AfterFirstCheck {
    inner: FilesystemStorage,
    then: Mutex<Option<Interleaving>>,
}

#[async_trait]
impl FileStorage for AfterFirstCheck {
    async fn store(
        &self,
        project: &ProjectId,
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        self.inner.store(project, object, data).await
    }

    async fn get(
        &self,
        project: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError> {
        self.inner.get(project, object).await
    }

    async fn exists(
        &self,
        project: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError> {
        let answer = self.inner.exists(project, object).await;
        let then = self.then.lock().await.take();
        if let Some(then) = then {
            then().await;
        }
        answer
    }

    async fn delete(
        &self,
        project: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError> {
        self.inner.delete(project, object).await
    }

    async fn incarnations(
        &self,
        project: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError> {
        self.inner.incarnations(project, hash).await
    }

    async fn scan_objects(
        &self,
        visitor: &mut dyn sideseat_ports::blobs::ObjectVisitor,
    ) -> Result<(), FileStorageError> {
        self.inner.scan_objects(visitor).await
    }

    async fn delete_project(&self, project: &ProjectId) -> Result<u64, FileStorageError> {
        self.inner.delete_project(project).await
    }

    async fn finalize_temp(
        &self,
        project: &ProjectId,
        object: ObjectName<'_>,
        temp_path: &std::path::Path,
        expected: sideseat_ports::blobs::ExpectedContent,
    ) -> Result<(), FileStorageError> {
        self.inner
            .finalize_temp(project, object, temp_path, expected)
            .await
    }
}

/// A reference checked as backed whose file is then collected, and associated again by another writer whose
/// bytes are not stored yet: the association this reference gets protects that new incarnation, which has no
/// bytes, so the reference must not be committed on the strength of the check made before it.
#[tokio::test]
async fn an_incoming_reference_is_backed_by_the_incarnation_it_is_associated_with() {
    let temp = tempfile::TempDir::new().unwrap();
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&AppStorage::init_for_test(temp.path().to_path_buf()), clock)
                .await
                .unwrap(),
        )));
    let project = ProjectId::from("default");
    let hash = "c".repeat(64);
    let blobs = FilesystemStorage::new(temp.path().join("files"));
    database
        .associate_file("trace-old", &project, &hash, Some("image/png"), 4, "sha256")
        .await
        .unwrap();
    let first = database
        .get_file(&project, &hash)
        .await
        .unwrap()
        .unwrap()
        .id;
    blobs
        .store(&project, ObjectName::new(&hash, first), b"png!")
        .await
        .unwrap();

    let collected = {
        let database = Arc::clone(&database);
        let blobs = FilesystemStorage::new(temp.path().join("files"));
        let (project, hash) = (project.clone(), hash.clone());
        Box::new(move || -> BoxFuture<'static, ()> {
            Box::pin(async move {
                database
                    .delete_trace_files(&project, &["trace-old".to_string()])
                    .await
                    .unwrap();
                database.sync_ref_count(&project, &hash).await.unwrap();
                let claim = database
                    .claim_file_for_deletion(&project, &hash)
                    .await
                    .unwrap()
                    .expect("collectable");
                blobs
                    .delete(&project, ObjectName::new(&hash, claim.incarnation))
                    .await
                    .unwrap();
                assert!(
                    database
                        .delete_file_if_unreferenced(&project, &hash, &claim)
                        .await
                        .unwrap()
                );
                // Another writer associates the same content again; its bytes are not stored yet.
                database
                    .associate_file(
                        "trace-other",
                        &project,
                        &hash,
                        Some("image/png"),
                        4,
                        "sha256",
                    )
                    .await
                    .unwrap();
            })
        }) as Interleaving
    };
    let storage: Arc<dyn FileStorage> = Arc::new(AfterFirstCheck {
        inner: FilesystemStorage::new(temp.path().join("files")),
        then: Mutex::new(Some(collected)),
    });
    let files = FileService::new(
        sideseat_core::config::FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
            quota_bytes: 64 * 1024 * 1024,
            filesystem_path: None,
            s3: None,
        },
        temp.path().join("files_temp"),
        storage,
        Arc::clone(&database),
        Arc::new(
            CacheService::new(&sideseat_core::config::CacheConfig {
                backend: sideseat_core::config::CacheBackendType::Memory,
                max_entries: 100,
                eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
                redis_url: None,
            })
            .await
            .unwrap(),
        ),
    )
    .await
    .unwrap();

    let uri = sideseat_core::utils::file_uri::build_file_uri(&hash, Some("image/png"));
    let (unbacked, failed, created) = reconcile_incoming_references(
        &[("default".to_string(), "trace-new".to_string(), uri)],
        &files,
        &[],
    )
    .await;

    assert!(unbacked.is_empty());
    assert_eq!(
        failed, 1,
        "the reference was committed on a check of an incarnation collected since"
    );
    assert_eq!(
        created,
        [("default".to_string(), "trace-new".to_string(), hash.clone())],
        "the association is the batch's to release"
    );
    assert_ne!(
        database
            .get_file(&project, &hash)
            .await
            .unwrap()
            .unwrap()
            .id,
        first
    );
}

/// Bytes finalized after their row was collected - the trace that associated them deleted meanwhile - are
/// deleted, not left under an incarnation no row will name again; bytes whose row is there are kept.
#[tokio::test]
async fn bytes_finalized_under_a_collected_row_are_retired() {
    let temp = tempfile::TempDir::new().unwrap();
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(
                &AppStorage::init_for_test(temp.path().to_path_buf()),
                Arc::new(TestClock),
            )
            .await
            .unwrap(),
        )));
    let files = FileService::new(
        sideseat_core::config::FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
            quota_bytes: 64 * 1024 * 1024,
            filesystem_path: None,
            s3: None,
        },
        temp.path().join("files_temp"),
        Arc::new(FilesystemStorage::new(temp.path().join("files"))),
        Arc::clone(&database),
        Arc::new(
            CacheService::new(&sideseat_core::config::CacheConfig {
                backend: sideseat_core::config::CacheBackendType::Memory,
                max_entries: 100,
                eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
                redis_url: None,
            })
            .await
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let project = ProjectId::from("default");
    let (collected, kept) = ("c".repeat(64), "d".repeat(64));
    let mut pending = Vec::new();
    for hash in [&collected, &kept] {
        database
            .associate_file("trace", &project, hash, Some("image/png"), 4, "sha256")
            .await
            .unwrap();
        let incarnation = database.get_file(&project, hash).await.unwrap().unwrap().id;
        let temp_path = temp.path().join(format!("{hash}.tmp"));
        std::fs::write(&temp_path, b"png!").unwrap();
        pending.push(PendingFinalization {
            project_id: project.clone(),
            hash: hash.clone(),
            temp_path,
            expected: ExpectedContent {
                len: 4,
                digest: *blake3::hash(b"png!").as_bytes(),
            },
            incarnation,
        });
    }
    let first = pending[0].incarnation;
    // The trace deleted and the file collected while the batch was still storing its bytes.
    database
        .delete_trace_files(&project, &["trace".to_string()])
        .await
        .unwrap();
    database.sync_ref_count(&project, &collected).await.unwrap();
    let claim = database
        .claim_file_for_deletion(&project, &collected)
        .await
        .unwrap()
        .expect("collectable");
    assert!(
        database
            .delete_file_if_unreferenced(&project, &collected, &claim)
            .await
            .unwrap()
    );
    let second = pending[1].incarnation;

    assert_eq!(finalize_pending_files(pending, &files, "test").await, 0);
    assert!(
        !files
            .storage()
            .exists(&project, ObjectName::new(&collected, first))
            .await
            .unwrap(),
        "bytes stranded under a collected incarnation"
    );
    assert!(
        files
            .storage()
            .exists(&project, ObjectName::new(&kept, second))
            .await
            .unwrap()
    );
}

/// Two executions writing the same media at once - two requests, a batch beside a redrive or a claim - each get a
/// temporary file of their own, holding all of what they decoded. They shared one, named by project and hash, so
/// one truncated and rewrote the file under the other, whose finalisation could store a partial copy under the
/// content's name for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_of_the_same_media_never_share_a_temporary_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&AppStorage::init_for_test(temp.path().to_path_buf()), clock)
                .await
                .unwrap(),
        )));
    let temp_dir = temp.path().join("files_temp");
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();
    let files = FileService::new(
        sideseat_core::config::FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
            quota_bytes: 64 * 1024 * 1024,
            filesystem_path: None,
            s3: None,
        },
        temp_dir,
        Arc::new(FilesystemStorage::new(temp.path().join("files"))),
        Arc::clone(&database),
        Arc::new(
            CacheService::new(&sideseat_core::config::CacheConfig {
                backend: sideseat_core::config::CacheBackendType::Memory,
                max_entries: 100,
                eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
                redis_url: None,
            })
            .await
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let decoded: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    let data = BASE64_STANDARD.encode(&decoded).into_bytes();
    let write = |trace: &str| PendingFileWrite {
        project_id: "default".to_string(),
        trace_id: trace.to_string(),
        hash: "d".repeat(64),
        media_type: Some("image/png".to_string()),
        size: decoded.len(),
        data: data.clone(),
        hash_algo: FILE_HASH_ALGORITHM.to_string(),
    };
    let (first, second) = (vec![write("trace-a")], vec![write("trace-b")]);
    let ((a, _, _), (b, _, _)) = tokio::join!(
        write_and_record_files(&first, &files),
        write_and_record_files(&second, &files),
    );
    let (a, b) = (&a[0], &b[0]);
    assert_ne!(
        a.temp_path, b.temp_path,
        "the two writes shared a temporary file"
    );
    for pending in [a, b] {
        assert_eq!(
            tokio::fs::read(&pending.temp_path).await.unwrap(),
            decoded,
            "a temporary file holds all of what its writer decoded"
        );
        assert_eq!(pending.expected.len, decoded.len() as u64);
    }
}
