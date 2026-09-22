//! ClickHouse metric repository.
//!
//! Provides high-throughput batch writes for normalized metrics.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};

use crate::ClickhouseError;
use sideseat_core::utils::json::json_to_opt_string;
use sideseat_ports::traits::FilterOptionRow;
use sideseat_ports::types::{
    ListMetricsParams, MetricAggregateRow, MetricRow as MetricResultRow, NormalizedMetric,
    ProjectId,
};
use sideseat_query_sql::{Backend, confirmations, dml, metrics as metric_sql};

use super::query::bind_analytics_values;

/// Row structure for inserting metrics into ClickHouse
#[derive(Row, Serialize)]
struct MetricRow {
    project_id: String,
    datapoint_id: String,
    metric_name: String,
    metric_description: Option<String>,
    metric_unit: Option<String>,
    metric_type: String,
    aggregation_temporality: Option<String>,
    is_monotonic: Option<u8>,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    timestamp: time::OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    start_timestamp: Option<time::OffsetDateTime>,
    value_int: Option<i64>,
    value_double: Option<f64>,
    histogram_count: Option<u64>,
    histogram_sum: Option<f64>,
    histogram_min: Option<f64>,
    histogram_max: Option<f64>,
    histogram_bucket_counts: Option<String>,
    histogram_explicit_bounds: Option<String>,
    exp_histogram_scale: Option<i32>,
    exp_histogram_zero_count: Option<u64>,
    exp_histogram_zero_threshold: Option<f64>,
    exp_histogram_positive: Option<String>,
    exp_histogram_negative: Option<String>,
    summary_count: Option<u64>,
    summary_sum: Option<f64>,
    summary_quantiles: Option<String>,
    exemplar_trace_id: Option<String>,
    exemplar_span_id: Option<String>,
    exemplar_value_int: Option<i64>,
    exemplar_value_double: Option<f64>,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    exemplar_timestamp: Option<time::OffsetDateTime>,
    exemplar_attributes: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    attributes: Option<String>,
    resource_attributes: Option<String>,
    flags: Option<i32>,
    raw_metric: Option<String>,
    scope_attributes: Option<String>,
    scope_schema_url: Option<String>,
    resource_schema_url: Option<String>,
    exemplars: Option<String>,
    /// The replacing engine's version. See `NormalizedMetric::ingested_at`.
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    ingested_at: time::OffsetDateTime,
    content_digest: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    hold_until: Option<time::OffsetDateTime>,
    logical_bytes: u64,
}

#[derive(Row, Deserialize)]
struct ChMetricRow {
    datapoint_id: String,
    metric_name: String,
    metric_description: Option<String>,
    metric_unit: Option<String>,
    metric_type: String,
    aggregation_temporality: Option<String>,
    is_monotonic: Option<u8>,
    timestamp_us: i64,
    start_timestamp_us: Option<i64>,
    value_int: Option<i64>,
    value_double: Option<f64>,
    histogram_count: Option<u64>,
    histogram_sum: Option<f64>,
    histogram_min: Option<f64>,
    histogram_max: Option<f64>,
    summary_count: Option<u64>,
    summary_sum: Option<f64>,
    exemplar_trace_id: Option<String>,
    exemplar_span_id: Option<String>,
    exemplar_timestamp_us: Option<i64>,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    attributes: Option<String>,
    resource_attributes: Option<String>,
    scope_attributes: Option<String>,
    scope_schema_url: Option<String>,
    resource_schema_url: Option<String>,
    exemplars: Option<String>,
    flags: Option<i32>,
    raw_metric: Option<String>,
    ingested_at_us: i64,
}

impl From<ChMetricRow> for MetricResultRow {
    fn from(row: ChMetricRow) -> Self {
        Self {
            datapoint_id: row.datapoint_id,
            metric_name: row.metric_name,
            metric_description: row.metric_description,
            metric_unit: row.metric_unit,
            metric_type: row.metric_type,
            aggregation_temporality: row.aggregation_temporality,
            is_monotonic: row.is_monotonic.map(|value| value != 0),
            timestamp: datetime_from_micros(row.timestamp_us),
            start_timestamp: row
                .start_timestamp_us
                .and_then(DateTime::from_timestamp_micros),
            value_int: row.value_int,
            value_double: row.value_double,
            histogram_count: row.histogram_count,
            histogram_sum: row.histogram_sum,
            histogram_min: row.histogram_min,
            histogram_max: row.histogram_max,
            summary_count: row.summary_count,
            summary_sum: row.summary_sum,
            exemplar_trace_id: row.exemplar_trace_id,
            exemplar_span_id: row.exemplar_span_id,
            exemplar_timestamp: row
                .exemplar_timestamp_us
                .and_then(DateTime::from_timestamp_micros),
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            service_name: row.service_name,
            service_version: row.service_version,
            service_namespace: row.service_namespace,
            service_instance_id: row.service_instance_id,
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            attributes: row.attributes,
            resource_attributes: row.resource_attributes,
            scope_attributes: row.scope_attributes,
            scope_schema_url: row.scope_schema_url,
            resource_schema_url: row.resource_schema_url,
            exemplars: row.exemplars,
            flags: row.flags,
            raw_metric: row.raw_metric,
            ingested_at: datetime_from_micros(row.ingested_at_us),
        }
    }
}

#[derive(Row, Deserialize)]
struct ChMetricAggregateRow {
    metric_name: String,
    metric_type: String,
    data_points: u64,
    value_sum: Option<f64>,
    value_min: Option<f64>,
    value_max: Option<f64>,
    value_avg: Option<f64>,
    latest_timestamp_us: i64,
}

impl From<ChMetricAggregateRow> for MetricAggregateRow {
    fn from(row: ChMetricAggregateRow) -> Self {
        Self {
            metric_name: row.metric_name,
            metric_type: row.metric_type,
            data_points: row.data_points,
            value_sum: row.value_sum,
            value_min: row.value_min,
            value_max: row.value_max,
            value_avg: row.value_avg,
            latest_timestamp: datetime_from_micros(row.latest_timestamp_us),
        }
    }
}

#[derive(Row, Deserialize)]
struct ChFilterOptionRow {
    value: Option<String>,
    count: u64,
}

/// Convert chrono DateTime to time OffsetDateTime for a storage-row column.
///
/// Via seconds plus the subsecond part, never nanoseconds-since-epoch. `timestamp_nanos_opt` is `None`
/// outside 1677-2262 - narrower than the `DateTime64(6)` column it feeds - and the fallback was the
/// **epoch**, which the schema's 90-day TTL then deleted. Ingestion refuses an unstorable instant
/// (`utils::time::is_storable`); this backstops a value that reached here anyway by clamping to the nearest
/// representable bound (`clamp_to_storable`) and logging, rather than the epoch (the one value the TTL
/// destroys) or an out-of-range year the driver cannot encode.
fn chrono_to_time(dt: chrono::DateTime<chrono::Utc>) -> time::OffsetDateTime {
    let (dt, clamped) = sideseat_core::utils::time::clamp_to_storable(dt);
    if clamped {
        tracing::error!(
            timestamp = %dt,
            "A timestamp outside the storable range reached a storage-row conversion; clamped to the \
             representable bound. Ingestion should have refused it - a write path is missing the is_storable \
             check."
        );
    }
    // In range now, so the conversion cannot fail; the epoch fallback is unreachable and only satisfies the
    // type.
    time::OffsetDateTime::from_unix_timestamp(dt.timestamp())
        .map(|t| t + time::Duration::nanoseconds(i64::from(dt.timestamp_subsec_nanos())))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

impl From<&NormalizedMetric> for MetricRow {
    fn from(metric: &NormalizedMetric) -> Self {
        let project_id = metric.project_id.clone().unwrap_or_default();
        if project_id.is_empty() {
            tracing::warn!(
                metric_name = %metric.metric_name,
                "Inserting metric with empty project_id - data isolation may be compromised"
            );
        }

        Self {
            project_id,
            datapoint_id: metric.datapoint_id.clone(),
            metric_name: metric.metric_name.clone(),
            metric_description: metric.metric_description.clone(),
            metric_unit: metric.metric_unit.clone(),
            metric_type: metric.metric_type.as_str().to_string(),
            aggregation_temporality: Some(metric.aggregation_temporality.as_str().to_string()),
            is_monotonic: metric.is_monotonic.map(|b| if b { 1 } else { 0 }),
            timestamp: chrono_to_time(metric.timestamp),
            start_timestamp: metric.start_timestamp.map(chrono_to_time),
            value_int: metric.value_int,
            value_double: metric.value_double,
            histogram_count: metric.histogram_count,
            histogram_sum: metric.histogram_sum,
            histogram_min: metric.histogram_min,
            histogram_max: metric.histogram_max,
            histogram_bucket_counts: json_to_opt_string(&metric.histogram_bucket_counts),
            histogram_explicit_bounds: json_to_opt_string(&metric.histogram_explicit_bounds),
            exp_histogram_scale: metric.exp_histogram_scale,
            exp_histogram_zero_count: metric.exp_histogram_zero_count,
            exp_histogram_zero_threshold: metric.exp_histogram_zero_threshold,
            exp_histogram_positive: json_to_opt_string(&metric.exp_histogram_positive),
            exp_histogram_negative: json_to_opt_string(&metric.exp_histogram_negative),
            summary_count: metric.summary_count,
            summary_sum: metric.summary_sum,
            summary_quantiles: json_to_opt_string(&metric.summary_quantiles),
            exemplar_trace_id: metric.exemplar_trace_id.clone(),
            exemplar_span_id: metric.exemplar_span_id.clone(),
            exemplar_value_int: metric.exemplar_value_int,
            exemplar_value_double: metric.exemplar_value_double,
            exemplar_timestamp: metric.exemplar_timestamp.map(chrono_to_time),
            exemplar_attributes: json_to_opt_string(&metric.exemplar_attributes),
            session_id: metric.session_id.clone(),
            user_id: metric.user_id.clone(),
            environment: metric.environment.clone(),
            service_name: metric.service_name.clone(),
            service_version: metric.service_version.clone(),
            service_namespace: metric.service_namespace.clone(),
            service_instance_id: metric.service_instance_id.clone(),
            scope_name: metric.scope_name.clone(),
            scope_version: metric.scope_version.clone(),
            attributes: json_to_opt_string(&metric.attributes),
            resource_attributes: json_to_opt_string(&metric.resource_attributes),
            flags: Some(metric.flags as i32),
            raw_metric: json_to_opt_string(&metric.raw_metric),
            scope_attributes: json_to_opt_string(&metric.scope_attributes),
            scope_schema_url: metric.scope_schema_url.clone(),
            resource_schema_url: metric.resource_schema_url.clone(),
            exemplars: json_to_opt_string(&metric.exemplars),
            // Direct conversion is used by tests; production goes through `ClickhouseRepository`, which
            // stamps the whole batch from its injected clock.
            ingested_at: chrono_to_time(metric.ingested_at.unwrap_or(chrono::DateTime::UNIX_EPOCH)),
            content_digest: metric.content_digest.clone(),
            hold_until: metric.hold_until.map(chrono_to_time),
            logical_bytes: metric.logical_bytes,
        }
    }
}

/// Insert a batch of metrics into ClickHouse
///
/// For distributed mode, inserts go directly to the local table (`otel_metrics_local`)
/// for better performance. In single-node mode, inserts go to `otel_metrics`.
pub async fn insert_batch(
    client: &Client,
    table_name: &str,
    metrics: &[NormalizedMetric],
) -> Result<(), ClickhouseError> {
    if metrics.is_empty() {
        return Ok(());
    }
    let target = dml::metric_write_target(Backend::Clickhouse, Some(table_name));

    let mut insert: clickhouse::insert::Insert<MetricRow> = client.insert(target.table()).await?;

    for metric in metrics {
        let row = MetricRow::from(metric);
        insert.write(&row).await?;
    }

    insert.end().await?;
    Ok(())
}

pub async fn list_metrics(
    client: &Client,
    params: &ListMetricsParams,
) -> Result<(Vec<MetricResultRow>, u64), ClickhouseError> {
    let page = metric_sql::list_metrics(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChMetricRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(Into::into).collect(), total))
}

pub async fn get_metric(
    client: &Client,
    project_id: &ProjectId,
    datapoint_id: &str,
) -> Result<Option<MetricResultRow>, ClickhouseError> {
    let query = metric_sql::get_metric(project_id.as_str(), datapoint_id, Backend::Clickhouse);
    let row: Option<ChMetricRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_optional()
        .await?;
    Ok(row.map(Into::into))
}

pub async fn matches_content(
    client: &Client,
    project_id: &ProjectId,
    records: &[(String, String)],
) -> Result<bool, ClickhouseError> {
    let Some(plan) = confirmations::metrics(project_id.as_str(), records, Backend::Clickhouse)
    else {
        return Ok(true);
    };
    let found: u64 = bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
        .fetch_one()
        .await?;
    Ok(found == plan.expected)
}

pub async fn aggregate_metrics(
    client: &Client,
    params: &ListMetricsParams,
) -> Result<Vec<MetricAggregateRow>, ClickhouseError> {
    let query = metric_sql::aggregate_metrics(params, Backend::Clickhouse);
    let rows: Vec<ChMetricAggregateRow> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_metric_filter_options(
    client: &Client,
    project_id: &ProjectId,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<HashMap<String, Vec<FilterOptionRow>>, ClickhouseError> {
    let mut result = HashMap::new();
    for option in metric_sql::metric_filter_options(
        project_id.as_str(),
        columns,
        from_timestamp,
        to_timestamp,
        Backend::Clickhouse,
    ) {
        let rows: Vec<ChFilterOptionRow> =
            bind_analytics_values(client.query(option.query.sql()), option.query.params())
                .fetch_all()
                .await?;
        result.insert(
            option.column,
            rows.into_iter()
                .filter_map(|row| {
                    row.value.map(|value| FilterOptionRow {
                        value,
                        count: row.count,
                    })
                })
                .collect(),
        );
    }
    Ok(result)
}

fn datetime_from_micros(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value).unwrap_or(DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_ports::types::MetricType;

    #[test]
    fn test_metric_row_from_normalized_metric() {
        let metric = NormalizedMetric {
            project_id: Some("test".to_string()),
            metric_name: "test.metric".to_string(),
            metric_type: MetricType::Gauge,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        };

        let row = MetricRow::from(&metric);
        assert_eq!(row.project_id, "test");
        assert_eq!(row.metric_name, "test.metric");
        assert_eq!(row.metric_type, "gauge");
    }
}
