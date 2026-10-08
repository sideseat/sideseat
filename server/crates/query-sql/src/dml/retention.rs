//! Retention statements: selecting what has expired and removing it, per backend.
//!
//! The legal-hold predicate (`hold_until`) is part of every one of them: retention must not take a row a hold
//! protects, and a statement that merely forgot the clause would look correct.

use chrono::{DateTime, Utc};

use super::{DmlStatement, MutationTarget};
use crate::Backend;
use crate::analytics::{QueryOperation, QueryValue};

use crate::winners::DUCKDB_WINNING_SPANS;

/// Prepare the private DuckDB table used to carry one atomic retention batch.
pub fn retention_prepare_batch() -> DmlStatement {
    retention_statement(
        "CREATE TEMP TABLE IF NOT EXISTS _retention_batch (\
            project_id VARCHAR NOT NULL, \
            trace_id VARCHAR NOT NULL, \
            span_id VARCHAR NOT NULL, \
            PRIMARY KEY (project_id, trace_id, span_id))",
        Vec::new(),
    )
}

/// Remove identities left by a previous transaction from the private retention batch.
pub fn retention_clear_batch() -> DmlStatement {
    retention_statement("DELETE FROM _retention_batch", Vec::new())
}

/// Select one oldest page of expired winning span identities.
pub fn retention_select_expired(
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        format!(
            "INSERT INTO _retention_batch \
             SELECT project_id, trace_id, span_id FROM {DUCKDB_WINNING_SPANS} \
             WHERE timestamp_start < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp_start ASC, project_id, trace_id, span_id \
             LIMIT ?"
        ),
        vec![
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Select one oldest page of expired winning span identities for one project.
pub fn retention_select_expired_for_project(
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        format!(
            "INSERT INTO _retention_batch \
             SELECT project_id, trace_id, span_id FROM {DUCKDB_WINNING_SPANS} \
             WHERE project_id = ? \
               AND timestamp_start < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp_start ASC, trace_id, span_id \
             LIMIT ?"
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Select one project's oldest identities without crossing the remaining physical-row budget.
pub fn retention_select_oldest(
    project_id: &str,
    now: DateTime<Utc>,
    identity_limit: i64,
    row_budget: i64,
    allow_first_identity_overshoot: bool,
) -> DmlStatement {
    let overshoot = if allow_first_identity_overshoot {
        " OR row_rank = 1"
    } else {
        ""
    };
    retention_statement(
        format!(
            "INSERT INTO _retention_batch \
             WITH winners AS ( \
                 SELECT project_id, trace_id, span_id, timestamp_start \
                 FROM {DUCKDB_WINNING_SPANS} \
                 WHERE project_id = ? \
                   AND (hold_until IS NULL OR hold_until < ?) \
             ), \
             ranked AS ( \
                 SELECT w.project_id, w.trace_id, w.span_id, \
                        ROW_NUMBER() OVER ( \
                            ORDER BY w.timestamp_start ASC, w.trace_id, w.span_id \
                        ) AS row_rank, \
                        SUM(r.revisions) OVER ( \
                            ORDER BY w.timestamp_start ASC, w.trace_id, w.span_id \
                        ) AS cumulative_rows \
                 FROM winners w \
                 JOIN ( \
                     SELECT project_id, trace_id, span_id, COUNT(*) AS revisions \
                     FROM otel_spans WHERE project_id = ? \
                     GROUP BY project_id, trace_id, span_id \
                 ) r \
                   ON r.project_id = w.project_id \
                  AND r.trace_id = w.trace_id \
                  AND r.span_id = w.span_id \
             ) \
             SELECT project_id, trace_id, span_id FROM ranked \
             WHERE row_rank <= ? AND (cumulative_rows <= ?{overshoot})"
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(identity_limit),
            QueryValue::Int64(row_budget),
        ],
    )
}

/// Projects whose logical winning-span count exceeds the configured per-project limit.
pub fn retention_projects_over_limit(max_spans: i64) -> DmlStatement {
    retention_statement(
        format!(
            "SELECT project_id, COUNT(*) AS span_count \
             FROM {DUCKDB_WINNING_SPANS} \
             GROUP BY project_id \
             HAVING COUNT(*) > ? \
             ORDER BY project_id"
        ),
        vec![QueryValue::Int64(max_spans)],
    )
}

/// One project's logical winning-span count when it exceeds the configured limit.
pub fn retention_project_over_limit(project_id: &str, max_spans: i64) -> DmlStatement {
    retention_statement(
        format!(
            "SELECT project_id, COUNT(*) AS span_count \
             FROM {DUCKDB_WINNING_SPANS} \
             WHERE project_id = ? \
             GROUP BY project_id \
             HAVING COUNT(*) > ?"
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(max_spans),
        ],
    )
}

/// Distinct traces represented by the currently selected retention batch.
pub fn retention_selected_traces(limit: i64) -> DmlStatement {
    retention_statement(
        "SELECT DISTINCT project_id, trace_id FROM _retention_batch \
         ORDER BY project_id, trace_id LIMIT ?",
        vec![QueryValue::Int64(limit)],
    )
}

/// Delete every physical revision of every selected logical span identity.
pub fn retention_delete_selected_spans() -> DmlStatement {
    retention_statement(
        "DELETE FROM otel_spans \
         WHERE (project_id, trace_id, span_id) \
               IN (SELECT project_id, trace_id, span_id FROM _retention_batch)",
        Vec::new(),
    )
}

/// Delete one oldest page of expired DuckDB metrics.
pub fn retention_delete_expired_metrics(
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        "DELETE FROM otel_metrics \
         WHERE rowid IN ( \
             SELECT rowid FROM otel_metrics \
             WHERE timestamp < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp ASC \
             LIMIT ? \
         )",
        vec![
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Delete one oldest page of expired DuckDB metrics for one project.
pub fn retention_delete_expired_metrics_for_project(
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        "DELETE FROM otel_metrics \
         WHERE rowid IN ( \
             SELECT rowid FROM otel_metrics \
             WHERE project_id = ? \
               AND timestamp < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp ASC \
             LIMIT ? \
         )",
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Delete one oldest page of expired DuckDB logs.
pub fn retention_delete_expired_logs(
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        "DELETE FROM otel_logs \
         WHERE rowid IN ( \
             SELECT rowid FROM otel_logs \
             WHERE timestamp < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp ASC, log_digest ASC, ordinal ASC \
             LIMIT ? \
         )",
        vec![
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Delete one oldest page of expired DuckDB logs for one project.
pub fn retention_delete_expired_logs_for_project(
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> DmlStatement {
    retention_statement(
        "DELETE FROM otel_logs \
         WHERE rowid IN ( \
             SELECT rowid FROM otel_logs \
             WHERE project_id = ? \
               AND timestamp < ? \
               AND (hold_until IS NULL OR hold_until < ?) \
             ORDER BY timestamp ASC, log_digest ASC, ordinal ASC \
             LIMIT ? \
         )",
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(cutoff.to_rfc3339()),
            QueryValue::String(now.to_rfc3339()),
            QueryValue::Int64(limit),
        ],
    )
}

/// Flush DuckDB's completed retention mutation.
pub fn retention_checkpoint() -> DmlStatement {
    retention_statement("CHECKPOINT", Vec::new())
}

/// Delete ClickHouse spans older than the given instant and wait for every replica.
pub fn retention_delete_expired_clickhouse(
    target: MutationTarget<'_>,
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DmlStatement {
    assert_eq!(
        target.backend,
        Backend::Clickhouse,
        "ClickHouse retention requires a ClickHouse mutation target"
    );
    retention_statement(
        format!(
            "ALTER TABLE {}{} DELETE WHERE timestamp_start < fromUnixTimestamp64Micro(?) \
             AND project_id = ? \
             AND (isNull(hold_until) OR hold_until < fromUnixTimestamp64Micro(?)) \
             SETTINGS mutations_sync = 2",
            target.table, target.on_cluster
        ),
        vec![
            QueryValue::Int64(cutoff.timestamp_micros()),
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(now.timestamp_micros()),
        ],
    )
}

/// Delete ClickHouse metrics older than the configured boundary.
pub fn retention_delete_expired_metrics_clickhouse(
    target: MutationTarget<'_>,
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DmlStatement {
    retention_delete_clickhouse_column(target, project_id, "timestamp", cutoff, now)
}

/// Delete ClickHouse logs older than the configured boundary.
pub fn retention_delete_expired_logs_clickhouse(
    target: MutationTarget<'_>,
    project_id: &str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DmlStatement {
    retention_delete_clickhouse_column(target, project_id, "timestamp", cutoff, now)
}

fn retention_delete_clickhouse_column(
    target: MutationTarget<'_>,
    project_id: &str,
    column: &'static str,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DmlStatement {
    assert_eq!(
        target.backend,
        Backend::Clickhouse,
        "ClickHouse retention requires a ClickHouse mutation target"
    );
    retention_statement(
        format!(
            "ALTER TABLE {}{} DELETE WHERE {column} < fromUnixTimestamp64Micro(?) \
             AND project_id = ? \
             AND (isNull(hold_until) OR hold_until < fromUnixTimestamp64Micro(?)) \
             SETTINGS mutations_sync = 2",
            target.table, target.on_cluster
        ),
        vec![
            QueryValue::Int64(cutoff.timestamp_micros()),
            QueryValue::String(project_id.to_string()),
            QueryValue::Int64(now.timestamp_micros()),
        ],
    )
}

fn retention_statement(sql: impl Into<String>, params: Vec<QueryValue>) -> DmlStatement {
    DmlStatement {
        operation: QueryOperation::EnforceRetention,
        sql: sql.into(),
        params,
    }
}
