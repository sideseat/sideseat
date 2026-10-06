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

pub mod raw;
pub mod retention;

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
///
/// The raw records too: they are the authority the rows are derived from, so a hold that protects the rows and
/// not their records would protect a cache and let the source go.
pub fn patch_project_hold(
    spans_target: MutationTarget<'_>,
    metrics_target: MutationTarget<'_>,
    logs_target: MutationTarget<'_>,
    raw_target: MutationTarget<'_>,
    raw_traces_target: MutationTarget<'_>,
    project_id: &str,
    hold_until: DateTime<Utc>,
) -> [DmlStatement; 5] {
    for other in [metrics_target, logs_target, raw_target, raw_traces_target] {
        assert_eq!(spans_target.backend, other.backend);
        assert_eq!(spans_target.on_cluster, other.on_cluster);
    }

    [
        patch_relation_hold(spans_target, project_id, hold_until),
        patch_relation_hold(metrics_target, project_id, hold_until),
        patch_relation_hold(logs_target, project_id, hold_until),
        patch_relation_hold(raw_target, project_id, hold_until),
        patch_relation_hold(raw_traces_target, project_id, hold_until),
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
    let (predicate, params) = span_pairs_predicate(project_id, spans)?;
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
    /// Remove every version of every project raw record.
    pub delete_raw: DmlStatement,
    /// Remove the project's reconciliation entries.
    pub delete_raw_pending: DmlStatement,
    /// Remove the project's trace index.
    pub delete_raw_traces: DmlStatement,
}

/// Build a complete project deletion plan for one backend.
pub fn delete_project_data(
    spans_target: MutationTarget<'_>,
    metrics_target: MutationTarget<'_>,
    logs_target: MutationTarget<'_>,
    raw_target: MutationTarget<'_>,
    raw_pending_target: MutationTarget<'_>,
    raw_traces_target: MutationTarget<'_>,
    project_id: &str,
) -> ProjectDeletePlan {
    for other in [raw_target, raw_pending_target, raw_traces_target] {
        assert_eq!(spans_target.backend, other.backend);
        assert_eq!(spans_target.on_cluster, other.on_cluster);
    }
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
    let delete_raw = render_delete(
        operation,
        raw_target,
        "project_id = ?".to_string(),
        project_param(),
    );
    let delete_raw_pending = render_delete(
        operation,
        raw_pending_target,
        "project_id = ?".to_string(),
        project_param(),
    );
    let delete_raw_traces = render_delete(
        operation,
        raw_traces_target,
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
            delete_raw,
            delete_raw_pending,
            delete_raw_traces,
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
            delete_raw,
            delete_raw_pending,
            delete_raw_traces,
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

fn delete_by_identities(
    operation: QueryOperation,
    target: MutationTarget<'_>,
    project_id: &str,
    column: &str,
    identities: &[String],
) -> Option<DmlStatement> {
    let (predicate, params) = identity_predicate(project_id, column, identities)?;
    Some(render_delete(operation, target, predicate, params))
}

fn identity_predicate(
    project_id: &str,
    column: &str,
    identities: &[String],
) -> Option<(String, Vec<QueryValue>)> {
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
    Some((predicate, params))
}

fn span_pairs_predicate(
    project_id: &str,
    spans: &[(String, String)],
) -> Option<(String, Vec<QueryValue>)> {
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
    Some((predicate, params))
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
#[path = "dml_tests.rs"]
mod tests;
