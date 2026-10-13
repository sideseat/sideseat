use chrono::{TimeZone, Utc};
use sideseat_ports::types::{ProjectId, SearchQuery};

use super::*;

fn request(expression: SearchExpr, signal: SearchSignal) -> SearchQuery {
    SearchQuery {
        project_id: ProjectId::from("p"),
        signal,
        expression,
        limit: 10,
        max_examined: 7,
        cursor: Some(SearchCursor {
            timestamp_us: 42,
            tie_breaker: "trace\0span".to_string(),
            ordinal: 0,
            started_at_us: 30,
        }),
        from_timestamp: Some(Utc.timestamp_micros(1).single().unwrap()),
        to_timestamp: Some(Utc.timestamp_micros(100).single().unwrap()),
    }
}

/// A document's bits name every field it carries, an empty one included, and only the truncated ones as
/// truncated; a document never indexed has none, so its record waits for the backfill.
#[test]
fn document_bits_name_the_indexed_and_the_truncated_fields() {
    use sideseat_ports::types::{SearchDocument, SearchFieldTerms};
    let field = |field, terms: &[&str], truncated| SearchFieldTerms {
        field,
        terms: terms.iter().map(|term| term.to_string()).collect(),
        truncated,
        text: String::new(),
    };
    let document = SearchDocument {
        indexed: true,
        fields: vec![
            field(SearchField::Prompt, &["a"], true),
            field(SearchField::Error, &[], false),
            field(SearchField::SpanName, &["b"], false),
        ],
    };
    assert_eq!(
        duckdb_document_bits(SearchSignal::Spans, &document),
        (0b11_0001, 0b00_0001)
    );
    let unindexed = SearchDocument {
        indexed: false,
        ..document
    };
    assert_eq!(
        duckdb_document_bits(SearchSignal::Spans, &unindexed),
        (0, 0)
    );
    assert_eq!(duckdb_every_field(SearchSignal::Spans), 63);
    assert_eq!(duckdb_every_field(SearchSignal::Logs), 15);
    assert_eq!(duckdb_field_bit(SearchSignal::Logs, SearchField::Prompt), 0);
}

/// DuckDB reads a record's terms from its own row, a field's column holding each term between spaces, so a term
/// matches only whole: no term table is joined, and the term binds spaced - for a span as for a log.
#[test]
fn a_record_s_terms_are_read_from_its_own_row() {
    for (signal, field, column) in [
        (SearchSignal::Logs, SearchField::Body, "search_body"),
        (SearchSignal::Spans, SearchField::Prompt, "search_prompt"),
    ] {
        let query = request(
            SearchExpr::Term {
                field: Some(field),
                term: "common".to_string(),
            },
            signal,
        );
        let through = SearchCursor {
            timestamp_us: 42,
            tie_breaker: "trace\0span".to_string(),
            ordinal: 0,
            started_at_us: 30,
        };
        for sql in [
            candidates(&query, Backend::Duckdb).query,
            arrivals(&query, &through, Backend::Duckdb),
        ] {
            assert!(!sql.sql().contains("hits AS"), "{}", sql.sql());
            assert!(!sql.sql().contains("_terms"), "{}", sql.sql());
            assert!(
                sql.sql()
                    .contains(&format!("contains(coalesce(r.{column}, ''), ?)")),
                "{}",
                sql.sql()
            );
            assert!(
                sql.params()
                    .iter()
                    .any(|value| matches!(value, QueryValue::String(text) if text == " common "))
            );
            assert_eq!(sql.sql().matches('?').count(), sql.params().len());
        }
    }
    assert_eq!(
        crate::search::duckdb_term_column(&["ab".to_string(), "abc".to_string()]).as_deref(),
        Some(" ab abc ")
    );
    assert_eq!(crate::search::duckdb_term_column(&[]), None);
}

#[test]
fn negated_phrase_stays_a_candidate_on_both_backends() {
    let expression = SearchExpr::Not(Box::new(SearchExpr::Phrase {
        field: Some(SearchField::Prompt),
        phrase: "foo bar".to_string(),
        terms: vec!["foo".to_string(), "bar".to_string()],
    }));
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let plan = candidates(&request(expression.clone(), SearchSignal::Spans), backend);
        assert!(plan.query.sql().contains("2 -"));
        assert!(plan.query.sql().contains("match_state != 0"));
        assert!(plan.query.sql().contains("LIMIT 8"));
    }
}

#[test]
fn duckdb_reads_terms_on_the_row_and_clickhouse_uses_native_token_functions() {
    let expression = SearchExpr::And(
        Box::new(SearchExpr::Term {
            field: Some(SearchField::Prompt),
            term: "foo".to_string(),
        }),
        Box::new(SearchExpr::Term {
            field: Some(SearchField::Completion),
            term: "bar".to_string(),
        }),
    );
    let duckdb = candidates(
        &request(expression.clone(), SearchSignal::Spans),
        Backend::Duckdb,
    );
    let sql = duckdb.query.sql();
    assert!(!sql.contains("span_terms"), "{sql}");
    assert!(
        sql.contains("contains(coalesce(r.search_prompt, ''), ?)"),
        "{sql}"
    );
    assert!(
        sql.contains("contains(coalesce(r.search_completion, ''), ?)"),
        "{sql}"
    );
    assert_eq!(sql.matches('?').count(), duckdb.query.params().len());
    assert!(duckdb.query.sql().contains("least("));
    let clickhouse = candidates(
        &request(expression, SearchSignal::Spans),
        Backend::Clickhouse,
    );
    assert!(
        clickhouse
            .query
            .sql()
            .contains("hasAllTokens(r.search_prompt")
    );
    assert!(
        clickhouse
            .query
            .sql()
            .contains("hasAllTokens(r.search_completion")
    );
}

#[test]
fn indexing_completeness_covers_the_whole_time_range() {
    let query = request(SearchExpr::MatchAll, SearchSignal::Spans);
    for backend in [Backend::Duckdb, Backend::Clickhouse] {
        let plan = indexing_complete(&query, backend);
        assert!(!plan.sql().contains("trace_id >"));
        assert!(plan.sql().contains("COUNT(*) = 0"));
        assert!(plan.sql().contains("timestamp_start"));
    }
    assert!(
        indexing_complete(&query, Backend::Duckdb)
            .sql()
            .contains("r.search_fields != 63")
    );
    assert!(
        indexing_complete(&query, Backend::Clickhouse)
            .sql()
            .contains("r.search_indexed = 0")
    );
}

#[test]
fn backfill_pages_only_unindexed_current_rows_in_identity_order() {
    let duckdb = backfill_sources("p", SearchSignal::Spans, 256, Backend::Duckdb);
    assert!(duckdb.sql().contains("r.search_fields != 63"));
    assert!(duckdb.sql().contains("ORDER BY trace_id, span_id"));
    assert!(duckdb.sql().contains("LIMIT ?"));

    let clickhouse = backfill_sources("p", SearchSignal::Logs, 256, Backend::Clickhouse);
    assert!(clickhouse.sql().contains("FROM otel_logs FINAL"));
    assert!(clickhouse.sql().contains("r.search_indexed = 0"));
    assert!(clickhouse.sql().contains("ORDER BY log_digest, ordinal"));
}

/// A page carries no text: its statement reads none of the source columns, on either backend. The source read binds
/// every placeholder, reads DuckDB rows by number with their revision for the caller to check, and on ClickHouse
/// narrows to the identities still at the key's revision before it reads any text.
#[test]
fn a_page_carries_no_text_and_its_sources_are_read_by_revision() {
    use sideseat_ports::types::{SearchRecordId, SearchSourceKey};
    for signal in [SearchSignal::Spans, SearchSignal::Logs] {
        let query = request(SearchExpr::MatchAll, signal);
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = candidates(&query, backend).query;
            let text = match signal {
                SearchSignal::Spans => ["r.messages", "r.exception_stacktrace", "r.span_name"],
                SearchSignal::Logs => ["r.raw_log", "r.body", "r.severity_text"],
            };
            for column in text {
                let selected = page.sql().matches(column).count();
                assert_eq!(selected, 0, "{column} in a page: {}", page.sql());
            }
            assert!(page.sql().contains("logical_bytes"), "{}", page.sql());
            let id = match signal {
                SearchSignal::Spans => SearchRecordId::Span {
                    trace_id: "t".into(),
                    span_id: "s".into(),
                },
                SearchSignal::Logs => SearchRecordId::Log {
                    log_digest: "d".into(),
                    ordinal: 0,
                },
            };
            let keys = [SearchSourceKey {
                id,
                row_id: Some(7),
                timestamp_us: 1_700_000_000_000_000,
                revision_us: 42,
                content_digest: "digest".into(),
            }];
            let sources = sources("project", signal, &keys, backend).expect("a read");
            assert_eq!(
                sources.sql().matches('?').count(),
                sources.params().len(),
                "{}",
                sources.sql()
            );
            assert!(sources.sql().contains("revision_us"), "{}", sources.sql());
            if backend == Backend::Clickhouse {
                assert!(
                    sources.sql().contains("revision_us, content_digest) IN"),
                    "{}",
                    sources.sql()
                );
            }
        }
    }
}
