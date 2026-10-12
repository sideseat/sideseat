//! Per-export cost of the trace write path - extraction through the embedded stores and the raw arbiter - over
//! the whole captured corpus, at several batch sizes.
//!
//! ```bash
//! cargo test --locked --release -p sideseat-server --test ingest_write_path -- --ignored --nocapture
//! ```
//!
//! What `ingest_hot_path` leaves out: the DuckDB and SQLite work a batch does, which the CPU profile of the
//! ingest benchmark attributes most of a server's time to. One thread, every export written as new telemetry -
//! each pass rewrites every trace and span id - so the figure is ingestion, not the idempotent redelivery path.
//! Batches are cut from the corpus in order, as the inline batcher cuts them from the queue. Staging is not
//! included: the payloads are written as a batch with no registration, the path `run_waves` takes for them.
//!
//! `BATCHES` (default `1,8,64`) picks the batch sizes, `PASSES` (default 2) the passes per size, `NO_MEDIA=1`
//! leaves out exports above 512 KB. Run it in the bench image to measure the target's figure:
//! `docker run --rm --cpus 1 --memory 2g sideseat-bench write-path`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use sideseat_core::storage::AppStorage;

fn corpus() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("req-") && (n.ends_with(".pb") || n.ends_with(".json"))
            }) {
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

fn decoded_corpus(no_media: bool) -> Vec<ExportTraceServiceRequest> {
    corpus()
        .iter()
        .filter_map(|path| {
            let bytes = std::fs::read(path).expect("read fixture");
            if no_media && bytes.len() > 512 * 1024 {
                return None;
            }
            Some(if path.extension().is_some_and(|e| e == "json") {
                serde_json::from_slice::<ExportTraceServiceRequest>(&bytes)
                    .expect("decode JSON fixture")
            } else {
                ExportTraceServiceRequest::decode(bytes.as_slice()).expect("decode fixture")
            })
        })
        .collect()
}

/// The ids of `request` mapped into pass `pass`: the same mapping for every export, so a trace whose spans arrive
/// in several exports stays one trace and parents still resolve.
fn renamed(request: &ExportTraceServiceRequest, pass: u32) -> ExportTraceServiceRequest {
    let mark = |id: &mut Vec<u8>| {
        if id.len() >= 4 {
            for (byte, key) in id
                .iter_mut()
                .zip(pass.wrapping_mul(0x9E37_79B9).to_be_bytes())
            {
                *byte ^= key;
            }
        }
    };
    let mut request = request.clone();
    for resource in &mut request.resource_spans {
        for scope in &mut resource.scope_spans {
            for span in &mut scope.spans {
                mark(&mut span.trace_id);
                mark(&mut span.span_id);
                mark(&mut span.parent_span_id);
                for link in &mut span.links {
                    mark(&mut link.trace_id);
                    mark(&mut link.span_id);
                }
            }
        }
    }
    request
}

fn span_count(request: &ExportTraceServiceRequest) -> usize {
    request
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .map(|ss| ss.spans.len())
        .sum()
}

async fn pipeline(temp: &tempfile::TempDir) -> sideseat_ingestion::traces::TracePipeline {
    use sideseat_core::config::{
        CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, RetentionConfig, StorageBackend,
    };
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    std::fs::create_dir_all(storage.subdir(sideseat_core::storage::DataSubdir::Duckdb))
        .expect("duckdb dir");
    let clock: Arc<dyn sideseat_ports::clock::Clock> =
        Arc::new(sideseat_server::runtime::clock::SystemClock);
    let duckdb: Arc<dyn sideseat_ports::traits::AnalyticsRepository + Send + Sync> =
        Arc::new(sideseat_adapter_duckdb::DuckdbRepository(Arc::new(
            sideseat_adapter_duckdb::DuckdbService::init(&storage, Arc::clone(&clock))
                .await
                .expect("duckdb"),
        )));
    let sqlite = Arc::new(
        sideseat_adapter_sqlite::SqliteService::init(&storage, Arc::clone(&clock))
            .await
            .expect("sqlite"),
    );
    let transactional: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync> =
        Arc::new(sideseat_adapter_sqlite::SqliteRepository(sqlite));
    let blobs: Arc<dyn sideseat_ports::blobs::FileStorage> = Arc::new(
        sideseat_adapter_blob_storage::FilesystemStorage::new(temp.path().join("files")),
    );
    let temp_files = storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp);
    std::fs::create_dir_all(&temp_files).expect("files temp");
    let files = Arc::new(
        sideseat_domain::files::FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 0,
                filesystem_path: Some(temp.path().join("files").display().to_string()),
                s3: None,
            },
            temp_files,
            Arc::clone(&blobs),
            Arc::clone(&transactional),
            Arc::new(
                sideseat_adapter_cache::CacheService::new(&CacheConfig {
                    backend: CacheBackendType::Memory,
                    max_entries: 16,
                    eviction_policy: EvictionPolicy::TinyLfu,
                    redis_url: None,
                })
                .await
                .expect("cache"),
            ),
        )
        .await
        .expect("file service"),
    );
    sideseat_ingestion::traces::TracePipeline::new(
        Arc::clone(&duckdb),
        Arc::new(sideseat_domain::pricing::PricingService::init_for_test().expect("pricing")),
        Arc::new(sideseat_messaging::TopicService::new(
            sideseat_adapter_topics::memory_backend(),
        )),
        Arc::clone(&files),
        Arc::new(sideseat_ingestion::staging::StagingService::new(
            Arc::clone(&blobs),
            Arc::clone(&transactional),
            Arc::clone(&duckdb),
            clock,
            RetentionConfig::default(),
            5,
        )),
    )
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn bench_ingest_write_path_per_export() {
    let batches: Vec<usize> = std::env::var("BATCHES")
        .unwrap_or_else(|_| "1,8,64".to_string())
        .split(',')
        .map(|size| size.trim().parse().expect("a batch size"))
        .collect();
    let passes: u32 = std::env::var("PASSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let no_media = std::env::var("NO_MEDIA").is_ok_and(|v| v == "1");
    let corpus = decoded_corpus(no_media);
    let spans: usize = corpus.iter().map(span_count).sum();
    println!(
        "\n[write-path] {} exports, {spans} spans per pass, {passes} pass(es) per batch size, one thread{}",
        corpus.len(),
        if no_media {
            ", exports > 512 KB left out"
        } else {
            ""
        }
    );
    println!("[write-path] | Batch | ms/export | us/span | exports/s | spans/s |");
    println!("[write-path] | ---: | ---: | ---: | ---: | ---: |");
    let mut pass = 0u32;
    for batch in batches {
        // A fresh store per batch size, so one size's history does not tax the next.
        let temp = tempfile::TempDir::new().expect("temp dir");
        let pipeline = pipeline(&temp).await;
        let mut elapsed = std::time::Duration::ZERO;
        for _ in 0..passes {
            pass += 1;
            let requests: Vec<ExportTraceServiceRequest> = corpus
                .iter()
                .map(|request| renamed(request, pass))
                .collect();
            let started = Instant::now();
            for chunk in requests.chunks(batch) {
                let outcomes = pipeline.run_waves_outcomes_for_test(chunk).await;
                assert!(
                    outcomes.iter().all(|outcome| !matches!(
                        outcome,
                        sideseat_ingestion::traces::IngestOutcome::Failed
                    )),
                    "every export of a batch is written"
                );
            }
            elapsed += started.elapsed();
        }
        let exports = corpus.len() as f64 * f64::from(passes);
        let total_spans = spans as f64 * f64::from(passes);
        println!(
            "[write-path] | {batch} | {:.2} | {:.0} | {:.0} | {:.0} |",
            elapsed.as_secs_f64() * 1e3 / exports,
            elapsed.as_secs_f64() * 1e6 / total_spans,
            exports / elapsed.as_secs_f64(),
            total_spans / elapsed.as_secs_f64()
        );
    }
}
