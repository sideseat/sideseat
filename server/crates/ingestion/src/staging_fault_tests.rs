//! Faults at each staging boundary leave nothing acknowledged that is not durable, and nothing durable that
//! cannot be found.
//!
//! The four boundaries a staged payload crosses: its bytes are published to the blob store, its registry row is
//! committed, and on retirement the row is deleted and then the bytes. A fault at each must leave one of two
//! states - refused, with no trace left behind, or recoverable by redrive - and never a live row naming bytes
//! that are gone. Registry faults are raised by a trigger inside the real SQLite statement; blob faults by a
//! wrapper around the real filesystem store. What a power loss does to the same boundaries is the durability
//! check's subject (`make test-durability`) and `StagingRetirement.tla`'s.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::test_support::{
    StagingFault, clear_staging_fault, inject_staging_fault, staged_payload_count,
};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::config::RetentionConfig;
use sideseat_core::storage::AppStorage;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::{ProjectId, StagedSignal};

use super::{StagedPayloadRef, StagingService};

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0).single().expect("fixed")
    }
}

/// The real filesystem store, with a switch per operation that makes it fail.
struct FaultyStorage {
    inner: FilesystemStorage,
    fail_store: AtomicBool,
    fail_delete: AtomicBool,
}

fn injected() -> FileStorageError {
    FileStorageError::Backend("injected blob fault".to_string())
}

#[async_trait]
impl FileStorage for FaultyStorage {
    async fn store(
        &self,
        project_id: &ProjectId,
        hash: &str,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        if self.fail_store.load(Ordering::SeqCst) {
            return Err(injected());
        }
        self.inner.store(project_id, hash, data).await
    }

    async fn get(&self, project_id: &ProjectId, hash: &str) -> Result<Vec<u8>, FileStorageError> {
        self.inner.get(project_id, hash).await
    }

    async fn exists(&self, project_id: &ProjectId, hash: &str) -> Result<bool, FileStorageError> {
        self.inner.exists(project_id, hash).await
    }

    async fn delete(&self, project_id: &ProjectId, hash: &str) -> Result<(), FileStorageError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(injected());
        }
        self.inner.delete(project_id, hash).await
    }

    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        self.inner.delete_project(project_id).await
    }

    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        hash: &str,
        temp_path: &std::path::Path,
    ) -> Result<(), FileStorageError> {
        self.inner.finalize_temp(project_id, hash, temp_path).await
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    sqlite: Arc<SqliteService>,
    storage: Arc<FaultyStorage>,
    service: StagingService,
    project: ProjectId,
}

async fn fixture() -> Fixture {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .expect("duckdb dir");
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let sqlite = Arc::new(
        SqliteService::init(&app_storage, Arc::clone(&clock))
            .await
            .expect("sqlite"),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::clone(&sqlite)));
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(DuckdbRepository(Arc::new(
            DuckdbService::init(&app_storage, Arc::clone(&clock))
                .await
                .expect("duckdb"),
        )));
    let storage = Arc::new(FaultyStorage {
        inner: FilesystemStorage::new(temp.path().join("staging")),
        fail_store: AtomicBool::new(false),
        fail_delete: AtomicBool::new(false),
    });
    let user = database
        .create_user("faults@example.test", None)
        .await
        .expect("user");
    let org = database
        .create_organization_with_owner("Faults", "faults", &user.id)
        .await
        .expect("org");
    let project = database
        .create_project(&org.id, "Faults")
        .await
        .expect("project");
    let service = StagingService::new(
        Arc::clone(&storage) as Arc<dyn FileStorage>,
        database,
        analytics,
        clock,
        RetentionConfig::default(),
        2,
    );
    Fixture {
        _temp: temp,
        sqlite,
        storage,
        service,
        project: ProjectId::from(project.id.as_str()),
    }
}

async fn stage(fixture: &Fixture) -> Result<StagedPayloadRef, super::StagingError> {
    fixture
        .service
        .stage(
            fixture.project.as_str(),
            StagedSignal::Traces,
            b"an export",
            Vec::new(),
            "partition".to_string(),
        )
        .await
}

/// Every blob the store holds, by walking it: a fault must not leave bytes nothing will collect.
fn blobs(fixture: &Fixture) -> usize {
    fn walk(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() { walk(&path) } else { 1 }
                })
                .sum()
        })
    }
    walk(&fixture._temp.path().join("staging"))
}

async fn registered(fixture: &Fixture) -> i64 {
    staged_payload_count(&fixture.sqlite).await.expect("count")
}

/// Publication fails: the export is refused and nothing is registered.
#[tokio::test]
async fn a_failed_publication_refuses_and_registers_nothing() {
    let fixture = fixture().await;
    fixture.storage.fail_store.store(true, Ordering::SeqCst);

    assert!(
        stage(&fixture).await.is_err(),
        "an unpublished payload cannot be acknowledged"
    );
    assert_eq!(registered(&fixture).await, 0);
    assert_eq!(blobs(&fixture), 0);
}

/// The registry commit fails after publication: the export is refused and its bytes are taken back.
#[tokio::test]
async fn a_failed_registration_refuses_and_removes_the_bytes() {
    let fixture = fixture().await;
    inject_staging_fault(&fixture.sqlite, StagingFault::Register)
        .await
        .expect("fault");

    assert!(
        stage(&fixture).await.is_err(),
        "an unregistered payload cannot be acknowledged"
    );
    assert_eq!(registered(&fixture).await, 0);
    assert_eq!(
        blobs(&fixture),
        0,
        "bytes no row names would never be collected"
    );
}

/// Retirement's registry delete fails: the payload stays registered with its bytes, and a later retirement
/// finishes the job - redrive's path.
#[tokio::test]
async fn a_failed_retirement_keeps_the_payload_whole_for_redrive() {
    let fixture = fixture().await;
    let reference = stage(&fixture).await.expect("stage");
    let (payload, _) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");
    inject_staging_fault(&fixture.sqlite, StagingFault::Retire)
        .await
        .expect("fault");

    assert!(fixture.service.release(&payload).await.is_err());
    let (still, bytes) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("load")
        .expect("a failed retirement leaves the payload registered");
    assert_eq!(still.id, payload.id);
    assert_eq!(bytes, b"an export", "and its bytes intact");

    clear_staging_fault(&fixture.sqlite, StagingFault::Retire)
        .await
        .expect("clear");
    fixture
        .service
        .release(&payload)
        .await
        .expect("retire again");
    assert_eq!(registered(&fixture).await, 0);
    assert_eq!(blobs(&fixture), 0);
}

/// The blob delete fails after the registry delete: the retirement stands, and what is left is an unnamed
/// blob - reclaimable garbage, never a row naming missing bytes.
#[tokio::test]
async fn a_failed_physical_deletion_leaves_garbage_not_a_dangling_row() {
    let fixture = fixture().await;
    let reference = stage(&fixture).await.expect("stage");
    let (payload, _) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");
    fixture.storage.fail_delete.store(true, Ordering::SeqCst);

    fixture
        .service
        .release(&payload)
        .await
        .expect("the registry deletion is the terminal fact");
    assert_eq!(registered(&fixture).await, 0);
    assert!(
        fixture
            .service
            .load(&reference.id)
            .await
            .expect("load")
            .is_none()
    );
    assert_eq!(blobs(&fixture), 1, "the bytes remain, unnamed");
    assert_eq!(
        fixture
            .service
            .classify_missing(&reference)
            .await
            .expect("classify"),
        super::MissingReference::Finished,
        "and the reference reads as retired, not lost"
    );
}
