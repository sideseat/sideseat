//! ClickHouse metric repository
//!
//! Provides high-throughput batch writes for normalized metrics.

use clickhouse::Client;
use clickhouse::Row;
use serde::Serialize;

use crate::data::clickhouse::ClickhouseError;
use crate::data::types::NormalizedMetric;
use crate::utils::json::json_to_opt_string;

/// Row structure for inserting metrics into ClickHouse
#[derive(Row, Serialize)]
struct MetricRow {
    project_id: String,
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
}

/// Convert chrono DateTime to time OffsetDateTime
fn chrono_to_time(dt: chrono::DateTime<chrono::Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(dt.timestamp_nanos_opt().unwrap_or(0) as i128)
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

    let mut insert: clickhouse::insert::Insert<MetricRow> = client.insert(table_name).await?;

    for metric in metrics {
        let row = MetricRow::from(metric);
        insert.write(&row).await?;
    }

    insert.end().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::MetricType;

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
