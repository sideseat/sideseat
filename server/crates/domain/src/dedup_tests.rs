//! The wrapper the server reads through answers for the whole port, not only the methods someone remembered.
//!
//! `DedupAnalyticsRepository` forwards method by method, and the port used to hand it defaults that returned
//! `NotImplemented` for an adapter that cannot do a thing. A forgotten forward therefore looked like a backend
//! limitation: the search-index backfill and pressure reclamation were silently off in every deployment, since
//! the server wraps every read in this type. The defaults are gone, so a forgotten forward no longer compiles -
//! and these tests drive both features *through the wrapper* against a real store, because a compile-time
//! guarantee says the method exists, not that the feature works.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsMaintenance, AnalyticsRepository, SearchIndex, SpanStore};
use sideseat_ports::types::{NormalizedSpan, ProjectId, SearchSignal};
use tempfile::TempDir;

use super::DedupAnalyticsRepository;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().expect("fixed")
    }
}

/// A wrapped DuckDB store, as the server assembles it.
async fn wrapped() -> (TempDir, DedupAnalyticsRepository) {
    let root = TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(root.path().to_path_buf());
    let service = DuckdbService::init(&storage, Arc::new(TestClock))
        .await
        .expect("duckdb");
    let inner: Box<dyn AnalyticsRepository + Send + Sync> =
        Box::new(DuckdbRepository(Arc::new(service)));
    (root, DedupAnalyticsRepository::new(inner))
}

fn span(trace: &str, id: &str, prompt: &str, bytes: u64) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("project".to_string()),
        trace_id: trace.to_string(),
        span_id: id.to_string(),
        span_name: "generation".to_string(),
        timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).single().expect("fixed"),
        ingested_at: Some(Utc.timestamp_opt(1_700_000_000, 0).single().expect("fixed")),
        input_preview: Some(prompt.to_string()),
        content_digest: format!("digest-{trace}-{id}"),
        logical_bytes: bytes,
        ..Default::default()
    }
}

/// The search backfill reaches rows with no index entry and indexes them, through the wrapper.
#[tokio::test]
async fn the_search_backfill_indexes_through_the_wrapper() {
    let (_root, store) = wrapped().await;
    let project = ProjectId::from("project");
    // Written without search terms, as a row stored before the index existed would be.
    store
        .insert_spans(vec![span("trace", "span", "hello backfill", 100)])
        .await
        .expect("insert");

    let pending = store
        .search_backfill_page(&project, SearchSignal::Spans, 8)
        .await
        .expect("the wrapper forwards the backfill page");
    assert_eq!(
        pending.len(),
        1,
        "the stored span has no index entry, so the backfill must offer it"
    );

    let indexed = crate::search::SearchService::backfill_project_page(
        &store,
        &project,
        SearchSignal::Spans,
        8,
    )
    .await
    .expect("the backfill writes through the wrapper");
    assert_eq!(indexed, 1);

    assert!(
        store
            .search_backfill_page(&project, SearchSignal::Spans, 8)
            .await
            .expect("second page")
            .is_empty(),
        "an indexed row must not be offered again, or the backfill never finishes"
    );
}

/// Pressure reclamation sees candidates through the wrapper: the path that keeps a project inside its budget.
#[tokio::test]
async fn pressure_reclamation_finds_candidates_through_the_wrapper() {
    let (_root, store) = wrapped().await;
    let project = ProjectId::from("project");
    store
        .insert_spans(vec![
            span("trace", "old", "first", 4_000),
            span("trace", "new", "second", 4_000),
        ])
        .await
        .expect("insert");

    let candidates = store
        .oldest_reclaimable_spans(&project, 4_000, TestClock.now(), 8)
        .await
        .expect("the wrapper forwards the candidate selection");
    assert!(
        !candidates.is_empty(),
        "a project over its target must offer something to reclaim, or the budget cannot be enforced"
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.trace_id == "trace"),
        "candidates belong to the project asked about: {candidates:?}"
    );
}
