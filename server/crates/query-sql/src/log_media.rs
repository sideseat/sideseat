//! The media references of stored log rows, for the release that must see what a trace's logs still name.
//!
//! DuckDB keeps them in `log_media_refs`, one row per media-bearing log and reference, written with the log rows and
//! deleted with them: `otel_logs` has no trace lookup, and a release reads a trace's references. A redelivery
//! appends its own rows beside the replaced row's, which go when the record's row does - so a file is held no
//! longer than the record lives, and a write never reads the table. Rows exist only for logs that hold media.
//! ClickHouse reads the logs themselves, by the trace id's bloom filter.

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

fn placeholders(count: usize, each: &str) -> String {
    std::iter::repeat_n(each, count)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text of every log field that can hold a `#!B64!#` reference, for the stored log rows of these traces.
///
/// `logs` is the relation the rows are read from: on ClickHouse every replica's (`every_replica_table`), so a log
/// one replica has committed and another has not yet received still keeps its file. Every version is read, not
/// `FINAL`'s: a replaced revision's references can only keep a file a little longer, never release one early. The
/// raw record holds the producer's strings escaped, its own references as written: the caller reads its values
/// unescaped, since a body is stored only where the raw record does not rebuild it, and the derived columns stored
/// beside it. A row is read only if it holds a reference in either spelling - `B64!#` is in both.
pub fn references_for_traces(
    project_id: &str,
    trace_ids: &[String],
    backend: Backend,
    logs: &str,
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let mut params = vec![QueryValue::String(project_id.to_string())];
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    let traces = placeholders(trace_ids.len(), "?");
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT uri FROM log_media_refs WHERE project_id = ? AND trace_id IN ({traces})"
        ),
        // The four columns apart, so the caller reads the derived ones' values only: a producer's key is never a
        // reference, and a scan of the serialised JSON would count one.
        Backend::Clickhouse => format!(
            "SELECT coalesce(raw_log, ''), coalesce(body, ''), coalesce(attributes, ''), coalesce(messages, '') FROM {logs} \
             WHERE project_id = ? AND trace_id IN ({traces}) AND position(concat(coalesce(raw_log, ''), ' ', \
             coalesce(body, ''), ' ', coalesce(attributes, ''), ' ', coalesce(messages, '')), 'B64!#') > 0 \
             SETTINGS skip_unavailable_shards = 0"
        ),
    };
    Some(ParameterizedQuery::new(sql, params))
}

/// One page of the traces whose stored logs hold media, in trace order after `after`.
pub fn media_trace_page(
    project_id: &str,
    after: Option<&str>,
    limit: usize,
    backend: Backend,
    logs: &str,
) -> ParameterizedQuery {
    let mut params = vec![QueryValue::String(project_id.to_string())];
    let after_clause = match after {
        Some(after) => {
            params.push(QueryValue::String(after.to_string()));
            " AND trace_id > ?"
        }
        None => "",
    };
    params.push(QueryValue::Int64(i64::try_from(limit).unwrap_or(i64::MAX)));
    let sql = match backend {
        Backend::Duckdb => format!(
            "SELECT DISTINCT trace_id FROM log_media_refs WHERE project_id = ?{after_clause} \
             ORDER BY trace_id LIMIT ?"
        ),
        Backend::Clickhouse => format!(
            "SELECT DISTINCT assumeNotNull(trace_id) AS trace FROM {logs} WHERE project_id = ? \
             AND isNotNull(trace_id){after_clause} AND position(concat(coalesce(raw_log, ''), ' ', \
             coalesce(body, ''), ' ', coalesce(attributes, ''), ' ', coalesce(messages, '')), 'B64!#') > 0 \
             ORDER BY trace LIMIT ? SETTINGS skip_unavailable_shards = 0"
        ),
    };
    ParameterizedQuery::new(sql, params)
}

/// Remove a redelivery's own reference rows for the references it writes again, so identical deliveries keep one
/// row each, while a reference the redelivery no longer names stays until the record's row goes. Keyed by digest,
/// which the reference table indexes.
pub fn duckdb_delete_rewritten(
    references: &[(String, String, u32, String)],
) -> Option<ParameterizedQuery> {
    if references.is_empty() {
        return None;
    }
    let mut params = Vec::with_capacity(references.len() * 5);
    for (_, digest, _, _) in references {
        params.push(QueryValue::String(digest.clone()));
    }
    for (project_id, digest, ordinal, uri) in references {
        params.push(QueryValue::String(project_id.clone()));
        params.push(QueryValue::String(digest.clone()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
        params.push(QueryValue::String(uri.clone()));
    }
    Some(ParameterizedQuery::new(
        format!(
            "DELETE FROM log_media_refs WHERE log_digest IN ({}) AND (project_id, log_digest, ordinal, uri) IN ({})",
            placeholders(references.len(), "?"),
            placeholders(references.len(), "(?, ?, ?, ?)")
        ),
        params,
    ))
}

/// Remove the references of every log of these traces, as the traces' log rows are deleted.
pub fn duckdb_delete_for_traces(
    project_id: &str,
    trace_ids: &[String],
) -> Option<ParameterizedQuery> {
    if trace_ids.is_empty() {
        return None;
    }
    let mut params = vec![QueryValue::String(project_id.to_string())];
    params.extend(trace_ids.iter().cloned().map(QueryValue::String));
    Some(ParameterizedQuery::new(
        format!(
            "DELETE FROM log_media_refs WHERE project_id = ? AND trace_id IN ({})",
            placeholders(trace_ids.len(), "?")
        ),
        params,
    ))
}

/// Remove the references of every log of these spans, as the spans' log rows are deleted.
pub fn duckdb_delete_for_spans(
    project_id: &str,
    spans: &[(String, String)],
) -> Option<ParameterizedQuery> {
    if spans.is_empty() {
        return None;
    }
    let mut params = vec![QueryValue::String(project_id.to_string())];
    for (trace_id, span_id) in spans {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    Some(ParameterizedQuery::new(
        format!(
            "DELETE FROM log_media_refs WHERE project_id = ? AND (trace_id, span_id) IN ({})",
            placeholders(spans.len(), "(?, ?)")
        ),
        params,
    ))
}

/// Remove every reference of a project, as its data is deleted.
pub fn duckdb_delete_for_project(project_id: &str) -> ParameterizedQuery {
    ParameterizedQuery::new(
        "DELETE FROM log_media_refs WHERE project_id = ?".to_string(),
        vec![QueryValue::String(project_id.to_string())],
    )
}
