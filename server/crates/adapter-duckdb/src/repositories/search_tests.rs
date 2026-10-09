use std::sync::Arc;

use super::*;
use crate::DuckdbService;
use sideseat_core::storage::AppStorage;
use sideseat_ports::types::{ProjectId, SearchExpr, SearchField, SearchFieldTerms};

async fn service() -> (tempfile::TempDir, DuckdbService) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("duckdb")).expect("duckdb dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let service = DuckdbService::init(&storage, Arc::new(crate::TestClock))
        .await
        .expect("service");
    (temp, service)
}

fn prompt(text: &str, truncated: bool) -> SearchDocument {
    SearchDocument {
        indexed: true,
        fields: vec![SearchFieldTerms {
            field: SearchField::Prompt,
            terms: text.split(' ').map(str::to_string).collect(),
            truncated,
            text: text.to_string(),
        }],
    }
}

fn span(span_id: &str, start_s: i64, search: SearchDocument) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("p".into()),
        trace_id: "t".into(),
        span_id: span_id.into(),
        span_name: "step".into(),
        content_digest: format!("digest-{span_id}"),
        timestamp_start: chrono::DateTime::from_timestamp(1_800_000_000 + start_s, 0).unwrap(),
        ingested_at: chrono::DateTime::from_timestamp(1_800_000_000, 0),
        search,
        ..Default::default()
    }
}

fn query(term: &str) -> SearchQuery {
    SearchQuery {
        project_id: ProjectId::from("p"),
        signal: SearchSignal::Spans,
        expression: SearchExpr::Term {
            field: Some(SearchField::Prompt),
            term: term.into(),
        },
        limit: 50,
        max_examined: 100,
        cursor: None,
        from_timestamp: None,
        to_timestamp: None,
    }
}

fn candidates(conn: &Connection, term: &str) -> Vec<(String, bool)> {
    search(conn, &query(term))
        .expect("search")
        .candidates
        .into_iter()
        .map(|candidate| {
            let SearchRecord::Span(record) = candidate.record else {
                panic!("a span search returns spans");
            };
            (record.span_id, candidate.document.indexed)
        })
        .collect()
}

/// A span is a candidate when the term is in the field (a match), when the field's terms were cut short or the
/// span is not indexed yet (unknown), and not when the field is indexed whole without it - the field's state read
/// from the span row, the term from the term table. The backfill indexes a span by setting its row's state too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn field_state_comes_from_the_row_and_the_backfill_sets_it() {
    let (_temp, service) = service().await;
    let spans = vec![
        span("matched", 4, prompt("alpha beta", false)),
        span("truncated", 3, prompt("gamma", true)),
        span("absent", 2, prompt("gamma", false)),
        span("unindexed", 1, SearchDocument::default()),
    ];
    service
        .write(|conn| crate::repositories::span::insert_batch(conn, &spans))
        .expect("write");
    {
        let conn = service.conn();
        assert_eq!(
            candidates(&conn, "alpha"),
            vec![
                ("matched".to_string(), false),
                ("truncated".to_string(), false),
                ("unindexed".to_string(), false),
            ],
            "indexed reports every field: these documents carry only the prompt"
        );
        let empty: u64 = conn
            .query_row(
                "SELECT count(*) FROM span_terms WHERE term = ''",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(
            empty, 0,
            "an indexed field's state is on the row, not an empty term"
        );
    }

    let backfill = SearchBackfillDocument {
        id: SearchRecordId::Span {
            trace_id: "t".into(),
            span_id: "unindexed".into(),
        },
        expected_content_digest: Some("digest-unindexed".into()),
        document: prompt("delta", false),
    };
    service
        .write(|conn| {
            write_backfill(
                conn,
                "p",
                SearchSignal::Spans,
                std::slice::from_ref(&backfill),
            )
        })
        .expect("backfill");
    let conn = service.conn();
    assert_eq!(
        candidates(&conn, "alpha")
            .into_iter()
            .map(|(span_id, _)| span_id)
            .collect::<Vec<_>>(),
        vec!["matched".to_string(), "truncated".to_string()],
        "the backfilled span's prompt is indexed whole and lacks the term"
    );
    assert_eq!(
        candidates(&conn, "delta")
            .into_iter()
            .map(|(span_id, _)| span_id)
            .collect::<Vec<_>>(),
        vec!["truncated".to_string(), "unindexed".to_string()],
    );
}

/// A ranged search answers from its range alone, its term matches read for the range's candidates: the spans
/// that hold the term inside it, and none outside it however many do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ranged_search_reads_its_term_matches_for_the_range() {
    let (_temp, service) = service().await;
    let spans: Vec<NormalizedSpan> = (0..40)
        .map(|second| span(&format!("s{second:02}"), second, prompt("common", false)))
        .chain([span("other", 15, prompt("rare", false))])
        .collect();
    service
        .write(|conn| crate::repositories::span::insert_batch(conn, &spans))
        .expect("write");
    let conn = service.conn();
    let at = |second: i64| chrono::DateTime::from_timestamp(1_800_000_000 + second, 0);
    let found = |term: &str| -> Vec<String> {
        search(
            &conn,
            &SearchQuery {
                from_timestamp: at(10),
                to_timestamp: at(19),
                ..query(term)
            },
        )
        .expect("search")
        .candidates
        .into_iter()
        .map(|candidate| match candidate.record {
            SearchRecord::Span(record) => record.span_id,
            _ => panic!("a span search returns spans"),
        })
        .collect()
    };
    assert_eq!(
        found("common"),
        (10..20)
            .rev()
            .map(|second| format!("s{second:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(found("rare"), vec!["other".to_string()]);
}
