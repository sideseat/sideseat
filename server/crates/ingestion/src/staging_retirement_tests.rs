//! A queue reference whose registry row is missing is classified, never acknowledged blindly.
//!
//! The protocol is modelled in `server/specs/StagingRetirement.tla`; these tests drive the real SQLite registry
//! through each of its cases. A power loss is simulated the way it would leave the store - the registration
//! row gone and the AUTOINCREMENT high-water mark rolled back with it - since SQLite cannot be made to lose a
//! commit on demand.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::config::{
    CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, RetentionConfig, StorageBackend,
};
use sideseat_core::storage::AppStorage;
use sideseat_domain::files::FileService;
use sideseat_ports::blobs::FileStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::{ProjectId, StagedSignal};

use super::{MissingReference, StagedPayloadRef, StagingService};

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0).single().expect("fixed")
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    sqlite: Arc<SqliteService>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    storage: Arc<dyn FileStorage>,
    /// Over the staging store, as in the server: staged blobs live in the project's file store.
    files: Arc<FileService>,
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
    let storage: Arc<dyn FileStorage> =
        Arc::new(FilesystemStorage::new(temp.path().join("staging")));
    let user = database
        .create_user("retirement@example.test", None)
        .await
        .expect("user");
    let org = database
        .create_organization_with_owner("Retirement", "retirement", &user.id)
        .await
        .expect("org");
    let project = database
        .create_project(&org.id, "Retirement")
        .await
        .expect("project");
    let temp_files = app_storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp);
    tokio::fs::create_dir_all(&temp_files)
        .await
        .expect("files temp dir");
    let files = Arc::new(
        FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 0,
                filesystem_path: Some(temp.path().join("staging").display().to_string()),
                s3: None,
            },
            temp_files,
            Arc::clone(&storage),
            Arc::clone(&database),
            Arc::new(
                CacheService::new(&CacheConfig {
                    backend: CacheBackendType::Memory,
                    max_entries: 16,
                    eviction_policy: EvictionPolicy::TinyLfu,
                    redis_url: None,
                })
                .await
                .expect("memory cache"),
            ),
        )
        .await
        .expect("file service"),
    );
    let service = StagingService::new(
        Arc::clone(&storage),
        Arc::clone(&database),
        Arc::clone(&analytics),
        clock,
        RetentionConfig::default(),
        2,
    );
    Fixture {
        _temp: temp,
        sqlite,
        database,
        analytics,
        storage,
        files,
        service,
        project: ProjectId::from(project.id.as_str()),
    }
}

async fn stage(fixture: &Fixture, body: &[u8]) -> StagedPayloadRef {
    fixture
        .service
        .stage(
            fixture.project.as_str(),
            StagedSignal::Traces,
            body,
            chrono::Utc::now(),
            Vec::new(),
            "partition".to_string(),
        )
        .await
        .expect("stage")
}

async fn lose_registration(fixture: &Fixture, reference: &StagedPayloadRef) {
    sideseat_adapter_sqlite::test_support::lose_staged_registration(
        &fixture.sqlite,
        &reference.id,
        reference.seq,
    )
    .await
    .expect("lose the registration");
}

async fn anomalies(fixture: &Fixture) -> Vec<(String, i64, i64)> {
    sideseat_adapter_sqlite::test_support::staging_anomalies(&fixture.sqlite)
        .await
        .expect("anomalies")
}

#[tokio::test]
async fn registrations_take_increasing_sequences() {
    let fixture = fixture().await;
    let first = stage(&fixture, b"first").await;
    let second = stage(&fixture, b"second").await;
    assert!(first.seq >= 1);
    assert_eq!(second.seq, first.seq + 1);
}

/// Retired by redrive or another consumer, or deleted with its project: finished, not a loss.
#[tokio::test]
async fn a_retired_payload_is_finished() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"retired").await;
    let (payload, _) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");
    fixture.service.release(&payload).await.expect("release");

    assert!(
        fixture
            .service
            .load(&reference.id)
            .await
            .expect("load")
            .is_none()
    );
    assert_eq!(
        fixture
            .service
            .classify_missing(&reference)
            .await
            .expect("classify"),
        MissingReference::Finished
    );
}

/// A project's deletion takes its staged payloads with it, registry rows before their blobs: a reference to one is
/// then finished, neither a lost registration nor a row whose blob is gone, which every delivery of its reference
/// failed to load until redrive happened to release it.
#[tokio::test]
async fn a_deleted_projects_payloads_go_with_it_and_their_references_finish() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"deleted with its project").await;
    let (payload, _) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");

    assert!(
        sideseat_domain::cleanup::cleanup_project(
            &fixture.database,
            &fixture.analytics,
            &fixture.files,
            &fixture.project,
        )
        .await
        .expect("cleanup"),
        "the project is deleted"
    );

    assert!(
        fixture
            .service
            .load(&reference.id)
            .await
            .expect("the payload's row outlived its blob")
            .is_none(),
        "the payload's row outlived its project"
    );
    assert!(
        !fixture
            .storage
            .exists(&fixture.project, &payload.blob_hash)
            .await
            .expect("exists"),
        "the payload's blob outlived its project"
    );
    assert_eq!(
        fixture
            .service
            .classify_missing(&reference)
            .await
            .expect("classify"),
        MissingReference::Finished
    );
    assert!(anomalies(&fixture).await.is_empty());
}

/// The registry rows go before the blobs: a deletion whose row delete fails keeps the payload whole - row and
/// blob, so a delivery still loads it - and the project fenced, and the sweep's retry finishes it.
#[tokio::test]
async fn a_failed_row_delete_keeps_the_blob_for_the_retry() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"deleted on the retry").await;
    sideseat_adapter_sqlite::test_support::inject_staging_fault(
        &fixture.sqlite,
        sideseat_adapter_sqlite::test_support::StagingFault::Retire,
    )
    .await
    .expect("inject");
    assert!(
        sideseat_domain::cleanup::cleanup_project(
            &fixture.database,
            &fixture.analytics,
            &fixture.files,
            &fixture.project,
        )
        .await
        .is_err(),
        "a deletion that could not remove the rows reports itself unfinished"
    );
    let (payload, _) = fixture
        .service
        .load(&reference.id)
        .await
        .expect("the row kept its blob")
        .expect("the row stays for the retry");

    sideseat_adapter_sqlite::test_support::clear_staging_fault(
        &fixture.sqlite,
        sideseat_adapter_sqlite::test_support::StagingFault::Retire,
    )
    .await
    .expect("clear");
    sideseat_domain::cleanup::finish_project_deletion(
        &fixture.database,
        &fixture.analytics,
        &fixture.files,
        &fixture.project,
    )
    .await
    .expect("the retry finishes");
    assert!(
        fixture
            .service
            .load(&reference.id)
            .await
            .expect("load")
            .is_none()
    );
    assert!(
        !fixture
            .storage
            .exists(&fixture.project, &payload.blob_hash)
            .await
            .expect("exists")
    );
}

/// A request that stored its blob before the fence can register after the deletion removed the rows - after the
/// project row itself is gone. The sweep of the deleted project's residual removes that payload too, row before
/// blob, so its reference finishes instead of failing to load.
#[tokio::test]
async fn a_payload_registered_after_the_project_went_is_collected() {
    let fixture = fixture().await;
    assert!(
        sideseat_domain::cleanup::cleanup_project(
            &fixture.database,
            &fixture.analytics,
            &fixture.files,
            &fixture.project,
        )
        .await
        .expect("cleanup")
    );
    // The repeated clean sweeps that remove the project row, at once.
    assert!(
        fixture
            .database
            .record_project_sweep(fixture.project.as_str(), true, 1, 0)
            .await
            .expect("sweep"),
        "the project row is removed"
    );
    let late = stage(&fixture, b"registered late").await;
    let (payload, _) = fixture
        .service
        .load(&late.id)
        .await
        .expect("load")
        .expect("staged");

    sideseat_domain::cleanup::advance_pending_deletions(
        &fixture.database,
        &fixture.analytics,
        &fixture.files,
        0,
    )
    .await
    .expect("sweep the residual");
    assert!(
        fixture
            .service
            .load(&late.id)
            .await
            .expect("the late row outlived its blob")
            .is_none(),
        "the late row outlived its project"
    );
    assert!(
        !fixture
            .storage
            .exists(&fixture.project, &payload.blob_hash)
            .await
            .expect("exists")
    );
    assert_eq!(
        fixture
            .service
            .classify_missing(&late)
            .await
            .expect("classify"),
        MissingReference::Finished
    );
}

/// The residual's sweep keeps the same order: a row it cannot remove keeps its blob, so its reference still loads
/// until a later check removes both.
#[tokio::test]
async fn a_residual_sweep_that_cannot_remove_a_row_keeps_its_blob() {
    let fixture = fixture().await;
    assert!(
        sideseat_domain::cleanup::cleanup_project(
            &fixture.database,
            &fixture.analytics,
            &fixture.files,
            &fixture.project,
        )
        .await
        .expect("cleanup")
    );
    assert!(
        fixture
            .database
            .record_project_sweep(fixture.project.as_str(), true, 1, 0)
            .await
            .expect("sweep")
    );
    let late = stage(&fixture, b"registered late, kept whole").await;
    sideseat_adapter_sqlite::test_support::inject_staging_fault(
        &fixture.sqlite,
        sideseat_adapter_sqlite::test_support::StagingFault::Retire,
    )
    .await
    .expect("inject");
    sideseat_domain::cleanup::advance_pending_deletions(
        &fixture.database,
        &fixture.analytics,
        &fixture.files,
        0,
    )
    .await
    .expect("sweep the residual");
    assert!(
        fixture
            .service
            .load(&late.id)
            .await
            .expect("the row kept its blob")
            .is_some(),
        "the row stays for a later check"
    );
}

/// The registration rolled back and nothing has reused its sequence: above the high-water mark.
#[tokio::test]
async fn a_lost_registration_is_above_the_mark() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"lost").await;
    lose_registration(&fixture, &reference).await;

    assert_eq!(
        fixture
            .service
            .classify_missing(&reference)
            .await
            .expect("classify"),
        MissingReference::Lost
    );
}

/// The registration rolled back and a later one took its sequence: held by a different payload.
#[tokio::test]
async fn a_lost_registration_is_found_when_its_sequence_is_reused() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"lost").await;
    lose_registration(&fixture, &reference).await;
    let successor = stage(&fixture, b"successor").await;
    assert_eq!(
        successor.seq, reference.seq,
        "the rolled-back mark hands the value out again"
    );

    assert_eq!(
        fixture
            .service
            .classify_missing(&reference)
            .await
            .expect("classify"),
        MissingReference::Lost
    );
    assert_eq!(
        fixture
            .service
            .classify_missing(&successor)
            .await
            .expect("classify"),
        MissingReference::Finished,
        "the successor itself is not missing, and its own sequence is its own"
    );
}

/// A lost registration is recorded durably and counted, once per reference.
#[tokio::test]
async fn a_lost_registration_is_recorded_and_counted() {
    let fixture = fixture().await;
    let reference = stage(&fixture, b"lost").await;
    lose_registration(&fixture, &reference).await;

    fixture
        .service
        .record_lost(&reference)
        .await
        .expect("record");
    fixture
        .service
        .record_lost(&reference)
        .await
        .expect("record again");

    assert_eq!(
        anomalies(&fixture).await,
        vec![(reference.id.clone(), reference.seq, 2)]
    );
}

/// What staging costs an export on the ack path and on the settle path, with the real filesystem blob store and
/// SQLite registry: the fixed cost group commit is to share. Run by hand:
///
/// ```text
/// cargo test --locked -p sideseat-ingestion --lib bench_staging_fixed_cost -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement of synced writes; see the doc comment"]
async fn bench_staging_fixed_cost() {
    const EXPORTS: usize = 200;
    // The corpus' median export.
    let body = vec![7u8; 7_528];
    let fixture = Arc::new(fixture().await);
    for concurrency in [1usize, 16] {
        let started = std::time::Instant::now();
        let mut references = Vec::with_capacity(EXPORTS);
        for chunk in (0..EXPORTS).collect::<Vec<_>>().chunks(concurrency) {
            let staged =
                futures::future::join_all(chunk.iter().map(|_| stage(&fixture, &body))).await;
            references.extend(staged);
        }
        let staging = started.elapsed();
        let started = std::time::Instant::now();
        for chunk in references.chunks(concurrency) {
            futures::future::join_all(chunk.iter().map(|reference| {
                let fixture = Arc::clone(&fixture);
                let id = reference.id.clone();
                async move {
                    let (payload, _) = fixture
                        .service
                        .load(&id)
                        .await
                        .expect("load")
                        .expect("staged");
                    fixture.service.release(&payload).await.expect("release");
                }
            }))
            .await;
        }
        let releasing = started.elapsed();
        println!(
            "[staging] {concurrency:>2} at a time: stage {:.2} ms/export, load + release {:.2} ms/export",
            staging.as_secs_f64() * 1000.0 / EXPORTS as f64,
            releasing.as_secs_f64() * 1000.0 / EXPORTS as f64
        );
    }
    // Its parts, one at a time: the blob write alone, then the registry row alone.
    let started = std::time::Instant::now();
    for index in 0..EXPORTS {
        fixture
            .service
            .storage
            .store(&fixture.project, &format!("{index:064x}"), &body)
            .await
            .expect("blob");
    }
    let blobs = started.elapsed();
    let started = std::time::Instant::now();
    for index in 0..EXPORTS {
        let payload = sideseat_ports::types::StagedPayload {
            id: format!("bench-{index}"),
            project_id: fixture.project.clone(),
            signal: StagedSignal::Traces,
            blob_hash: format!("{index:064x}"),
            byte_len: body.len() as u64,
            created_at: Utc::now(),
            redrive_attempts: 0,
            unconfirmed: false,
            records: Vec::new(),
        };
        fixture
            .service
            .database
            .create_staged_payload(&payload)
            .await
            .expect("row");
    }
    let rows = started.elapsed();
    println!(
        "[staging] parts: blob {:.2} ms/export, registry row {:.2} ms/export",
        blobs.as_secs_f64() * 1000.0 / EXPORTS as f64,
        rows.as_secs_f64() * 1000.0 / EXPORTS as f64
    );
}
