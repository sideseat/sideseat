use super::*;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::SpanStore;
use sideseat_ports::types::{ProjectId, SearchQuery, SearchSignal};
use tempfile::TempDir;

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_100, 0).unwrap()
    }
}

#[test]
fn tokenizer_contract_covers_unicode_code_json_cjk_and_diacritics() {
    assert_eq!(
        tokenize("Crème brûlée, HTTP_2 foo.bar 中文 JSON:{\"a\":1}"),
        [
            "crème", "brûlée", "http", "2", "foo", "bar", "中文", "json", "a", "1"
        ]
    );
}

#[test]
fn parser_preserves_field_boolean_and_exact_phrase_structure() {
    let parsed = parse(
        r#"prompt:"hello world" AND NOT (error:timeout OR tool_name:delete)"#,
        SearchSignal::Spans,
    )
    .unwrap();
    assert!(matches!(parsed, SearchExpr::And(_, _)));
}

#[test]
fn truncated_negation_is_unknown_under_nesting() {
    let document = SearchDocument {
        indexed: true,
        fields: vec![SearchFieldTerms {
            field: SearchField::Prompt,
            terms: vec!["a".into()],
            truncated: true,
            text: "a z".into(),
        }],
    };
    let expression = SearchExpr::Not(Box::new(SearchExpr::Or(
        Box::new(SearchExpr::Term {
            field: Some(SearchField::Prompt),
            term: "missing".into(),
        }),
        Box::new(SearchExpr::Term {
            field: Some(SearchField::Prompt),
            term: "also_missing".into(),
        }),
    )));
    assert_eq!(
        evaluate(&expression, &document, SearchSignal::Spans, true),
        Truth::Unknown
    );
}

#[test]
fn phrase_is_verified_consecutively() {
    let document = SearchDocument {
        indexed: true,
        fields: vec![field_terms(SearchField::Prompt, "foo x bar foo bar".into())],
    };
    let expression = SearchExpr::Phrase {
        field: Some(SearchField::Prompt),
        phrase: "foo bar".into(),
        terms: vec!["foo".into(), "bar".into()],
    };
    assert_eq!(
        evaluate(&expression, &document, SearchSignal::Spans, true),
        Truth::True
    );
}

#[test]
fn cap_and_recall_floor_are_explicit() {
    let source = (0..SEARCH_TERMS_PER_FIELD + 10)
        .map(|index| format!("term{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let indexed = field_terms(SearchField::Prompt, source);
    assert!(indexed.truncated);
    assert_eq!(indexed.terms.len(), SEARCH_TERMS_PER_FIELD);
}

#[tokio::test]
async fn duckdb_search_replaces_terms_and_cursor_advances_over_empty_pages() {
    let directory = TempDir::new().unwrap();
    let storage = AppStorage::init_for_test(directory.path().to_path_buf());
    let service = Arc::new(
        DuckdbService::init(&storage, Arc::new(FixedClock))
            .await
            .unwrap(),
    );
    let repository = DuckdbRepository(service);
    let timestamp = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut spans = vec![
        NormalizedSpan {
            project_id: Some("search-project".into()),
            trace_id: "trace-a".into(),
            span_id: "span-a".into(),
            span_name: "first".into(),
            timestamp_start: timestamp,
            input_preview: Some("alpha x beta".into()),
            ingested_at: Some(timestamp),
            ..Default::default()
        },
        NormalizedSpan {
            project_id: Some("search-project".into()),
            trace_id: "trace-b".into(),
            span_id: "span-b".into(),
            span_name: "second".into(),
            timestamp_start: timestamp,
            input_preview: Some("alpha beta".into()),
            ingested_at: Some(timestamp),
            ..Default::default()
        },
    ];
    index_spans(&mut spans);
    repository.insert_spans(spans).await.unwrap();

    let expression = parse(r#"prompt:"alpha beta""#, SearchSignal::Spans).unwrap();
    let first = SearchService::execute(
        &repository,
        &SearchQuery {
            project_id: ProjectId::from("search-project"),
            signal: SearchSignal::Spans,
            expression: expression.clone(),
            limit: 1,
            max_examined: 1,
            cursor: None,
            from_timestamp: None,
            to_timestamp: None,
        },
    )
    .await
    .unwrap();
    assert!(first.hits.is_empty());
    assert_eq!(first.examined, 1);
    let cursor = first.next_cursor.expect("empty page still advances");

    let second = SearchService::execute(
        &repository,
        &SearchQuery {
            project_id: ProjectId::from("search-project"),
            signal: SearchSignal::Spans,
            expression,
            limit: 1,
            max_examined: 1,
            cursor: Some(cursor),
            from_timestamp: None,
            to_timestamp: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(second.hits.len(), 1);
    assert!(matches!(
        &second.hits[0].record,
        SearchRecord::Span(row) if row.trace_id == "trace-b"
    ));

    let mut correction = NormalizedSpan {
        project_id: Some("search-project".into()),
        trace_id: "trace-b".into(),
        span_id: "span-b".into(),
        span_name: "second".into(),
        timestamp_start: timestamp,
        input_preview: Some("gamma delta".into()),
        ingested_at: DateTime::from_timestamp(1_700_000_001, 0),
        ..Default::default()
    };
    index_spans(std::slice::from_mut(&mut correction));
    repository.insert_spans(vec![correction]).await.unwrap();

    let old = SearchService::execute(
        &repository,
        &SearchQuery {
            project_id: ProjectId::from("search-project"),
            signal: SearchSignal::Spans,
            expression: parse("prompt:beta", SearchSignal::Spans).unwrap(),
            limit: 10,
            max_examined: 10,
            cursor: None,
            from_timestamp: None,
            to_timestamp: None,
        },
    )
    .await
    .unwrap();
    assert!(old.hits.iter().all(|hit| {
        !matches!(
            &hit.record,
            SearchRecord::Span(row) if row.trace_id == "trace-b"
        )
    }));
    let corrected = SearchService::execute(
        &repository,
        &SearchQuery {
            project_id: ProjectId::from("search-project"),
            signal: SearchSignal::Spans,
            expression: parse("prompt:gamma", SearchSignal::Spans).unwrap(),
            limit: 10,
            max_examined: 10,
            cursor: None,
            from_timestamp: None,
            to_timestamp: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(corrected.hits.len(), 1);
}
