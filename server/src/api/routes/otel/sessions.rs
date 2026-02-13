//! Session API endpoints

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use super::OtelApiState;
use super::filters::{columns, parse_filters};
use super::traces::{FilterOptionDto, FilterOptionsResponse};
use super::types::{SessionDetailDto, SessionSummaryDto, StringOrArray, TraceInSessionDto};
use crate::api::auth::{ProjectRead, ProjectWrite, SessionRead};
use crate::api::extractors::{ValidatedJson, ValidatedQuery};
use crate::api::types::{
    ApiError, OrderBy, PaginatedResponse, default_limit, default_page, parse_timestamp_param,
    validate_ids_batch, validate_limit, validate_page,
};
use crate::data::types::{ListSessionsParams, SessionRow};

#[derive(Debug, Deserialize, Validate)]
pub struct ListSessionsQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    pub order_by: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<StringOrArray>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    pub filters: Option<String>,
}

/// List sessions with pagination and filters
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/sessions",
    tag = "sessions",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("page" = Option<u32>, Query, description = "Page number"),
        ("limit" = Option<u32>, Query, description = "Items per page"),
        ("order_by" = Option<String>, Query, description = "Sort field (e.g., start_time:desc)"),
        ("user_id" = Option<String>, Query, description = "Filter by user ID"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)")
    ),
    responses(
        (status = 200, description = "List of sessions with pagination metadata")
    )
)]
pub async fn list_sessions(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListSessionsQuery>,
) -> Result<(HeaderMap, Json<PaginatedResponse<SessionSummaryDto>>), ApiError> {
    // Parse order_by
    let order_by = if let Some(ref ob) = query.order_by {
        Some(OrderBy::parse(ob, columns::SESSION_SORTABLE)?)
    } else {
        None
    };

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse advanced filters
    let filters = if let Some(ref filters_json) = query.filters {
        parse_filters(filters_json, columns::SESSION_FILTERABLE)?
    } else {
        vec![]
    };

    let params = ListSessionsParams {
        project_id: auth.project_id.clone(),
        page: query.page,
        limit: query.limit,
        order_by,
        user_id: query.user_id,
        environment: query.environment.map(|e| e.into_vec()),
        from_timestamp,
        to_timestamp,
        filters,
    };

    let repo = state.analytics.repository();
    let (rows, total) = repo
        .list_sessions(&params)
        .await
        .map_err(ApiError::from_data)?;

    let data: Vec<SessionSummaryDto> = rows.into_iter().map(session_row_to_summary).collect();

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    // Compute Last-Modified from most recent session (HTTP-date format per RFC 7231)
    if let Some(latest) = data.first() {
        let http_date = latest
            .start_time
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        if let Ok(last_modified) = HeaderValue::from_str(&http_date) {
            headers.insert(header::LAST_MODIFIED, last_modified);
        }
    }

    Ok((
        headers,
        Json(PaginatedResponse::new(data, query.page, query.limit, total)),
    ))
}

/// Get a single session with nested trace summaries
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/sessions/{session_id}",
    tag = "sessions",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("session_id" = String, Path, description = "Session ID")
    ),
    responses(
        (status = 200, description = "Session details with traces", body = SessionDetailDto),
        (status = 404, description = "Session not found")
    )
)]
pub async fn get_session(
    State(state): State<OtelApiState>,
    auth: SessionRead,
) -> Result<Json<SessionDetailDto>, ApiError> {
    let project_id = &auth.project_id;
    let session_id = &auth.session_id;

    let repo = state.analytics.repository();
    let session = repo
        .get_session(project_id, session_id)
        .await
        .map_err(ApiError::from_data)?;

    let session = session.ok_or_else(|| {
        ApiError::not_found(
            "SESSION_NOT_FOUND",
            format!("Session not found: {}", session_id),
        )
    })?;

    let traces = repo
        .get_traces_for_session(project_id, session_id)
        .await
        .map_err(ApiError::from_data)?;

    Ok(Json(SessionDetailDto {
        summary: session_row_to_summary(session),
        traces: traces
            .into_iter()
            .map(|t| TraceInSessionDto {
                trace_id: t.trace_id,
                trace_name: t.trace_name,
                start_time: t.start_time,
                end_time: t.end_time,
                duration_ms: t.duration_ms,
                total_tokens: t.total_tokens,
                reasoning_tokens: t.reasoning_tokens,
                total_cost: t.total_cost,
                tags: t.tags,
            })
            .collect(),
    }))
}

pub(crate) fn session_row_to_summary(row: SessionRow) -> SessionSummaryDto {
    SessionSummaryDto {
        session_id: row.session_id,
        user_id: row.user_id,
        environment: row.environment,
        start_time: row.start_time,
        end_time: row.end_time,
        trace_count: row.trace_count,
        span_count: row.span_count,
        observation_count: row.observation_count,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        total_tokens: row.total_tokens,
        cache_read_tokens: row.cache_read_tokens,
        cache_write_tokens: row.cache_write_tokens,
        reasoning_tokens: row.reasoning_tokens,
        input_cost: row.input_cost,
        output_cost: row.output_cost,
        cache_read_cost: row.cache_read_cost,
        cache_write_cost: row.cache_write_cost,
        reasoning_cost: row.reasoning_cost,
        total_cost: row.total_cost,
    }
}

// --- Filter options ---

#[derive(Debug, Deserialize, Validate)]
pub struct SessionFilterOptionsQuery {
    /// Comma-separated list of columns to get options for
    pub columns: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

/// Get distinct values with counts for filterable columns
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/sessions/filter-options",
    tag = "sessions",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("columns" = Option<String>, Query, description = "Comma-separated list of columns"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)")
    ),
    responses(
        (status = 200, description = "Filter options", body = FilterOptionsResponse)
    )
)]
pub async fn get_session_filter_options(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<SessionFilterOptionsQuery>,
) -> Result<(HeaderMap, Json<FilterOptionsResponse>), ApiError> {
    use std::collections::HashMap;

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse requested columns (default to all options columns)
    let columns: Vec<String> = query
        .columns
        .as_ref()
        .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["environment".to_string(), "user_id".to_string()]);

    let repo = state.analytics.repository();
    let column_options = repo
        .get_session_filter_options(&auth.project_id, &columns, from_timestamp, to_timestamp)
        .await
        .map_err(ApiError::from_data)?;

    // Convert to DTO format
    let options: HashMap<String, Vec<FilterOptionDto>> = column_options
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                v.into_iter()
                    .map(|o| FilterOptionDto {
                        value: o.value,
                        count: o.count,
                    })
                    .collect(),
            )
        })
        .collect();

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=30"),
    );

    Ok((headers, Json(FilterOptionsResponse { options })))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct DeleteSessionsBody {
    #[validate(custom(function = "validate_ids_batch"))]
    pub session_ids: Vec<String>,
}

/// Delete multiple sessions by IDs (deletes all traces in the sessions)
#[utoipa::path(
    delete,
    path = "/api/v1/project/{project_id}/otel/sessions",
    tag = "sessions",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    request_body = DeleteSessionsBody,
    responses(
        (status = 204, description = "Sessions deleted successfully"),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn delete_sessions(
    State(state): State<OtelApiState>,
    auth: ProjectWrite,
    ValidatedJson(body): ValidatedJson<DeleteSessionsBody>,
) -> Result<StatusCode, ApiError> {
    let analytics_repo = state.analytics.repository();

    // Get trace_ids for these sessions BEFORE deletion (for file cleanup)
    let trace_ids_for_cleanup = if state.file_service.is_enabled() {
        analytics_repo
            .get_trace_ids_for_sessions(&auth.project_id, &body.session_ids)
            .await
            .map_err(ApiError::from_data)?
    } else {
        vec![]
    };

    // Delete from analytics backend
    analytics_repo
        .delete_sessions(&auth.project_id, &body.session_ids)
        .await
        .map_err(ApiError::from_data)?;

    // Cleanup files associated with deleted traces
    if !trace_ids_for_cleanup.is_empty()
        && let Err(e) = state
            .file_service
            .cleanup_traces(&auth.project_id, &trace_ids_for_cleanup)
            .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            traces = trace_ids_for_cleanup.len(),
            "Failed to cleanup files after session deletion"
        );
    }

    // Cleanup favorites for deleted sessions
    let repo = state.database.repository();
    if let Err(e) = repo
        .delete_favorites_by_entity("session", &body.session_ids, &auth.project_id)
        .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            sessions = body.session_ids.len(),
            "Failed to cleanup session favorites after deletion"
        );
    }

    // Also cleanup favorites for deleted traces within these sessions
    if !trace_ids_for_cleanup.is_empty()
        && let Err(e) = repo
            .delete_favorites_by_entity("trace", &trace_ids_for_cleanup, &auth.project_id)
            .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            traces = trace_ids_for_cleanup.len(),
            "Failed to cleanup trace favorites after session deletion"
        );
    }

    Ok(StatusCode::NO_CONTENT)
}
