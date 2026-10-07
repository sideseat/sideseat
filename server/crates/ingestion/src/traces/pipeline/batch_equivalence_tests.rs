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
