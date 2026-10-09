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

/// DuckDB reads a search's term rows for its candidates alone - the winners its time range and cursor admit -
/// so a narrow range does not aggregate every record of the project that holds a term. The values bind in the
/// order the statement names them.
#[test]
fn term_matches_are_read_for_the_candidates_only() {
    let term = SearchExpr::Term {
        field: Some(SearchField::Prompt),
        term: "common".to_string(),
    };
    for signal in [SearchSignal::Spans, SearchSignal::Logs] {
        let query = request(term.clone(), signal);
        let scored = candidates(&query, Backend::Duckdb).query;
        let through = SearchCursor {
            timestamp_us: 42,
            tie_breaker: "trace\0span".to_string(),
            ordinal: 0,
            started_at_us: 30,
        };
        let missed = arrivals(&query, &through, Backend::Duckdb);
        for statement in [&scored, &missed] {
            let sql = statement.sql();
            let scope = sql
                .find(", candidates AS (SELECT * FROM winners r WHERE ")
                .expect("candidates");
            let hits = sql.find(", hits AS (").expect("hits");
            assert!(scope < hits, "{sql}");
            assert!(sql[hits..].contains("FROM candidates)"), "{sql}");
            assert!(sql.contains("FROM candidates r LEFT JOIN hits h"), "{sql}");
            assert_eq!(sql.matches('?').count(), statement.params().len(), "{sql}");
            // The term binds after the range and cursor the candidates read, and before nothing it precedes.
            let at = statement
                .params()
                .iter()
                .position(|value| matches!(value, QueryValue::String(text) if text == "common"))
                .expect("the term is bound");
            let range = statement
                .params()
                .iter()
                .position(|value| matches!(value, QueryValue::Int64(1)))
                .expect("the range is bound");
            assert!(range < at, "{sql}");
        }
        let clickhouse = candidates(&query, Backend::Clickhouse).query;
        assert!(!clickhouse.sql().contains("candidates AS"));
    }
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
fn duckdb_uses_term_relations_and_clickhouse_uses_native_token_functions() {
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
    assert!(duckdb.query.sql().contains("FROM span_terms"));
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
