//! A trace's media is owned by the trace, and its logs take part in the release as its spans do.

use super::tests::{TestClock, create_file_service, setup_test};
use super::*;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::SpanStore;
use tempfile::TempDir;
use tokio::fs;

/// A trace's file is owned by the trace, and its logs take part in the release as its spans do: when span
/// retention runs first while a log still names the file, the file survives, and it is released when the log
/// expires too. Driven through the real retention pass, which records the intent before each delete and reconciles
/// the trace's survivors after it.
#[tokio::test]
async fn a_file_a_log_still_names_survives_span_retention_until_the_log_expires() {
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
    fs::create_dir_all(temp_dir.path().join("files"))
        .await
        .unwrap();
    fs::create_dir_all(temp_dir.path().join("files_temp"))
        .await
        .unwrap();
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
    tokio::fs::create_dir_all(analytics_dir.path().join("duckdb"))
        .await
        .unwrap();
    let analytics_storage = AppStorage::init_for_test(analytics_dir.path().to_path_buf());
    let duck = Arc::new(
        DuckdbService::init(&analytics_storage, std::sync::Arc::new(TestClock))
            .await
            .expect("duckdb"),
    );
    let project = ProjectId::from("default");
    let hash = "c".repeat(64);
    let uri = format!("#!B64!#image/png::{hash}");
    database
        .upsert_file(&project, &hash, Some("image/png"), 5, "sha256")
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
    database
        .insert_trace_file("trace1", &project, &hash)
        .await
        .unwrap();

    // The span is two days old, the log one hour: a one-day retention expires the span only.
    let now = TestClock.now();
    SpanStore::insert_spans(
        &DuckdbRepository(Arc::clone(&duck)),
        vec![sideseat_ports::types::NormalizedSpan {
            project_id: Some("default".to_string()),
            trace_id: "trace1".to_string(),
            span_id: "old-span".to_string(),
            span_name: "expired".to_string(),
            messages: Some(format!(r#"[{{"content":"see {uri}"}}]"#)),
            timestamp_start: now - chrono::TimeDelta::days(2),
            ..Default::default()
        }],
    )
    .await
    .expect("insert the span");
    sideseat_ports::traits::LogStore::insert_logs(
        &DuckdbRepository(Arc::clone(&duck)),
        &[sideseat_ports::types::NormalizedLog {
            project_id: Some("default".to_string()),
            log_digest: "d".repeat(64),
            trace_id: Some("trace1".to_string()),
            span_id: Some("another-span".to_string()),
            timestamp: now - chrono::TimeDelta::hours(1),
            raw_log: serde_json::json!({"body": {"bytesValue": uri.clone()}}),
            media: vec![uri.clone()],
            ..Default::default()
        }],
    )
    .await
    .expect("insert the log");

    let reconciler: Arc<dyn sideseat_ports::blobs::RetentionFileReconciler> = service.clone();
    let retain = |minutes: u64| sideseat_core::config::RetentionConfig {
        max_age_minutes: Some(minutes),
        ..Default::default()
    };
    duck.run_retention_to_completion(
        &retain(24 * 60),
        u64::MAX,
        Some(Arc::clone(&reconciler)),
        database.clone(),
        Arc::clone(&governance),
    )
    .await
    .expect("span retention");
    assert!(
        service.file_exists(&project, &hash).await.unwrap(),
        "span retention released a file a stored log still names"
    );

    duck.run_retention_to_completion(
        &retain(30),
        u64::MAX,
        Some(reconciler),
        database.clone(),
        governance,
    )
    .await
    .expect("log retention");
    assert!(
        !service.file_exists(&project, &hash).await.unwrap(),
        "the file outlived the last record that named it"
    );
}
