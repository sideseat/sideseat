//! The consumer acknowledges a reference whose registry row is missing only when it can say why.
//!
//! It used to acknowledge every such reference, so an export whose registration was lost after the 200 was
//! dropped without a trace. A retired reference is still acknowledged quietly; a lost one is acknowledged only
//! after its anomaly row is durable; and if that row cannot be written the reference stays queued.

use sideseat_ports::types::StagedSignal;

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;

/// A second service over the test's SQLite file, for what the ports do not expose.
async fn sqlite(temp: &tempfile::TempDir) -> sideseat_adapter_sqlite::SqliteService {
    let storage = sideseat_core::storage::AppStorage::init_for_test(temp.path().to_path_buf());
    sideseat_adapter_sqlite::SqliteService::init(&storage, Arc::new(FixedClock))
        .await
        .expect("sqlite")
}

#[derive(Debug)]
struct FixedClock;

impl sideseat_ports::clock::Clock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
    }
}

async fn anomaly_ids(service: &sideseat_adapter_sqlite::SqliteService) -> Vec<String> {
    sideseat_adapter_sqlite::test_support::staging_anomalies(service)
        .await
        .expect("anomalies")
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

async fn staged(pipeline: &TracePipeline, body: &[u8]) -> StagedPayloadRef {
    pipeline
        .staging
        .stage(
            "default",
            StagedSignal::Traces,
            body,
            chrono::Utc::now(),
            Vec::new(),
            "p".to_string(),
        )
        .await
        .expect("stage")
}

#[tokio::test]
async fn a_retired_reference_is_acknowledged_without_an_anomaly() {
    let (temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let reference = staged(&pipeline, b"retired").await;
    let (payload, _) = pipeline
        .staging
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged");
    pipeline.staging.release(&payload).await.expect("release");

    assert!(pipeline.process_staged_reference(&reference).await);
    assert!(anomaly_ids(&sqlite(&temp).await).await.is_empty());
}

#[tokio::test]
async fn a_lost_reference_is_recorded_before_it_is_acknowledged() {
    let (temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    // A reference no registration ever committed: its sequence is above the mark.
    let lost = StagedPayloadRef {
        id: "lost-after-acknowledgement".to_string(),
        partition_key: "p".to_string(),
        seq: 1_000,
    };

    assert!(pipeline.process_staged_reference(&lost).await);
    assert_eq!(
        anomaly_ids(&sqlite(&temp).await).await,
        vec![lost.id.clone()]
    );
}

#[tokio::test]
async fn a_lost_reference_that_cannot_be_recorded_stays_queued() {
    let (temp, _analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    sideseat_adapter_sqlite::test_support::break_staging_anomalies(&sqlite(&temp).await)
        .await
        .expect("make the anomaly write fail");
    let lost = StagedPayloadRef {
        id: "lost-after-acknowledgement".to_string(),
        partition_key: "p".to_string(),
        seq: 1_000,
    };

    assert!(
        !pipeline.process_staged_reference(&lost).await,
        "an unrecorded loss must not be acknowledged"
    );
}
