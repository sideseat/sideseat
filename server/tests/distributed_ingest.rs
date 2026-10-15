//! Ingest end to end over the distributed analytics backend: exports staged, written to ClickHouse, settled and
//! answered through the path a transport takes.
//!
//! ClickHouse drops, in a merge soon after it writes them, rows older than its tables' TTL by their own timestamp. An export of such
//! records was written, never found stored, retried to its redrive cap, quarantined and answered 503 for ever. It
//! is now answered at once, its old records rejected by name of the horizon - unless the project holds a legal hold,
//! under which ClickHouse keeps them.
//!
//! Needs a live ClickHouse, as `clickhouse_parity` does, and skips without `SIDESEAT_TEST_CLICKHOUSE_URL`:
//! `make test-clickhouse` runs it.

#[path = "support/stack.rs"]
mod stack;

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_core::config::ClickhouseConfig;
use sideseat_core::utils::collection_fence::CollectionFence;
use sideseat_domain::raw_payload::RawContent;
use sideseat_ingestion::received::ReceivedPayload;
use sideseat_ingestion::signals::export_signal;
use sideseat_ingestion::staging::StagingDisposition;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::{ProjectId, StagedRecord, StagedSignal};

const URL_ENV: &str = "SIDESEAT_TEST_CLICKHOUSE_URL";
const PROJECT: &str = "distributed";

#[derive(Debug)]
struct Now;

impl Clock for Now {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    stores: stack::Stores,
    /// The test database, read past the tenant row policy.
    clickhouse: clickhouse::Client,
}

/// The stack over a fresh ClickHouse database named `database`, or `None` with no ClickHouse to run against.
async fn harness(database: &str) -> Option<Harness> {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("skipped: set {URL_ENV} (make test-clickhouse)");
        return None;
    };
    let user = std::env::var("SIDESEAT_TEST_CLICKHOUSE_USER").ok();
    let password = std::env::var("SIDESEAT_TEST_CLICKHOUSE_PASSWORD").ok();
    let mut bootstrap = clickhouse::Client::default().with_url(&url);
    if let Some(user) = &user {
        bootstrap = bootstrap.with_user(user);
    }
    if let Some(password) = &password {
        bootstrap = bootstrap.with_password(password);
    }
    for statement in [
        format!("DROP DATABASE IF EXISTS {database}"),
        format!("CREATE DATABASE {database}"),
    ] {
        bootstrap
            .query(&statement)
            .execute()
            .await
            .expect("test database");
    }
    let config = ClickhouseConfig {
        url,
        database: database.to_string(),
        user,
        password,
        timeout_secs: 30,
        compression: false,
        async_insert: false,
        wait_for_async_insert: true,
        cluster: None,
        distributed: false,
        insert_quorum: 0,
    };
    let dir = tempfile::TempDir::new().expect("temp dir");
    let stores = stack::stores(
        dir.path(),
        Arc::new(Now),
        CollectionFence::open(),
        Some(&config),
    )
    .await;
    stores.create_project(PROJECT, PROJECT, Utc::now()).await;
    let clickhouse = bootstrap
        .with_database(database)
        .with_option("SQL_sideseat_maintenance", "1");
    Some(Harness {
        _dir: dir,
        stores,
        clickhouse,
    })
}

fn nanos(at: DateTime<Utc>) -> u64 {
    u64::try_from(at.timestamp_nanos_opt().expect("nanos")).expect("after 1970")
}

/// An export of one trace whose spans start at `starts`.
fn trace_export(trace: u8, starts: &[DateTime<Utc>]) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: starts
                    .iter()
                    .enumerate()
                    .map(|(n, start)| Span {
                        trace_id: vec![trace; 16],
                        span_id: vec![u8::try_from(n + 1).expect("few spans"); 8],
                        name: format!("step {n}"),
                        start_time_unix_nano: nanos(*start),
                        end_time_unix_nano: nanos(*start + TimeDelta::seconds(1)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn received(request: &impl Message) -> ReceivedPayload {
    ReceivedPayload::new(request.encode_to_vec(), RawContent::Protobuf, Utc::now())
}

fn long_ago() -> DateTime<Utc> {
    Utc::now() - TimeDelta::days(200)
}

fn recently() -> DateTime<Utc> {
    Utc::now() - TimeDelta::hours(1)
}

/// Spans older than the horizon are answered at once as rejected, by name of the horizon, and nothing is left
/// staged; the recent ones of the same export are stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spans_older_than_the_horizon_are_rejected_not_retried_for_ever() {
    let Some(harness) = harness("distributed_ingest_spans").await else {
        return;
    };
    let stores = &harness.stores;

    let old = trace_export(0x11, &[long_ago(), long_ago()]);
    let body = received(&old);
    let answer = export_signal(&stores.traces, old, stores.context(PROJECT, &body))
        .await
        .expect("answered, not refused");
    let partial = answer.partial_success.expect("the old spans are rejected");
    assert_eq!(partial.rejected_spans, 2);
    assert!(
        partial.error_message.contains("90-day retention horizon"),
        "{}",
        partial.error_message
    );

    let mixed = trace_export(0x12, &[long_ago(), recently()]);
    let body = received(&mixed);
    let answer = export_signal(&stores.traces, mixed, stores.context(PROJECT, &body))
        .await
        .expect("answered");
    assert_eq!(
        answer.partial_success.expect("one rejected").rejected_spans,
        1
    );
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty(),
        "an export was left staged, to be retried to its cap"
    );
    // ClickHouse applies the TTL in a merge soon after the insert: forced, the old span is gone.
    harness
        .clickhouse
        .query("OPTIMIZE TABLE otel_spans FINAL")
        .execute()
        .await
        .expect("merge");
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::from(stores.analytics.repository());
    let spans = analytics
        .get_spans_for_trace(&ProjectId::from(PROJECT), &"12".repeat(16), 10)
        .await
        .expect("spans");
    assert_eq!(spans.len(), 1, "the recent span is stored");
}

/// Under a legal hold ClickHouse keeps old rows, so the export is not rejected: it is stored and settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_project_keeps_spans_older_than_the_horizon() {
    let Some(harness) = harness("distributed_ingest_held").await else {
        return;
    };
    let stores = &harness.stores;
    stores
        .governance
        .set_hold(&ProjectId::from(PROJECT), Utc::now() + TimeDelta::days(30))
        .await
        .expect("hold");

    let old = trace_export(0x21, &[long_ago()]);
    let body = received(&old);
    let answer = export_signal(&stores.traces, old, stores.context(PROJECT, &body))
        .await
        .expect("stored under the hold");
    assert!(answer.partial_success.is_none(), "{answer:?}");
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty()
    );
}

/// Data points and log records follow the same rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_and_logs_older_than_the_horizon_are_rejected() {
    let Some(harness) = harness("distributed_ingest_signals").await else {
        return;
    };
    let stores = &harness.stores;
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "queue.depth".to_string(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: nanos(long_ago()),
                            value: Some(number_data_point::Value::AsInt(1)),
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
    let body = received(&metrics);
    let answer = export_signal(&stores.metrics, metrics, stores.context(PROJECT, &body))
        .await
        .expect("answered");
    assert_eq!(
        answer
            .partial_success
            .expect("rejected")
            .rejected_data_points,
        1
    );
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    time_unix_nano: nanos(long_ago()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let body = received(&logs);
    let answer = export_signal(&stores.logs, logs, stores.context(PROJECT, &body))
        .await
        .expect("answered");
    assert_eq!(
        answer
            .partial_success
            .expect("rejected")
            .rejected_log_records,
        1
    );
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty()
    );
}

/// A payload already staged with records older than the horizon - staged before it passed, or by an earlier
/// build - settles as deliberately absent rather than staying pending until its cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_staged_payload_older_than_the_horizon_settles_as_absent() {
    let Some(harness) = harness("distributed_ingest_staged").await else {
        return;
    };
    let stores = &harness.stores;
    let old = trace_export(0x31, &[long_ago()]);
    let body = received(&old);
    let reference = stores
        .staging
        .stage(
            PROJECT,
            StagedSignal::Traces,
            &body.staged(),
            body.received_at,
            vec![StagedRecord::Span {
                trace_id: "31".repeat(16),
                span_id: "01".repeat(8),
                content_digest: "never-stored".to_string(),
                timestamp: long_ago(),
            }],
            sideseat_ingestion::staging::StagedWrite {
                partition_key: String::new(),
                raw_id: None,
            },
        )
        .await
        .expect("staged");
    let (payload, _) = stores
        .staging
        .load(&reference.id)
        .await
        .expect("load")
        .expect("staged payload");
    assert_eq!(
        stores.staging.settle(&payload).await.expect("settled"),
        StagingDisposition::DeliberatelyAbsent
    );
    assert!(
        stores
            .staging
            .pending(10)
            .await
            .expect("pending")
            .is_empty()
    );
    assert_eq!(
        stores
            .staging
            .redrive_once(&stores.pipeline, 10)
            .await
            .expect("the sweep"),
        0,
        "nothing left for the sweep to redrive"
    );
}

/// A held project's old data points, staged and then redriven - their export interrupted - are written under the
/// hold, as the export would have written them, so ClickHouse keeps them and the payload settles. Written
/// ungoverned, they lost the hold and were dropped, and the payload was retried to its cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_projects_old_data_points_redriven_are_kept() {
    use sideseat_ingestion::signals::Signal;

    let Some(harness) = harness("distributed_ingest_redrive").await else {
        return;
    };
    let stores = &harness.stores;
    stores
        .governance
        .set_hold(&ProjectId::from(PROJECT), Utc::now() + TimeDelta::days(30))
        .await
        .expect("hold");
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "queue.depth".to_string(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: nanos(long_ago()),
                            value: Some(number_data_point::Value::AsInt(7)),
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
    let body = received(&metrics);
    let mut request = metrics.clone();
    stores.metrics.inject_project_id(&mut request, PROJECT);
    stores
        .staging
        .stage(
            PROJECT,
            StagedSignal::Metrics,
            &body.staged(),
            body.received_at,
            stores.metrics.staged_records(&request, body.received_at),
            sideseat_ingestion::staging::StagedWrite {
                partition_key: String::new(),
                raw_id: None,
            },
        )
        .await
        .expect("staged, and then the export was interrupted");

    stores
        .staging
        .redrive_once(&stores.pipeline, 10)
        .await
        .expect("the sweep");

    // ClickHouse applies the TTL in a merge, soon after the insert rather than in it: forced, a row the hold
    // does not keep is gone.
    harness
        .clickhouse
        .query("OPTIMIZE TABLE otel_metrics FINAL")
        .execute()
        .await
        .expect("merge");
    let kept: u64 = harness
        .clickhouse
        .query("SELECT count() FROM otel_metrics")
        .fetch_one()
        .await
        .expect("count");
    assert_eq!(
        kept, 1,
        "the redriven data point lost the hold and was dropped"
    );
}

/// A raw record whose every span is already past the horizon when it lands is never read and never owned. The
/// table's TTL removes it only in a merge, some time after the insert, and until then it was read as live - by
/// the reconciler, a re-derivation and survivor reconciliation - and the ingest's confirm fence granted its
/// trace ownership of bytes the merge was about to take.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_landing_past_the_horizon_is_neither_read_nor_owned() {
    let Some(harness) = harness("distributed_ingest_raw_ttl").await else {
        return;
    };
    let stores = &harness.stores;
    let project = ProjectId::from(PROJECT);
    // A merge applies the TTL at a time of its choosing; held off, the expired row stays for the readers to hide.
    harness
        .clickhouse
        .query("SYSTEM STOP TTL MERGES otel_raw")
        .execute()
        .await
        .expect("stop TTL merges");

    let old = trace_export(0x41, &[long_ago(), long_ago()]);
    let body = received(&old);
    let answer = export_signal(&stores.traces, old, stores.context(PROJECT, &body))
        .await
        .expect("answered");
    assert_eq!(answer.partial_success.expect("rejected").rejected_spans, 2);

    // No merge has run: the row is still in the table, so only the readers can hide it.
    let stored: u64 = harness
        .clickhouse
        .query("SELECT count() FROM otel_raw WHERE project_id = ?")
        .bind(PROJECT)
        .fetch_one()
        .await
        .expect("count");
    assert_eq!(stored, 1, "the insert kept the raw record until a merge");
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::from(stores.analytics.repository());
    assert!(
        analytics
            .raw_records_page(&project, None, 10)
            .await
            .expect("page")
            .is_empty(),
        "a record past its time to live reads as absent"
    );
    let database = stores.database.repository();
    assert!(
        database
            .raw_owners_of_traces(&project, &["41".repeat(16)])
            .await
            .expect("owners")
            .is_empty(),
        "and its trace owns nothing"
    );
}

/// The same, with merges forced while the export is written, so the expired record's row can be gone by the time
/// the ingest finds no live version: whether it is past its time to live is decided from what it holds against
/// the store's clock, not from whether its row is still there to ask about, and its trace still owns nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_merged_away_while_landing_past_the_horizon_is_still_not_owned() {
    let Some(harness) = harness("distributed_ingest_raw_ttl_merged").await else {
        return;
    };
    let stores = &harness.stores;
    let project = ProjectId::from(PROJECT);
    let merging = {
        let client = harness.clickhouse.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                client
                    .query("OPTIMIZE TABLE otel_raw FINAL")
                    .execute()
                    .await
                    .expect("a merge");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
    };
    for trace in 0x51..0x55u8 {
        let old = trace_export(trace, &[long_ago()]);
        let body = received(&old);
        export_signal(&stores.traces, old, stores.context(PROJECT, &body))
            .await
            .expect("answered");
    }
    merging.await.expect("merges");
    let traces: Vec<String> = (0x51..0x55u8)
        .map(|t| format!("{t:02x}").repeat(16))
        .collect();
    assert!(
        stores
            .database
            .repository()
            .raw_owners_of_traces(&project, &traces)
            .await
            .expect("owners")
            .is_empty(),
        "no trace owns a record past its time to live"
    );
}
