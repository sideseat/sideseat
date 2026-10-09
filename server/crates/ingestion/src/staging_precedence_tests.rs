//! A correction of a datapoint wins by when it was received, not by when it was written.
//!
//! A metrics export is written inside its request and settled after; a requester retrying an earlier export
//! after its settlement failed, or redrive holding a copy it loaded, writes it after a correction received later
//! was stored. Stored at the write's own instant, the late copy replaced the correction, and the correction's own
//! settlement then failed and wrote it back, for as long as either was retried. Stored at its receipt, the late
//! copy loses to the correction and settles as superseded.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::config::RetentionConfig;
use sideseat_core::storage::AppStorage;
use sideseat_ports::blobs::FileStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::{ProjectId, StagedPayload, StagedRecord, StagedSignal};

use super::{StagingDisposition, StagingService};

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0).single().expect("fixed")
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
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
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&app_storage, Arc::clone(&clock))
                .await
                .expect("sqlite"),
        )));
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(DuckdbRepository(Arc::new(
            DuckdbService::init(&app_storage, Arc::clone(&clock))
                .await
                .expect("duckdb"),
        )));
    let storage: Arc<dyn FileStorage> =
        Arc::new(FilesystemStorage::new(temp.path().join("staging")));
    let user = database
        .create_user("precedence@example.test", None)
        .await
        .expect("user");
    let org = database
        .create_organization_with_owner("Precedence", "precedence", &user.id)
        .await
        .expect("org");
    let project = database
        .create_project(&org.id, "Precedence")
        .await
        .expect("project");
    let service = StagingService::new(
        storage,
        Arc::clone(&database),
        Arc::clone(&analytics),
        clock,
        RetentionConfig::default(),
        2,
    );
    Fixture {
        _temp: temp,
        analytics,
        database,
        service,
        project: ProjectId::from(project.id.as_str()),
    }
}

/// One gauge datapoint of the same series at the same instant, measuring `value`: two of them are revisions of
/// one datapoint.
fn export(project: &ProjectId, value: f64) -> ExportMetricsServiceRequest {
    let mut request = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource::default()),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "queue.depth".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: 1_704_067_000_000_000_000,
                            value: Some(number_data_point::Value::AsDouble(value)),
                            ..Default::default()
                        }],
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    crate::otlp::inject_project_id_metrics(&mut request, project.as_str());
    request
}

/// Stage `request` as received at `received_at`, returning its registration.
async fn staged(
    fixture: &Fixture,
    request: &ExportMetricsServiceRequest,
    received_at: DateTime<Utc>,
) -> StagedPayload {
    let records = crate::metrics::extract_metrics_batch(request)
        .into_iter()
        .map(|metric| StagedRecord::Metric {
            datapoint_id: metric.datapoint_id,
            content_digest: metric.content_digest,
            timestamp: metric.timestamp,
        })
        .collect();
    let reference = fixture
        .service
        .stage(
            fixture.project.as_str(),
            StagedSignal::Metrics,
            &request.encode_to_vec(),
            received_at,
            records,
            "series".to_string(),
        )
        .await
        .expect("stage");
    fixture
        .service
        .registration(&reference.id)
        .await
        .expect("registration")
        .expect("staged")
}

async fn write(
    fixture: &Fixture,
    request: &ExportMetricsServiceRequest,
    received_at: DateTime<Utc>,
) {
    let stored = crate::metrics::ingest(
        request,
        fixture.analytics.as_ref(),
        fixture.database.as_ref(),
        received_at,
    )
    .await
    .expect("ingest");
    assert_eq!(stored.stored, 1);
}

/// The earlier export is written after the correction received after it: the correction stays the stored value,
/// and the late copy settles as superseded instead of staying pending to be written again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_copy_of_a_datapoint_loses_to_a_later_correction_and_settles() {
    let fixture = fixture().await;
    let first = export(&fixture.project, 1.0);
    let correction = export(&fixture.project, 2.0);
    let received_at = Utc::now();
    let first_staged = staged(&fixture, &first, received_at).await;
    let correction_at = received_at + TimeDelta::seconds(1);
    let correction_staged = staged(&fixture, &correction, correction_at).await;

    write(&fixture, &correction, correction_at).await;
    assert_eq!(
        fixture
            .service
            .settle(&correction_staged)
            .await
            .expect("settle"),
        StagingDisposition::Confirmed
    );

    write(&fixture, &first, received_at).await;
    assert_eq!(
        fixture.service.settle(&first_staged).await.expect("settle"),
        StagingDisposition::Confirmed,
        "the late copy settles as superseded"
    );

    let datapoint = crate::metrics::extract_metrics_batch(&correction)
        .remove(0)
        .datapoint_id;
    let stored = fixture
        .analytics
        .get_metric(&fixture.project, &datapoint)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(
        stored.value_double,
        Some(2.0),
        "the correction received later is the stored value"
    );
    assert!(
        fixture
            .service
            .pending(10)
            .await
            .expect("pending")
            .is_empty(),
        "an export stayed pending, to be written again"
    );
}

/// A datapoint keeps only its winning revision, so an export never written whose datapoint a correction received
/// after it superseded settles on that correction alone - writing it would store nothing - while one received
/// after the stored revision is owed its write and stays pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unwritten_export_settles_only_on_a_correction_received_after_it() {
    let fixture = fixture().await;
    let first = export(&fixture.project, 1.0);
    let correction = export(&fixture.project, 2.0);
    let last = export(&fixture.project, 3.0);
    let received_at = Utc::now();
    let first_staged = staged(&fixture, &first, received_at).await;
    let correction_at = received_at + TimeDelta::seconds(1);
    let correction_staged = staged(&fixture, &correction, correction_at).await;
    let last_staged = staged(&fixture, &last, correction_at + TimeDelta::seconds(1)).await;

    write(&fixture, &correction, correction_at).await;
    assert_eq!(
        fixture
            .service
            .settle(&correction_staged)
            .await
            .expect("settle"),
        StagingDisposition::Confirmed
    );
    assert_eq!(
        fixture.service.settle(&first_staged).await.expect("settle"),
        StagingDisposition::Confirmed,
        "an export a later correction superseded settles unwritten"
    );
    assert_eq!(
        fixture.service.settle(&last_staged).await.expect("settle"),
        StagingDisposition::Pending,
        "an export received after the stored correction settled unwritten, and its value was lost"
    );
}

/// An export whose content an earlier export stored is not settled on that copy: a correction received in
/// between and written later would take the datapoint over. Written, it moves the datapoint to its own receipt,
/// so the correction loses when it is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resent_value_is_settled_only_once_stored_at_its_own_receipt() {
    let fixture = fixture().await;
    let first = export(&fixture.project, 1.0);
    let correction = export(&fixture.project, 2.0);
    let resent = export(&fixture.project, 1.0);
    let received_at = Utc::now();
    let correction_at = received_at + TimeDelta::seconds(1);
    let resent_at = correction_at + TimeDelta::seconds(1);
    staged(&fixture, &first, received_at).await;
    let correction_staged = staged(&fixture, &correction, correction_at).await;
    let resent_staged = staged(&fixture, &resent, resent_at).await;

    write(&fixture, &first, received_at).await;
    assert_eq!(
        fixture
            .service
            .settle(&resent_staged)
            .await
            .expect("settle"),
        StagingDisposition::Pending,
        "settled on the first export's copy, the value received last is lost to the correction"
    );

    write(&fixture, &resent, resent_at).await;
    assert_eq!(
        fixture
            .service
            .settle(&resent_staged)
            .await
            .expect("settle"),
        StagingDisposition::Confirmed
    );
    write(&fixture, &correction, correction_at).await;
    assert_eq!(
        fixture
            .service
            .settle(&correction_staged)
            .await
            .expect("settle"),
        StagingDisposition::Confirmed,
        "the correction is superseded by the value received after it"
    );
    let datapoint = crate::metrics::extract_metrics_batch(&resent)
        .remove(0)
        .datapoint_id;
    let stored = fixture
        .analytics
        .get_metric(&fixture.project, &datapoint)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(
        stored.value_double,
        Some(1.0),
        "the value received last is the stored value"
    );
}

/// Two exports received in the same microsecond have no order: whichever is written last is the datapoint, and
/// the other settles as superseded rather than staying pending to be written again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_received_in_one_microsecond_settle_whichever_is_stored() {
    let fixture = fixture().await;
    let first = export(&fixture.project, 1.0);
    let other = export(&fixture.project, 2.0);
    let received_at = Utc::now();
    let first_staged = staged(&fixture, &first, received_at).await;
    let other_staged = staged(&fixture, &other, received_at).await;

    write(&fixture, &first, received_at).await;
    write(&fixture, &other, received_at).await;
    for (staged, name) in [
        (&other_staged, "the stored one"),
        (&first_staged, "the other"),
    ] {
        assert_eq!(
            fixture.service.settle(staged).await.expect("settle"),
            StagingDisposition::Confirmed,
            "{name} settles"
        );
    }
}
