//! The storage gate's corpus replayed in process, so the stores it leaves hold the same bytes on every run.
//!
//! `make footprint-storage` posts the pinned corpus to a live server and measures what it stored. Two runs of it
//! differ: every export is stored at its receipt, the instant it arrived, and a server's checkpoint lands wherever
//! its timer does. Here the corpus goes through the very path an export takes - `export_signal`, with staging,
//! admission, the trace pipeline and settlement - but each export's receipt is fixed by its place in the corpus,
//! every other clock reads one instant, no timed checkpoint runs - the write-ahead log is checkpointed where its
//! size says, on one engine thread - and projects get ids derived from their tenants. `scripts/perf/storage-footprint.py --store` then attributes the stores it leaves.
//!
//! ```bash
//! SIDESEAT_STORAGE_GATE_DIR=/tmp/gate SIDESEAT_STORAGE_GATE_PASSES=1 \
//!   cargo test --locked -p sideseat-server --test storage_gate -- --ignored --nocapture
//! ```
//!
//! A pass after the first replays the corpus again as new telemetry: every trace and span id rewritten by the
//! pass, every instant moved on by a day per pass, so it is stored rather than recognised as a re-send. More
//! passes divide the figure's resolution, a DuckDB block, over more items.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::metrics::v1::metric;
use prost::Message;
use sideseat_domain::raw_payload::RawContent;
use sideseat_ingestion::received::ReceivedPayload;
use sideseat_ingestion::signals::{Signal, export_signal};
use sideseat_ports::clock::Clock;
use sideseat_server::app::storage::AnalyticsService;

#[path = "support/stack.rs"]
mod stack;

use stack::Stores;

/// The instant every clock but an export's receipt reads.
fn epoch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0)
        .single()
        .expect("instant")
}

#[derive(Debug)]
struct GateClock;

impl Clock for GateClock {
    fn now(&self) -> DateTime<Utc> {
        epoch()
    }
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Traces,
    Logs,
    Metrics,
}

struct Export {
    kind: Kind,
    tenant: String,
    content: RawContent,
    body: Vec<u8>,
}

/// The pinned corpus, in the order the live gate posts it: the manifest's.
fn corpus() -> Vec<Export> {
    let root = repo_root();
    let manifest: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(root.join("scripts/perf/storage-footprint-corpus.json"))
            .expect("the corpus manifest"),
    )
    .expect("the corpus manifest is JSON");
    manifest
        .keys()
        .map(|path| {
            let relative = Path::new(path);
            let name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a file name");
            let kind = match name.split('-').next() {
                Some("req") => Kind::Traces,
                Some("logs") => Kind::Logs,
                Some("metrics") => Kind::Metrics,
                _ => panic!("{path} is no export"),
            };
            let corpus = if kind == Kind::Metrics {
                "server/tests/fixtures/metrics"
            } else {
                "server/tests/fixtures/messages"
            };
            let tenant = relative
                .strip_prefix(corpus)
                .ok()
                .and_then(|inner| inner.components().next())
                .map(|first| first.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_else(|| panic!("{path} is outside its corpus"));
            Export {
                kind,
                tenant,
                content: if name.ends_with(".json") {
                    RawContent::Json
                } else {
                    RawContent::Protobuf
                },
                body: std::fs::read(root.join(path)).expect("a pinned fixture"),
            }
        })
        .collect()
}

fn decode<T: Message + Default + for<'de> serde::Deserialize<'de>>(export: &Export) -> T {
    match export.content {
        RawContent::Protobuf => T::decode(export.body.as_slice()).expect("protobuf export"),
        RawContent::Json => serde_json::from_slice(&export.body).expect("OTLP/JSON export"),
    }
}

/// An id made the pass's own: the same length, so a 16-byte trace id stays one.
fn rewrite_id(id: &mut [u8], pass: usize) {
    if id.is_empty() {
        return;
    }
    let key = (pass as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .to_be_bytes();
    for (index, byte) in id.iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
}

fn shift(nanos: &mut u64, pass: usize) {
    if *nanos != 0 {
        *nanos += pass as u64 * 86_400_000_000_000;
    }
}

fn traces_for_pass(
    mut request: ExportTraceServiceRequest,
    pass: usize,
) -> ExportTraceServiceRequest {
    for resource in &mut request.resource_spans {
        for scope in &mut resource.scope_spans {
            for span in &mut scope.spans {
                rewrite_id(&mut span.trace_id, pass);
                rewrite_id(&mut span.span_id, pass);
                rewrite_id(&mut span.parent_span_id, pass);
                shift(&mut span.start_time_unix_nano, pass);
                shift(&mut span.end_time_unix_nano, pass);
                for event in &mut span.events {
                    shift(&mut event.time_unix_nano, pass);
                }
                for link in &mut span.links {
                    rewrite_id(&mut link.trace_id, pass);
                    rewrite_id(&mut link.span_id, pass);
                }
            }
        }
    }
    request
}

fn logs_for_pass(mut request: ExportLogsServiceRequest, pass: usize) -> ExportLogsServiceRequest {
    for resource in &mut request.resource_logs {
        for scope in &mut resource.scope_logs {
            for record in &mut scope.log_records {
                rewrite_id(&mut record.trace_id, pass);
                rewrite_id(&mut record.span_id, pass);
                shift(&mut record.time_unix_nano, pass);
                shift(&mut record.observed_time_unix_nano, pass);
            }
        }
    }
    request
}

/// A pass's exemplars, rewritten as their datapoints are: a sampled span the pass's own, its instant a day on.
fn exemplars_for_pass(
    exemplars: &mut [opentelemetry_proto::tonic::metrics::v1::Exemplar],
    pass: usize,
) {
    for exemplar in exemplars {
        shift(&mut exemplar.time_unix_nano, pass);
        rewrite_id(&mut exemplar.trace_id, pass);
        rewrite_id(&mut exemplar.span_id, pass);
    }
}

fn metrics_for_pass(
    mut request: ExportMetricsServiceRequest,
    pass: usize,
) -> ExportMetricsServiceRequest {
    for resource in &mut request.resource_metrics {
        for scope in &mut resource.scope_metrics {
            for metric in &mut scope.metrics {
                match metric.data.as_mut() {
                    Some(metric::Data::Gauge(gauge)) => {
                        for point in &mut gauge.data_points {
                            shift(&mut point.time_unix_nano, pass);
                            shift(&mut point.start_time_unix_nano, pass);
                            exemplars_for_pass(&mut point.exemplars, pass);
                        }
                    }
                    Some(metric::Data::Sum(sum)) => {
                        for point in &mut sum.data_points {
                            shift(&mut point.time_unix_nano, pass);
                            shift(&mut point.start_time_unix_nano, pass);
                            exemplars_for_pass(&mut point.exemplars, pass);
                        }
                    }
                    Some(metric::Data::Histogram(histogram)) => {
                        for point in &mut histogram.data_points {
                            shift(&mut point.time_unix_nano, pass);
                            shift(&mut point.start_time_unix_nano, pass);
                            exemplars_for_pass(&mut point.exemplars, pass);
                        }
                    }
                    Some(metric::Data::ExponentialHistogram(histogram)) => {
                        for point in &mut histogram.data_points {
                            shift(&mut point.time_unix_nano, pass);
                            shift(&mut point.start_time_unix_nano, pass);
                            exemplars_for_pass(&mut point.exemplars, pass);
                        }
                    }
                    Some(metric::Data::Summary(summary)) => {
                        for point in &mut summary.data_points {
                            shift(&mut point.time_unix_nano, pass);
                            shift(&mut point.start_time_unix_nano, pass);
                        }
                    }
                    None => {}
                }
            }
        }
    }
    request
}

/// A project for each tenant, its id derived from the tenant's place among them.
async fn projects(stores: &Stores, exports: &[Export]) -> BTreeMap<String, String> {
    let tenants: std::collections::BTreeSet<&str> = exports
        .iter()
        .map(|export| export.tenant.as_str())
        .collect();
    let mut ids = BTreeMap::new();
    for (index, tenant) in tenants.into_iter().enumerate() {
        let id = format!("gate-{index:03}");
        stores.create_project(&id, tenant, epoch()).await;
        ids.insert(tenant.to_string(), id);
    }
    ids
}

async fn send<S: Signal>(
    stores: &Stores,
    signal: &S,
    project_id: &str,
    request: S::Request,
    received: &ReceivedPayload,
) {
    if let Err(error) = export_signal(signal, request, stores.context(project_id, received)).await {
        panic!("an export of {project_id} was refused: {error:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "builds the storage gate's stores; `make storage-gate` measures them"]
async fn the_pinned_corpus_replays_into_the_same_stores() {
    let Ok(dir) = std::env::var("SIDESEAT_STORAGE_GATE_DIR") else {
        panic!("set SIDESEAT_STORAGE_GATE_DIR to the directory the stores are written to");
    };
    let passes: usize = std::env::var("SIDESEAT_STORAGE_GATE_PASSES")
        .ok()
        .map(|value| value.parse().expect("a pass count"))
        .unwrap_or(1);
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let root = PathBuf::from(dir);
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("a fresh directory");
    }
    let exports = corpus();
    let stores = stack::stores(&root, Arc::new(GateClock)).await;
    // One engine thread and the default checkpoint threshold, set rather than assumed, as
    // `scripts/perf/storage_gate_figures.py` uses: DuckDB lays a checkpoint's segments out in the order its threads
    // finish them, so a store checkpointed by several can differ between two runs of one input, and the server
    // sizes its threads from the host. `SIDESEAT_STORAGE_GATE_THREADS` replays on another count, to show what
    // depends on it.
    let AnalyticsService::Duckdb(duckdb) = stores.analytics.as_ref() else {
        panic!("the gate replays into the embedded stores");
    };
    let threads: usize = std::env::var("SIDESEAT_STORAGE_GATE_THREADS")
        .ok()
        .map_or(1, |value| value.parse().expect("a thread count"));
    duckdb
        .conn()
        .execute_batch(&format!(
            "SET threads = {threads}; SET checkpoint_threshold = '16MiB';"
        ))
        .expect("the engine pinned");
    let projects = projects(&stores, &exports).await;
    let started = std::time::Instant::now();
    for pass in 0..passes {
        for (index, export) in exports.iter().enumerate() {
            let project_id = projects[&export.tenant].as_str();
            // One millisecond apart, in corpus order, pass after pass.
            let received_at =
                epoch() + TimeDelta::milliseconds((pass * exports.len() + index) as i64);
            // The first pass sends the fixture's own bytes; a later one its rewrite, encoded as protobuf.
            match export.kind {
                Kind::Traces => {
                    let request = traces_for_pass(decode(export), pass);
                    let received = received_for(export, &request, pass, received_at);
                    send(&stores, &stores.traces, project_id, request, &received).await;
                }
                Kind::Logs => {
                    let request = logs_for_pass(decode(export), pass);
                    let received = received_for(export, &request, pass, received_at);
                    send(&stores, &stores.logs, project_id, request, &received).await;
                }
                Kind::Metrics => {
                    let request = metrics_for_pass(decode(export), pass);
                    let received = received_for(export, &request, pass, received_at);
                    send(&stores, &stores.metrics, project_id, request, &received).await;
                }
            }
        }
    }
    let replayed = started.elapsed();
    // Every export settled as it was answered: none is left for the redrive, which would write after the
    // measurement.
    assert_eq!(
        stores
            .staging
            .redrive_once(&stores.pipeline, 1)
            .await
            .expect("the redrive"),
        0,
        "an export left a staged payload unsettled"
    );
    stores
        .analytics
        .checkpoint()
        .await
        .expect("checkpoint duckdb");
    stores
        .database
        .checkpoint()
        .await
        .expect("checkpoint sqlite");
    println!(
        "[storage-gate] replayed {} exports x {passes} in {:.1}s into {}",
        exports.len(),
        replayed.as_secs_f64(),
        root.display()
    );
}

fn received_for<T: Message>(
    export: &Export,
    request: &T,
    pass: usize,
    received_at: DateTime<Utc>,
) -> ReceivedPayload {
    if pass == 0 {
        ReceivedPayload::new(export.body.clone(), export.content, received_at)
    } else {
        ReceivedPayload::new(request.encode_to_vec(), RawContent::Protobuf, received_at)
    }
}
