//! Trace query endpoint handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::otel::query::{Cursor, TraceFilter};
use crate::otel::storage::sqlite::TraceSummary;
use crate::otel::{OtelError, OtelManager};

/// Create trace routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new().route("/", get(get_traces)).route("/{trace_id}", get(get_trace)).with_state(otel)
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
    pub root_span_id: Option<String>,
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
            root_span_id: t.root_span_id,
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
    let filter = TraceFilter {
        service_name: params.service,
        framework: params.framework,
        agent_name: params.agent,
        has_errors: params.errors_only,
        search: params.search,
        ..Default::default()
    };

    let cursor = params.cursor.as_ref().and_then(|c| Cursor::decode(c));
    let limit = params.limit.unwrap_or(50).min(100);

    match otel.query_engine.query_traces(&filter, cursor.as_ref(), limit).await {
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

/// GET /otel/traces/{trace_id} - Get single trace
pub async fn get_trace(
    State(otel): State<Arc<OtelManager>>,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    match crate::otel::storage::sqlite::traces::get_trace(otel.storage.sqlite().pool(), &trace_id)
        .await
    {
        Ok(Some(trace)) => Json(TraceSummaryDto::from(trace)).into_response(),
        Ok(None) => OtelError::TraceNotFound(trace_id).into_response(),
        Err(e) => e.into_response(),
    }
}
