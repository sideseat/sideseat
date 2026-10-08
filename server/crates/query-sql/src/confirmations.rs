//! Strict producer-content confirmation reads for staged OTLP deliveries.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};
use crate::keyed::{distinct_keys, duckdb_keyed};

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
/// DuckDB selects winners **after** restricting to the requested identities, which it reads by `span_id` alone
/// through that column's index ([`crate::keyed`]); the winner of an identity depends only on that identity's
/// revisions, so restricting first is the same answer. Ranking first, as this once did, numbered every
/// revision in the table, and filtering by project and identity in one scan read every row. The caller binds
/// at most [`crate::keyed::KEYED_CHUNK`] identities per query. ClickHouse's `FINAL` already merges per sorting
/// key, and the identities are a prefix of it.
fn winning_spans_with_digest(
    project_id: &str,
    records: &BTreeSet<(String, String, String)>,
    select: &str,
    backend: Backend,
) -> ParameterizedQuery {
    let triples = std::iter::repeat_n("(?, ?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::new();
    let source = match backend {
        Backend::Duckdb => {
            let identities: BTreeSet<_> = records
                .iter()
                .map(|(trace_id, span_id, _)| (trace_id, span_id))
                .collect();
            let keys = distinct_keys(identities.iter().map(|(_, span_id)| span_id.as_str()));
            params.extend(
                keys.iter()
                    .map(|key| QueryValue::String((*key).to_string())),
            );
            params.push(QueryValue::String(project_id.to_string()));
            let pairs = std::iter::repeat_n("(?, ?)", identities.len())
                .collect::<Vec<_>>()
                .join(", ");
            for (trace_id, span_id) in identities {
                params.push(QueryValue::String(trace_id.clone()));
                params.push(QueryValue::String(span_id.clone()));
            }
            let keyed = duckdb_keyed(
                "otel_spans",
                "span_id",
                "project_id, trace_id, span_id, content_digest, ingested_at",
                keys.len(),
            );
            format!(
                "(SELECT trace_id, span_id, content_digest FROM {keyed} \
                 WHERE project_id = ? AND (trace_id, span_id) IN ({pairs}) \
                 QUALIFY ROW_NUMBER() OVER (\
                 PARTITION BY trace_id, span_id ORDER BY ingested_at DESC, keyed_rowid DESC) = 1)"
            )
        }
        Backend::Clickhouse => {
            params.push(QueryValue::String(project_id.to_string()));
            "otel_spans FINAL".to_string()
        }
    };
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

/// The `(datapoint_id, content_digest, timestamp)` records, confirmed when each stored datapoint carries that
/// digest.
///
/// A datapoint's instant is part of its identity, so its rows all carry the `timestamp` it was staged with, and
/// rows are appended in roughly the order of their instants. DuckDB therefore reads only the row groups whose
/// `timestamp` range holds the records' range - their zone maps - rather than the table, and needs no index: one
/// on `datapoint_id` measured 85 bytes per point, more than half the per-point target. A correction replaces its
/// row, so there is one row per datapoint to count. ClickHouse reads the datapoints through their skip index.
pub fn metrics(
    project_id: &str,
    records: &[(String, String, DateTime<Utc>)],
    backend: Backend,
) -> Option<ConfirmationQuery> {
    let instants = instant_range(records.iter().map(|(_, _, timestamp)| *timestamp))?;
    let records: BTreeSet<_> = records
        .iter()
        .map(|(datapoint_id, digest, _)| (datapoint_id.clone(), digest.clone()))
        .collect();
    let tuples = std::iter::repeat_n("(?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let (source, mut params) = by_instant("otel_metrics", instants, backend);
    params.push(QueryValue::String(project_id.to_string()));
    for (datapoint_id, digest) in &records {
        params.push(QueryValue::String(datapoint_id.clone()));
        params.push(QueryValue::String(digest.clone()));
    }
    Some(ConfirmationQuery {
        query: ParameterizedQuery::new(
            format!(
                "SELECT count() FROM {source} \
                 project_id = ? AND (datapoint_id, content_digest) IN ({tuples}){}",
                sequential_consistency(backend)
            ),
            params,
        ),
        expected: records.len() as u64,
    })
}

/// The `(log_digest, ordinal, instant)` records, confirmed when each is stored.
///
/// A record's own instant - its time or observed time - is part of its digest, so DuckDB reads only the row
/// groups of the records' instants, as [`metrics`] does. A record carrying no time of its own is stored under its
/// delivery's receipt time, which another delivery replaces, so a list holding one is read unbounded: the
/// caller separates them, and pays the whole-table read only for those.
pub fn logs(
    project_id: &str,
    records: &[(String, u32, Option<DateTime<Utc>>)],
    backend: Backend,
) -> Option<ConfirmationQuery> {
    if records.is_empty() {
        return None;
    }
    let instants = records
        .iter()
        .map(|(_, _, instant)| *instant)
        .collect::<Option<Vec<_>>>()
        .and_then(instant_range);
    let records: BTreeSet<_> = records
        .iter()
        .map(|(digest, ordinal, _)| (digest.clone(), *ordinal))
        .collect();
    let tuples = std::iter::repeat_n("(?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let (source, mut params) = match instants {
        Some(instants) => by_instant("otel_logs", instants, backend),
        None => (
            match backend {
                Backend::Duckdb => "otel_logs WHERE".to_string(),
                Backend::Clickhouse => "otel_logs FINAL WHERE".to_string(),
            },
            Vec::new(),
        ),
    };
    params.push(QueryValue::String(project_id.to_string()));
    for (digest, ordinal) in &records {
        params.push(QueryValue::String(digest.clone()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
    }
    Some(ConfirmationQuery {
        query: ParameterizedQuery::new(
            format!(
                "SELECT count() FROM {source} \
                 project_id = ? AND (log_digest, ordinal) IN ({tuples}){}",
                sequential_consistency(backend)
            ),
            params,
        ),
        expected: records.len() as u64,
    })
}

/// The earliest and latest of `timestamps` in epoch microseconds, `None` when there are none.
pub fn instant_range(timestamps: impl IntoIterator<Item = DateTime<Utc>>) -> Option<(i64, i64)> {
    timestamps
        .into_iter()
        .map(|timestamp| timestamp.timestamp_micros())
        .fold(None, |range, us| match range {
            None => Some((us, us)),
            Some((low, high)) => Some((low.min(us), high.max(us))),
        })
}

/// `table WHERE` with DuckDB's instant bounds in front - the condition its zone maps read - so the caller's
/// conditions follow it; ClickHouse merges `FINAL` and needs no bound.
fn by_instant(table: &str, (low, high): (i64, i64), backend: Backend) -> (String, Vec<QueryValue>) {
    match backend {
        Backend::Duckdb => (
            format!(
                "{table} WHERE \"timestamp\" BETWEEN make_timestamp(?::BIGINT) AND make_timestamp(?::BIGINT) AND"
            ),
            vec![QueryValue::Int64(low), QueryValue::Int64(high)],
        ),
        Backend::Clickhouse => (format!("{table} FINAL WHERE"), Vec::new()),
    }
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
        assert!(!sql.contains("SELECT * FROM otel_spans"), "{sql}");
        // The span ids alone inside the keyed rows, so the scan carries one filter and reads the index.
        assert!(
            sql.contains("FROM otel_spans WHERE span_id IN (?, ?))"),
            "{sql}"
        );
        // two distinct span ids + project + two distinct identities + three triples
        assert_eq!(query.params().len(), 2 + 1 + 2 * 2 + 3 * 3);
        assert!(sql.starts_with("SELECT trace_id, span_id, content_digest FROM"));
    }
}
