//! Typed reads over metric datapoint winners.

use chrono::{DateTime, Utc};
use sideseat_core::constants::QUERY_MAX_FILTER_SUGGESTIONS;
use sideseat_ports::types::{ListMetricsParams, OrderDirection};

use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

#[derive(Debug, Clone, PartialEq)]
pub struct MetricPage {
    pub count: ParameterizedQuery,
    pub rows: ParameterizedQuery,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricFilterOptionQuery {
    pub column: String,
    pub query: ParameterizedQuery,
}

pub fn list_metrics(params: &ListMetricsParams, backend: Backend) -> MetricPage {
    let (conditions, values) = filters(params, backend);
    let source = winning_metrics(backend);
    let where_clause = conditions.join(" AND ");
    let count = ParameterizedQuery::new(
        format!(
            "SELECT {} AS count FROM {source} m WHERE {where_clause}",
            count(backend)
        ),
        values.clone(),
    );
    let order = metric_order(params);
    let offset = params.page.saturating_sub(1) as u64 * params.limit as u64;
    let rows = ParameterizedQuery::new(
        format!(
            "SELECT {} FROM {source} m WHERE {where_clause} \
             ORDER BY {order} LIMIT {} OFFSET {offset}",
            projection(backend),
            params.limit
        ),
        values,
    );
    MetricPage { count, rows }
}

pub fn get_metric(project_id: &str, datapoint_id: &str, backend: Backend) -> ParameterizedQuery {
    ParameterizedQuery::new(
        format!(
            "SELECT {} FROM {} m WHERE m.project_id = ? AND m.datapoint_id = ? \
             ORDER BY m.ingested_at DESC LIMIT 1",
            projection(backend),
            winning_metrics(backend)
        ),
        vec![
            QueryValue::String(project_id.to_string()),
            QueryValue::String(datapoint_id.to_string()),
        ],
    )
}

pub fn aggregate_metrics(params: &ListMetricsParams, backend: Backend) -> ParameterizedQuery {
    let (conditions, values) = filters(params, backend);
    let numeric = match backend {
        Backend::Duckdb => {
            "COALESCE(CAST(m.value_int AS DOUBLE), m.value_double, m.histogram_sum, m.summary_sum)"
        }
        Backend::Clickhouse => {
            "coalesce(toFloat64(m.value_int), m.value_double, m.histogram_sum, m.summary_sum)"
        }
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    };
    let latest = timestamp_micros("MAX(m.timestamp)", backend);
    ParameterizedQuery::new(
        format!(
            "SELECT m.metric_name, m.metric_type, {} AS data_points, \
             SUM({numeric}) AS value_sum, MIN({numeric}) AS value_min, \
             MAX({numeric}) AS value_max, AVG({numeric}) AS value_avg, \
             {latest} AS latest_timestamp_us \
             FROM {} m WHERE {} GROUP BY m.metric_name, m.metric_type \
             ORDER BY m.metric_name ASC, m.metric_type ASC",
            count(backend),
            winning_metrics(backend),
            conditions.join(" AND ")
        ),
        values,
    )
}

pub fn metric_filter_options(
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    backend: Backend,
) -> Vec<MetricFilterOptionQuery> {
    const ALLOWED: &[&str] = &[
        "metric_name",
        "metric_type",
        "service_name",
        "environment",
        "scope_name",
    ];
    columns
        .iter()
        .filter(|column| ALLOWED.contains(&column.as_str()))
        .map(|column| {
            let mut conditions = vec![
                "m.project_id = ?".to_string(),
                format!("m.{column} IS NOT NULL"),
                format!("m.{column} != ''"),
            ];
            let mut values = vec![QueryValue::String(project_id.to_string())];
            push_time_filter(
                &mut conditions,
                &mut values,
                from_timestamp,
                to_timestamp,
                backend,
            );
            MetricFilterOptionQuery {
                column: column.clone(),
                query: ParameterizedQuery::new(
                    format!(
                        "SELECT {} AS value, {} AS count FROM {} m WHERE {} \
                         GROUP BY m.{column} ORDER BY count DESC, value ASC \
                         LIMIT {QUERY_MAX_FILTER_SUGGESTIONS}",
                        string_value(&format!("m.{column}"), backend),
                        count(backend),
                        winning_metrics(backend),
                        conditions.join(" AND ")
                    ),
                    values,
                ),
            }
        })
        .collect()
}

fn filters(params: &ListMetricsParams, backend: Backend) -> (Vec<String>, Vec<QueryValue>) {
    let mut conditions = vec!["m.project_id = ?".to_string()];
    let mut values = vec![QueryValue::String(params.project_id.to_string())];
    for (column, value) in [
        ("metric_name", params.metric_name.as_ref()),
        ("metric_type", params.metric_type.as_ref()),
        ("service_name", params.service_name.as_ref()),
        ("exemplar_trace_id", params.exemplar_trace_id.as_ref()),
    ] {
        if let Some(value) = value {
            conditions.push(format!("m.{column} = ?"));
            values.push(QueryValue::String(value.clone()));
        }
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
        conditions.push(timestamp_predicate("m.timestamp", ">=", backend));
        values.push(timestamp_value(from, backend));
    }
    if let Some(to) = to {
        conditions.push(timestamp_predicate("m.timestamp", "<=", backend));
        values.push(timestamp_value(to, backend));
    }
}

fn metric_order(params: &ListMetricsParams) -> String {
    let (column, direction) = params
        .order_by
        .as_ref()
        .and_then(|order| {
            matches!(
                order.column.as_str(),
                "timestamp" | "ingested_at" | "metric_name"
            )
            .then_some((order.column.as_str(), order.direction))
        })
        .unwrap_or(("timestamp", OrderDirection::Desc));
    let direction = match direction {
        OrderDirection::Asc => "ASC",
        OrderDirection::Desc => "DESC",
    };
    format!("m.{column} {direction}, m.datapoint_id {direction}")
}

fn projection(backend: Backend) -> String {
    format!(
        "m.datapoint_id, m.metric_name, m.metric_description, m.metric_unit, \
         m.metric_type, m.aggregation_temporality, m.is_monotonic, \
         {} AS timestamp_us, {} AS start_timestamp_us, \
         m.value_int, m.value_double, m.histogram_count, m.histogram_sum, \
         m.histogram_min, m.histogram_max, m.summary_count, m.summary_sum, \
         m.exemplar_trace_id, m.exemplar_span_id, {} AS exemplar_timestamp_us, \
         m.session_id, m.user_id, m.environment, m.service_name, m.service_version, \
         m.service_namespace, m.service_instance_id, m.scope_name, m.scope_version, \
         {}, {}, {}, m.scope_schema_url, m.resource_schema_url, {}, \
         m.flags, {}, {} AS ingested_at_us",
        nullable_timestamp_micros("m.timestamp", backend),
        nullable_timestamp_micros("m.start_timestamp", backend),
        nullable_timestamp_micros("m.exemplar_timestamp", backend),
        string_value("m.attributes", backend),
        string_value("m.resource_attributes", backend),
        string_value("m.scope_attributes", backend),
        string_value("m.exemplars", backend),
        string_value("m.raw_metric", backend),
        nullable_timestamp_micros("m.ingested_at", backend),
    )
}

fn winning_metrics(backend: Backend) -> &'static str {
    match backend {
        Backend::Duckdb => "otel_metrics",
        Backend::Clickhouse => "(SELECT * FROM otel_metrics FINAL)",
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
        Backend::Clickhouse => format!("toString({column})"),
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
    match backend {
        Backend::Duckdb => format!("EPOCH_US({column})"),
        Backend::Clickhouse => format!("toInt64(toUnixTimestamp64Micro({column}))"),
        Backend::Sqlite | Backend::Postgres => unreachable!(),
    }
}

fn timestamp_predicate(column: &str, operator: &str, backend: Backend) -> String {
    match backend {
        Backend::Duckdb => format!("{column} {operator} CAST(? AS TIMESTAMP)"),
        Backend::Clickhouse => {
            format!("{column} {operator} fromUnixTimestamp64Micro(?)")
        }
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
    fn metric_reads_are_tenant_scoped_parameterized_and_winner_aware() {
        let params = ListMetricsParams {
            project_id: ProjectId::from("tenant-'quoted"),
            page: 2,
            limit: 17,
            metric_name: Some("latency' OR 1=1".to_string()),
            ..Default::default()
        };
        for backend in [Backend::Duckdb, Backend::Clickhouse] {
            let page = list_metrics(&params, backend);
            for query in [&page.count, &page.rows] {
                assert_eq!(query.sql().matches('?').count(), query.params().len());
                assert!(!query.sql().contains("tenant-'quoted"));
                assert!(!query.sql().contains("OR 1=1"));
            }
            if backend == Backend::Clickhouse {
                assert!(page.rows.sql().contains("FINAL"));
                assert!(!page.rows.sql().contains("FINAL m"));
            }
        }
    }
}
