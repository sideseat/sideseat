//! Typed reads over OTLP log-record winners.

use chrono::{DateTime, Utc};
use sideseat_core::core::constants::QUERY_MAX_FILTER_SUGGESTIONS;
use sideseat_ports::types::ListLogsParams;

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

#[derive(Debug, Clone, PartialEq)]
pub struct LogPage {
    pub count: ParameterizedQuery,
    pub rows: ParameterizedQuery,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogFilterOptionQuery {
    pub column: String,
    pub query: ParameterizedQuery,
}

pub fn list_logs(params: &ListLogsParams, backend: Backend) -> LogPage {
    let (conditions, values) = filters(params, backend);
    let source = winning_logs(backend);
    let predicate = conditions.join(" AND ");
    let count = ParameterizedQuery::new(
        format!(
            "SELECT {} AS count FROM {source} l WHERE {predicate}",
            count(backend)
        ),
        values.clone(),
    );
    let offset = params.page.saturating_sub(1) as u64 * params.limit as u64;
    let rows = ParameterizedQuery::new(
        format!(
            "SELECT {} FROM {source} l WHERE {predicate} \
             ORDER BY l.timestamp DESC, l.log_digest DESC, l.ordinal DESC \
             LIMIT {} OFFSET {offset}",
            projection(backend),
            params.limit
        ),
        values,
    );
    LogPage { count, rows }
}

pub fn get_log(
    project_id: &str,
    log_digest: &str,
    ordinal: u32,
    backend: Backend,
) -> ParameterizedQuery {
    ParameterizedQuery::new(
        format!(
            "SELECT {} FROM {} l \
             WHERE l.project_id = ? AND l.log_digest = ? AND l.ordinal = ? \
             ORDER BY l.ingested_at DESC LIMIT 1",
            projection(backend),
            winning_logs(backend)
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(log_digest.to_string()),
            QueryValue::Int64(i64::from(ordinal)),
        ],
    )
}

pub fn log_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    backend: Backend,
) -> Vec<LogFilterOptionQuery> {
    const ALLOWED: &[&str] = &[
        "severity_text",
        "service_name",
        "environment",
        "event_name",
        "scope_name",
    ];
    columns
        .iter()
        .filter(|column| ALLOWED.contains(&column.as_str()))
        .map(|column| {
            let mut conditions = vec![
                "l.project_id = ?".to_string(),
                format!("l.{column} IS NOT NULL"),
                format!("l.{column} != ''"),
            ];
            let mut values = vec![QueryValue::String(project_id.to_string())];
            push_time_filter(
                &mut conditions,
                &mut values,
                from_timestamp,
                to_timestamp,
                backend,
            );
            LogFilterOptionQuery {
                column: column.clone(),
                query: ParameterizedQuery::new(
                    format!(
                        "SELECT {} AS value, {} AS count FROM {} l WHERE {} \
                         GROUP BY l.{column} ORDER BY count DESC, value ASC \
                         LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                        string_value(&format!("l.{column}"), backend),
                        count(backend),
                        winning_logs(backend),
                        conditions.join(" AND ")
                    ),
                    values,
                ),
            }
        })
        .collect()
}

fn filters(params: &ListLogsParams, backend: Backend) -> (Vec<String>, Vec<QueryValue>) {
    let mut conditions = vec!["l.project_id = ?".to_string()];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    for (column, value) in [
        ("trace_id", params.trace_id.as_ref()),
        ("span_id", params.span_id.as_ref()),
        ("severity_text", params.severity_text.as_ref()),
        ("service_name", params.service_name.as_ref()),
        ("environment", params.environment.as_ref()),
    ] {
        if let Some(value) = value {
            conditions.push(format!("l.{column} = ?"));
            values.push(QueryValue::String(value.clone()));
        }
    }
    if let Some(minimum) = params.severity_number_min {
        conditions.push("l.severity_number >= ?".to_string());
        values.push(QueryValue::Int64(i64::from(minimum)));
    }
    push_time_filter(
        &mut conditions,
        &mut values,
        params.from_timestamp,
        params.to_timestamp,
        backend,
    );
    (conditions, values)
}

fn push_time_filter(
    conditions: &mut Vec<String>,
    values: &mut Vec<QueryValue>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    backend: Backend,
) {
    if let Some(from) = from {
        conditions.push(timestamp_predicate("l.timestamp", ">=", backend));
        values.push(timestamp_value(from, backend));
    }
    if let Some(to) = to {
        conditions.push(timestamp_predicate("l.timestamp", "<=", backend));
        values.push(timestamp_value(to, backend));
    }
}

fn projection(backend: Backend) -> String {
    format!(
        "l.log_digest, l.ordinal, {} AS timestamp_us, {} AS time_us, \
         {} AS observed_time_us, l.severity_number, l.severity_text, {}, l.body_text, {}, \
         l.dropped_attributes_count, l.flags, l.trace_id, l.span_id, l.event_name, \
         l.session_id, l.user_id, l.environment, l.service_name, l.service_version, \
         l.service_namespace, l.service_instance_id, {}, l.scope_name, l.scope_version, {}, \
         l.scope_schema_url, l.resource_schema_url, {}, {} AS ingested_at_us",
        timestamp_micros("l.timestamp", backend),
        nullable_timestamp_micros("l.time", backend),
        nullable_timestamp_micros("l.observed_time", backend),
        string_value("l.body", backend),
        string_value("l.attributes", backend),
        string_value("l.resource_attributes", backend),
        string_value("l.scope_attributes", backend),
        string_value("l.raw_log", backend),
        timestamp_micros("l.ingested_at", backend),
    )
}

fn winning_logs(backend: Backend) -> &'static str {
    match backend {
        Backend::Duckdb => "otel_logs",
        Backend::Clickhouse => "(SELECT * FROM otel_logs FINAL)",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn count(backend: Backend) -> &'static str {
    match backend {
        Backend::Duckdb => "COUNT(*)",
        Backend::Clickhouse => "count()",
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn string_value(column: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("CAST({column} AS VARCHAR)"),
        Backend::Clickhouse => format!("toNullable(toString({column}))"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn timestamp_micros(column: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("EPOCH_US({column})"),
        Backend::Clickhouse => format!("toInt64(toUnixTimestamp64Micro({column}))"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn nullable_timestamp_micros(column: &str, backend: Backend) -> String {
    timestamp_micros(column, backend)
}

fn timestamp_predicate(column: &str, operator: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("{column} {operator} CAST(? AS TIMESTAMP)"),
        Backend::Clickhouse => format!("{column} {operator} fromUnixTimestamp64Micro(?)"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn timestamp_value(value: DateTime<Utc>, backend: Backend) -> QueryValue {
    match backend {
        Backend::Duckdb => QueryValue::String(value.to_rfc3339()),
        Backend::Clickhouse => QueryValue::Int64(value.timestamp_micros()),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::types::ProjectId;

    #[test]
    fn log_reads_are_scoped_parameterized_and_have_a_total_order() {
        let params = ListLogsParams {
            project_id: ProjectId::from("tenant-'quoted"),
            page: 3,
            limit: 11,
            trace_id: Some("trace' OR 1=1".to_string()),
            ..Default::default()
        };
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_logs(&params, backend);
            assert_eq!(
                page.rows.sql().matches('?').count(),
                page.rows.params().len()
            );
            assert!(!page.rows.sql().contains("tenant-'quoted"));
            assert!(!page.rows.sql().contains("OR 1=1"));
            assert!(
                page.rows
                    .sql()
                    .contains("timestamp DESC, l.log_digest DESC, l.ordinal DESC")
            );
            if backend == Backend::Clickhouse {
                assert!(page.rows.sql().contains("FINAL"));
                assert!(!page.rows.sql().contains("FINAL l"));
            }
        }
    }
}
