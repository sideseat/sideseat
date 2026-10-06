//! Per-span cost of each stage of the trace-ingest hot path, over the whole captured corpus.
//!
//! ```bash
//! cargo test --locked --release -p sideseat-server --test ingest_hot_path -- --ignored --nocapture
//! ```
//!
//! The throughput target is stated per core of a specific machine, so the stages are timed single-threaded
//! and reported in microseconds per span. Run on the target host (or its container image) this is the
//! measured correction between the development machine and the target: the same binary, the same corpus, the
//! same stages.
//!
//! Stages: protobuf decode, protobuf re-encode, zstd at levels 1 and 3 over the encoded export, and the
//! current CPU phase of ingestion (`process_request`: attribute and message extraction, SideML, enrichment,
//! flattening). Every stage runs over every request `ITERATIONS` times (default 5) and the fastest pass is
//! reported, so a scheduling hiccup does not inflate a figure.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use sideseat_domain::pricing::PricingService;
use sideseat_ingestion::traces::extract::ExtractionMode;

fn corpus() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("req-") && (n.ends_with(".pb") || n.ends_with(".json")))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages"),
        &mut out,
    );
    out.sort();
    out
}

/// Every export as protobuf bytes; JSON captures are converted, because protobuf is what the hot path decodes.
fn encoded_corpus() -> Vec<Vec<u8>> {
    corpus()
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).expect("read fixture");
            if path.extension().is_some_and(|e| e == "json") {
                serde_json::from_slice::<ExportTraceServiceRequest>(&bytes)
                    .expect("decode JSON fixture")
                    .encode_to_vec()
            } else {
                bytes
            }
        })
        .collect()
}

fn span_count(request: &ExportTraceServiceRequest) -> usize {
    request
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .map(|ss| ss.spans.len())
        .sum()
}

fn fastest(iterations: u32, mut pass: impl FnMut()) -> Duration {
    (0..iterations)
        .map(|_| {
            let started = Instant::now();
            pass();
            started.elapsed()
        })
        .min()
        .unwrap_or_default()
}

#[test]
#[ignore]
fn bench_ingest_hot_path_per_span() {
    let iterations = std::env::var("ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5u32);
    // Media-heavy exports dominate bytes, not spans; `NO_MEDIA=1` leaves out exports above 512 KB so the figure
    // can be read for ordinary traffic too.
    let no_media = std::env::var("NO_MEDIA").is_ok_and(|v| v == "1");
    let encoded: Vec<Vec<u8>> = encoded_corpus()
        .into_iter()
        .filter(|bytes| !no_media || bytes.len() <= 512 * 1024)
        .collect();
    let decoded: Vec<ExportTraceServiceRequest> = encoded
        .iter()
        .map(|bytes| ExportTraceServiceRequest::decode(bytes.as_slice()).expect("decode"))
        .collect();
    let spans: usize = decoded.iter().map(span_count).sum();
    let bytes: usize = encoded.iter().map(Vec::len).sum();
    let pricing = PricingService::init_for_test().expect("offline pricing service");

    let decode = fastest(iterations, || {
        for bytes in &encoded {
            std::hint::black_box(ExportTraceServiceRequest::decode(bytes.as_slice()).unwrap());
        }
    });
    let encode = fastest(iterations, || {
        for request in &decoded {
            std::hint::black_box(request.encode_to_vec());
        }
    });
    let zstd = |level: i32| {
        let mut compressed = 0usize;
        let time = fastest(iterations, || {
            compressed = 0;
            for bytes in &encoded {
                compressed += zstd::bulk::compress(bytes, level).unwrap().len();
            }
        });
        (time, compressed)
    };
    let (zstd1, zstd1_bytes) = zstd(1);
    let (zstd3, zstd3_bytes) = zstd(3);
    let extract = fastest(iterations, || {
        for request in &decoded {
            std::hint::black_box(sideseat_ingestion::traces::process_request_for_test_with_mode(
                request,
                &pricing,
                ExtractionMode::PerCarrier,
            ));
        }
    });

    let per_span = |d: Duration| d.as_secs_f64() * 1e6 / spans as f64;
    println!(
        "\n[hot-path] {} exports, {spans} spans, {:.1} MB, fastest of {iterations} passes, one thread{}",
        encoded.len(),
        bytes as f64 / 1e6,
        if no_media { ", exports > 512 KB left out" } else { "" }
    );
    println!("[hot-path] | Stage | us/span | MB/s |");
    println!("[hot-path] | --- | ---: | ---: |");
    for (stage, time) in [
        ("protobuf decode", decode),
        ("protobuf encode", encode),
        ("zstd level 1", zstd1),
        ("zstd level 3", zstd3),
        ("process_request (extraction, SideML, enrichment)", extract),
    ] {
        println!(
            "[hot-path] | {stage} | {:.2} | {:.0} |",
            per_span(time),
            bytes as f64 / time.as_secs_f64() / 1e6
        );
    }
    println!(
        "[hot-path] zstd ratio per export: level 1 {:.2}x, level 3 {:.2}x",
        bytes as f64 / zstd1_bytes as f64,
        bytes as f64 / zstd3_bytes as f64
    );
}
