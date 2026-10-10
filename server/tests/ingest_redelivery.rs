//! An export the server answered with an error after it had written it, and the client sent again, is stored
//! once: one raw record holding exactly the bytes sent, one row per span, log record and datapoint.
//!
//! A 503 after the write is the realistic way a client comes to send an export twice. The server writes the
//! export's raw record and rows, then fails to settle its staged payload, and answers 503; the client retries the
//! same bytes at a new receipt; and the staging redrive later replays the first delivery too. Here the settlement
//! fails at the real registry statement (a trigger on `staged_payloads`), so each step is the one the server takes.
//! The same holds for deliveries that race: the client gave up waiting and sent again while the first was still
//! being written.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ingestion::received::ReceivedPayload;
use sideseat_ingestion::signals::{SignalExportError, export_signal};
use sideseat_ports::clock::Clock;
use sideseat_server::app::storage::{AnalyticsService, TransactionalService};

#[path = "support/stack.rs"]
mod stack;

const PROJECT: &str = "redelivery";

/// A raw record's id, version and decoded bytes.
type RawVersion = (String, i64, Vec<u8>);

/// A span row: its trace, its span, its content digest and its receipt in microseconds.
type SpanRow = (String, String, String, i64);

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0)
        .single()
        .expect("instant")
}

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        start()
    }
}

fn text(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    }
}

/// A model call and the two tool calls under it, with their prompt.
fn trace_export() -> ExportTraceServiceRequest {
    let nanos = |second: u64| 1_790_000_000_000_000_000 + second * 1_000_000_000;
    let span = |id: u8, parent: Option<u8>, name: &str, second: u64| Span {
        trace_id: vec![0x7a; 16],
        span_id: vec![id; 8],
        parent_span_id: parent.map(|parent| vec![parent; 8]).unwrap_or_default(),
        name: name.to_string(),
        start_time_unix_nano: nanos(second),
        end_time_unix_nano: nanos(second + 1),
        attributes: vec![
            KeyValue {
                key: "gen_ai.operation.name".to_string(),
                value: Some(text("chat")),
            },
            KeyValue {
                key: "gen_ai.prompt".to_string(),
                value: Some(text("What is the weather in Paris?")),
            },
        ],
        ..Default::default()
    };
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![
                    span(1, None, "chat", 0),
                    span(2, Some(1), "tool weather", 1),
                    span(3, Some(1), "tool forecast", 2),
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn log_export() -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..3)
                    .map(|n| LogRecord {
                        time_unix_nano: 1_790_000_000_000_000_000 + n,
                        body: Some(text(&format!("step {n}"))),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn metric_export() -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "queue.depth".to_string(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: (0..3)
                            .map(|n| NumberDataPoint {
                                time_unix_nano: 1_790_000_000_000_000_000 + n,
                                value: Some(number_data_point::Value::AsInt(n as i64)),
                                ..Default::default()
                            })
                            .collect(),
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    stores: stack::Stores,
}

async fn harness() -> Harness {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let stores = stack::stores(dir.path(), Arc::new(FixedClock)).await;
    stores.create_project(PROJECT, PROJECT, start()).await;
    Harness { _dir: dir, stores }
}

/// Make the staging registry refuse to retire a payload, at the real statement, until `clear`.
async fn refuse_settlement(stores: &stack::Stores, refuse: bool) {
    let TransactionalService::Sqlite(sqlite) = stores.database.as_ref() else {
        panic!("embedded stores");
    };
    let statement = if refuse {
        "CREATE TRIGGER redelivery_fault BEFORE DELETE ON staged_payloads \
         BEGIN SELECT RAISE(FAIL, 'injected settlement fault'); END"
    } else {
        "DROP TRIGGER IF EXISTS redelivery_fault"
    };
    sqlx::query(statement)
        .execute(sqlite.pool())
        .await
        .expect("the fault");
}

fn received(bytes: &[u8], second: i64) -> ReceivedPayload {
    received_as(bytes, RawContent::Protobuf, second)
}

fn received_as(bytes: &[u8], content: RawContent, second: i64) -> ReceivedPayload {
    ReceivedPayload::new(
        bytes.to_vec(),
        content,
        start() + TimeDelta::seconds(second),
    )
}

fn duckdb(stores: &stack::Stores) -> &sideseat_adapter_duckdb::DuckdbService {
    let AnalyticsService::Duckdb(duckdb) = stores.analytics.as_ref() else {
        panic!("embedded stores");
    };
    duckdb
}

/// Every version of every raw record the project holds, decoded: each must be exactly the bytes sent.
fn raw_records(stores: &stack::Stores) -> Vec<RawVersion> {
    let conn = duckdb(stores).conn();
    let mut statement = conn
        .prepare(
            "SELECT raw_id, version, record FROM otel_raw WHERE project_id = ? \
             ORDER BY raw_id, version",
        )
        .expect("prepare");
    statement
        .query_map([PROJECT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .expect("query")
        .map(|row| {
            let (raw_id, version, record) = row.expect("row");
            let (_, bytes) = raw_payload::decode(&record, |_| None).expect("a raw record decodes");
            (raw_id, version, bytes)
        })
        .collect()
}

/// Rows in `table` for the project, and the identities among them.
fn rows_and_identities(stores: &stack::Stores, table: &str, identity: &str) -> (i64, i64) {
    duckdb(stores)
        .conn()
        .query_row(
            &format!(
                "SELECT count(*), count(DISTINCT ({identity})) FROM {table} WHERE project_id = ?"
            ),
            [PROJECT],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("count")
}

/// What one trace export sent twice and redriven once leaves: one raw record, every version of it the bytes
/// sent, and one row per span.
fn assert_stored_once(stores: &stack::Stores, sent: &[u8]) {
    let records = raw_records(stores);
    let ids: std::collections::BTreeSet<&str> =
        records.iter().map(|(raw_id, ..)| raw_id.as_str()).collect();
    assert_eq!(ids.len(), 1, "one export, {} raw records", ids.len());
    for (raw_id, version, bytes) in &records {
        assert_eq!(
            bytes.as_slice(),
            sent,
            "{raw_id} v{version} holds bytes the export was not"
        );
    }
    assert_eq!(
        rows_and_identities(stores, "otel_spans", "trace_id, span_id"),
        (3, 3),
        "one row per span"
    );
}

/// An export answered 503 after its write, sent again, then redriven, is stored once - as protobuf or as
/// OTLP/JSON, the two bodies an exporter sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_answered_503_after_its_write_and_sent_again_is_stored_once() {
    let protobuf = trace_export().encode_to_vec();
    let json = serde_json::to_vec(&trace_export()).expect("OTLP/JSON");
    for (sent, content) in [(protobuf, RawContent::Protobuf), (json, RawContent::Json)] {
        let harness = harness().await;
        let stores = &harness.stores;

        refuse_settlement(stores, true).await;
        let first = received_as(&sent, content, 0);
        let answer = export_signal(
            &stores.traces,
            trace_export(),
            stores.context(PROJECT, &first),
        )
        .await;
        assert!(
            matches!(answer, Err(SignalExportError::StoreUnavailable)),
            "the settlement fault answers 503: {answer:?}"
        );
        assert_eq!(
            raw_records(stores).len(),
            1,
            "the 503 came after the raw write"
        );
        refuse_settlement(stores, false).await;

        let again = received_as(&sent, content, 1);
        export_signal(
            &stores.traces,
            trace_export(),
            stores.context(PROJECT, &again),
        )
        .await
        .expect("the retry is stored");
        stores
            .staging
            .redrive_once(&stores.pipeline, 10)
            .await
            .expect("the redrive");

        assert_stored_once(stores, &sent);
        assert_eq!(
            stores
                .staging
                .redrive_once(&stores.pipeline, 10)
                .await
                .expect("a second redrive"),
            0,
            "a delivery is left unsettled"
        );
    }
}

/// Deliveries of one export that race - the client gave up waiting and sent again - are stored once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_deliveries_of_one_export_are_stored_once() {
    let harness = harness().await;
    let stores = &harness.stores;
    let sent = trace_export().encode_to_vec();
    let deliveries: Vec<ReceivedPayload> = (0..8).map(|n| received(&sent, n)).collect();
    let answers = futures::future::join_all(deliveries.iter().map(|delivery| {
        export_signal(
            &stores.traces,
            trace_export(),
            stores.context(PROJECT, delivery),
        )
    }))
    .await;
    for answer in answers {
        answer.expect("every delivery is answered");
    }
    assert_stored_once(stores, &sent);
}

/// Log records and datapoints answered 503 after their write and sent again are one row each.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_and_metrics_answered_503_after_their_write_and_sent_again_are_stored_once() {
    let harness = harness().await;
    let stores = &harness.stores;
    let logs = log_export().encode_to_vec();
    let metrics = metric_export().encode_to_vec();

    refuse_settlement(stores, true).await;
    let (first_logs, first_metrics) = (received(&logs, 0), received(&metrics, 0));
    let log_answer = export_signal(
        &stores.logs,
        log_export(),
        stores.context(PROJECT, &first_logs),
    )
    .await;
    let metric_answer = export_signal(
        &stores.metrics,
        metric_export(),
        stores.context(PROJECT, &first_metrics),
    )
    .await;
    assert!(
        matches!(log_answer, Err(SignalExportError::StoreUnavailable)),
        "{log_answer:?}"
    );
    assert!(
        matches!(metric_answer, Err(SignalExportError::StoreUnavailable)),
        "{metric_answer:?}"
    );
    refuse_settlement(stores, false).await;

    let (again_logs, again_metrics) = (received(&logs, 1), received(&metrics, 1));
    export_signal(
        &stores.logs,
        log_export(),
        stores.context(PROJECT, &again_logs),
    )
    .await
    .expect("the logs retry");
    export_signal(
        &stores.metrics,
        metric_export(),
        stores.context(PROJECT, &again_metrics),
    )
    .await
    .expect("the metrics retry");
    stores
        .staging
        .redrive_once(&stores.pipeline, 10)
        .await
        .expect("the redrive");

    assert_eq!(
        rows_and_identities(stores, "otel_logs", "log_digest, ordinal"),
        (3, 3),
        "one row per log record"
    );
    assert_eq!(
        rows_and_identities(stores, "otel_metrics", "datapoint_id"),
        (3, 3),
        "one row per datapoint"
    );
}

/// Two exports that carry one span between them - an SDK flushing a span again in its next batch - and the second
/// correcting it.
fn sharing_exports() -> [ExportTraceServiceRequest; 2] {
    let first = trace_export();
    let mut second = trace_export();
    let spans = &mut second.resource_spans[0].scope_spans[0].spans;
    // The model call again, with its output now; the first tool call again, unchanged; a third tool call.
    spans[0].attributes.push(KeyValue {
        key: "gen_ai.completion".to_string(),
        value: Some(text("Sunny, 24 degrees")),
    });
    spans[2] = Span {
        span_id: vec![4; 8],
        name: "tool alerts".to_string(),
        ..spans[2].clone()
    };
    [first, second]
}

/// Everything the project stores of its traces: each raw record's versions, decoded, and every span row with its
/// content and receipt.
fn stored_state(stores: &stack::Stores) -> (Vec<RawVersion>, Vec<SpanRow>) {
    let records = raw_records(stores);
    let conn = duckdb(stores).conn();
    let mut statement = conn
        .prepare(
            "SELECT trace_id, span_id, content_digest, epoch_us(ingested_at) FROM otel_spans \
             WHERE project_id = ? ORDER BY ALL",
        )
        .expect("prepare");
    let rows = statement
        .query_map([PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    (records, rows)
}

/// Two different exports sharing span identities are stored alike whatever order they arrive in, and together:
/// the raw records, their versions and the span contents do not depend on it, and no record is rewritten as a
/// union of others, since each already holds its own content.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exports_sharing_spans_are_stored_alike_in_any_order() {
    let exports = sharing_exports();
    let bodies: Vec<Vec<u8>> = exports.iter().map(Message::encode_to_vec).collect();
    // Each export keeps its own receipt, whichever reaches the server first.
    let deliveries: Vec<ReceivedPayload> = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| received(body, index as i64))
        .collect();
    let mut states = Vec::new();
    for order in [[0, 1], [1, 0]] {
        let harness = harness().await;
        let stores = &harness.stores;
        for index in order {
            export_signal(
                &stores.traces,
                exports[index].clone(),
                stores.context(PROJECT, &deliveries[index]),
            )
            .await
            .expect("stored");
        }
        states.push(stored_state(stores));
    }
    let harness = harness().await;
    let stores = &harness.stores;
    let together = futures::future::join_all((0..2).map(|index| {
        export_signal(
            &stores.traces,
            exports[index].clone(),
            stores.context(PROJECT, &deliveries[index]),
        )
    }))
    .await;
    for answer in together {
        answer.expect("stored");
    }
    states.push(stored_state(stores));

    // The raw records and their versions, and the span contents. Not the span rows' receipts: a span sent again,
    // unchanged, in a later export takes the receipt of whichever export arrives last, which the span slice's
    // receipt precedence settles.
    let contents = |rows: &[SpanRow]| -> std::collections::BTreeSet<(String, String, String)> {
        rows.iter()
            .map(|(trace, span, digest, _)| (trace.clone(), span.clone(), digest.clone()))
            .collect()
    };
    for (records, rows) in &states[1..] {
        assert_eq!(
            records, &states[0].0,
            "the raw records depend on the arrival order"
        );
        assert_eq!(
            contents(rows),
            contents(&states[0].1),
            "the span contents depend on the arrival order"
        );
    }
    let (records, _) = &states[0];
    for (raw_id, version, bytes) in records {
        assert!(
            bodies.iter().any(|body| body == bytes),
            "{raw_id} v{version} holds bytes no export was"
        );
    }
    assert_eq!(records.len(), 2, "one version of each export's own record");
}

/// The image the media exports carry.
fn image() -> Vec<u8> {
    (0..3_000u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

/// The image as base64, the way a model call carries one inline.
fn image_text() -> String {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD.encode(image())
}

/// An export of one span in trace `trace` whose prompt carries the image: the same image in every export.
fn media_export(trace: u8) -> ExportTraceServiceRequest {
    let prompt = format!(
        r#"[{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{}"}}}}]}}]"#,
        image_text()
    );
    let mut export = trace_export();
    let spans = &mut export.resource_spans[0].scope_spans[0].spans;
    spans.truncate(1);
    spans[0].trace_id = vec![trace; 16];
    spans[0].attributes[1].value = Some(text(&prompt));
    export
}

/// Every file under `directory`, as its path's components below it.
fn files_under(directory: &std::path::Path) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            // A store that never wrote here has no directory.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("{}: {error}", next.display()),
        };
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(
                    path.strip_prefix(directory)
                        .expect("under the directory")
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                );
            }
        }
    }
    found.sort();
    found
}

/// Two exports that carry one image, each delivered several times and all at once, store the image once: one
/// object in the blob store, at the image's address and holding its bytes, one file row, an association per trace,
/// and nothing left of the staging blobs or of any write's temporary files.
///
/// The deliveries overlap while they wait on the stores; the inline batcher then writes their media one batch at a
/// time. Two writers of one object at the same instant - two processes on one blob store - are the blob store's
/// own test (`store_publishes_atomically_and_leaves_no_temporary_files`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exports_sharing_an_image_delivered_at_once_store_it_once() {
    let harness = harness().await;
    let stores = &harness.stores;
    let exports = [media_export(0x51), media_export(0x52)];
    let bodies: Vec<Vec<u8>> = exports.iter().map(Message::encode_to_vec).collect();
    let deliveries: Vec<(usize, ReceivedPayload)> = (0..8)
        .map(|n| (n % 2, received(&bodies[n % 2], n as i64)))
        .collect();
    let answers = futures::future::join_all(deliveries.iter().map(|(index, delivery)| {
        export_signal(
            &stores.traces,
            exports[*index].clone(),
            stores.context(PROJECT, delivery),
        )
    }))
    .await;
    for answer in answers {
        answer.expect("every delivery is answered");
    }
    assert_eq!(
        stores
            .staging
            .redrive_once(&stores.pipeline, 10)
            .await
            .expect("the redrive"),
        0,
        "a delivery is left unsettled"
    );

    let storage =
        sideseat_core::storage::AppStorage::init_for_test(harness._dir.path().to_path_buf());
    let files = storage.subdir(sideseat_core::storage::DataSubdir::Files);
    let [media] = sideseat_domain::raw_payload::media_in(image_text().as_bytes())
        .try_into()
        .expect("the image is one media object");
    let hash = media.hash_hex();
    let address = vec![
        PROJECT.to_string(),
        hash[..2].to_string(),
        hash[2..4].to_string(),
        hash.clone(),
    ];
    assert_eq!(
        files_under(&files),
        vec![address.clone()],
        "one object, at the image's address"
    );
    assert_eq!(
        std::fs::read(address.iter().fold(files, |path, part| path.join(part)))
            .expect("the object"),
        image(),
        "the object holds the image"
    );
    assert_eq!(
        files_under(&storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp)),
        Vec::<Vec<String>>::new(),
        "a write left a temporary file"
    );
    let TransactionalService::Sqlite(sqlite) = stores.database.as_ref() else {
        panic!("embedded stores");
    };
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT file_hash, size_bytes FROM files WHERE project_id = ?")
            .bind(PROJECT)
            .fetch_all(sqlite.pool())
            .await
            .expect("the file rows");
    assert_eq!(rows, vec![(hash.clone(), 3_000)], "one file row, the image");
    let traces: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT trace_id, pending_writers, durable FROM trace_files \
         WHERE project_id = ? AND file_hash = ? ORDER BY trace_id",
    )
    .bind(PROJECT)
    .bind(&hash)
    .fetch_all(sqlite.pool())
    .await
    .expect("the associations");
    assert_eq!(
        traces,
        vec![("51".repeat(16), 0, 1), ("52".repeat(16), 0, 1)],
        "one settled, durable association per trace that carries the image"
    );
}
