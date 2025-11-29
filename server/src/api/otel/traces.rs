//! Trace query endpoint handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::otel::query::{AttributeFilter, Cursor, TraceFilter};
use crate::otel::storage::sqlite::{TraceSummary, get_trace_attributes};
use crate::otel::{OtelError, OtelManager};

/// Create trace routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new()
        .route("/", get(get_traces))
        .route("/filters", get(get_filter_options))
        .route("/{trace_id}", get(get_trace).delete(delete_trace))
        .with_state(otel)
}

/// Query params for trace listing
#[derive(Debug, Deserialize)]
pub struct TraceQueryParams {
    pub service: Option<String>,
    pub framework: Option<String>,
    pub agent: Option<String>,
    pub errors_only: Option<bool>,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    /// Attribute filters as JSON string: [{"key":"env","op":"eq","value":"prod"}]
    pub attributes: Option<String>,
}

/// Trace list response
#[derive(Debug, Serialize)]
pub struct TraceListResponse {
    pub traces: Vec<TraceSummaryDto>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Trace summary DTO
#[derive(Debug, Serialize)]
pub struct TraceSummaryDto {
    pub trace_id: String,
    pub session_id: Option<String>,
    pub root_span_id: Option<String>,
    pub root_span_name: Option<String>,
    pub service_name: String,
    pub detected_framework: String,
    pub span_count: i32,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub has_errors: bool,
}

impl From<TraceSummary> for TraceSummaryDto {
    fn from(t: TraceSummary) -> Self {
        Self {
            trace_id: t.trace_id,
            session_id: t.session_id,
            root_span_id: t.root_span_id,
            root_span_name: t.root_span_name,
            service_name: t.service_name,
            detected_framework: t.detected_framework,
            span_count: t.span_count,
            start_time_ns: t.start_time_ns,
            end_time_ns: t.end_time_ns,
            duration_ns: t.duration_ns,
            total_input_tokens: t.total_input_tokens,
            total_output_tokens: t.total_output_tokens,
            total_tokens: t.total_tokens,
            has_errors: t.has_errors,
        }
    }
}

/// GET /otel/traces - List traces with filters
pub async fn get_traces(
    State(otel): State<Arc<OtelManager>>,
    Query(params): Query<TraceQueryParams>,
) -> impl IntoResponse {
    // Parse attribute filters from JSON string
    let attributes = params
        .attributes
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<AttributeFilter>>(s).ok())
        .unwrap_or_default();

    let filter = TraceFilter {
        service_name: params.service,
        framework: params.framework,
        agent_name: params.agent,
        has_errors: params.errors_only,
        search: params.search,
        attributes,
        ..Default::default()
    };

    let cursor = params.cursor.as_ref().and_then(|c| Cursor::decode(c));
    let limit = params.limit.unwrap_or(50).min(100);

    // Get attribute cache for EAV filtering
    let attr_cache = otel.attribute_cache();

    match otel
        .query_engine
        .query_traces(&filter, cursor.as_ref(), limit, Some(attr_cache.as_ref()))
        .await
    {
        Ok(result) => {
            let next_cursor = result.next_cursor_string();
            let has_more = result.has_more;
            let response = TraceListResponse {
                traces: result.items.into_iter().map(|t| t.into()).collect(),
                next_cursor,
                has_more,
            };
            Json(response).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// Trace detail response with attributes
#[derive(Debug, Serialize)]
pub struct TraceDetailDto {
    #[serde(flatten)]
    pub summary: TraceSummaryDto,
    /// All indexed attributes for this trace
    pub attributes: HashMap<String, serde_json::Value>,
}

/// GET /otel/traces/{trace_id} - Get single trace with attributes
pub async fn get_trace(
    State(otel): State<Arc<OtelManager>>,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    let pool = &otel.pool;

    // Get trace summary
    let trace = match crate::otel::storage::sqlite::traces::get_trace(pool, &trace_id).await {
        Ok(Some(trace)) => trace,
        Ok(None) => return OtelError::TraceNotFound(trace_id).into_response(),
        Err(e) => return e.into_response(),
    };

    // Get trace attributes
    let attributes = match get_trace_attributes(pool, &trace_id).await {
        Ok(attrs) => attrs,
        Err(e) => {
            tracing::warn!("Failed to get trace attributes: {}", e);
            HashMap::new()
        }
    };

    let response = TraceDetailDto { summary: trace.into(), attributes };
    Json(response).into_response()
}

/// DELETE /otel/traces/{trace_id} - Delete a trace and all associated data
pub async fn delete_trace(
    State(otel): State<Arc<OtelManager>>,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    match crate::otel::storage::sqlite::traces::delete_trace(&otel.pool, &trace_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => OtelError::TraceNotFound(trace_id).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Filter options response for UI dropdowns
#[derive(Debug, Serialize)]
pub struct FilterOptionsResponse {
    /// Available service names
    pub services: Vec<String>,
    /// Available frameworks
    pub frameworks: Vec<String>,
    /// Available indexed attribute keys
    pub attributes: Vec<AttributeKeyInfo>,
}

/// Attribute key information for filter discovery
#[derive(Debug, Serialize)]
pub struct AttributeKeyInfo {
    /// Attribute key name
    pub key: String,
    /// Value type (string, number, bool)
    pub key_type: String,
    /// Entity type (trace, span)
    pub entity_type: String,
    /// Sample distinct values (for dropdowns)
    pub sample_values: Vec<String>,
}

/// GET /otel/traces/filters - Get available filter options
pub async fn get_filter_options(State(otel): State<Arc<OtelManager>>) -> impl IntoResponse {
    use crate::otel::storage::sqlite::{get_all_attribute_keys, get_attribute_distinct_values};

    let pool = &otel.pool;

    // Get distinct services
    let services = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT service_name FROM traces ORDER BY service_name LIMIT 100",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // Get distinct frameworks
    let frameworks = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT detected_framework FROM traces ORDER BY detected_framework LIMIT 100",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // Get all attribute keys
    let attr_keys = match get_all_attribute_keys(pool).await {
        Ok(keys) => keys,
        Err(e) => {
            tracing::warn!("Failed to get attribute keys: {}", e);
            vec![]
        }
    };

    // Get sample values for each attribute key
    let mut attributes = Vec::new();
    for key in attr_keys {
        let sample_values =
            get_attribute_distinct_values(pool, &key.key_name, &key.entity_type, 20)
                .await
                .unwrap_or_default();

        attributes.push(AttributeKeyInfo {
            key: key.key_name,
            key_type: key.key_type,
            entity_type: key.entity_type,
            sample_values,
        });
    }

    let response = FilterOptionsResponse { services, frameworks, attributes };
    Json(response).into_response()
}
