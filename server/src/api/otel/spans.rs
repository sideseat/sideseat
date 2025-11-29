//! Span query endpoint handlers

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::otel::query::{Cursor, SpanFilter};
use crate::otel::storage::sqlite::{EventIndex, SpanIndex, get_events_by_span, get_span_by_id};
use crate::otel::{OtelError, OtelManager};

/// Create span routes
pub fn create_routes(otel: Arc<OtelManager>) -> Router {
    Router::new()
        .route("/", get(get_spans))
        .route("/{span_id}", get(get_span_detail))
        .route("/{span_id}/events", get(get_span_events))
        .with_state(otel)
}

/// Query params for span listing
#[derive(Debug, Deserialize)]
pub struct SpanQueryParams {
    pub trace_id: Option<String>,
    pub service: Option<String>,
    pub framework: Option<String>,
    pub category: Option<String>,
    pub kind: Option<i32>,
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
    pub session_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub span_kind: i32,
    pub service_name: String,
    pub detected_framework: String,
    pub detected_category: Option<String>,
    // Gen AI fields
    pub gen_ai_system: Option<String>,
    pub gen_ai_operation_name: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub gen_ai_response_model: Option<String>,
    // Timing
    pub start_time_ns: i64,
    pub end_time_ns: Option<i64>,
    pub duration_ns: Option<i64>,
    // Performance metrics
    pub time_to_first_token_ms: Option<i64>,
    pub request_duration_ms: Option<i64>,
    // Status
    pub status_code: i32,
    // Token usage
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
    pub usage_total_tokens: Option<i64>,
    pub usage_cache_read_tokens: Option<i64>,
    pub usage_cache_write_tokens: Option<i64>,
    // Full data (optional, only included for detail views)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl From<SpanIndex> for SpanDto {
    fn from(s: SpanIndex) -> Self {
        // Parse data_json if available
        let data = s.data_json.as_ref().and_then(|j| serde_json::from_str(j).ok());

        Self {
            span_id: s.span_id,
            trace_id: s.trace_id,
            session_id: s.session_id,
            parent_span_id: s.parent_span_id,
            span_name: s.span_name,
            span_kind: s.span_kind,
            service_name: s.service_name,
            detected_framework: s.detected_framework,
            detected_category: s.detected_category,
            gen_ai_system: s.gen_ai_system,
            gen_ai_operation_name: s.gen_ai_operation_name,
            gen_ai_agent_name: s.gen_ai_agent_name,
            gen_ai_tool_name: s.gen_ai_tool_name,
            gen_ai_request_model: s.gen_ai_request_model,
            gen_ai_response_model: s.gen_ai_response_model,
            start_time_ns: s.start_time_ns,
            end_time_ns: s.end_time_ns,
            duration_ns: s.duration_ns,
            time_to_first_token_ms: s.time_to_first_token_ms,
            request_duration_ms: s.request_duration_ms,
            status_code: s.status_code,
            usage_input_tokens: s.usage_input_tokens,
            usage_output_tokens: s.usage_output_tokens,
            usage_total_tokens: s.usage_total_tokens,
            usage_cache_read_tokens: s.usage_cache_read_tokens,
            usage_cache_write_tokens: s.usage_cache_write_tokens,
            data,
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

/// Span detail response with events
#[derive(Debug, Serialize)]
pub struct SpanDetailDto {
    #[serde(flatten)]
    pub span: SpanDto,
    /// Events attached to this span
    pub events: Vec<EventDto>,
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
        span_kind: params.kind,
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

/// GET /otel/spans/{span_id} - Get single span with events
pub async fn get_span_detail(
    State(otel): State<Arc<OtelManager>>,
    Path(span_id): Path<String>,
) -> impl IntoResponse {
    let pool = &otel.pool;

    // Get the span
    let span = match get_span_by_id(pool, &span_id).await {
        Ok(Some(span)) => span,
        Ok(None) => return OtelError::SpanNotFound(span_id).into_response(),
        Err(e) => return e.into_response(),
    };

    // Get events for this span
    let events = match get_events_by_span(pool, &span_id).await {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!("Failed to get span events: {}", e);
            vec![]
        }
    };

    let response =
        SpanDetailDto { span: span.into(), events: events.into_iter().map(|e| e.into()).collect() };
    Json(response).into_response()
}

/// Event DTO for span events
#[derive(Debug, Serialize)]
pub struct EventDto {
    pub id: i64,
    pub span_id: String,
    pub trace_id: String,
    pub event_name: String,
    pub event_time_ns: i64,
    pub content_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

impl From<EventIndex> for EventDto {
    fn from(e: EventIndex) -> Self {
        let attributes = e.attributes_json.as_ref().and_then(|j| serde_json::from_str(j).ok());
        Self {
            id: e.id,
            span_id: e.span_id,
            trace_id: e.trace_id,
            event_name: e.event_name,
            event_time_ns: e.event_time_ns,
            content_preview: e.content_preview,
            attributes,
        }
    }
}

/// Event list response
#[derive(Debug, Serialize)]
pub struct EventListResponse {
    pub events: Vec<EventDto>,
}

/// GET /otel/spans/{span_id}/events - Get events for a span
pub async fn get_span_events(
    State(otel): State<Arc<OtelManager>>,
    Path(span_id): Path<String>,
) -> impl IntoResponse {
    match get_events_by_span(&otel.pool, &span_id).await {
        Ok(events) => {
            let response =
                EventListResponse { events: events.into_iter().map(|e| e.into()).collect() };
            Json(response).into_response()
        }
        Err(e) => e.into_response(),
    }
}
