//! Messages API endpoints for conversation history

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::OtelApiState;
use super::types::{BlockDto, MessagesMetadataDto, MessagesResponseDto};
use crate::api::auth::{SessionRead, SpanRead, TraceRead};
use crate::api::types::{ApiError, parse_timestamp_param};
use crate::data::types::MessageQueryParams;
use crate::domain::sideml::{
    ExtractedTools, FeedOptions, FeedResult, extract_tools_from_rows, process_spans,
};

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
    pub role: Option<String>,
}

impl MessagesQuery {
    fn to_feed_options(&self) -> FeedOptions {
        FeedOptions::new().with_role(self.role.clone())
    }
}

/// GET /traces/{trace_id}/spans/{span_id}/messages - Get conversation messages for a span
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/spans/{span_id}/messages",
    tag = "spans",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("trace_id" = String, Path, description = "Trace ID"),
        ("span_id" = String, Path, description = "Span ID"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("role" = Option<String>, Query, description = "Filter by role (user, assistant, etc.)")
    ),
    responses(
        (status = 200, description = "Messages for the span", body = MessagesResponseDto)
    )
)]
pub async fn get_span_messages(
    State(state): State<OtelApiState>,
    auth: SpanRead,
    axum::extract::Query(query): axum::extract::Query<MessagesQuery>,
) -> Result<Json<MessagesResponseDto>, ApiError> {
    let project_id = &auth.project_id;
    let span_id = &auth.span_id;

    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    let options = query.to_feed_options();

    // Fetch raw span rows
    let repo = state.analytics.repository();
    let params = MessageQueryParams {
        project_id: project_id.to_string(),
        span_id: Some(span_id.to_string()),
        from_timestamp,
        to_timestamp,
        ..Default::default()
    };
    let result = repo
        .get_messages(&params)
        .await
        .map_err(ApiError::from_data)?;

    // Process through feed pipeline
    let processed = process_spans(result.rows, &options);

    let response = build_messages_response(processed, None);
    Ok(Json(response))
}

/// GET /traces/{trace_id}/messages - Get conversation messages for a trace
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/traces/{trace_id}/messages",
    tag = "traces",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("trace_id" = String, Path, description = "Trace ID"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("role" = Option<String>, Query, description = "Filter by role (user, assistant, etc.)")
    ),
    responses(
        (status = 200, description = "Messages for the trace", body = MessagesResponseDto)
    )
)]
pub async fn get_trace_messages(
    State(state): State<OtelApiState>,
    auth: TraceRead,
    axum::extract::Query(query): axum::extract::Query<MessagesQuery>,
) -> Result<Json<MessagesResponseDto>, ApiError> {
    let project_id = &auth.project_id;
    let trace_id = &auth.trace_id;

    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // History filtering is automatic (duplicates are detected and filtered)
    let options = query.to_feed_options();

    // Fetch trace metadata for session_id and totals
    let repo = state.analytics.repository();
    let trace = repo
        .get_trace(project_id, trace_id)
        .await
        .map_err(ApiError::from_data)?;

    // Session-aware loading: if trace belongs to a session, load ALL session spans
    // so cross-trace prefix stripping can remove history re-sent from prior traces
    let session_id = trace
        .as_ref()
        .and_then(|t| t.session_id.as_ref())
        .filter(|s| !s.is_empty());

    let result = if let Some(sid) = session_id {
        let params = MessageQueryParams {
            project_id: project_id.to_string(),
            session_id: Some(sid.to_string()),
            from_timestamp,
            to_timestamp,
            ..Default::default()
        };
        repo.get_messages(&params)
            .await
            .map_err(ApiError::from_data)?
    } else {
        let params = MessageQueryParams {
            project_id: project_id.to_string(),
            trace_id: Some(trace_id.to_string()),
            from_timestamp,
            to_timestamp,
            ..Default::default()
        };
        repo.get_messages(&params)
            .await
            .map_err(ApiError::from_data)?
    };

    // When session-loaded, scope tool extraction to the target trace's rows
    // BEFORE consuming rows into process_spans (which needs ownership).
    let scoped_tools = if session_id.is_some() {
        Some(extract_tools_from_rows(
            result.rows.iter().filter(|r| r.trace_id == *trace_id),
        ))
    } else {
        None
    };

    // Process through feed pipeline (auto-routes to multi-trace if needed)
    let mut processed = process_spans(result.rows, &options);

    // If session-loaded, retain only the target trace's blocks and apply scoped tools.
    // scoped_tools is Some iff session_id.is_some(), so use it as the single guard.
    if let Some(scoped_tools) = scoped_tools {
        scope_feed_to_trace(&mut processed, scoped_tools, trace_id);
    }

    // Use trace-level totals for metadata (matches trace endpoint)
    let trace_totals = trace.map(|t| (t.total_tokens, t.total_cost));
    let response = build_messages_response(processed, trace_totals);
    Ok(Json(response))
}

/// GET /sessions/{session_id}/messages - Get conversation messages for a session
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/sessions/{session_id}/messages",
    tag = "sessions",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("session_id" = String, Path, description = "Session ID"),
        ("from_timestamp" = Option<String>, Query, description = "Filter from timestamp (ISO 8601)"),
        ("to_timestamp" = Option<String>, Query, description = "Filter to timestamp (ISO 8601)"),
        ("role" = Option<String>, Query, description = "Filter by role (user, assistant, etc.)")
    ),
    responses(
        (status = 200, description = "Messages for the session", body = MessagesResponseDto)
    )
)]
pub async fn get_session_messages(
    State(state): State<OtelApiState>,
    auth: SessionRead,
    axum::extract::Query(query): axum::extract::Query<MessagesQuery>,
) -> Result<Json<MessagesResponseDto>, ApiError> {
    let project_id = &auth.project_id;
    let session_id = &auth.session_id;

    let from_timestamp = parse_timestamp_param(&query.from_timestamp)?;
    let to_timestamp = parse_timestamp_param(&query.to_timestamp)?;

    // History filtering is automatic (duplicates are detected and filtered)
    let options = query.to_feed_options();

    // Fetch raw span rows
    let repo = state.analytics.repository();
    let params = MessageQueryParams {
        project_id: project_id.to_string(),
        session_id: Some(session_id.to_string()),
        from_timestamp,
        to_timestamp,
        ..Default::default()
    };
    let result = repo
        .get_messages(&params)
        .await
        .map_err(ApiError::from_data)?;

    // Process through feed pipeline
    let processed = process_spans(result.rows, &options);

    let response = build_messages_response(processed, None);
    Ok(Json(response))
}

/// Scope a session-loaded FeedResult to a single trace.
pub(crate) fn scope_feed_to_trace(
    processed: &mut FeedResult,
    scoped_tools: ExtractedTools,
    trace_id: &str,
) {
    processed.messages.retain(|b| b.trace_id == trace_id);
    processed.metadata.block_count = processed.messages.len();
    processed.metadata.span_count = processed
        .messages
        .iter()
        .map(|b| &b.span_id)
        .collect::<HashSet<_>>()
        .len();
    processed.tool_definitions = scoped_tools.tool_definitions;
    processed.tool_names = scoped_tools.tool_names;
}

/// Build messages response from processed messages.
///
/// If `trace_totals` is provided, use trace-level token/cost totals.
/// Otherwise, aggregate from message spans.
pub(crate) fn build_messages_response(
    processed: FeedResult,
    trace_totals: Option<(i64, f64)>,
) -> MessagesResponseDto {
    let mut messages_dto = Vec::new();
    let mut start_time: Option<DateTime<Utc>> = None;
    let mut end_time: Option<DateTime<Utc>> = None;

    // Track seen span_ids to avoid counting tokens multiple times per span
    let mut seen_spans: HashSet<String> = HashSet::new();
    let mut aggregated_tokens = 0i64;
    let mut aggregated_cost = 0.0f64;

    for block in &processed.messages {
        // Aggregate tokens/cost from spans (only if not using trace totals)
        if trace_totals.is_none() && seen_spans.insert(block.span_id.clone()) {
            aggregated_tokens += block.tokens.unwrap_or(0);
            aggregated_cost += block.cost.unwrap_or(0.0);
        }

        if start_time.is_none_or(|t| block.timestamp < t) {
            start_time = Some(block.timestamp);
        }
        if end_time.is_none_or(|t| block.timestamp > t) {
            end_time = Some(block.timestamp);
        }

        messages_dto.push(BlockDto::from_block_entry(block));
    }

    let total_messages = messages_dto.len() as i64;
    let (total_tokens, total_cost) = trace_totals.unwrap_or((aggregated_tokens, aggregated_cost));

    MessagesResponseDto {
        messages: messages_dto,
        metadata: MessagesMetadataDto {
            total_messages,
            total_tokens,
            total_cost,
            start_time: start_time.unwrap_or_else(Utc::now),
            end_time,
        },
        tool_definitions: processed.tool_definitions,
        tool_names: processed.tool_names,
    }
}
