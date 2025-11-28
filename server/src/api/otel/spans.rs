//! Span query endpoint handlers

use axum::{
    Json, Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::otel::OtelManager;
use crate::otel::query::{Cursor, SpanFilter};
use crate::otel::storage::sqlite::SpanIndex;

/// Create span routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new().route("/", get(get_spans)).with_state(otel)
}

/// Query params for span listing
#[derive(Debug, Deserialize)]
pub struct SpanQueryParams {
    pub trace_id: Option<String>,
    pub service: Option<String>,
    pub framework: Option<String>,
    pub category: Option<String>,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub model: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

/// Span DTO
#[derive(Debug, Serialize)]
pub struct SpanDto {
    pub span_id: String,
    pub trace_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub service_name: String,
    pub detected_framework: String,
    pub detected_category: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    pub status_code: i32,
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
}

impl From<SpanIndex> for SpanDto {
    fn from(s: SpanIndex) -> Self {
        Self {
            span_id: s.span_id,
            trace_id: s.trace_id,
            parent_span_id: s.parent_span_id,
            span_name: s.span_name,
            service_name: s.service_name,
            detected_framework: s.detected_framework,
            detected_category: s.detected_category,
            gen_ai_agent_name: s.gen_ai_agent_name,
            gen_ai_tool_name: s.gen_ai_tool_name,
            gen_ai_request_model: s.gen_ai_request_model,
            start_time_ns: s.start_time_ns,
            end_time_ns: s.end_time_ns,
            duration_ns: s.duration_ns,
            status_code: s.status_code,
            usage_input_tokens: s.usage_input_tokens,
            usage_output_tokens: s.usage_output_tokens,
        }
    }
}

/// Span list response
#[derive(Debug, Serialize)]
pub struct SpanListResponse {
    pub spans: Vec<SpanDto>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// GET /otel/spans - List spans with filters
pub async fn get_spans(
    State(otel): State<Arc<OtelManager>>,
    Query(params): Query<SpanQueryParams>,
) -> impl IntoResponse {
    let cursor = params.cursor.as_ref().and_then(|c| Cursor::decode(c));

    let filter = SpanFilter {
        trace_id: params.trace_id,
        service_name: params.service,
        framework: params.framework,
        category: params.category,
        agent_name: params.agent,
        tool_name: params.tool,
        model: params.model,
        cursor_timestamp: cursor.as_ref().map(|c| c.timestamp),
        cursor_id: cursor.as_ref().map(|c| c.id.clone()),
        ..Default::default()
    };

    let limit = params.limit.unwrap_or(100).min(1000);

    match otel.query_engine.query_spans(&filter, limit + 1).await {
        Ok(spans) => {
            let has_more = spans.len() > limit;
            let spans: Vec<SpanIndex> = spans.into_iter().take(limit).collect();

            let next_cursor = if has_more {
                spans
                    .last()
                    .map(|s| Cursor { timestamp: s.start_time_ns, id: s.span_id.clone() }.encode())
            } else {
                None
            };

            let response = SpanListResponse {
                spans: spans.into_iter().map(|s| s.into()).collect(),
                next_cursor,
                has_more,
            };
            Json(response).into_response()
        }
        Err(e) => e.into_response(),
    }
}
