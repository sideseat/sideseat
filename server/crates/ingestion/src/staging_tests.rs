use super::*;
use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::types::NormalizedMetric;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0).single().unwrap()
    }
}

#[tokio::test]
async fn confirmed_retries_release_and_cap_exhaustion_holds_bytes() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .unwrap();
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let sqlite = Arc::new(
        SqliteService::init(&app_storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(sqlite));
    let duck = Arc::new(
        DuckdbService::init(&app_storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(DuckdbRepository(duck));
    let storage: Arc<dyn FileStorage> =
        Arc::new(FilesystemStorage::new(temp.path().join("staging")));
    let service = StagingService::new(
        Arc::clone(&storage),
        Arc::clone(&database),
        Arc::clone(&analytics),
        Arc::clone(&clock),
        RetentionConfig::default(),
        2,
    );

    let user = database
        .create_user("staging@example.test", None)
        .await
        .unwrap();
    let org = database
        .create_organization_with_owner("Staging", "staging", &user.id)
        .await
        .unwrap();
    let project = database.create_project(&org.id, "Staging").await.unwrap();
    let project_id = ProjectId::from(project.id.as_str());
    let metric = NormalizedMetric {
        project_id: Some(project.id.clone()),
        datapoint_id: "same-identity".to_string(),
        content_digest: "new-content".to_string(),
        timestamp: clock.now(),
        ingested_at: Some(clock.now()),
        ..Default::default()
    };
    analytics
        .insert_metrics(std::slice::from_ref(&metric))
        .await
        .unwrap();

    for _ in 0..2 {
        let reference = service
            .stage(
                &project.id,
                StagedSignal::Metrics,
                b"byte-identical-export",
                clock.now(),
                vec![StagedRecord::Metric {
                    datapoint_id: metric.datapoint_id.clone(),
                    content_digest: metric.content_digest.clone(),
                    timestamp: metric.timestamp,
                }],
                "series".to_string(),
            )
            .await
            .unwrap();
        let (payload, _) = service.load(&reference.id).await.unwrap().unwrap();
        assert_eq!(
            service.settle(&payload).await.unwrap(),
            StagingDisposition::Confirmed
        );
        assert!(
            database
                .get_staged_payload(&reference.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !storage
                .exists(&project_id, &payload.blob_hash)
                .await
                .unwrap()
        );
    }

    let missing = service
        .stage(
            &project.id,
            StagedSignal::Metrics,
            b"missing-export",
            clock.now(),
            vec![StagedRecord::Metric {
                datapoint_id: "missing".to_string(),
                content_digest: "never-written".to_string(),
                timestamp: clock.now(),
            }],
            String::new(),
        )
        .await
        .unwrap();
    assert!(!service.note_failed_attempt(&missing.id).await.unwrap());
    assert!(service.note_failed_attempt(&missing.id).await.unwrap());
    let held = database
        .get_staged_payload(&missing.id)
        .await
        .unwrap()
        .unwrap();
    assert!(held.unconfirmed);
    assert_eq!(held.redrive_attempts, 2);
    assert!(storage.exists(&project_id, &held.blob_hash).await.unwrap());
    assert!(
        database
            .pending_staged_payloads(10)
            .await
            .unwrap()
            .is_empty()
    );
}
