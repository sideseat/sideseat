//! Strict producer-content confirmation reads for staged OTLP deliveries.

use std::collections::BTreeSet;

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmationQuery {
    pub query: ParameterizedQuery,
    pub expected: u64,
}

pub fn spans(
    project_id: &str,
    records: &[(String, String, String)],
    backend: Backend,
) -> Option<ConfirmationQuery> {
    let records: BTreeSet<_> = records.iter().cloned().collect();
    if records.is_empty() {
        return None;
    }
    let expected = records.len() as u64;
    let query = winning_spans_with_digest(project_id, &records, "count()", backend);
    Some(ConfirmationQuery { query, expected })
}

/// The `(trace_id, span_id, content_digest)` records whose winning revision carries exactly that digest.
///
/// One query for a whole batch. The redelivery check used [`spans`] for the batch and fell back to one
/// query per span whenever the batch was mixed - which is every batch of new spans - so a request of N
/// spans ran N+1 queries, each over the whole table.
pub fn matching_spans(
    project_id: &str,
    records: &[(String, String, String)],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    let records: BTreeSet<_> = records.iter().cloned().collect();
    if records.is_empty() {
        return None;
    }
    Some(winning_spans_with_digest(
        project_id,
        &records,
        "trace_id, span_id, content_digest",
        backend,
    ))
}

/// `SELECT {select}` over the winning revisions of the requested identities whose digest matches.
///
/// DuckDB selects winners **after** restricting to the requested identities and only the three columns the
/// comparison needs. Ranking first, as this did, numbered every revision of every span in the table and
/// materialised all of their columns, so each confirmation cost time proportional to the whole table. The
/// winner of an identity depends only on that identity's revisions, so filtering first is the same answer.
/// ClickHouse's `FINAL` already merges per sorting key, and the identities are a prefix of it.
fn winning_spans_with_digest(
    project_id: &str,
    records: &BTreeSet<(String, String, String)>,
    select: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let triples = std::iter::repeat_n("(?, ?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = vec![QueryValue::String(project_id.to_string())];
    let source = match backend {
        Backend::Duckdb => {
            let identities: BTreeSet<_> = records
                .iter()
                .map(|(trace_id, span_id, _)| (trace_id, span_id))
                .collect();
            let pairs = std::iter::repeat_n("(?, ?)", identities.len())
                .collect::<Vec<_>>()
                .join(", ");
            for (trace_id, span_id) in identities {
                params.push(QueryValue::String(trace_id.clone()));
                params.push(QueryValue::String(span_id.clone()));
            }
            format!(
                "(SELECT trace_id, span_id, content_digest FROM otel_spans \
                 WHERE project_id = ? AND (trace_id, span_id) IN ({pairs}) \
                 QUALIFY ROW_NUMBER() OVER (\
                 PARTITION BY trace_id, span_id ORDER BY ingested_at DESC, rowid DESC) = 1)"
            )
        }
        Backend::Clickhouse => "otel_spans FINAL".to_string(),
    };
    // The project bind is first in both: DuckDB consumes it inside the subquery, ClickHouse here.
    let filter = match backend {
        Backend::Duckdb => "",
        Backend::Clickhouse => "project_id = ? AND ",
    };
    for (trace_id, span_id, digest) in records {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
        params.push(QueryValue::String(digest.clone()));
    }
    ParameterizedQuery::new(
        format!(
            "SELECT {select} FROM {source} \
             WHERE {filter}(trace_id, span_id, content_digest) IN ({triples}){}",
            sequential_consistency(backend)
        ),
        params,
    )
}

pub fn metrics(
    project_id: &str,
    records: &[(String, String)],
    backend: Backend,
) -> Option<ConfirmationQuery> {
    let records: BTreeSet<_> = records.iter().cloned().collect();
    if records.is_empty() {
        return None;
    }
    let tuples = std::iter::repeat_n("(?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let source = match backend {
        Backend::Duckdb => "otel_metrics",
        Backend::Clickhouse => "otel_metrics FINAL",
    };
    let mut params = vec![QueryValue::String(project_id.to_string())];
    for (datapoint_id, digest) in &records {
        params.push(QueryValue::String(datapoint_id.clone()));
        params.push(QueryValue::String(digest.clone()));
    }
    Some(ConfirmationQuery {
        query: ParameterizedQuery::new(
            format!(
                "SELECT count() FROM {source} \
                 WHERE project_id = ? AND (datapoint_id, content_digest) IN ({tuples}){}",
                sequential_consistency(backend)
            ),
            params,
        ),
        expected: records.len() as u64,
    })
}

pub fn logs(
    project_id: &str,
    records: &[(String, u32)],
    backend: Backend,
) -> Option<ConfirmationQuery> {
    let records: BTreeSet<_> = records.iter().cloned().collect();
    if records.is_empty() {
        return None;
    }
    let tuples = std::iter::repeat_n("(?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let source = match backend {
        Backend::Duckdb => "otel_logs",
        Backend::Clickhouse => "otel_logs FINAL",
    };
    let mut params = vec![QueryValue::String(project_id.to_string())];
    for (digest, ordinal) in &records {
        params.push(QueryValue::String(digest.clone()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
    }
    Some(ConfirmationQuery {
        query: ParameterizedQuery::new(
            format!(
                "SELECT count() FROM {source} \
                 WHERE project_id = ? AND (log_digest, ordinal) IN ({tuples}){}",
                sequential_consistency(backend)
            ),
            params,
        ),
        expected: records.len() as u64,
    })
}

fn sequential_consistency(backend: Backend) -> &'static str {
    match backend {
        Backend::Clickhouse => " SETTINGS select_sequential_consistency = 1",
        Backend::Duckdb => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_is_strict_deduplicated_and_sequential_on_clickhouse() {
        let expected = vec![
            (
                "trace".to_string(),
                "span".to_string(),
                "digest".to_string(),
            ),
            (
                "trace".to_string(),
                "span".to_string(),
                "digest".to_string(),
            ),
        ];
        let query = spans("project", &expected, Backend::Clickhouse).unwrap();
        assert_eq!(query.expected, 1);
        assert!(query.query.sql().contains("content_digest"));
        assert!(
            query
                .query
                .sql()
                .ends_with("SETTINGS select_sequential_consistency = 1")
        );
        assert_eq!(query.query.params().len(), 4);
    }
    /// DuckDB ranks only the requested identities' revisions: ranking the whole table first made every
    /// confirmation cost time proportional to everything ever stored.
    #[test]
    fn duckdb_selects_winners_after_restricting_to_the_requested_identities() {
        let records = vec![
            ("t".to_string(), "a".to_string(), "d1".to_string()),
            ("t".to_string(), "a".to_string(), "d2".to_string()),
            ("t".to_string(), "b".to_string(), "d3".to_string()),
        ];
        let query = matching_spans("project", &records, Backend::Duckdb).unwrap();
        let sql = query.sql();
        let filter = sql.find("(trace_id, span_id) IN").unwrap();
        let rank = sql.find("ROW_NUMBER()").unwrap();
        assert!(filter < rank, "{sql}");
        assert!(!sql.contains("SELECT *"), "{sql}");
        // project + two distinct identities + three triples
        assert_eq!(query.params().len(), 1 + 2 * 2 + 3 * 3);
        assert!(sql.starts_with("SELECT trace_id, span_id, content_digest FROM"));
    }
}
