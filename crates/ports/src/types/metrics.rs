//! Metric read models and query parameters.

use chrono::{DateTime, Utc};

use super::{ProjectId, order::OrderBy};

#[derive(Debug, Clone)]
pub struct MetricRow {
    pub datapoint_id: String,
    pub metric_name: String,
    pub metric_description: Option<String>,
    pub metric_unit: Option<String>,
    pub metric_type: String,
    pub aggregation_temporality: Option<String>,
    pub is_monotonic: Option<bool>,
    pub timestamp: DateTime<Utc>,
    pub start_timestamp: Option<DateTime<Utc>>,
    pub value_int: Option<i64>,
    pub value_double: Option<f64>,
    pub histogram_count: Option<u64>,
    pub histogram_sum: Option<f64>,
    pub histogram_min: Option<f64>,
    pub histogram_max: Option<f64>,
    pub summary_count: Option<u64>,
    pub summary_sum: Option<f64>,
    pub exemplar_trace_id: Option<String>,
    pub exemplar_span_id: Option<String>,
    pub exemplar_timestamp: Option<DateTime<Utc>>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub service_namespace: Option<String>,
    pub service_instance_id: Option<String>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub attributes: Option<String>,
    pub resource_attributes: Option<String>,
    pub scope_attributes: Option<String>,
    pub scope_schema_url: Option<String>,
    pub resource_schema_url: Option<String>,
    pub exemplars: Option<String>,
    pub flags: Option<i32>,
    pub raw_metric: Option<String>,
    pub ingested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct ListMetricsParams {
    pub project_id: ProjectId,
    pub page: u32,
    pub limit: u32,
    pub order_by: Option<OrderBy>,
    pub metric_name: Option<String>,
    pub metric_type: Option<String>,
    pub service_name: Option<String>,
    pub exemplar_trace_id: Option<String>,
    pub from_timestamp: Option<DateTime<Utc>>,
    pub to_timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct MetricAggregateRow {
    pub metric_name: String,
    pub metric_type: String,
    pub data_points: u64,
    pub value_sum: Option<f64>,
    pub value_min: Option<f64>,
    pub value_max: Option<f64>,
    pub value_avg: Option<f64>,
    pub latest_timestamp: DateTime<Utc>,
}
