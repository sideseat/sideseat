//! Typed tenant-scoped mutations shared by analytical adapters.
//!
//! Values are always bound. The only rendered identifiers are adapter configuration selected by
//! the composition root, and they are validated before interpolation.

use chrono::{DateTime, Utc};
use sideseat_core::utils::sql::is_plain_identifier;
use sideseat_ports::types::{
    SearchBackfillDocument, SearchDocument, SearchField, SearchRecordId, SearchSignal,
};

use crate::Backend;
use crate::analytics::{QueryOperation, QueryValue};

/// Backend-specific mutation destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationTarget<'a> {
    backend: Backend,
    table: &'a str,
    on_cluster: &'a str,
}

impl<'a> MutationTarget<'a> {
    /// A transactional DuckDB table.
    pub fn duckdb(table: &'a str) -> Self {
        validate_table(table);
        Self {
            backend: Backend::Duckdb,
            table,
            on_cluster: "",
        }
    }

    /// A ClickHouse local mutation table and its optional `ON CLUSTER` clause.
    pub fn clickhouse(table: &'a str, on_cluster: &'a str) -> Self {
        validate_table(table);
        validate_on_cluster(on_cluster);
        Self {
            backend: Backend::Clickhouse,
            table,
            on_cluster,
        }
    }
}

/// Validated table selected for a driver's typed bulk-write API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteTarget<'a> {
    operation: QueryOperation,
    table: &'a str,
}

impl WriteTarget<'_> {
    pub fn operation(&self) -> QueryOperation {
        self.operation
    }

    pub fn table(&self) -> &str {
        self.table
    }
}

/// Select the span table used by a typed bulk writer.
pub fn span_write_target(backend: Backend, configured_table: Option<&str>) -> WriteTarget<'_> {
    write_target(
        QueryOperation::UpsertSpans,
        backend,
        "otel_spans",
        configured_table,
    )
}

/// Select the metric table used by a typed bulk writer.
pub fn metric_write_target(backend: Backend, configured_table: Option<&str>) -> WriteTarget<'_> {
    write_target(
        QueryOperation::UpsertMetrics,
        backend,
        "otel_metrics",
        configured_table,
    )
}

/// Select the log table used by a typed bulk writer.
pub fn log_write_target(backend: Backend, configured_table: Option<&str>) -> WriteTarget<'_> {
    write_target(
        QueryOperation::UpsertLogs,
        backend,
        "otel_logs",
        configured_table,
    )
}

fn write_target<'a>(
    operation: QueryOperation,
    backend: Backend,
    embedded_table: &'static str,
    configured_table: Option<&'a str>,
) -> WriteTarget<'a> {
    let table = match backend {
        Backend::Duckdb => {
            assert!(
                configured_table.is_none(),
                "DuckDB write targets are selected by the shared DML layer"
            );
            embedded_table
        }
        Backend::Clickhouse => {
            configured_table.expect("ClickHouse write target comes from adapter configuration")
        }
    };
    validate_table(table);
    WriteTarget { operation, table }
}

/// One executable statement belonging to a tenant-scoped DML operation.
#[derive(Debug, Clone, PartialEq)]
pub struct DmlStatement {
    operation: QueryOperation,
    sql: String,
    params: Vec<QueryValue>,
}

/// Stamp every signal table for one project with a legal-hold deadline.
pub fn patch_project_hold(
    spans_target: MutationTarget<'_>,
    metrics_target: MutationTarget<'_>,
    logs_target: MutationTarget<'_>,
    project_id: &str,
    hold_until: DateTime<Utc>,
) -> [DmlStatement; 3] {
    assert_eq!(spans_target.backend, metrics_target.backend);
    assert_eq!(spans_target.backend, logs_target.backend);
    assert_eq!(spans_target.on_cluster, metrics_target.on_cluster);
    assert_eq!(spans_target.on_cluster, logs_target.on_cluster);

    [
        patch_relation_hold(spans_target, project_id, hold_until),
        patch_relation_hold(metrics_target, project_id, hold_until),
        patch_relation_hold(logs_target, project_id, hold_until),
    ]
}

pub fn patch_relation_hold(
    target: MutationTarget<'_>,
    project_id: &str,
    hold_until: DateTime<Utc>,
) -> DmlStatement {
    let (sql, hold_value) = match target.backend {
        Backend::Duckdb => (
            format!(
                "UPDATE {} SET hold_until = ? WHERE project_id = ?",
                target.table
            ),
            QueryValue::String(hold_until.to_rfc3339()),
        ),
        Backend::Clickhouse => (
            format!(
                "ALTER TABLE {}{} UPDATE hold_until = fromUnixTimestamp64Micro(?) \
                 WHERE project_id = ? SETTINGS mutations_sync = 2",
                target.table, target.on_cluster
            ),
            QueryValue::Int64(hold_until.timestamp_micros()),
        ),
    };
    DmlStatement {
        operation: QueryOperation::EnforceRetention,
        sql,
        params: vec![hold_value, QueryValue::String(project_id.to_string())],
    }
}

impl DmlStatement {
    pub fn operation(&self) -> QueryOperation {
        self.operation
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn params(&self) -> &[QueryValue] {
        &self.params
    }
}

/// Update only the derived search columns of one current ClickHouse identity.
///
/// Terms are alphanumeric tokens and therefore cannot contain spaces; binding one space-joined
/// string keeps values parameterised while reconstructing the array server-side.
pub fn search_backfill_update(
    target: MutationTarget<'_>,
    project_id: &str,
    signal: SearchSignal,
    document: &SearchBackfillDocument,
) -> DmlStatement {
    assert_eq!(target.backend, Backend::Clickhouse);
    let fields: &[SearchField] = match signal {
        SearchSignal::Spans => &[
            SearchField::Prompt,
            SearchField::Completion,
            SearchField::ToolName,
            SearchField::ToolArgs,
            SearchField::Error,
            SearchField::SpanName,
        ],
        SearchSignal::Logs => &[
            SearchField::Body,
            SearchField::EventName,
            SearchField::Severity,
            SearchField::Attributes,
        ],
    };
    let mut assignments = vec!["search_indexed = 1".to_string()];
    let mut params = Vec::with_capacity(fields.len() * 2 + 3);
    for field in fields {
        let terms = search_field(&document.document, *field);
        assignments.push(format!(
            "search_{name} = arrayFilter(value -> notEmpty(value), splitByChar(' ', ?))",
            name = field.as_str()
        ));
        assignments.push(format!("search_{}_truncated = ?", field.as_str()));
        params.push(QueryValue::String(terms.terms.join(" ")));
        params.push(QueryValue::Int64(i64::from(terms.truncated)));
    }
    params.push(QueryValue::String(project_id.to_string()));
    let predicate = match (&document.id, signal) {
        (SearchRecordId::Span { trace_id, span_id }, SearchSignal::Spans) => {
            params.push(QueryValue::String(trace_id.clone()));
            params.push(QueryValue::String(span_id.clone()));
            params.push(QueryValue::String(
                document
                    .expected_content_digest
                    .clone()
                    .expect("span search backfill requires its observed content digest"),
            ));
            "project_id = ? AND trace_id = ? AND span_id = ? \
             AND content_digest = ? AND search_indexed = 0"
        }
        (
            SearchRecordId::Log {
                log_digest,
                ordinal,
            },
            SearchSignal::Logs,
        ) => {
            params.push(QueryValue::String(log_digest.clone()));
            params.push(QueryValue::Int64(i64::from(*ordinal)));
            "project_id = ? AND log_digest = ? AND ordinal = ? AND search_indexed = 0"
        }
        _ => panic!("search backfill signal and record identity disagree"),
    };
    DmlStatement {
        operation: match signal {
            SearchSignal::Spans => QueryOperation::UpsertSpans,
            SearchSignal::Logs => QueryOperation::UpsertLogs,
        },
        sql: format!(
            "ALTER TABLE {}{} UPDATE {} WHERE {} SETTINGS mutations_sync = 2",
            target.table,
            target.on_cluster,
            assignments.join(", "),
            predicate
        ),
        params,
    }
}

fn search_field(
    document: &SearchDocument,
    field: SearchField,
) -> &sideseat_ports::types::SearchFieldTerms {
    document
        .field(field)
        .unwrap_or_else(|| panic!("domain search document omitted {}", field.as_str()))
}

/// Delete every span row belonging to the requested trace identities.
pub fn delete_traces(
    target: MutationTarget<'_>,
    project_id: &str,
    trace_ids: &[String],
) -> Option<DmlStatement> {
    delete_by_identities(
        QueryOperation::DeleteTraces,
        target,
        project_id,
        "trace_id",
        trace_ids,
    )
}

/// Delete every span row belonging to the requested `(trace_id, span_id)` identities.
pub fn delete_spans(
    target: MutationTarget<'_>,
    project_id: &str,
    spans: &[(String, String)],
) -> Option<DmlStatement> {
    if spans.is_empty() {
        return None;
    }

    let identities = std::iter::repeat_n("(?, ?)", spans.len())
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = format!("project_id = ? AND (trace_id, span_id) IN ({identities})");
    let mut params = Vec::with_capacity(1 + spans.len() * 2);
    params.push(QueryValue::String(project_id.to_string()));
    for (trace_id, span_id) in spans {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }

    Some(render_delete(
        QueryOperation::DeleteSpans,
        target,
        predicate,
        params,
    ))
}

/// Delete logs correlated to requested traces.
pub fn delete_logs_for_traces(
    target: MutationTarget<'_>,
    project_id: &str,
    trace_ids: &[String],
) -> Option<DmlStatement> {
    delete_by_identities(
        QueryOperation::DeleteTraces,
        target,
        project_id,
        "trace_id",
        trace_ids,
    )
}

/// Delete logs correlated to requested span identities.
pub fn delete_logs_for_spans(
    target: MutationTarget<'_>,
    project_id: &str,
    spans: &[(String, String)],
) -> Option<DmlStatement> {
    if spans.is_empty() {
        return None;
    }
    let identities = std::iter::repeat_n("(?, ?)", spans.len())
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = format!("project_id = ? AND (trace_id, span_id) IN ({identities})");
    let mut params = vec![QueryValue::String(project_id.to_string())];
    for (trace_id, span_id) in spans {
        params.push(QueryValue::String(trace_id.clone()));
        params.push(QueryValue::String(span_id.clone()));
    }
    Some(render_delete(
        QueryOperation::DeleteSpans,
        target,
        predicate,
        params,
    ))
}

/// Delete every log row owned by one project.
pub fn delete_project_logs(target: MutationTarget<'_>, project_id: &str) -> DmlStatement {
    render_delete(
        QueryOperation::DeleteProjectData,
        target,
        "project_id = ?".to_string(),
        vec![QueryValue::String(project_id.to_string())],
    )
}

/// Delete every span row belonging to traces resolved from selected canonical sessions.
pub fn delete_session_traces(
    target: MutationTarget<'_>,
    project_id: &str,
    trace_ids: &[String],
) -> Option<DmlStatement> {
    delete_by_identities(
        QueryOperation::DeleteSessions,
        target,
        project_id,
        "trace_id",
        trace_ids,
    )
}

/// Statements needed to remove every analytical row owned by one project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDeletePlan {
    /// ClickHouse estimates the returned span count before mutating; DuckDB reports affected rows.
    pub count_spans: Option<DmlStatement>,
    /// Remove all project spans.
    pub delete_spans: DmlStatement,
    /// ClickHouse checks an optional deployment table before mutating it.
    pub metrics_table_exists: Option<DmlStatement>,
    /// Remove all project metrics.
    pub delete_metrics: DmlStatement,
    /// Remove all project logs.
    pub delete_logs: DmlStatement,
}

/// Build a complete project deletion plan for one backend.
pub fn delete_project_data(
    spans_target: MutationTarget<'_>,
    metrics_target: MutationTarget<'_>,
    logs_target: MutationTarget<'_>,
    project_id: &str,
) -> ProjectDeletePlan {
    assert_eq!(
        spans_target.backend, metrics_target.backend,
        "project deletion targets must use the same backend"
    );
    assert_eq!(
        spans_target.on_cluster, metrics_target.on_cluster,
        "project deletion targets must use the same cluster"
    );
    assert_eq!(
        spans_target.backend, logs_target.backend,
        "project deletion targets must use the same backend"
    );
    assert_eq!(
        spans_target.on_cluster, logs_target.on_cluster,
        "project deletion targets must use the same cluster"
    );

    let operation = QueryOperation::DeleteProjectData;
    let project_param = || vec![QueryValue::String(project_id.to_string())];
    let delete_spans = render_delete(
        operation,
        spans_target,
        "project_id = ?".to_string(),
        project_param(),
    );
    let delete_metrics = render_delete(
        operation,
        metrics_target,
        "project_id = ?".to_string(),
        project_param(),
    );
    let delete_logs = render_delete(
        operation,
        logs_target,
        "project_id = ?".to_string(),
        project_param(),
    );

    match spans_target.backend {
        Backend::Duckdb => ProjectDeletePlan {
            count_spans: None,
            delete_spans,
            metrics_table_exists: None,
            delete_metrics,
            delete_logs,
        },
        Backend::Clickhouse => ProjectDeletePlan {
            count_spans: Some(DmlStatement {
                operation,
                sql: "SELECT count() FROM otel_spans FINAL WHERE project_id = ?".to_string(),
                params: project_param(),
            }),
            delete_spans,
            metrics_table_exists: Some(DmlStatement {
                operation,
                sql: "SELECT count() FROM system.tables \
                      WHERE database = currentDatabase() AND name = ?"
                    .to_string(),
                params: vec![QueryValue::String(metrics_target.table.to_string())],
            }),
            delete_metrics,
            delete_logs,
        },
    }
}

/// Delete every row one derived DuckDB relation attributes to a project.
pub fn delete_project_relation(target: MutationTarget<'_>, project_id: &str) -> DmlStatement {
    render_delete(
        QueryOperation::DeleteProjectData,
        target,
        "project_id = ?".to_string(),
        vec![QueryValue::String(project_id.to_string())],
    )
}

/// Read stored versions for one project's candidate metric identities.
pub fn metric_winner_probe(project_id: &str, datapoint_ids: &[&str]) -> Option<DmlStatement> {
    if datapoint_ids.is_empty() {
        return None;
    }
    let placeholders = std::iter::repeat_n("?", datapoint_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(1 + datapoint_ids.len());
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(
        datapoint_ids
            .iter()
            .map(|id| QueryValue::String((*id).to_string())),
    );
    Some(DmlStatement {
        operation: QueryOperation::UpsertMetrics,
        sql: format!(
            "SELECT datapoint_id, epoch_us(ingested_at) FROM otel_metrics \
             WHERE project_id = ? AND datapoint_id IN ({placeholders})"
        ),
        params,
    })
}

/// Remove stored metric rows immediately before their winning replacements are appended.
pub fn delete_metric_winners(project_id: &str, datapoint_ids: &[&str]) -> Option<DmlStatement> {
    if datapoint_ids.is_empty() {
        return None;
    }
    let placeholders = std::iter::repeat_n("?", datapoint_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(1 + datapoint_ids.len());
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(
        datapoint_ids
            .iter()
            .map(|id| QueryValue::String((*id).to_string())),
    );
    Some(DmlStatement {
        operation: QueryOperation::UpsertMetrics,
        sql: format!(
            "DELETE FROM otel_metrics \
             WHERE project_id = ? AND datapoint_id IN ({placeholders})"
        ),
        params,
    })
}

/// Remove retried DuckDB log identities immediately before their replacements are appended.
pub fn delete_log_winners(project_id: &str, identities: &[(&str, u32)]) -> Option<DmlStatement> {
    if identities.is_empty() {
        return None;
    }
    let placeholders = std::iter::repeat_n("(?, ?)", identities.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::with_capacity(1 + identities.len() * 2);
    params.push(QueryValue::String(project_id.to_string()));
    for (digest, ordinal) in identities {
        params.push(QueryValue::String((*digest).to_string()));
        params.push(QueryValue::Int64(i64::from(*ordinal)));
    }
    Some(DmlStatement {
        operation: QueryOperation::UpsertLogs,
        sql: format!(
            "DELETE FROM otel_logs \
             WHERE project_id = ? AND (log_digest, ordinal) IN ({placeholders})"
        ),
        params,
    })
}

const DUCKDB_WINNING_SPANS: &str = "(SELECT * FROM otel_spans \
    QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
    ORDER BY ingested_at DESC, rowid DESC) = 1)";

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

fn delete_by_identities(
    operation: QueryOperation,
    target: MutationTarget<'_>,
    project_id: &str,
    column: &str,
    identities: &[String],
) -> Option<DmlStatement> {
    if identities.is_empty() {
        return None;
    }

    let placeholders = std::iter::repeat_n("?", identities.len())
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = format!("project_id = ? AND {column} IN ({placeholders})");
    let mut params = Vec::with_capacity(1 + identities.len());
    params.push(QueryValue::String(project_id.to_string()));
    params.extend(identities.iter().cloned().map(QueryValue::String));

    Some(render_delete(operation, target, predicate, params))
}

fn render_delete(
    operation: QueryOperation,
    target: MutationTarget<'_>,
    predicate: String,
    params: Vec<QueryValue>,
) -> DmlStatement {
    let sql = match target.backend {
        Backend::Duckdb => format!("DELETE FROM {} WHERE {predicate}", target.table),
        Backend::Clickhouse => format!(
            "ALTER TABLE {}{} DELETE WHERE {predicate} SETTINGS mutations_sync = 2",
            target.table, target.on_cluster
        ),
    };
    DmlStatement {
        operation,
        sql,
        params,
    }
}

fn validate_table(table: &str) {
    assert!(
        is_plain_identifier(table),
        "invalid SQL mutation table: {table:?}"
    );
}

fn validate_on_cluster(on_cluster: &str) {
    if on_cluster.is_empty() {
        return;
    }
    let cluster = on_cluster
        .strip_prefix(" ON CLUSTER ")
        .filter(|cluster| !cluster.is_empty())
        .expect("ClickHouse mutation cluster must be an ` ON CLUSTER <name>` clause");
    assert!(
        cluster
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
        "invalid ClickHouse mutation cluster: {cluster:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_delete_is_tenant_scoped_and_parameterized() {
        let ids = vec!["trace-'one".to_string(), "trace-two".to_string()];
        for target in [
            MutationTarget::duckdb("otel_spans"),
            MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER telemetry"),
        ] {
            let query = delete_traces(target, "tenant-'quoted", &ids).expect("non-empty delete");
            assert_eq!(query.operation(), QueryOperation::DeleteTraces);
            assert_eq!(query.sql().matches('?').count(), query.params().len());
            assert!(!query.sql().contains("tenant-'quoted"));
            assert!(!query.sql().contains("trace-'one"));
            assert!(query.sql().contains("project_id = ?"));
        }
    }

    #[test]
    fn span_delete_preserves_tuple_bind_order() {
        let spans = vec![
            ("trace-a".to_string(), "span-a".to_string()),
            ("trace-b".to_string(), "span-b".to_string()),
        ];
        let query =
            delete_spans(MutationTarget::duckdb("otel_spans"), "p", &spans).expect("delete");
        assert_eq!(
            query.params(),
            &[
                QueryValue::String("p".to_string()),
                QueryValue::String("trace-a".to_string()),
                QueryValue::String("span-a".to_string()),
                QueryValue::String("trace-b".to_string()),
                QueryValue::String("span-b".to_string()),
            ]
        );
    }

    #[test]
    fn clickhouse_delete_waits_for_every_replica() {
        let ids = vec!["t".to_string()];
        let query = delete_traces(
            MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod-1"),
            "p",
            &ids,
        )
        .expect("delete");
        assert_eq!(
            query.sql(),
            "ALTER TABLE otel_spans_local ON CLUSTER prod-1 DELETE WHERE project_id = ? AND \
             trace_id IN (?) SETTINGS mutations_sync = 2"
        );
    }

    #[test]
    fn empty_identity_sets_do_not_issue_broad_mutations() {
        assert!(delete_traces(MutationTarget::duckdb("otel_spans"), "p", &[]).is_none());
        assert!(delete_spans(MutationTarget::duckdb("otel_spans"), "p", &[]).is_none());
    }

    #[test]
    fn project_delete_plan_covers_every_backend_table() {
        let duckdb = delete_project_data(
            MutationTarget::duckdb("otel_spans"),
            MutationTarget::duckdb("otel_metrics"),
            MutationTarget::duckdb("otel_logs"),
            "tenant-'quoted",
        );
        assert!(duckdb.count_spans.is_none());
        assert!(duckdb.metrics_table_exists.is_none());
        assert_eq!(
            duckdb.delete_spans.sql(),
            "DELETE FROM otel_spans WHERE project_id = ?"
        );
        assert_eq!(
            duckdb.delete_metrics.sql(),
            "DELETE FROM otel_metrics WHERE project_id = ?"
        );
        assert_eq!(
            duckdb.delete_logs.sql(),
            "DELETE FROM otel_logs WHERE project_id = ?"
        );

        let clickhouse = delete_project_data(
            MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
            MutationTarget::clickhouse("otel_metrics_local", " ON CLUSTER prod"),
            MutationTarget::clickhouse("otel_logs_local", " ON CLUSTER prod"),
            "tenant-'quoted",
        );
        assert!(clickhouse.count_spans.is_some());
        assert!(clickhouse.metrics_table_exists.is_some());
        for statement in [
            clickhouse.count_spans.as_ref().expect("count"),
            &clickhouse.delete_spans,
            clickhouse
                .metrics_table_exists
                .as_ref()
                .expect("table check"),
            &clickhouse.delete_metrics,
            &clickhouse.delete_logs,
        ] {
            assert_eq!(
                statement.sql().matches('?').count(),
                statement.params().len()
            );
            assert!(!statement.sql().contains("tenant-'quoted"));
        }
    }

    #[test]
    fn analytical_write_targets_are_validated_capabilities() {
        let duck_span = span_write_target(Backend::Duckdb, None);
        assert_eq!(duck_span.operation(), QueryOperation::UpsertSpans);
        assert_eq!(duck_span.table(), "otel_spans");

        let click_metric =
            metric_write_target(Backend::Clickhouse, Some("otel_metrics_distributed"));
        assert_eq!(click_metric.operation(), QueryOperation::UpsertMetrics);
        assert_eq!(click_metric.table(), "otel_metrics_distributed");
    }

    #[test]
    fn retention_plan_is_parameterized_and_revision_aware() {
        let cutoff = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let now = DateTime::from_timestamp(1_700_000_100, 0).unwrap();
        let expired = retention_select_expired(cutoff, now, 100);
        assert_eq!(expired.operation(), QueryOperation::EnforceRetention);
        assert_eq!(expired.sql().matches('?').count(), expired.params().len());
        assert!(expired.sql().contains("ROW_NUMBER()"));
        assert!(expired.sql().contains("hold_until"));

        let oldest = retention_select_oldest("tenant-'quoted", now, 10, 50, true);
        assert_eq!(oldest.sql().matches('?').count(), oldest.params().len());
        assert!(!oldest.sql().contains("tenant-'quoted"));
        assert!(oldest.sql().contains("hold_until"));
        assert!(
            oldest
                .sql()
                .contains("cumulative_rows <= ? OR row_rank = 1")
        );

        let clickhouse = retention_delete_expired_clickhouse(
            MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
            "tenant-'quoted",
            cutoff,
            now,
        );
        assert_eq!(
            clickhouse.sql(),
            "ALTER TABLE otel_spans_local ON CLUSTER prod DELETE WHERE timestamp_start < \
             fromUnixTimestamp64Micro(?) AND project_id = ? AND (isNull(hold_until) OR hold_until < \
             fromUnixTimestamp64Micro(?)) SETTINGS mutations_sync = 2"
        );
        assert_eq!(
            clickhouse.params(),
            &[
                QueryValue::Int64(1_700_000_000_000_000),
                QueryValue::String("tenant-'quoted".to_string()),
                QueryValue::Int64(1_700_000_100_000_000),
            ]
        );
        assert!(!clickhouse.sql().contains("tenant-'quoted"));
    }

    #[test]
    fn metric_upsert_statements_share_identity_bind_order() {
        let ids = ["dp-'one", "dp-two"];
        let probe = metric_winner_probe("tenant-'quoted", &ids).expect("probe");
        let delete = delete_metric_winners("tenant-'quoted", &ids).expect("delete");
        assert_eq!(probe.params(), delete.params());
        assert_eq!(probe.sql().matches('?').count(), probe.params().len());
        assert_eq!(delete.sql().matches('?').count(), delete.params().len());
        for value in ["tenant-'quoted", "dp-'one", "dp-two"] {
            assert!(!probe.sql().contains(value));
            assert!(!delete.sql().contains(value));
        }
    }

    #[test]
    fn search_backfill_is_parameterized_and_revision_guarded() {
        let document = SearchBackfillDocument {
            id: SearchRecordId::Span {
                trace_id: "trace-'quoted".to_string(),
                span_id: "span".to_string(),
            },
            expected_content_digest: Some("digest-'quoted".to_string()),
            document: SearchDocument {
                indexed: true,
                fields: [
                    SearchField::Prompt,
                    SearchField::Completion,
                    SearchField::ToolName,
                    SearchField::ToolArgs,
                    SearchField::Error,
                    SearchField::SpanName,
                ]
                .into_iter()
                .map(|field| sideseat_ports::types::SearchFieldTerms {
                    field,
                    terms: vec!["alpha".to_string()],
                    truncated: false,
                    text: String::new(),
                })
                .collect(),
            },
        };
        let statement = search_backfill_update(
            MutationTarget::clickhouse("otel_spans_local", " ON CLUSTER prod"),
            "tenant-'quoted",
            SearchSignal::Spans,
            &document,
        );
        assert_eq!(
            statement.sql().matches('?').count(),
            statement.params().len()
        );
        assert!(statement.sql().contains("content_digest = ?"));
        assert!(statement.sql().contains("search_indexed = 0"));
        assert!(statement.sql().contains("mutations_sync = 2"));
        for value in ["tenant-'quoted", "trace-'quoted", "digest-'quoted"] {
            assert!(!statement.sql().contains(value));
        }
    }

    #[test]
    #[should_panic(expected = "invalid SQL mutation table")]
    fn mutation_target_rejects_sql_in_table_name() {
        MutationTarget::clickhouse("otel_spans; DROP TABLE users", "");
    }

    #[test]
    #[should_panic(expected = "invalid ClickHouse mutation cluster")]
    fn mutation_target_rejects_sql_in_cluster_name() {
        MutationTarget::clickhouse("otel_spans", " ON CLUSTER prod; DROP TABLE users");
    }
}
