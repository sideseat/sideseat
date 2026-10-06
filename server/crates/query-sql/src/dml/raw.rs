//! Statements for the raw-record reconciliation queue and the trace index.
//!
//! Every statement that removes span rows is preceded by one of these, which enqueues the raw records the rows
//! about to go name - the protocol in `server/specs/RawRecordOwnership.tla`. Enqueued first, so a crash between
//! the two leaves a queue entry for a record whose rows still exist, which the reconciler finds unchanged, rather
//! than deleted rows and no entry. Built from the same predicate as the delete it precedes, so the two cannot
//! disagree about which rows they mean.

use chrono::{DateTime, Utc};

use super::{
    DmlStatement, MutationTarget, delete_by_identities, identity_predicate, render_delete,
};
use crate::Backend;
use crate::analytics::{QueryOperation, QueryValue};

/// The table reconciliation entries are written to and read from.
pub const RAW_PENDING_TABLE: &str = "otel_raw_pending";

/// The trace index: which records hold spans of which trace.
pub const RAW_TRACES_TABLE: &str = "otel_raw_traces";

/// Enqueue every record that holds spans of these traces.
///
/// Found through the trace index rather than the span rows: a record whose spans' rows already expired, or
/// whose only naming row was a superseded revision a ClickHouse merge removed, is named by no row, and a
/// deletion must still reach it.
pub fn enqueue_raw_for_traces(
    backend: Backend,
    project_id: &str,
    trace_ids: &[String],
) -> Option<DmlStatement> {
    let (predicate, params) = identity_predicate(project_id, "trace_id", trace_ids)?;
    Some(enqueue_raw_where(
        backend,
        RAW_TRACES_TABLE,
        &predicate,
        params,
    ))
}

/// Enqueue every record that holds spans of the traces these spans belong to.
pub fn enqueue_raw_for_spans(
    backend: Backend,
    project_id: &str,
    spans: &[(String, String)],
) -> Option<DmlStatement> {
    let mut traces: Vec<String> = spans.iter().map(|(trace_id, _)| trace_id.clone()).collect();
    traces.sort_unstable();
    traces.dedup();
    enqueue_raw_for_traces(backend, project_id, &traces)
}

/// Enqueue these records by id: a repair, a reconciler's re-check, a restore.
pub fn enqueue_raw_records(
    backend: Backend,
    project_id: &str,
    raw_ids: &[String],
) -> Option<DmlStatement> {
    if raw_ids.is_empty() {
        return None;
    }
    let rows = std::iter::repeat_n(
        match backend {
            Backend::Duckdb => "(?, ?, CAST(uuid() AS VARCHAR), CAST(now() AS TIMESTAMP))",
            Backend::Clickhouse => "(?, ?, toString(generateUUIDv4()), now64(6))",
        },
        raw_ids.len(),
    )
    .collect::<Vec<_>>()
    .join(", ");
    let mut params = Vec::with_capacity(raw_ids.len() * 2);
    for raw_id in raw_ids {
        params.push(QueryValue::String(project_id.to_string()));
        params.push(QueryValue::String(raw_id.clone()));
    }
    Some(DmlStatement {
        operation: QueryOperation::RawReconciliation,
        sql: format!(
            "INSERT INTO {RAW_PENDING_TABLE} (project_id, raw_id, token, enqueued_at) VALUES {rows}"
        ),
        params,
    })
}

/// Delete every version of these raw records, or their trace-index rows: the same statement for either table.
pub fn delete_raw_records(
    target: MutationTarget<'_>,
    project_id: &str,
    raw_ids: &[String],
) -> Option<DmlStatement> {
    delete_by_identities(
        QueryOperation::RawReconciliation,
        target,
        project_id,
        "raw_id",
        raw_ids,
    )
}

/// Remove exactly these `(raw_id, token)` entries of one project.
pub fn clear_raw_pending(
    target: MutationTarget<'_>,
    project_id: &str,
    entries: &[(String, String)],
) -> Option<DmlStatement> {
    if entries.is_empty() {
        return None;
    }
    let identities = std::iter::repeat_n("(?, ?)", entries.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(1 + entries.len() * 2);
    params.push(QueryValue::String(project_id.to_string()));
    for (raw_id, token) in entries {
        params.push(QueryValue::String(raw_id.clone()));
        params.push(QueryValue::String(token.clone()));
    }
    Some(render_delete(
        QueryOperation::RawReconciliation,
        target,
        format!("project_id = ? AND (raw_id, token) IN ({identities})"),
        params,
    ))
}

/// Enqueue the records named by the span identities in DuckDB's private retention batch.
pub fn enqueue_raw_for_retention_batch() -> DmlStatement {
    DmlStatement {
        operation: QueryOperation::EnforceRetention,
        sql: format!(
            "INSERT INTO {RAW_PENDING_TABLE} (project_id, raw_id, token, enqueued_at) \
             SELECT project_id, raw_id, CAST(uuid() AS VARCHAR), CAST(now() AS TIMESTAMP) FROM ( \
                 SELECT DISTINCT s.project_id, s.raw_id FROM otel_spans s \
                 JOIN _retention_batch b \
                   ON b.project_id = s.project_id AND b.trace_id = s.trace_id AND b.span_id = s.span_id \
                 WHERE s.raw_id IS NOT NULL)"
        ),
        params: Vec::new(),
    }
}

/// Enqueue the records named by the ClickHouse span rows retention is about to expire; the predicate is the
/// one [`retention_delete_expired_clickhouse`] deletes by.
pub fn enqueue_raw_for_expired_clickhouse(
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DmlStatement {
    let mut statement = enqueue_raw_where(
        Backend::Clickhouse,
        "otel_spans",
        "timestamp_start < fromUnixTimestamp64Micro(?) AND project_id = ? \
         AND (isNull(hold_until) OR hold_until < fromUnixTimestamp64Micro(?))",
        vec![
            QueryValue::Int64(cutoff.timestamp_micros()),
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(now.timestamp_micros()),
        ],
    );
    statement.operation = QueryOperation::EnforceRetention;
    statement
}

fn enqueue_raw_where(
    backend: Backend,
    source: &str,
    predicate: &str,
    params: Vec<QueryValue>,
) -> DmlStatement {
    let (token, at) = match backend {
        Backend::Duckdb => ("CAST(uuid() AS VARCHAR)", "CAST(now() AS TIMESTAMP)"),
        Backend::Clickhouse => ("toString(generateUUIDv4())", "now64(6)"),
    };
    DmlStatement {
        operation: QueryOperation::RawReconciliation,
        sql: format!(
            "INSERT INTO {RAW_PENDING_TABLE} (project_id, raw_id, token, enqueued_at) \
             SELECT project_id, raw_id, {token}, {at} FROM ( \
                 SELECT DISTINCT project_id, raw_id FROM {source} \
                 WHERE raw_id IS NOT NULL AND {predicate})"
        ),
        params,
    }
}
