//! Span API endpoints

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use super::OtelApiState;
use super::filters::{columns, parse_filters};
use super::traces::{FilterOptionDto, FilterOptionsResponse};
use super::types::{SpanDetailDto, SpanSummaryDto, StringOrArray};
use crate::api::auth::{ProjectRead, ProjectWrite, SpanRead, TraceRead};
use crate::api::extractors::{ValidatedJson, ValidatedQuery};
use crate::api::types::{
    ApiError, OrderBy, PaginatedResponse, default_limit, default_page, parse_timestamp_param,
    validate_limit, validate_page,
};
use crate::data::types::{
    ListSpansParams, filter_observations, get_observation_cost, get_observation_tokens,
    get_observation_type, is_observation,
};

#[derive(Debug, Deserialize, Validate)]
pub struct ListSpansQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    pub order_by: Option<String>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<StringOrArray>,
    pub span_category: Option<String>,
    pub observation_type: Option<String>,
    pub framework: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub status_code: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    pub filters: Option<String>,
    /// Include raw OTLP span JSON in response (default: false)
    #[serde(default)]
    pub include_raw_span: bool,
    /// Filter to observations only (spans with observation_type OR gen_ai_request_model)
    pub is_observation: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SpanDetailQuery {
    /// Include raw OTLP span JSON in response (default: false)
    #[serde(default)]
    pub include_raw_span: bool,
}

/// List spans with pagination and filters
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/spans",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("page" = Option<u32>, Query, description = "Page number"),
        ("limit" = Option<u32>, Query, description = "Items per page"),
        ("order_by" = Option<String>, Query, description = "Sort field (e.g., timestamp_start:desc)"),
        ("trace_id" = Option<String>, Query, description = "Filter by trace ID"),
        ("session_id" = Option<String>, Query, description = "Filter by session ID"),
        ("user_id" = Option<String>, Query, description = "Filter by user ID"),
        ("span_category" = Option<String>, Query, description = "Filter by span category"),
        ("observation_type" = Option<String>, Query, description = "Filter by observation type"),
        ("framework" = Option<String>, Query, description = "Filter by framework"),
        ("gen_ai_request_model" = Option<String>, Query, description = "Filter by model"),
        ("status_code" = Option<String>, Query, description = "Filter by status code"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("include_raw_span" = Option<bool>, Query, description = "Include raw OTLP span JSON (default: false)"),
        ("is_observation" = Option<bool>, Query, description = "Filter to observations only (spans with observation_type OR gen_ai_request_model)")
    ),
    responses(
        (status = 200, description = "List of spans with pagination metadata")
    )
)]
pub async fn list_spans(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListSpansQuery>,
) -> Result<(HeaderMap, Json<PaginatedResponse<SpanSummaryDto>>), ApiError> {
    // Parse order_by
    let order_by = if let Some(ref ob) = query.order_by {
        Some(OrderBy::parse(ob, columns::SPAN_SORTABLE)?)
    } else {
        None
    };

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse advanced filters
    let filters = if let Some(ref filters_json) = query.filters {
        parse_filters(filters_json, columns::SPAN_FILTERABLE)?
    } else {
        vec![]
    };

    let params = ListSpansParams {
        project_id: auth.project_id.clone(),
        page: query.page,
        limit: query.limit,
        order_by,
        trace_id: query.trace_id,
        session_id: query.session_id,
        user_id: query.user_id,
        environment: query.environment.map(|e| e.into_vec()),
        span_category: query.span_category,
        observation_type: query.observation_type,
        framework: query.framework,
        gen_ai_request_model: query.gen_ai_request_model,
        status_code: query.status_code,
        from_timestamp,
        to_timestamp,
        filters,
        is_observation: query.is_observation,
    };

    let repo = state.analytics.repository();
    let (rows, total) = repo
        .list_spans(&params)
        .await
        .map_err(ApiError::from_data)?;

    // Bulk fetch event and link counts (avoids N+1 queries)
    let span_keys: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.trace_id.clone(), r.span_id.clone()))
        .collect();
    let counts = repo
        .get_span_counts_bulk(&auth.project_id, &span_keys)
        .await
        .map_err(ApiError::from_data)?;

    // Count observations (GenAI spans) in results for metrics
    let observation_count = filter_observations(&rows).len();
    tracing::debug!(
        total = rows.len(),
        observations = observation_count,
        "Span list query results"
    );

    let include_raw_span = query.include_raw_span;
    let data: Vec<SpanSummaryDto> = rows
        .iter()
        .map(|row| {
            let key = (row.trace_id.clone(), row.span_id.clone());
            let c = counts.get(&key);
            SpanSummaryDto::from_row(
                row,
                c.map(|c| c.event_count).unwrap_or(0),
                c.map(|c| c.link_count).unwrap_or(0),
                include_raw_span,
            )
        })
        .collect();

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    Ok((
        headers,
        Json(PaginatedResponse::new(data, query.page, query.limit, total)),
    ))
}

/// List spans for a specific trace
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/spans",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("trace_id" = String, Path, description = "Trace ID"),
        ("page" = Option<u32>, Query, description = "Page number"),
        ("limit" = Option<u32>, Query, description = "Items per page"),
        ("order_by" = Option<String>, Query, description = "Sort field"),
        ("include_raw_span" = Option<bool>, Query, description = "Include raw OTLP span JSON (default: false)")
    ),
    responses(
        (status = 200, description = "List of spans for the trace with pagination metadata")
    )
)]
pub async fn list_trace_spans(
    State(state): State<OtelApiState>,
    auth: TraceRead,
    ValidatedQuery(query): ValidatedQuery<ListSpansQuery>,
) -> Result<Json<PaginatedResponse<SpanSummaryDto>>, ApiError> {
    let project_id = &auth.project_id;
    let trace_id = auth.trace_id.clone();

    // Parse order_by
    let order_by = if let Some(ref ob) = query.order_by {
        Some(OrderBy::parse(ob, columns::SPAN_SORTABLE)?)
    } else {
        None
    };

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse advanced filters
    let filters = if let Some(ref filters_json) = query.filters {
        parse_filters(filters_json, columns::SPAN_FILTERABLE)?
    } else {
        vec![]
    };

    let params = ListSpansParams {
        project_id: project_id.to_string(),
        page: query.page,
        limit: query.limit,
        order_by,
        trace_id: Some(trace_id), // Fixed from path
        session_id: query.session_id,
        user_id: query.user_id,
        environment: query.environment.map(|e| e.into_vec()),
        span_category: query.span_category,
        observation_type: query.observation_type,
        framework: query.framework,
        gen_ai_request_model: query.gen_ai_request_model,
        status_code: query.status_code,
        from_timestamp,
        to_timestamp,
        filters,
        is_observation: None,
    };

    let repo = state.analytics.repository();
    let (rows, total) = repo
        .list_spans(&params)
        .await
        .map_err(ApiError::from_data)?;

    let span_keys: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.trace_id.clone(), r.span_id.clone()))
        .collect();
    let counts = repo
        .get_span_counts_bulk(project_id, &span_keys)
        .await
        .map_err(ApiError::from_data)?;

    let include_raw_span = query.include_raw_span;
    let data: Vec<SpanSummaryDto> = rows
        .iter()
        .map(|row| {
            let key = (row.trace_id.clone(), row.span_id.clone());
            let c = counts.get(&key);
            SpanSummaryDto::from_row(
                row,
                c.map(|c| c.event_count).unwrap_or(0),
                c.map(|c| c.link_count).unwrap_or(0),
                include_raw_span,
            )
        })
        .collect();

    Ok(Json(PaginatedResponse::new(
        data,
        query.page,
        query.limit,
        total,
    )))
}

/// Get a single span
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/spans/{span_id}",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("trace_id" = String, Path, description = "Trace ID"),
        ("span_id" = String, Path, description = "Span ID"),
        ("include_raw_span" = Option<bool>, Query, description = "Include raw OTLP span JSON (default: false)")
    ),
    responses(
        (status = 200, description = "Span details", body = SpanDetailDto),
        (status = 404, description = "Span not found")
    )
)]
pub async fn get_span(
    State(state): State<OtelApiState>,
    auth: SpanRead,
    Query(query): Query<SpanDetailQuery>,
) -> Result<Json<SpanDetailDto>, ApiError> {
    let project_id = &auth.project_id;
    let trace_id = &auth.trace_id;
    let span_id = &auth.span_id;

    let include_raw_span = query.include_raw_span;
    let repo = state.analytics.repository();

    let span = repo
        .get_span(project_id, trace_id, span_id)
        .await
        .map_err(ApiError::from_data)?;

    let span = span.ok_or_else(|| {
        ApiError::not_found(
            "SPAN_NOT_FOUND",
            format!("Span not found: {}/{}", trace_id, span_id),
        )
    })?;

    // Fetch event and link counts
    let span_keys = vec![(trace_id.to_string(), span_id.to_string())];
    let counts = repo
        .get_span_counts_bulk(project_id, &span_keys)
        .await
        .map_err(ApiError::from_data)?;

    let key = (trace_id.to_string(), span_id.to_string());
    let count = counts.get(&key);
    let event_count = count.map(|c| c.event_count).unwrap_or(0);
    let link_count = count.map(|c| c.link_count).unwrap_or(0);

    // Log span details including observation metrics using converters
    let is_obs = is_observation(&span);
    if is_obs {
        let obs_type = get_observation_type(&span);
        let obs_cost = get_observation_cost(&span);
        let obs_tokens = get_observation_tokens(&span);
        tracing::debug!(
            span_id = %span_id,
            observation_type = ?obs_type,
            total_tokens = obs_tokens.total_tokens,
            total_cost = obs_cost,
            event_count = event_count,
            "Retrieved observation span"
        );
    } else {
        tracing::debug!(
            span_id = %span_id,
            is_observation = false,
            event_count = event_count,
            link_count = link_count,
            "Retrieved span details"
        );
    }

    Ok(Json(SpanDetailDto {
        summary: SpanSummaryDto::from_row(&span, event_count, link_count, include_raw_span),
    }))
}

// --- Filter options ---

#[derive(Debug, Deserialize, Validate)]
pub struct SpanFilterOptionsQuery {
    /// Comma-separated list of columns to get options for
    pub columns: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    /// Filter to observations only (GenAI spans), default: true
    #[serde(default = "default_observations_only")]
    pub observations_only: bool,
}

fn default_observations_only() -> bool {
    true
}

/// Get distinct values with counts for filterable span columns
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/spans/filter-options",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("columns" = Option<String>, Query, description = "Comma-separated list of columns"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("observations_only" = Option<bool>, Query, description = "Filter to observations only (default: true)")
    ),
    responses(
        (status = 200, description = "Filter options", body = FilterOptionsResponse)
    )
)]
pub async fn get_span_filter_options(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<SpanFilterOptionsQuery>,
) -> Result<(HeaderMap, Json<FilterOptionsResponse>), ApiError> {
    use std::collections::HashMap;

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse requested columns (default to observation-relevant columns)
    let columns: Vec<String> = query
        .columns
        .as_ref()
        .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
        .unwrap_or_else(|| {
            vec![
                "observation_type".to_string(),
                "gen_ai_request_model".to_string(),
                "framework".to_string(),
                "status_code".to_string(),
                "span_category".to_string(),
                "environment".to_string(),
                "gen_ai_agent_name".to_string(),
                "gen_ai_system".to_string(),
            ]
        });

    let repo = state.analytics.repository();
    let column_options = repo
        .get_span_filter_options(
            &auth.project_id,
            &columns,
            from_timestamp,
            to_timestamp,
            query.observations_only,
        )
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

// --- Delete operations ---

#[derive(Debug, Deserialize, serde::Serialize, ToSchema)]
pub struct SpanIdentifier {
    pub trace_id: String,
    pub span_id: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct DeleteSpansBody {
    #[validate(length(min = 1, max = 1000, message = "spans must contain 1-1000 items"))]
    pub spans: Vec<SpanIdentifier>,
}

/// Delete multiple spans by (trace_id, span_id) pairs
#[utoipa::path(
    delete,
    path = "/api/v1/project/{project_id}/otel/spans",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    request_body = DeleteSpansBody,
    responses(
        (status = 204, description = "Spans deleted successfully"),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn delete_spans(
    State(state): State<OtelApiState>,
    auth: ProjectWrite,
    ValidatedJson(body): ValidatedJson<DeleteSpansBody>,
) -> Result<StatusCode, ApiError> {
    // Convert to tuple format for repository
    let span_pairs: Vec<(String, String)> = body
        .spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();

    // Delete from analytics backend
    let analytics_repo = state.analytics.repository();
    analytics_repo
        .delete_spans(&auth.project_id, &span_pairs)
        .await
        .map_err(ApiError::from_data)?;

    // Cleanup favorites for deleted spans
    let span_ids: Vec<String> = body
        .spans
        .iter()
        .map(|s| format!("{}:{}", s.trace_id, s.span_id))
        .collect();

    let repo = state.database.repository();
    if let Err(e) = repo
        .delete_favorites_by_entity("span", &span_ids, &auth.project_id)
        .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            spans = body.spans.len(),
            "Failed to cleanup favorites after span deletion"
        );
    }

    Ok(StatusCode::NO_CONTENT)
}
