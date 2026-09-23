//! Metric read API endpoints.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::OtelApiState;
use super::traces::{FilterOptionDto, FilterOptionsResponse};
use crate::auth::ProjectRead;
use crate::extractors::ValidatedQuery;
use crate::types::{
    ApiError, PaginatedResponse, default_limit, default_page, parse_order_by,
    parse_timestamp_param, validate_limit, validate_page,
};
use sideseat_ports::types::{ListMetricsParams, MetricAggregateRow, MetricRow};

const METRIC_SORTABLE: &[&str] = &["timestamp", "ingested_at", "metric_name"];

#[derive(Debug, Deserialize, Validate)]
pub struct ListMetricsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    pub order_by: Option<String>,
    pub metric_name: Option<String>,
    pub metric_type: Option<String>,
    pub service_name: Option<String>,
    pub exemplar_trace_id: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    #[serde(default)]
    pub include_raw_metric: bool,
}

#[derive(Debug, Deserialize, Validate)]
pub struct MetricFilterOptionsQuery {
    pub columns: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetricDto {
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
    pub attributes: Option<serde_json::Value>,
    pub resource_attributes: Option<serde_json::Value>,
    pub scope_attributes: Option<serde_json::Value>,
    pub scope_schema_url: Option<String>,
    pub resource_schema_url: Option<String>,
    pub exemplars: Option<serde_json::Value>,
    pub flags: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_metric: Option<serde_json::Value>,
    pub ingested_at: DateTime<Utc>,
}

impl MetricDto {
    fn from_row(row: MetricRow, include_raw_metric: bool) -> Self {
        Self {
            datapoint_id: row.datapoint_id,
            metric_name: row.metric_name,
            metric_description: row.metric_description,
            metric_unit: row.metric_unit,
            metric_type: row.metric_type,
            aggregation_temporality: row.aggregation_temporality,
            is_monotonic: row.is_monotonic,
            timestamp: row.timestamp,
            start_timestamp: row.start_timestamp,
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
            exemplar_timestamp: row.exemplar_timestamp,
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            service_name: row.service_name,
            service_version: row.service_version,
            service_namespace: row.service_namespace,
            service_instance_id: row.service_instance_id,
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            attributes: parse_json(row.attributes),
            resource_attributes: parse_json(row.resource_attributes),
            scope_attributes: parse_json(row.scope_attributes),
            scope_schema_url: row.scope_schema_url,
            resource_schema_url: row.resource_schema_url,
            exemplars: parse_json(row.exemplars),
            flags: row.flags,
            raw_metric: include_raw_metric
                .then(|| parse_json(row.raw_metric))
                .flatten(),
            ingested_at: row.ingested_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetricAggregateDto {
    pub metric_name: String,
    pub metric_type: String,
    pub data_points: u64,
    pub value_sum: Option<f64>,
    pub value_min: Option<f64>,
    pub value_max: Option<f64>,
    pub value_avg: Option<f64>,
    pub latest_timestamp: DateTime<Utc>,
}

impl From<MetricAggregateRow> for MetricAggregateDto {
    fn from(row: MetricAggregateRow) -> Self {
        Self {
            metric_name: row.metric_name,
            metric_type: row.metric_type,
            data_points: row.data_points,
            value_sum: row.value_sum,
            value_min: row.value_min,
            value_max: row.value_max,
            value_avg: row.value_avg,
            latest_timestamp: row.latest_timestamp,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetricAggregatesResponse {
    pub data: Vec<MetricAggregateDto>,
}

/// List winning metric datapoints.
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/metrics",
    tag = "metrics",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("page" = Option<u32>, Query, description = "Page number"),
        ("limit" = Option<u32>, Query, description = "Items per page"),
        ("order_by" = Option<String>, Query, description = "timestamp, ingested_at, or metric_name"),
        ("metric_name" = Option<String>, Query, description = "Filter by metric name"),
        ("metric_type" = Option<String>, Query, description = "Filter by metric type"),
        ("service_name" = Option<String>, Query, description = "Filter by service name"),
        ("exemplar_trace_id" = Option<String>, Query, description = "Filter by exemplar trace ID"),
        ("from_timestamp" = Option<String>, Query, description = "Inclusive ISO 8601 lower bound"),
        ("to_timestamp" = Option<String>, Query, description = "Inclusive ISO 8601 upper bound"),
        ("include_raw_metric" = Option<bool>, Query, description = "Include raw OTLP metric JSON")
    ),
    responses((status = 200, description = "Metric datapoints"))
)]
pub async fn list_metrics(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListMetricsQuery>,
) -> Result<Json<PaginatedResponse<MetricDto>>, ApiError> {
    let params = metric_params(&auth.project_id, &query)?;
    let (rows, total) = state
        .analytics
        .list_metrics(&params)
        .await
        .map_err(ApiError::from_data)?;
    let data = rows
        .into_iter()
        .map(|row| MetricDto::from_row(row, query.include_raw_metric))
        .collect();
    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

/// Read one metric datapoint by deterministic identity.
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/metrics/{datapoint_id}",
    tag = "metrics",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("datapoint_id" = String, Path, description = "Deterministic datapoint ID"),
        ("include_raw_metric" = Option<bool>, Query, description = "Include raw OTLP metric JSON")
    ),
    responses(
        (status = 200, description = "Metric datapoint", body = MetricDto),
        (status = 404, description = "Metric datapoint not found")
    )
)]
pub async fn get_metric(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    Path(datapoint_id): Path<String>,
    ValidatedQuery(query): ValidatedQuery<MetricDetailQuery>,
) -> Result<Json<MetricDto>, ApiError> {
    let row = state
        .analytics
        .get_metric(&auth.project_id, &datapoint_id)
        .await
        .map_err(ApiError::from_data)?
        .ok_or_else(|| {
            ApiError::not_found(
                "METRIC_NOT_FOUND",
                format!("Metric datapoint not found: {datapoint_id}"),
            )
        })?;
    Ok(Json(MetricDto::from_row(row, query.include_raw_metric)))
}

#[derive(Debug, Deserialize, Validate)]
pub struct MetricDetailQuery {
    #[serde(default)]
    pub include_raw_metric: bool,
}

/// Aggregate filtered metric datapoints by metric name and type.
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/metrics/aggregates",
    tag = "metrics",
    responses((status = 200, description = "Metric aggregates", body = MetricAggregatesResponse))
)]
pub async fn aggregate_metrics(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListMetricsQuery>,
) -> Result<Json<MetricAggregatesResponse>, ApiError> {
    let params = metric_params(&auth.project_id, &query)?;
    let data = state
        .analytics
        .aggregate_metrics(&params)
        .await
        .map_err(ApiError::from_data)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(MetricAggregatesResponse { data }))
}

/// Read filter options for metric dimensions.
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/metrics/filter-options",
    tag = "metrics",
    responses((status = 200, description = "Metric filter options", body = FilterOptionsResponse))
)]
pub async fn get_metric_filter_options(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<MetricFilterOptionsQuery>,
) -> Result<Json<FilterOptionsResponse>, ApiError> {
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;
    let columns: Vec<String> = query
        .columns
        .as_deref()
        .map(|columns| {
            columns
                .split(',')
                .map(str::trim)
                .filter(|column| !column.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            [
                "metric_name",
                "metric_type",
                "service_name",
                "environment",
                "scope_name",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        });
    let options: HashMap<String, Vec<FilterOptionDto>> = state
        .analytics
        .get_metric_filter_options(&auth.project_id, &columns, from_timestamp, to_timestamp)
        .await
        .map_err(ApiError::from_data)?
        .into_iter()
        .map(|(column, values)| {
            (
                column,
                values
                    .into_iter()
                    .map(|value| FilterOptionDto {
                        value: value.value,
                        count: value.count,
                    })
                    .collect(),
            )
        })
        .collect();
    Ok(Json(FilterOptionsResponse { options }))
}

fn metric_params(
    project_id: &sideseat_ports::types::ProjectId,
    query: &ListMetricsQuery,
) -> Result<ListMetricsParams, ApiError> {
    Ok(ListMetricsParams {
        project_id: project_id.clone(),
        page: query.page,
        limit: query.limit,
        order_by: query
            .order_by
            .as_deref()
            .map(|order| parse_order_by(order, METRIC_SORTABLE))
            .transpose()?,
        metric_name: query.metric_name.clone(),
        metric_type: query.metric_type.clone(),
        service_name: query.service_name.clone(),
        exemplar_trace_id: query.exemplar_trace_id.clone(),
        from_timestamp: parse_timestamp_param(&query.from_timestamp)?,
        to_timestamp: parse_timestamp_param(&query.to_timestamp)?,
    })
}

fn parse_json(value: Option<String>) -> Option<serde_json::Value> {
    value.and_then(|value| serde_json::from_str(&value).ok())
}
