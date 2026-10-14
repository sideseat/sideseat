use super::*;
use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS;
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, StorageGovernance};
use tempfile::TempDir;
use tokio::fs;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }
}

/// The incarnation of the first file row a fresh store creates, which each of these tests has one of: its
/// bytes are stored under it (`ObjectName`). `the_first_row_is_incarnation_one` holds the assumption.
const FIRST: i64 = 1;

fn at(hash: &str) -> ObjectName<'_> {
    ObjectName::new(hash, FIRST)
}

#[tokio::test]
async fn the_first_row_is_incarnation_one() {
    let (_temp_dir, database, _storage) = setup_test().await;
    database
        .upsert_file(
            &ProjectId::from("project1"),
            &test_hash(),
            None,
            1,
            "sha256",
        )
        .await
        .unwrap();
    let row = database
        .get_file(&ProjectId::from("project1"), &test_hash())
        .await
        .unwrap()
        .expect("the row");
    assert_eq!(row.id, FIRST);
}

fn test_hash() -> String {
    "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string()
}

async fn setup_test() -> (
    TempDir,
    Arc<dyn TransactionalRepository + Send + Sync>,
    Arc<dyn FileStorage>,
) {
    let temp_dir = TempDir::new().unwrap();

    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let sqlite_service = SqliteService::init(&app_storage, Arc::new(TestClock))
        .await
        .unwrap();
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(sqlite_service)));

    // Create filesystem storage
    let storage_path = temp_dir.path().join("files");
    fs::create_dir_all(&storage_path).await.unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(FilesystemStorage::new(storage_path));

    (temp_dir, database, storage)
}

#[test]
fn test_parse_temp_filename_valid() {
    let hash = test_hash();
    let filename = format!("my-project_{}", hash);

    let result = parse_temp_filename(&filename);
    assert!(result.is_some());

    let (project_id, parsed_hash) = result.unwrap();
    assert_eq!(project_id, "my-project");
    assert_eq!(parsed_hash, hash);
}

#[test]
fn test_parse_temp_filename_project_with_underscore() {
    let hash = test_hash();
    let filename = format!("my_project_{}", hash);

    let result = parse_temp_filename(&filename);
    assert!(result.is_some());

    let (project_id, parsed_hash) = result.unwrap();
    assert_eq!(project_id, "my_project");
    assert_eq!(parsed_hash, hash);
}

#[test]
fn test_parse_temp_filename_invalid_too_short() {
    let result = parse_temp_filename("short_abc");
    assert!(result.is_none());
}

#[test]
fn test_parse_temp_filename_invalid_hash() {
    // Hash with invalid chars
    let filename = "project_g1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    let result = parse_temp_filename(filename);
    assert!(result.is_none());
}

#[test]
fn test_parse_temp_filename_no_separator() {
    let hash = test_hash();
    let result = parse_temp_filename(&hash);
    assert!(result.is_none());
}

#[tokio::test]
async fn test_cleanup_empty_dir() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.total_processed(), 0);
}

#[tokio::test]
async fn test_cleanup_nonexistent_dir() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("nonexistent");

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.total_processed(), 0);
}

#[tokio::test]
async fn test_cleanup_orphan_temp_no_metadata() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();

    // Create temp file without database record
    let hash = test_hash();
    let filename = format!("project1_{}", hash);
    let file_path = temp_path.join(&filename);
    fs::write(&file_path, b"test content").await.unwrap();

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.orphaned_deleted, 1);
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_cleanup_temp_with_metadata_finalize() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();

    let hash = test_hash();
    let filename = format!("project1_{}", hash);
    let file_path = temp_path.join(&filename);
    fs::write(&file_path, b"test content").await.unwrap();

    // Create database record via repository trait
    let repo = database.as_ref();
    repo.upsert_file(
        &ProjectId::from("project1"),
        &hash,
        Some("text/plain"),
        12,
        "sha256",
    )
    .await
    .unwrap();

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.finalized, 1);
    assert!(!file_path.exists());

    // Verify file was moved to permanent storage
    let data = storage
        .get(&ProjectId::from("project1"), at(&hash))
        .await
        .unwrap();
    assert_eq!(data, b"test content");
}

#[tokio::test]
async fn test_cleanup_temp_already_in_storage() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();

    let hash = test_hash();
    let filename = format!("project1_{}", hash);
    let file_path = temp_path.join(&filename);
    fs::write(&file_path, b"test content").await.unwrap();

    // Create database record and store in permanent storage
    let repo = database.as_ref();
    repo.upsert_file(
        &ProjectId::from("project1"),
        &hash,
        Some("text/plain"),
        12,
        "sha256",
    )
    .await
    .unwrap();
    storage
        .store(&ProjectId::from("project1"), at(&hash), b"test content")
        .await
        .unwrap();

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.already_exists, 1);
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_cleanup_invalid_filename() {
    let (temp_dir, database, storage) = setup_test().await;
    let temp_path = temp_dir.path().join("temp");
    fs::create_dir_all(&temp_path).await.unwrap();

    // Create temp file with invalid format
    let file_path = temp_path.join("invalid_filename.txt");
    fs::write(&file_path, b"test content").await.unwrap();

    let stats = cleanup_orphan_temp_files(&temp_path, &storage, &database)
        .await
        .unwrap();

    assert_eq!(stats.invalid_deleted, 1);
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_cleanup_zero_ref_files() {
    let (temp_dir, database, storage) = setup_test().await;

    let hash = test_hash();

    // Store file
    storage
        .store(&ProjectId::from("project1"), at(&hash), b"test content")
        .await
        .unwrap();

    // Create database record with ref_count=0 via repository trait
    let repo = database.as_ref();
    repo.upsert_file(
        &ProjectId::from("project1"),
        &hash,
        Some("text/plain"),
        12,
        "sha256",
    )
    .await
    .unwrap();

    // Decrement to 0
    repo.decrement_ref_count(&ProjectId::from("project1"), &hash)
        .await
        .unwrap();

    // Run cleanup
    let deleted = cleanup_zero_ref_files(&storage, &database, FILE_DELETION_CLAIM_STALE_SECS)
        .await
        .unwrap();

    assert_eq!(deleted, 1);

    // File should be gone
    assert!(
        !storage
            .exists(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap()
    );

    // Cleanup temp_dir to avoid unused warning
    drop(temp_dir);
}

/// While this process's lease is lost the sweep deletes nothing - neither an unreferenced file nor one an
/// abandoned claim names, the step the media race resumes - and once the lease is held again it collects both.
#[tokio::test]
async fn a_closed_fence_stops_the_sweep_and_an_open_one_resumes_it() {
    let (_temp_dir, database, storage) = setup_test().await;
    let project = ProjectId::from("project1");
    let unreferenced = test_hash();
    let abandoned = "c".repeat(64);
    let mut names = Vec::new();
    for hash in [&unreferenced, &abandoned] {
        database
            .upsert_file(&project, hash, Some("text/plain"), 7, "sha256")
            .await
            .unwrap();
        database.decrement_ref_count(&project, hash).await.unwrap();
        let incarnation = database.get_file(&project, hash).await.unwrap().unwrap().id;
        storage
            .store(&project, ObjectName::new(hash, incarnation), b"content")
            .await
            .unwrap();
        names.push((hash.clone(), incarnation));
    }
    database
        .claim_file_for_deletion(&project, &abandoned)
        .await
        .unwrap()
        .expect("claimed, then abandoned");
    // Every claim is abandoned at this age.
    let stale_secs = -100_000;
    let (held, receiver) = tokio::sync::watch::channel(None);
    let fence = CollectionFence::following(receiver);

    let deleted = cleanup_zero_ref_files_governed(&storage, &database, stale_secs, None, &fence)
        .await
        .unwrap();
    assert_eq!(deleted, 0);
    for (hash, incarnation) in &names {
        let name = ObjectName::new(hash, *incarnation);
        assert!(storage.exists(&project, name).await.unwrap(), "{hash}");
        assert!(
            database.get_file(&project, hash).await.unwrap().is_some(),
            "{hash}"
        );
    }

    held.send(Some(
        sideseat_core::utils::collection_fence::HeldUntil::new(
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            0,
        ),
    ))
    .unwrap();
    let deleted = cleanup_zero_ref_files_governed(&storage, &database, stale_secs, None, &fence)
        .await
        .unwrap();
    assert_eq!(deleted, 2);
    for (hash, incarnation) in &names {
        let name = ObjectName::new(hash, *incarnation);
        assert!(!storage.exists(&project, name).await.unwrap(), "{hash}");
    }
}

#[tokio::test]
async fn project_hold_exempts_orphan_files_from_gc_until_the_hold_expires() {
    let temp_dir = TempDir::new().unwrap();
    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let sqlite = Arc::new(
        SqliteService::init(&app_storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let repository = Arc::new(SqliteRepository(sqlite));
    let database: Arc<dyn TransactionalRepository + Send + Sync> = repository.clone();
    let governance_port: Arc<dyn StorageGovernance + Send + Sync> = repository;
    let duck = Arc::new(
        DuckdbService::init(&app_storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(DuckdbRepository(duck));
    let governance = Arc::new(crate::storage_governance::StorageGovernanceService::new(
        Arc::clone(&database),
        Arc::clone(&governance_port),
        analytics,
        Arc::clone(&clock),
        u64::MAX,
    ));
    let storage_path = temp_dir.path().join("held-files");
    fs::create_dir_all(&storage_path).await.unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(FilesystemStorage::new(storage_path));
    let project_id = ProjectId::from("project1");
    let hash = test_hash();
    storage
        .store(&project_id, at(&hash), b"held content")
        .await
        .unwrap();
    database
        .upsert_file(&project_id, &hash, Some("text/plain"), 12, "sha256")
        .await
        .unwrap();
    database
        .decrement_ref_count(&project_id, &hash)
        .await
        .unwrap();
    assert_eq!(
        governance
            .reconcile_project(&project_id)
            .await
            .unwrap()
            .logical_bytes,
        12
    );
    governance_port
        .set_project_hold(
            &project_id,
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap(),
            clock.now(),
        )
        .await
        .unwrap();

    assert_eq!(
        cleanup_zero_ref_files_governed(
            &storage,
            &database,
            FILE_DELETION_CLAIM_STALE_SECS,
            Some(&governance),
            &CollectionFence::open(),
        )
        .await
        .unwrap(),
        0
    );
    assert!(storage.exists(&project_id, at(&hash)).await.unwrap());

    governance_port
        .clear_project_hold(&project_id)
        .await
        .unwrap();
    assert_eq!(
        cleanup_zero_ref_files_governed(
            &storage,
            &database,
            FILE_DELETION_CLAIM_STALE_SECS,
            Some(&governance),
            &CollectionFence::open(),
        )
        .await
        .unwrap(),
        1
    );
    assert!(!storage.exists(&project_id, at(&hash)).await.unwrap());
    assert_eq!(
        governance_port
            .project_storage_usage(&project_id)
            .await
            .unwrap()
            .logical_bytes,
        0,
        "governed file GC reconciles quota before releasing its project fence"
    );
}

/// A deletion abandoned mid-way is finished by the next sweep, not stranded.
///
/// This is the crash the fence makes possible: the claim is durable, so nothing releases it, and the
/// ordinary zero-reference path cannot pick the file up because claiming it fails. Without resumption
/// the bytes stay on disk forever and every ingestion naming that file keeps failing its batch on a
/// claim nobody holds.
#[tokio::test]
async fn a_deletion_abandoned_by_a_crash_is_finished_by_the_next_sweep() {
    let (temp_dir, database, storage) = setup_test().await;
    let hash = test_hash();
    storage
        .store(&ProjectId::from("project1"), at(&hash), b"test content")
        .await
        .unwrap();
    let repo = database.as_ref();
    repo.upsert_file(
        &ProjectId::from("project1"),
        &hash,
        Some("text/plain"),
        12,
        "sha256",
    )
    .await
    .unwrap();
    repo.decrement_ref_count(&ProjectId::from("project1"), &hash)
        .await
        .unwrap();

    // The crash: claimed, then nothing further.
    assert!(
        repo.claim_file_for_deletion(&ProjectId::from("project1"), &hash)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        cleanup_zero_ref_files(&storage, &database, FILE_DELETION_CLAIM_STALE_SECS)
            .await
            .unwrap(),
        0,
        "a fresh claim is a deletion in progress; the sweep must leave it alone"
    );
    assert!(
        storage
            .exists(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap(),
        "and must not touch its bytes"
    );

    // Treated as abandoned, the same sweep finishes it.
    assert_eq!(
        cleanup_zero_ref_files(&storage, &database, 0)
            .await
            .unwrap(),
        1,
        "an abandoned claim is resumed"
    );
    assert!(
        !storage
            .exists(&ProjectId::from("project1"), at(&hash))
            .await
            .unwrap()
    );
    assert!(
        repo.get_stale_claimed_files(0).await.unwrap().is_empty(),
        "and nothing is left claimed"
    );

    drop(temp_dir);
}

/// A delete given up at the lease's deadline may still land, so its claim stays: released, it would let an
/// ingest find the bytes and count on them as they went.
#[tokio::test]
async fn an_abandoned_delete_keeps_its_claim() {
    struct HangingDeletes(Arc<dyn FileStorage>);

    #[async_trait::async_trait]
    impl FileStorage for HangingDeletes {
        async fn store(
            &self,
            project_id: &ProjectId,
            object: ObjectName<'_>,
            data: &[u8],
        ) -> Result<(), FileStorageError> {
            self.0.store(project_id, object, data).await
        }
        async fn get(
            &self,
            project_id: &ProjectId,
            object: ObjectName<'_>,
        ) -> Result<Vec<u8>, FileStorageError> {
            self.0.get(project_id, object).await
        }
        async fn exists(
            &self,
            project_id: &ProjectId,
            object: ObjectName<'_>,
        ) -> Result<bool, FileStorageError> {
            self.0.exists(project_id, object).await
        }
        async fn delete(&self, _: &ProjectId, _: ObjectName<'_>) -> Result<(), FileStorageError> {
            std::future::pending().await
        }
        async fn incarnations(
            &self,
            project_id: &ProjectId,
            hash: &str,
        ) -> Result<Vec<i64>, FileStorageError> {
            self.0.incarnations(project_id, hash).await
        }
        async fn scan_objects(
            &self,
            visitor: &mut dyn sideseat_ports::blobs::ObjectVisitor,
        ) -> Result<(), FileStorageError> {
            self.0.scan_objects(visitor).await
        }
        async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
            self.0.delete_project(project_id).await
        }
        async fn finalize_temp(
            &self,
            project_id: &ProjectId,
            object: ObjectName<'_>,
            temp_path: &Path,
        ) -> Result<(), FileStorageError> {
            self.0.finalize_temp(project_id, object, temp_path).await
        }
    }

    let (_temp_dir, database, inner) = setup_test().await;
    let project = ProjectId::from("project1");
    let hash = test_hash();
    database
        .upsert_file(&project, &hash, Some("text/plain"), 7, "sha256")
        .await
        .unwrap();
    database.decrement_ref_count(&project, &hash).await.unwrap();
    inner.store(&project, at(&hash), b"content").await.unwrap();
    let (_held, receiver) = tokio::sync::watch::channel(Some(
        sideseat_core::utils::collection_fence::HeldUntil::new(
            tokio::time::Instant::now() + std::time::Duration::from_millis(100),
            0,
        ),
    ));
    let fence = CollectionFence::following(receiver);
    let storage: Arc<dyn FileStorage> = Arc::new(crate::files::fenced_storage::FencedStorage::new(
        Arc::new(HangingDeletes(inner)),
        fence.clone(),
    ));

    let deleted = cleanup_zero_ref_files_governed(
        &storage,
        &database,
        FILE_DELETION_CLAIM_STALE_SECS,
        None,
        &fence,
    )
    .await
    .unwrap();

    assert_eq!(deleted, 0);
    assert!(
        database
            .claim_file_for_deletion(&project, &hash)
            .await
            .unwrap()
            .is_none(),
        "the abandoned delete's claim was released"
    );
}

/// Bytes an ingest stores between a collection's delete of them and its delete of their row - its association
/// outrun by the collection - are deleted once more after the row, not left under an incarnation nothing names.
#[tokio::test]
async fn bytes_stored_between_the_byte_and_row_deletes_are_collected() {
    struct FinalizedAfterFirstDelete {
        inner: Arc<dyn FileStorage>,
        deletes: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl FileStorage for FinalizedAfterFirstDelete {
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
            self.inner.delete(project_id, object).await?;
            if self
                .deletes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                // The late ingest's finalization, landing right after the first delete.
                self.inner.store(project_id, object, b"late").await?;
            }
            Ok(())
        }
        async fn incarnations(
            &self,
            project_id: &ProjectId,
            hash: &str,
        ) -> Result<Vec<i64>, FileStorageError> {
            self.inner.incarnations(project_id, hash).await
        }
        async fn scan_objects(
            &self,
            visitor: &mut dyn sideseat_ports::blobs::ObjectVisitor,
        ) -> Result<(), FileStorageError> {
            self.inner.scan_objects(visitor).await
        }
        async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
            self.inner.delete_project(project_id).await
        }
        async fn finalize_temp(
            &self,
            project_id: &ProjectId,
            object: ObjectName<'_>,
            temp_path: &Path,
        ) -> Result<(), FileStorageError> {
            self.inner
                .finalize_temp(project_id, object, temp_path)
                .await
        }
    }

    let (_temp_dir, database, inner) = setup_test().await;
    let project = ProjectId::from("project1");
    let hash = test_hash();
    database
        .upsert_file(&project, &hash, Some("text/plain"), 7, "sha256")
        .await
        .unwrap();
    database.decrement_ref_count(&project, &hash).await.unwrap();
    inner.store(&project, at(&hash), b"content").await.unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(FinalizedAfterFirstDelete {
        inner: Arc::clone(&inner),
        deletes: std::sync::atomic::AtomicUsize::new(0),
    });

    let deleted = cleanup_zero_ref_files(&storage, &database, FILE_DELETION_CLAIM_STALE_SECS)
        .await
        .unwrap();

    assert_eq!(deleted, 1);
    assert!(database.get_file(&project, &hash).await.unwrap().is_none());
    assert!(
        !inner.exists(&project, at(&hash)).await.unwrap(),
        "bytes stored between the deletes were left with no row"
    );
}
