//! A survivor reconciliation that skips an association for a pending writer reports the trace deferred, and the
//! retention sweep keeps that trace's cleanup owed (`server/specs/SurvivorRelease.tla`): the writer can still
//! abort, which leaves the durable association referenced by nothing.

use super::tests::{TestClock, create_file_service, setup_test};
use super::*;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::SpanStore;
use tempfile::TempDir;
use tokio::fs;

struct Fixture {
    _dirs: (TempDir, TempDir),
    service: Arc<FileService>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    governance: Arc<dyn sideseat_ports::traits::StorageGovernance + Send + Sync>,
    duck: Arc<DuckdbService>,
    project: ProjectId,
    hash: String,
}

/// A trace whose only span, two days old, names a picture, associated durably as its committed batch left it.
async fn a_trace_with_a_durable_picture() -> Fixture {
    let (temp_dir, _, cache) = setup_test().await;
    let sqlite = SqliteRepository(Arc::new(
        SqliteService::init(
            &AppStorage::init_for_test(temp_dir.path().join("governed")),
            Arc::new(TestClock),
        )
        .await
        .unwrap(),
    ));
    let database: Arc<dyn TransactionalRepository + Send + Sync> = Arc::new(sqlite.clone());
    let governance: Arc<dyn sideseat_ports::traits::StorageGovernance + Send + Sync> =
        Arc::new(sqlite);
    for dir in ["files", "files_temp"] {
        fs::create_dir_all(temp_dir.path().join(dir)).await.unwrap();
    }
    let config = FilesConfig {
        enabled: true,
        storage: sideseat_core::config::StorageBackend::Filesystem,
        quota_bytes: 1024 * 1024,
        filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
        s3: None,
    };
    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = Arc::new(
        create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap(),
    );
    let analytics_dir = TempDir::new().unwrap();
    fs::create_dir_all(analytics_dir.path().join("duckdb"))
        .await
        .unwrap();
    let duck = Arc::new(
        DuckdbService::init(
            &AppStorage::init_for_test(analytics_dir.path().to_path_buf()),
            Arc::new(TestClock),
        )
        .await
        .expect("duckdb"),
    );

    let project = ProjectId::from("default");
    let hash = "e".repeat(64);
    database
        .associate_file("trace1", &project, &hash, Some("image/png"), 5, "sha256")
        .await
        .unwrap();
    let incarnation = database
        .get_file(&project, &hash)
        .await
        .unwrap()
        .unwrap()
        .id;
    service
        .storage
        .store(
            &project,
            sideseat_ports::blobs::ObjectName::new(&hash, incarnation),
            b"bytes",
        )
        .await
        .unwrap();
    SpanStore::insert_spans(
        &DuckdbRepository(Arc::clone(&duck)),
        vec![sideseat_ports::types::NormalizedSpan {
            project_id: Some("default".to_string()),
            trace_id: "trace1".to_string(),
            span_id: "old-span".to_string(),
            span_name: "expired".to_string(),
            messages: Some(format!(r#"[{{"content":"see #!B64!#image/png::{hash}"}}]"#)),
            timestamp_start: TestClock.now() - chrono::TimeDelta::days(2),
            ..Default::default()
        }],
    )
    .await
    .expect("insert the span");
    database
        .confirm_trace_file_associations(&[(
            "default".to_string(),
            "trace1".to_string(),
            hash.clone(),
        )])
        .await
        .unwrap();

    Fixture {
        _dirs: (temp_dir, analytics_dir),
        service,
        database,
        governance,
        duck,
        project,
        hash,
    }
}

/// A redelivery's batch associates the file again: one writer pending on the durable association.
async fn a_writer_associates(fixture: &Fixture) {
    fixture
        .database
        .associate_file(
            "trace1",
            &fixture.project,
            &fixture.hash,
            Some("image/png"),
            5,
            "sha256",
        )
        .await
        .unwrap();
}

/// Its span is dropped: the writer resolves, and the durable association stays.
async fn the_writer_aborts(fixture: &Fixture) {
    fixture
        .database
        .release_trace_file_association(&fixture.project, "trace1", &fixture.hash)
        .await
        .unwrap();
}

async fn associated(fixture: &Fixture) -> bool {
    !fixture
        .database
        .get_file_hashes_for_traces(&fixture.project, &["trace1".to_string()])
        .await
        .unwrap()
        .is_empty()
}

/// Skipped for the pending writer, the trace is reported deferred; once the writer has aborted, a reconciliation
/// releases the association and reports nothing.
#[tokio::test]
async fn an_association_skipped_for_a_pending_writer_is_reported_deferred() {
    let fixture = a_trace_with_a_durable_picture().await;
    sideseat_ports::traits::SpanStore::delete_spans(
        &DuckdbRepository(Arc::clone(&fixture.duck)),
        &fixture.project,
        &[("trace1".to_string(), "old-span".to_string())],
    )
    .await
    .unwrap();
    a_writer_associates(&fixture).await;

    let deferred = fixture
        .service
        .reconcile_trace_survivors(
            &fixture.project,
            &["trace1".to_string()],
            &DuckdbRepository(Arc::clone(&fixture.duck)),
        )
        .await
        .expect("reconcile");
    assert_eq!(deferred, vec!["trace1".to_string()], "the skip is reported");
    assert!(associated(&fixture).await, "kept for the writer");

    the_writer_aborts(&fixture).await;
    assert!(
        associated(&fixture).await,
        "the abort keeps the durable association, which nothing references now"
    );
    let deferred = fixture
        .service
        .reconcile_trace_survivors(
            &fixture.project,
            &["trace1".to_string()],
            &DuckdbRepository(Arc::clone(&fixture.duck)),
        )
        .await
        .expect("reconcile");
    assert!(deferred.is_empty(), "nothing is left owed");
    assert!(!associated(&fixture).await, "released on the retry");
}

/// The retention sweep completes a trace's cleanup only when its reconciliation released what it owed: a trace
/// deferred for a pending writer stays recorded, so the sweep comes back once the writer has aborted.
#[tokio::test]
async fn retention_keeps_a_deferred_traces_cleanup_recorded() {
    let fixture = a_trace_with_a_durable_picture().await;
    a_writer_associates(&fixture).await;
    let reconciler: Arc<dyn sideseat_ports::blobs::RetentionFileReconciler> =
        fixture.service.clone();
    fixture
        .duck
        .run_retention_to_completion(
            &sideseat_core::config::RetentionConfig {
                max_age_minutes: Some(24 * 60),
                ..Default::default()
            },
            u64::MAX,
            Some(Arc::clone(&reconciler)),
            fixture.database.clone(),
            Arc::clone(&fixture.governance),
        )
        .await
        .expect("span retention");
    assert!(associated(&fixture).await, "kept for the writer");
    the_writer_aborts(&fixture).await;

    let owed = fixture
        .database
        .claim_retention_cleanup(16, 0)
        .await
        .unwrap();
    assert_eq!(
        owed.iter()
            .map(|(project, trace, _)| (project.as_str(), trace.as_str()))
            .collect::<Vec<_>>(),
        vec![("default", "trace1")],
        "the deferred trace's cleanup is still owed"
    );
    let deferred = reconciler
        .reconcile_trace_survivors(
            &fixture.project,
            &["trace1".to_string()],
            &DuckdbRepository(Arc::clone(&fixture.duck)),
        )
        .await
        .expect("the sweep's retry");
    assert!(deferred.is_empty());
    assert!(!associated(&fixture).await, "released on the retry");
}
