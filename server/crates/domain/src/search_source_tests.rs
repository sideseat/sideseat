//! A search reads its page's text a bounded chunk at a time: within DuckDB's memory limit whatever the page holds,
//! and a row that changes between the page and its chunk is a non-match, never an error.

use super::*;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{EntityQuery, SpanStore};
use sideseat_ports::types::{ProjectId, SearchCursor, SearchQuery, SearchSignal, SearchSourceKey};
use tempfile::TempDir;

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_100, 0).unwrap()
    }
}

const PROJECT: &str = "chunks";

async fn store() -> (TempDir, Arc<DuckdbService>, DuckdbRepository) {
    let directory = TempDir::new().unwrap();
    let storage = AppStorage::init_for_test(directory.path().to_path_buf());
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(FixedClock))
            .await
            .unwrap(),
    );
    let repository = DuckdbRepository(Arc::clone(&service));
    (directory, service, repository)
}

/// `count` spans, one trace each, at one instant, each naming `needle` in a stack trace of `bytes` bytes.
fn spans(count: usize, bytes: usize) -> Vec<NormalizedSpan> {
    let timestamp = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut spans: Vec<NormalizedSpan> = (0..count)
        .map(|n| {
            // Distinct text per span, in words a few kilobytes long, so neither compression nor the tokenizer
            // makes it small.
            let word = format!("{n:08x}").repeat(512);
            let mut text = String::from("needle");
            while text.len() < bytes {
                text.push(' ');
                text.push_str(&word);
            }
            NormalizedSpan {
                project_id: Some(PROJECT.into()),
                trace_id: format!("trace-{n:03}"),
                span_id: format!("span-{n:03}"),
                span_name: "large".into(),
                timestamp_start: timestamp,
                logical_bytes: text.len() as u64,
                exception_stacktrace: Some(text),
                ingested_at: Some(timestamp),
                ..Default::default()
            }
        })
        .collect();
    index_spans(&mut spans);
    spans
}

fn query(limit: u32) -> SearchQuery {
    SearchQuery {
        project_id: ProjectId::from(PROJECT),
        signal: SearchSignal::Spans,
        expression: parse("needle", SearchSignal::Spans).unwrap(),
        limit,
        max_examined: 10_000,
        cursor: None,
        from_timestamp: None,
        to_timestamp: None,
    }
}

/// A page whose text is several times DuckDB's memory limit is searched within it. Read whole and ordered with its
/// text, as the page once was, it failed with an out-of-memory error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_of_large_texts_is_searched_within_the_memory_limit() {
    let (_directory, service, repository) = store().await;
    repository.insert_spans(spans(40, 3 << 20)).await.unwrap();
    service
        .conn()
        .execute_batch("CHECKPOINT; SET memory_limit = '64MB'")
        .unwrap();

    let page = SearchService::execute(&repository, &query(100))
        .await
        .expect("a search within the memory limit");
    assert_eq!(page.hits.len(), 40);
    assert_eq!(page.examined, 40);
}

/// The repository, but the first source read deletes one candidate's span, supersedes another's, and deletes and
/// writes a third again before it reads - as a concurrent delete, a correction and a redelivery after a delete
/// landing between the page and its chunk would.
struct ChangingUnderneath {
    inner: DuckdbRepository,
    changed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl SearchIndex for ChangingUnderneath {
    async fn search(&self, query: &SearchQuery) -> Result<SearchPage, DataError> {
        self.inner.search(query).await
    }

    async fn search_sources(
        &self,
        query: &SearchQuery,
        keys: &[SearchSourceKey],
    ) -> Result<Vec<Option<SearchSource>>, DataError> {
        if !self.changed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.inner
                .delete_traces(&ProjectId::from(PROJECT), &["trace-001".to_string()])
                .await?;
            // Deleted, checkpointed and written again: the new revision can take the row number the page found,
            // and is a different revision all the same.
            self.inner
                .delete_traces(&ProjectId::from(PROJECT), &["trace-004".to_string()])
                .await?;
            self.inner
                .0
                .conn()
                .execute_batch("CHECKPOINT")
                .map_err(|error| DataError::Conflict(error.to_string()))?;
            let mut rewritten = spans(5, 64).remove(4);
            rewritten.ingested_at = Some(DateTime::from_timestamp(1_700_000_060, 0).unwrap());
            rewritten.exception_stacktrace = Some("needle, written again".into());
            let mut rewritten = vec![rewritten];
            index_spans(&mut rewritten);
            self.inner.insert_spans(rewritten).await?;
            let mut correction = spans(3, 64).remove(2);
            correction.ingested_at = Some(DateTime::from_timestamp(1_700_000_050, 0).unwrap());
            correction.exception_stacktrace = Some("corrected, no longer a match".into());
            let mut corrected = vec![correction];
            index_spans(&mut corrected);
            self.inner.insert_spans(corrected).await?;
        }
        self.inner.search_sources(query, keys).await
    }

    async fn search_arrivals_detected(
        &self,
        query: &SearchQuery,
        through: &SearchCursor,
    ) -> Result<bool, DataError> {
        self.inner.search_arrivals_detected(query, through).await
    }

    async fn search_backfill_page(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        limit: usize,
    ) -> Result<Vec<sideseat_ports::types::SearchBackfillSource>, DataError> {
        self.inner
            .search_backfill_page(project_id, signal, limit)
            .await
    }

    async fn write_search_backfill(
        &self,
        project_id: &ProjectId,
        signal: SearchSignal,
        documents: &[SearchBackfillDocument],
    ) -> Result<(), DataError> {
        self.inner
            .write_search_backfill(project_id, signal, documents)
            .await
    }
}

/// A candidate deleted, one superseded and one written again between the page and the read of its text are each
/// skipped as a non-match: no error, and still examined, so `examined` means what it meant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_gone_before_its_text_is_read_is_examined_and_skipped() {
    let (_directory, _service, repository) = store().await;
    repository.insert_spans(spans(5, 64)).await.unwrap();
    let changing = ChangingUnderneath {
        inner: repository,
        changed: std::sync::atomic::AtomicBool::new(false),
    };

    let page = SearchService::execute(&changing, &query(100))
        .await
        .expect("no error for rows that changed underneath");
    let hits: Vec<String> = page
        .hits
        .iter()
        .map(|hit| match &hit.record {
            SearchRecord::Span(span) => span.trace_id.clone(),
            SearchRecord::Log(_) => panic!("a log"),
        })
        .collect();
    assert_eq!(
        hits,
        ["trace-000", "trace-003"],
        "a revision written after the page is not this page's, wherever it is stored"
    );
    assert_eq!(
        page.examined, 5,
        "the deleted, the superseded and the rewritten were examined"
    );
}

/// A key answers only for its own revision. A row number DuckDB gives again once its row is gone, to the same
/// span written anew, would present exactly this: the identity the key names at the number it names, at another
/// revision - so its text is not the page's to read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_answers_only_for_its_own_revision() {
    let (_directory, _service, repository) = store().await;
    repository.insert_spans(spans(3, 64)).await.unwrap();
    let page = repository.search(&query(100)).await.unwrap();
    let mut keys: Vec<SearchSourceKey> = page
        .candidates
        .iter()
        .map(|candidate| candidate.key.clone())
        .collect();
    keys[1].revision_us += 1;
    keys[2].content_digest.push('x');
    let sources = repository.search_sources(&query(100), &keys).await.unwrap();
    assert_eq!(
        sources.iter().map(Option::is_some).collect::<Vec<_>>(),
        [true, false, false],
        "the right revision is read; another instant or another content at the same row is not"
    );
}
