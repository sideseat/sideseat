//! Batching changes nothing a reader can see: the same exports, one at a time or in batches of any size, store
//! the same rows and are answered the same.
//!
//! The inline path is about to persist through batches, so this is the property it rests on. The corpus is
//! sampled - every thirtieth captured export - because each store pays a synced commit per write and the
//! whole corpus one at a time would not fit a unit test's budget.

use std::path::Path;

use prost::Message;
use sideseat_ports::types::{ListSpansParams, ProjectId};

use super::pipeline_tests::pipeline_over_a_temp_store_with;
use super::*;

fn sample() -> Vec<ExportTraceServiceRequest> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/messages");
    let mut paths: Vec<_> = walk(&root)
        .into_iter()
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "pb")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("req-"))
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .step_by(30)
        .filter_map(|path| {
            ExportTraceServiceRequest::decode(std::fs::read(path).ok()?.as_slice()).ok()
        })
        .filter(|request| !request.resource_spans.is_empty())
        .collect()
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .flatten()
                .flat_map(|entry| {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path)
                    } else {
                        vec![path]
                    }
                })
                .collect()
        },
    )
}

/// A row's debug form without its `ingested_at` field, the one value a re-ingestion legitimately changes.
fn without_ingested_at(row: &str) -> String {
    let Some(start) = row.find("ingested_at: ") else {
        return row.to_string();
    };
    let end = row[start..]
        .find(", ")
        .map_or(row.len(), |offset| start + offset + 2);
    format!("{}{}", &row[..start], &row[end..])
}

/// Every stored span of the default project, as a reader sees it, without the instant it was ingested.
async fn stored(analytics: &(dyn AnalyticsRepository + Send + Sync)) -> Vec<String> {
    let (rows, _) = analytics
        .list_spans(&ListSpansParams {
            project_id: ProjectId::from("default"),
            page: 1,
            limit: 100_000,
            ..Default::default()
        })
        .await
        .expect("list spans");
    let mut rows: Vec<String> = rows
        .iter()
        .map(|row| without_ingested_at(&format!("{row:?}")))
        .collect();
    // And every raw record, the authority the rows derive from: which record, its bytes, why they differ
    // from what was received, and its traces. Its receipt time and its version - a timestamp too - are the
    // values a re-ingestion legitimately changes.
    let mut after = None;
    loop {
        let page = analytics
            .raw_records_page(&ProjectId::from("default"), after.clone(), 256)
            .await
            .expect("raw records");
        let Some(last) = page.last() else { break };
        after = Some((last.received_at, last.raw_id.clone()));
        rows.extend(page.iter().map(|record| {
            let mut traces = record.trace_ids.clone();
            traces.sort();
            format!(
                "raw {} {:?} {:?} {}",
                record.raw_id,
                record.origin,
                traces,
                blake3::hash(&record.record).to_hex()
            )
        }));
    }
    rows.sort();
    rows
}

async fn ingest(
    requests: &[ExportTraceServiceRequest],
    batch: usize,
) -> (Vec<IngestOutcome>, Vec<String>) {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let mut outcomes = Vec::new();
    for chunk in requests.chunks(batch) {
        outcomes.extend(pipeline.run_batch_outcomes_for_test(chunk).await);
    }
    (outcomes, stored(analytics.as_ref()).await)
}

/// One at a time through the single-export entry point, `ingest_now`.
async fn ingest_singly(
    requests: &[ExportTraceServiceRequest],
) -> (Vec<IngestOutcome>, Vec<String>) {
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let mut outcomes = Vec::new();
    for request in requests {
        let received = ReceivedPayload::new(
            request.encode_to_vec(),
            sideseat_domain::raw_payload::RawContent::Protobuf,
        );
        outcomes.push(pipeline.ingest_now(request, &received).await);
    }
    (outcomes, stored(analytics.as_ref()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_size_changes_nothing_a_reader_sees() {
    let requests = sample();
    assert!(
        requests.len() >= 20,
        "the sample is the corpus, not a handful"
    );

    let (alone, rows) = ingest_singly(&requests).await;
    assert!(rows.len() >= 200, "{} rows: too few to compare", rows.len());
    for size in [1, 7, requests.len()] {
        let (outcomes, batched) = ingest(&requests, size).await;
        assert_eq!(
            outcomes, alone,
            "batches of {size} answered the exports differently"
        );
        assert_eq!(
            batched.len(),
            rows.len(),
            "batches of {size} stored a different number of spans"
        );
        assert_eq!(batched, rows, "batches of {size} stored different rows");
    }
}

/// Concurrent inline exports are written together and still answered, and stored, as if each were alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_inline_exports_are_batched_and_answered_alone() {
    let requests = sample();
    let (alone, rows) = ingest_singly(&requests).await;

    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(false).await;
    let batcher = Arc::new(InlineBatcher::new(Arc::new(pipeline)));
    let tasks: Vec<_> = requests
        .iter()
        .cloned()
        .map(|request| {
            let batcher = Arc::clone(&batcher);
            tokio::spawn(async move {
                let received = ReceivedPayload::new(
                    request.encode_to_vec(),
                    sideseat_domain::raw_payload::RawContent::Protobuf,
                );
                batcher.ingest(&request, &received, None).await
            })
        })
        .collect();
    let mut outcomes = Vec::with_capacity(tasks.len());
    for task in tasks {
        outcomes.push(task.await.expect("ingest task"));
    }

    assert_eq!(outcomes, alone, "a batch answered an export differently");
    assert_eq!(
        stored(analytics.as_ref()).await,
        rows,
        "batched exports stored different rows"
    );
    let batches = batcher.batches.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        batches * 2 <= requests.len(),
        "{batches} batches for {} concurrent exports: they were not grouped",
        requests.len()
    );
}

/// An export of one span on trace `trace` whose input is `value`.
fn one_span(trace: u8, value: String) -> ExportTraceServiceRequest {
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![trace; 16],
                    span_id: vec![1; 8],
                    name: "generation".into(),
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_100,
                    attributes: vec![KeyValue {
                        key: "input.value".into(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(value)),
                        }),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn received_of(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
    )
}

/// An export naming a file that a later export of its batch supplies is stored as it is alone, arriving first:
/// its reference finds nothing and is replaced with a note. In one wave the later export's file was stored before
/// the reference was checked, so the reference held - and what was stored depended on the grouping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reference_to_a_file_a_later_export_supplies_is_stored_as_alone() {
    use base64::Engine;
    let picture = base64::engine::general_purpose::STANDARD
        .encode((0..4096u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    let supplier = one_span(43, format!("data:image/png;base64,{picture}"));
    // The file's hash, learned from a store of its own.
    let hash = {
        let (_temp, _analytics, database, pipeline) = pipeline_over_a_temp_store_with(true).await;
        pipeline
            .ingest_now(&supplier, &received_of(&supplier))
            .await;
        database
            .get_file_hashes_for_traces(&ProjectId::from("default"), &[hex::encode([43u8; 16])])
            .await
            .expect("hashes")
            .into_iter()
            .next()
            .expect("the supplier's file")
    };
    let referrer = one_span(
        42,
        format!(
            "look at {}",
            sideseat_core::utils::file_uri::build_file_uri(&hash, Some("image/png"))
        ),
    );
    let requests = [referrer, supplier];

    // The rows, and the text of every field of the referrer's span that can hold a reference: where the note
    // replaces an unbacked one.
    async fn seen(
        analytics: &(dyn AnalyticsRepository + Send + Sync),
    ) -> (Vec<String>, Vec<String>) {
        let mut fields = analytics
            .file_reference_fields_for_traces(
                &ProjectId::from("default"),
                &[hex::encode([42u8; 16])],
            )
            .await
            .expect("reference fields");
        fields.sort();
        (stored(analytics).await, fields)
    }
    let sequential = {
        let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(true).await;
        for request in &requests {
            pipeline.ingest_now(request, &received_of(request)).await;
        }
        seen(analytics.as_ref()).await
    };
    assert!(
        sequential.1.iter().all(|field| !field.contains(&hash)),
        "premise: alone and first, the reference is replaced with a note"
    );
    let (_temp, analytics, _database, pipeline) = pipeline_over_a_temp_store_with(true).await;
    let outcomes = pipeline.run_waves_outcomes_for_test(&requests).await;
    assert!(
        outcomes
            .iter()
            .all(|outcome| *outcome == IngestOutcome::Stored),
        "{outcomes:?}"
    );
    assert_eq!(
        seen(analytics.as_ref()).await,
        sequential,
        "batched, the reference was backed by a file that arrived after it"
    );
}
