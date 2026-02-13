//! Trace API endpoints

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use super::OtelApiState;
use super::filters::{columns, parse_filters};
use super::types::{SpanDetailDto, SpanSummaryDto, StringOrArray, TraceDetailDto, TraceSummaryDto};
use crate::api::auth::{ProjectRead, ProjectWrite, TraceRead};
use crate::api::extractors::{ValidatedJson, ValidatedQuery};
use crate::api::types::{
    ApiError, OrderBy, PaginatedResponse, default_limit, default_page, parse_timestamp_param,
    validate_ids_batch, validate_limit, validate_page,
};
use crate::data::types::{ListTracesParams, TraceRow, find_root_span};

#[derive(Debug, Deserialize, Validate)]
pub struct ListTracesQuery {
    #[serde(default = "default_page")]
    #[validate(custom(function = "validate_page"))]
    pub page: u32,
    #[serde(default = "default_limit")]
    #[validate(custom(function = "validate_limit"))]
    pub limit: u32,
    pub order_by: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<StringOrArray>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    pub filters: Option<String>,
    /// Include non-GenAI traces (default: false, showing only GenAI traces)
    #[serde(default)]
    pub include_nongenai: bool,
}

/// List traces with pagination and filters
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces",
    tag = "traces",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("page" = Option<u32>, Query, description = "Page number"),
        ("limit" = Option<u32>, Query, description = "Items per page"),
        ("order_by" = Option<String>, Query, description = "Sort field (e.g., start_time:desc)"),
        ("session_id" = Option<String>, Query, description = "Filter by session ID"),
        ("user_id" = Option<String>, Query, description = "Filter by user ID"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("include_nongenai" = Option<bool>, Query, description = "Include non-GenAI traces (default: false)")
    ),
    responses(
        (status = 200, description = "List of traces with pagination metadata")
    )
)]
pub async fn list_traces(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<ListTracesQuery>,
) -> Result<(HeaderMap, Json<PaginatedResponse<TraceSummaryDto>>), ApiError> {
    // Parse order_by
    let order_by = if let Some(ref ob) = query.order_by {
        Some(OrderBy::parse(ob, columns::TRACE_SORTABLE)?)
    } else {
        None
    };

    // Parse timestamps
    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // Parse advanced filters
    let filters = if let Some(ref filters_json) = query.filters {
        parse_filters(filters_json, columns::TRACE_FILTERABLE)?
    } else {
        vec![]
    };

    let params = ListTracesParams {
        project_id: auth.project_id.clone(),
        page: query.page,
        limit: query.limit,
        order_by,
        session_id: query.session_id,
        user_id: query.user_id,
        environment: query.environment.map(|e| e.into_vec()),
        from_timestamp,
        to_timestamp,
        filters,
        include_nongenai: query.include_nongenai,
    };

    let repo = state.analytics.repository();
    let (rows, total) = repo
        .list_traces(&params)
        .await
        .map_err(ApiError::from_data)?;

    let data: Vec<TraceSummaryDto> = rows.into_iter().map(trace_row_to_summary).collect();

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    // Compute Last-Modified from most recent trace (HTTP-date format per RFC 7231)
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

/// Maximum spans to return in trace detail to prevent OOM
pub(crate) const MAX_SPANS_PER_TRACE: usize = 2500;

#[derive(Debug, Deserialize)]
pub struct TraceDetailQuery {
    /// Include raw OTLP span JSON in response (default: false)
    #[serde(default)]
    pub include_raw_span: bool,
}

/// Get a single trace with all nested spans
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}",
    tag = "traces",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("trace_id" = String, Path, description = "Trace ID"),
        ("include_raw_span" = Option<bool>, Query, description = "Include raw OTLP span JSON (default: false)")
    ),
    responses(
        (status = 200, description = "Trace details with spans", body = TraceDetailDto),
        (status = 404, description = "Trace not found")
    )
)]
pub async fn get_trace(
    State(state): State<OtelApiState>,
    auth: TraceRead,
    Query(query): Query<TraceDetailQuery>,
) -> Result<(HeaderMap, Json<TraceDetailDto>), ApiError> {
    let project_id = &auth.project_id;
    let trace_id = &auth.trace_id;

    let include_raw_span = query.include_raw_span;
    let repo = state.analytics.repository();

    // Fetch trace and spans (spans already deduplicated in repository)
    let trace = repo
        .get_trace(project_id, trace_id)
        .await
        .map_err(ApiError::from_data)?;
    let spans = repo
        .get_spans_for_trace(project_id, trace_id)
        .await
        .map_err(ApiError::from_data)?;

    // Bulk fetch event and link counts
    let span_keys: Vec<(String, String)> = spans
        .iter()
        .map(|r| (r.trace_id.clone(), r.span_id.clone()))
        .collect();
    let span_counts = repo
        .get_span_counts_bulk(project_id, &span_keys)
        .await
        .map_err(ApiError::from_data)?;

    let trace = trace.ok_or_else(|| {
        ApiError::not_found("TRACE_NOT_FOUND", format!("Trace not found: {}", trace_id))
    })?;

    // Warn if trace has no root span (unusual structure)
    if !spans.is_empty() && find_root_span(&spans).is_none() {
        tracing::warn!(trace_id = %trace_id, span_count = spans.len(), "Trace has no root span (all spans have parent_span_id)");
    }

    // Build span details with pagination guard
    let span_count = spans.len();
    let spans_truncated = span_count > MAX_SPANS_PER_TRACE;
    let spans_to_process = if spans_truncated {
        &spans[..MAX_SPANS_PER_TRACE]
    } else {
        &spans[..]
    };

    let span_details: Vec<SpanDetailDto> = spans_to_process
        .iter()
        .map(|span| {
            let key = (span.trace_id.clone(), span.span_id.clone());
            let counts = span_counts.get(&key);
            let event_count = counts.map(|c| c.event_count).unwrap_or(0);
            let link_count = counts.map(|c| c.link_count).unwrap_or(0);

            SpanDetailDto {
                summary: SpanSummaryDto::from_row(span, event_count, link_count, include_raw_span),
            }
        })
        .collect();

    if spans_truncated {
        tracing::warn!(trace_id = %trace_id, total = span_count, returned = MAX_SPANS_PER_TRACE, "Trace response truncated");
    }

    let summary = trace_row_to_summary(trace);

    // Compute ETag from span_count and end_time
    let end_time_str = summary
        .end_time
        .map(|dt| dt.timestamp_millis().to_string())
        .unwrap_or_else(|| "none".to_string());
    let etag_value = format!(
        "W/\"{}-{}-{}\"",
        summary.span_count, end_time_str, spans_truncated
    );

    let mut headers = HeaderMap::new();
    if let Ok(etag) = HeaderValue::from_str(&etag_value) {
        headers.insert(header::ETAG, etag);
    }

    Ok((
        headers,
        Json(TraceDetailDto {
            summary,
            spans: span_details,
        }),
    ))
}

pub(crate) fn trace_row_to_summary(row: TraceRow) -> TraceSummaryDto {
    TraceSummaryDto {
        trace_id: row.trace_id,
        trace_name: row.trace_name,
        start_time: row.start_time,
        end_time: row.end_time,
        duration_ms: row.duration_ms,
        session_id: row.session_id,
        user_id: row.user_id,
        environment: row.environment,
        span_count: row.span_count,
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
        tags: row.tags,
        observation_count: row.observation_count,
        metadata: row
            .metadata
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
        input_preview: row.input_preview,
        output_preview: row.output_preview,
        has_error: row.has_error,
    }
}

// --- Delete operations ---

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct DeleteTracesBody {
    #[validate(custom(function = "validate_ids_batch"))]
    pub trace_ids: Vec<String>,
}

/// Delete multiple traces by IDs
#[utoipa::path(
    delete,
    path = "/api/v1/project/{project_id}/otel/traces",
    tag = "traces",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    request_body = DeleteTracesBody,
    responses(
        (status = 204, description = "Traces deleted successfully"),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn delete_traces(
    State(state): State<OtelApiState>,
    auth: ProjectWrite,
    ValidatedJson(body): ValidatedJson<DeleteTracesBody>,
) -> Result<StatusCode, ApiError> {
    // Delete from analytics backend
    let analytics_repo = state.analytics.repository();
    analytics_repo
        .delete_traces(&auth.project_id, &body.trace_ids)
        .await
        .map_err(ApiError::from_data)?;

    // Cleanup files associated with deleted traces
    if state.file_service.is_enabled()
        && let Err(e) = state
            .file_service
            .cleanup_traces(&auth.project_id, &body.trace_ids)
            .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            traces = body.trace_ids.len(),
            "Failed to cleanup files after trace deletion"
        );
    }

    // Cleanup favorites for deleted traces
    let repo = state.database.repository();
    if let Err(e) = repo
        .delete_favorites_by_entity("trace", &body.trace_ids, &auth.project_id)
        .await
    {
        tracing::warn!(
            error = %e,
            project_id = %auth.project_id,
            traces = body.trace_ids.len(),
            "Failed to cleanup favorites after trace deletion"
        );
    }

    Ok(StatusCode::NO_CONTENT)
}

// --- Filter options ---

#[derive(Debug, Deserialize, Validate)]
pub struct FilterOptionsQuery {
    /// Comma-separated list of columns to get options for
    pub columns: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

/// Response type for filter options
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct FilterOptionsResponse {
    pub options: std::collections::HashMap<String, Vec<FilterOptionDto>>,
}

/// Single filter option with value and count
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct FilterOptionDto {
    pub value: String,
    pub count: u64,
}

/// Get distinct values with counts for filterable columns
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/filter-options",
    tag = "traces",
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
pub async fn get_trace_filter_options(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    ValidatedQuery(query): ValidatedQuery<FilterOptionsQuery>,
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
        .unwrap_or_else(|| {
            vec![
                "environment".to_string(),
                "trace_name".to_string(),
                "user_id".to_string(),
                "session_id".to_string(),
                "gen_ai_request_model".to_string(),
                "gen_ai_system".to_string(),
                "framework".to_string(),
            ]
        });

    let repo = state.analytics.repository();
    let column_options = repo
        .get_trace_filter_options(&auth.project_id, &columns, from_timestamp, to_timestamp)
        .await
        .map_err(ApiError::from_data)?;
    let tags_options = repo
        .get_trace_tags_options(&auth.project_id, from_timestamp, to_timestamp)
        .await
        .map_err(ApiError::from_data)?;

    // Convert to DTO format
    let mut options: HashMap<String, Vec<FilterOptionDto>> = column_options
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

    // Add tags
    options.insert(
        "tags".to_string(),
        tags_options
            .into_iter()
            .map(|o| FilterOptionDto {
                value: o.value,
                count: o.count,
            })
            .collect(),
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=30"),
    );

    Ok((headers, Json(FilterOptionsResponse { options })))
}
