//! OTLP log read and trace-correlation endpoints.

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::OtelApiState;
use super::traces::{FilterOptionDto, FilterOptionsResponse};
use crate::auth::{ProjectRead, SpanRead, TraceRead};
use crate::extractors::ValidatedQuery;
use crate::types::{
    ApiError, PaginatedResponse, default_limit, default_page, parse_timestamp_param,
    validate_limit, validate_page,
};
use sideseat_ports::types::{ListLogsParams, LogRow, ProjectId};

#[derive(Debug, Deserialize, Validate)]
pub struct ListLogsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    pub severity_text: Option<String>,
    pub severity_number_min: Option<i32>,
    pub service_name: Option<String>,
    pub environment: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    #[serde(default)]
    pub include_raw_log: bool,
}

#[derive(Debug, Deserialize, Validate)]
pub struct LogFilterOptionsQuery {
    pub columns: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogDto {
    pub log_digest: String,
    pub ordinal: u32,
    pub timestamp: DateTime<Utc>,
    pub time: Option<DateTime<Utc>>,
    pub observed_time: Option<DateTime<Utc>>,
    pub severity_number: i32,
    pub severity_text: Option<String>,
    pub body: Option<serde_json::Value>,
    pub body_text: Option<String>,
    pub attributes: Option<serde_json::Value>,
    pub dropped_attributes_count: u32,
    pub flags: u32,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub event_name: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub service_namespace: Option<String>,
    pub service_instance_id: Option<String>,
    pub resource_attributes: Option<serde_json::Value>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub scope_attributes: Option<serde_json::Value>,
    pub scope_schema_url: Option<String>,
    pub resource_schema_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_log: Option<serde_json::Value>,
    pub ingested_at: DateTime<Utc>,
}

impl LogDto {
    pub(super) fn from_row(row: LogRow, include_raw_log: bool) -> Self {
        Self {
            log_digest: row.log_digest,
            ordinal: row.ordinal,
            timestamp: row.timestamp,
            time: row.time,
            observed_time: row.observed_time,
            severity_number: row.severity_number,
            severity_text: row.severity_text,
            body: parse_json(row.body),
            body_text: row.body_text,
            attributes: parse_json(row.attributes),
            dropped_attributes_count: row.dropped_attributes_count,
            flags: row.flags,
            trace_id: row.trace_id,
            span_id: row.span_id,
            event_name: row.event_name,
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            service_name: row.service_name,
            service_version: row.service_version,
            service_namespace: row.service_namespace,
            service_instance_id: row.service_instance_id,
            resource_attributes: parse_json(row.resource_attributes),
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            scope_attributes: parse_json(row.scope_attributes),
            scope_schema_url: row.scope_schema_url,
            resource_schema_url: row.resource_schema_url,
            raw_log: include_raw_log.then(|| parse_json(row.raw_log)).flatten(),
            ingested_at: row.ingested_at,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/logs",
    tag = "logs",
    responses((status = 200, description = "Log records"))
)]
pub async fn list_logs(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListLogsQuery>,
) -> Result<Json<PaginatedResponse<LogDto>>, ApiError> {
    list(&state, &auth.project_id, None, None, query).await
}

#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/logs",
    tag = "logs",
    responses((status = 200, description = "Log records correlated to a trace"))
)]
pub async fn list_trace_logs(
    State(state): State<OtelApiState>,
    auth: TraceRead,
    ValidatedQuery(query): ValidatedQuery<ListLogsQuery>,
) -> Result<Json<PaginatedResponse<LogDto>>, ApiError> {
    list(&state, &auth.project_id, Some(auth.trace_id), None, query).await
}

#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/spans/{span_id}/logs",
    tag = "logs",
    responses((status = 200, description = "Log records correlated to a span"))
)]
pub async fn list_span_logs(
    State(state): State<OtelApiState>,
    auth: SpanRead,
    ValidatedQuery(query): ValidatedQuery<ListLogsQuery>,
) -> Result<Json<PaginatedResponse<LogDto>>, ApiError> {
    list(
        &state,
        &auth.project_id,
        Some(auth.trace_id),
        Some(auth.span_id),
        query,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/logs/filter-options",
    tag = "logs",
    responses((status = 200, description = "Log filter options", body = FilterOptionsResponse))
)]
pub async fn get_log_filter_options(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<LogFilterOptionsQuery>,
) -> Result<Json<FilterOptionsResponse>, ApiError> {
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
                "severity_text",
                "service_name",
                "environment",
                "event_name",
                "scope_name",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        });
    let options: HashMap<String, Vec<FilterOptionDto>> = state
        .analytics
        .get_log_filter_options(
            &auth.project_id,
            &columns,
            parse_timestamp_param(&query.from_timestamp)?,
            parse_timestamp_param(&query.to_timestamp)?,
        )
        .await
        .map_err(ApiError::from_data)?
        .into_iter()
        .map(|(column, rows)| {
            (
                column,
                rows.into_iter()
                    .map(|row| FilterOptionDto {
                        value: row.value,
                        count: row.count,
                    })
                    .collect(),
            )
        })
        .collect();
    Ok(Json(FilterOptionsResponse { options }))
}

async fn list(
    state: &OtelApiState,
    project_id: &ProjectId,
    trace_id: Option<String>,
    span_id: Option<String>,
    query: ListLogsQuery,
) -> Result<Json<PaginatedResponse<LogDto>>, ApiError> {
    let params = ListLogsParams {
        project_id: project_id.clone(),
        page: query.page,
        limit: query.limit,
        trace_id,
        span_id,
        severity_text: query.severity_text,
        severity_number_min: query.severity_number_min,
        service_name: query.service_name,
        environment: query.environment,
        from_timestamp: parse_timestamp_param(&query.from_timestamp)?,
        to_timestamp: parse_timestamp_param(&query.to_timestamp)?,
    };
    let (rows, total) = state
        .analytics
        .list_logs(&params)
        .await
        .map_err(ApiError::from_data)?;
    let data = rows
        .into_iter()
        .map(|row| LogDto::from_row(row, query.include_raw_log))
        .collect();
    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

fn parse_json(value: Option<String>) -> Option<serde_json::Value> {
    value.and_then(|value| serde_json::from_str(&value).ok())
}
