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
    let tuples = std::iter::repeat_n("(?, ?, ?)", records.len())
        .collect::<Vec<_>>()
        .join(", ");
    let source = match backend {
        Backend::Duckdb => {
            "(SELECT * FROM otel_spans QUALIFY ROW_NUMBER() OVER (\
             PARTITION BY project_id, trace_id, span_id \
             ORDER BY ingested_at DESC, rowid DESC) = 1)"
        }
        Backend::Clickhouse => "otel_spans FINAL",
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics backend", backend.name())
        }
    };
    let mut params = vec![QueryValue::String(project_id.to_string())];
    for (trace_id, span_id, digest) in &records {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
        params.push(QueryValue::String(digest.clone()));
    }
    Some(ConfirmationQuery {
        query: ParameterizedQuery::new(
            format!(
                "SELECT count() FROM {source} \
                 WHERE project_id = ? AND (trace_id, span_id, content_digest) IN ({tuples}){}",
                sequential_consistency(backend)
            ),
            params,
        ),
        expected: records.len() as u64,
    })
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
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics backend", backend.name())
        }
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
        Backend::Sqlite | Backend::Postgres => {
            panic!("{} is not an analytics backend", backend.name())
        }
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
        Backend::Sqlite | Backend::Postgres => unreachable!(),
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
}
