use super::*;
use chrono::TimeZone;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::traits::StorageGovernance;
use sideseat_ports::types::{MetricType, NormalizedLog, NormalizedMetric, NormalizedSpan};
use tokio::sync::Barrier;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
            .single()
            .unwrap()
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    service: Arc<StorageGovernanceService>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    project_id: ProjectId,
}

async fn harness(ordinary_bytes: u64) -> Harness {
    let temp = tempfile::TempDir::new().unwrap();
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let sqlite = Arc::new(
        SqliteService::init(&storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let repository = Arc::new(SqliteRepository(sqlite));
    let database: Arc<dyn TransactionalRepository + Send + Sync> = repository.clone();
    let governance: Arc<dyn StorageGovernance + Send + Sync> = repository.clone();
    let duck = Arc::new(
        DuckdbService::init(&storage, Arc::clone(&clock))
            .await
            .unwrap(),
    );
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(DuckdbRepository(duck));

    let user = database
        .create_user("quota@example.test", None)
        .await
        .unwrap();
    let org = database
        .create_organization_with_owner("Quota", "quota", &user.id)
        .await
        .unwrap();
    let project = database.create_project(&org.id, "Quota").await.unwrap();
    let project_id = ProjectId::from(project.id.as_str());
    let service = Arc::new(StorageGovernanceService::new(
        database,
        governance,
        Arc::clone(&analytics),
        clock,
        REQUIRED_MAINTENANCE_RESERVE + ordinary_bytes,
    ));
    Harness {
        _temp: temp,
        service,
        analytics,
        project_id,
    }
}

fn metric(project_id: &ProjectId, id: &str, bytes: u64) -> NormalizedMetric {
    NormalizedMetric {
        project_id: Some(project_id.to_string()),
        datapoint_id: id.to_string(),
        metric_name: id.to_string(),
        metric_type: MetricType::Gauge,
        timestamp: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
        ingested_at: Some(
            Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
                .single()
                .unwrap(),
        ),
        logical_bytes: bytes,
        ..Default::default()
    }
}

fn span(project_id: &ProjectId, id: &str, bytes: u64) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: format!("trace-{id}"),
        span_id: id.to_string(),
        content_digest: format!("digest-{id}"),
        span_name: id.to_string(),
        timestamp_start: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
        ingested_at: Some(TestClock.now()),
        logical_bytes: bytes,
        ..Default::default()
    }
}

#[tokio::test]
async fn quota_reclaims_oldest_data_before_admitting_and_refuses_without_silent_loss() {
    let reclaim_harness = harness(1_050).await;
    reclaim_harness
        .analytics
        .insert_spans(vec![span(
            &reclaim_harness.project_id,
            "reclaimable",
            1_000,
        )])
        .await
        .unwrap();
    reclaim_harness
        .service
        .reconcile_project(&reclaim_harness.project_id)
        .await
        .unwrap();

    let admitted = reclaim_harness
        .service
        .admit(&reclaim_harness.project_id, 80)
        .await;
    assert!(
        admitted.is_ok(),
        "reclaimable bytes should be removed first"
    );
    assert_eq!(
        reclaim_harness
            .analytics
            .project_logical_bytes(&reclaim_harness.project_id)
            .await
            .unwrap(),
        0
    );
    assert!(
        reclaim_harness
            .service
            .database
            .deletion_is_journaled(
                &reclaim_harness.project_id,
                DeletionScope::Span,
                "trace-reclaimable",
                Some("reclaimable"),
            )
            .await
            .unwrap(),
        "pressure reclamation must be replayable after restore"
    );
    assert!(
        reclaim_harness
            .service
            .governance
            .held_transactional_bytes(&reclaim_harness.project_id)
            .await
            .unwrap()
            > 0,
        "journal and cleanup intent remain quota-counted"
    );

    let held_harness = harness(1_050).await;
    let mut held = span(&held_harness.project_id, "held", 1_000);
    held.hold_until = Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap());
    held_harness
        .analytics
        .insert_spans(vec![held])
        .await
        .unwrap();
    held_harness
        .service
        .reconcile_project(&held_harness.project_id)
        .await
        .unwrap();
    let held_bytes_before = held_harness
        .analytics
        .project_logical_bytes(&held_harness.project_id)
        .await
        .unwrap();

    let refused = held_harness
        .service
        .admit(&held_harness.project_id, 80)
        .await;
    assert!(matches!(
        refused,
        Err(GovernanceError::QuotaExceeded { .. })
    ));
    assert_eq!(
        held_harness
            .analytics
            .project_held_logical_bytes(&held_harness.project_id, TestClock.now())
            .await
            .unwrap(),
        held_bytes_before,
        "refusal must leave held data intact"
    );
    assert_eq!(
        held_harness
            .service
            .reconcile_project(&held_harness.project_id)
            .await
            .unwrap()
            .logical_bytes,
        held_bytes_before,
        "reported usage converges to the stored logical-byte sum"
    );
}

#[tokio::test]
async fn restore_quota_repair_reclaims_to_a_fixed_point_and_reports_holds() {
    let reclaim = harness(1_050).await;
    reclaim
        .analytics
        .insert_spans(vec![
            span(&reclaim.project_id, "oldest", 800),
            span(&reclaim.project_id, "newest", 800),
        ])
        .await
        .unwrap();
    let report = reclaim
        .service
        .repair_quota_after_restore(&reclaim.project_id)
        .await
        .unwrap();
    assert_eq!(report.remaining_overage_bytes, 0);
    assert!(report.reclaimed_span_bytes >= 800);
    assert!(!report.blocked_by_project_hold);
    assert!(report.usage_after_bytes <= report.ordinary_limit_bytes);

    let held = harness(100).await;
    held.analytics
        .insert_spans(vec![span(&held.project_id, "held", 1_000)])
        .await
        .unwrap();
    held.service
        .set_hold(
            &held.project_id,
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap(),
        )
        .await
        .unwrap();
    let report = held
        .service
        .repair_quota_after_restore(&held.project_id)
        .await
        .unwrap();
    assert!(report.blocked_by_project_hold);
    assert!(report.remaining_overage_bytes > 0);
    assert_eq!(report.reclaimed_span_bytes, 0);
    assert_eq!(
        held.analytics
            .project_logical_bytes(&held.project_id)
            .await
            .unwrap(),
        1_000
    );
}

#[tokio::test]
async fn exhausted_maintenance_reserve_refuses_before_pressure_deletion() {
    let harness = harness(1_050).await;
    let quota = harness.service.quota_bytes();
    harness
        .analytics
        .insert_spans(vec![span(&harness.project_id, "must-survive", quota)])
        .await
        .unwrap();
    harness
        .service
        .reconcile_project(&harness.project_id)
        .await
        .unwrap();
    let stored_before = harness
        .analytics
        .project_logical_bytes(&harness.project_id)
        .await
        .unwrap();

    let refused = harness.service.admit(&harness.project_id, 1).await;
    assert!(matches!(
        refused,
        Err(GovernanceError::MaintenanceReserveExhausted { .. })
    ));
    assert_eq!(
        harness
            .analytics
            .project_logical_bytes(&harness.project_id)
            .await
            .unwrap(),
        stored_before,
        "journal exhaustion must fail before deleting the analytical row"
    );
    assert!(
        !harness
            .service
            .database
            .deletion_is_journaled(
                &harness.project_id,
                DeletionScope::Span,
                "trace-must-survive",
                Some("must-survive"),
            )
            .await
            .unwrap(),
        "a refused pressure deletion must not claim that it happened"
    );
}

#[tokio::test]
async fn writers_admitted_before_hold_are_patched_after_commit_for_every_signal() {
    let harness = harness(1_000).await;
    let mut span = span(&harness.project_id, "in-flight", 123);
    let mut metric = metric(&harness.project_id, "in-flight", 124);
    let mut log = NormalizedLog {
        project_id: Some(harness.project_id.to_string()),
        log_digest: "in-flight-log".to_string(),
        ordinal: 0,
        timestamp: TestClock.now(),
        ingested_at: Some(TestClock.now()),
        logical_bytes: 125,
        ..Default::default()
    };
    harness
        .service
        .stamp_spans(std::slice::from_mut(&mut span))
        .await
        .unwrap();
    harness
        .service
        .stamp_metrics(std::slice::from_mut(&mut metric))
        .await
        .unwrap();
    harness
        .service
        .stamp_logs(std::slice::from_mut(&mut log))
        .await
        .unwrap();
    assert!(span.hold_until.is_none());
    assert!(metric.hold_until.is_none());
    assert!(log.hold_until.is_none());

    let ready = Arc::new(Barrier::new(2));
    let proceed = Arc::new(Barrier::new(2));
    let analytics = Arc::clone(&harness.analytics);
    let service = Arc::clone(&harness.service);
    let project_id = harness.project_id.clone();
    let writer_ready = Arc::clone(&ready);
    let writer_proceed = Arc::clone(&proceed);
    let writer = tokio::spawn(async move {
        writer_ready.wait().await;
        writer_proceed.wait().await;
        analytics.insert_spans(vec![span]).await.unwrap();
        analytics.insert_metrics(&[metric]).await.unwrap();
        analytics.insert_logs(&[log]).await.unwrap();
        service.patch_after_write(&[project_id]).await.unwrap();
    });

    ready.wait().await;
    harness
        .service
        .set_hold(
            &harness.project_id,
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap(),
        )
        .await
        .unwrap();
    proceed.wait().await;
    writer.await.unwrap();

    let total = harness
        .analytics
        .project_logical_bytes(&harness.project_id)
        .await
        .unwrap();
    assert!(
        total >= 123 + 124 + 125,
        "source signals remain part of the held project"
    );
    assert_eq!(
        harness
            .analytics
            .project_held_logical_bytes(&harness.project_id, TestClock.now())
            .await
            .unwrap(),
        total,
        "the post-write fence must patch every source signal"
    );
}

#[tokio::test]
async fn hold_waits_for_an_already_running_retention_lease() {
    let harness = harness(1_000).await;
    harness
        .analytics
        .insert_metrics(&[metric(&harness.project_id, "survivor", 50)])
        .await
        .unwrap();
    let owner = harness
        .service
        .acquire_maintenance(&harness.project_id, "retention-test")
        .await
        .unwrap();

    let service = Arc::clone(&harness.service);
    let project_id = harness.project_id.clone();
    let setter = tokio::spawn(async move {
        service
            .set_hold(
                &project_id,
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap(),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(!setter.is_finished(), "hold bypassed the retention fence");
    harness
        .service
        .release_maintenance(&harness.project_id, &owner)
        .await;
    setter.await.unwrap().unwrap();

    assert_eq!(
        harness
            .analytics
            .project_held_logical_bytes(&harness.project_id, TestClock.now())
            .await
            .unwrap(),
        50
    );
}

#[tokio::test]
async fn strict_confirmation_survives_hold_patch_and_byte_identical_retry_for_all_signals() {
    let harness = harness(10_000).await;
    let now = TestClock.now();
    let span = NormalizedSpan {
        project_id: Some(harness.project_id.to_string()),
        trace_id: "trace".to_string(),
        span_id: "span".to_string(),
        content_digest: "span-content".to_string(),
        span_name: "test".to_string(),
        timestamp_start: now,
        logical_bytes: 101,
        ..Default::default()
    };
    let metric_row = metric(&harness.project_id, "metric", 102);
    let metric_digest = metric_row.content_digest.clone();
    let metric_instant = metric_row.timestamp;
    let log = NormalizedLog {
        project_id: Some(harness.project_id.to_string()),
        log_digest: "log-content".to_string(),
        ordinal: 0,
        timestamp: now,
        ingested_at: Some(now),
        logical_bytes: 103,
        ..Default::default()
    };
    harness
        .analytics
        .insert_spans(vec![span.clone()])
        .await
        .unwrap();
    harness
        .analytics
        .insert_metrics(std::slice::from_ref(&metric_row))
        .await
        .unwrap();
    harness
        .analytics
        .insert_logs(std::slice::from_ref(&log))
        .await
        .unwrap();

    harness
        .service
        .set_hold(
            &harness.project_id,
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).single().unwrap(),
        )
        .await
        .unwrap();

    let mut retry_span = span;
    let mut retry_metric = metric_row;
    let mut retry_log = log;
    harness
        .service
        .stamp_spans(std::slice::from_mut(&mut retry_span))
        .await
        .unwrap();
    harness
        .service
        .stamp_metrics(std::slice::from_mut(&mut retry_metric))
        .await
        .unwrap();
    harness
        .service
        .stamp_logs(std::slice::from_mut(&mut retry_log))
        .await
        .unwrap();
    harness
        .analytics
        .insert_spans(vec![retry_span])
        .await
        .unwrap();
    harness
        .analytics
        .insert_metrics(&[retry_metric])
        .await
        .unwrap();
    harness.analytics.insert_logs(&[retry_log]).await.unwrap();

    assert!(
        harness
            .analytics
            .spans_match_content(
                &harness.project_id,
                &[(
                    "trace".to_string(),
                    "span".to_string(),
                    "span-content".to_string(),
                )],
            )
            .await
            .unwrap()
    );
    assert_eq!(
        harness
            .analytics
            .metric_winners(
                &harness.project_id,
                &[("metric".to_string(), metric_instant)],
            )
            .await
            .unwrap()
            .get("metric")
            .map(|winner| winner.content_digest.clone()),
        Some(metric_digest)
    );
    assert!(
        harness
            .analytics
            .logs_match_content(
                &harness.project_id,
                &[("log-content".to_string(), 0, Some(now))],
            )
            .await
            .unwrap()
    );
}
