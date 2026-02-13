//! Feed API endpoints for project-wide message and span feeds
//!
//! Provides cursor-based pagination for real-time activity feeds.

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::OtelApiState;
use super::types::{
    BlockDto, FeedMessagesMetadata, FeedMessagesResponse, FeedPagination, FeedSpansResponse,
    SpanSummaryDto,
};
use crate::api::auth::ProjectRead;
use crate::api::types::{ApiError, parse_timestamp_param};
use crate::data::types::{FeedMessagesParams, FeedSpansParams, deduplicate_spans};
use crate::domain::sideml::{FeedOptions, process_feed};

// ============================================================================
// Constants
// ============================================================================

const DEFAULT_FEED_LIMIT: u32 = 50;
const MAX_FEED_LIMIT: u32 = 500;

// ============================================================================
// Query parameters
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct FeedMessagesQuery {
    /// Maximum number of spans to return (default: 50, max: 500)
    pub limit: Option<u32>,
    /// Cursor for pagination (base64 encoded: ingested_at_us:span_id)
    pub cursor: Option<String>,
    /// Filter by event time >= start_time (ISO 8601)
    pub start_time: Option<String>,
    /// Filter by event time < end_time (ISO 8601)
    pub end_time: Option<String>,
    /// Filter by message role (user, assistant, tool, system)
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FeedSpansQuery {
    /// Maximum number of spans to return (default: 50, max: 500)
    pub limit: Option<u32>,
    /// Cursor for pagination (base64 encoded: ingested_at_us:span_id)
    pub cursor: Option<String>,
    /// Filter by event time >= start_time (ISO 8601)
    pub start_time: Option<String>,
    /// Filter by event time < end_time (ISO 8601)
    pub end_time: Option<String>,
    /// Filter to observations only (spans with observation_type OR gen_ai_request_model)
    pub is_observation: Option<bool>,
    /// Include raw_span in response
    pub include_raw_span: Option<bool>,
}

// ============================================================================
// Cursor encoding/decoding
// ============================================================================

/// Encode cursor from (ingested_at, span_id)
fn encode_cursor(ingested_at: DateTime<Utc>, span_id: &str) -> String {
    let cursor_str = format!("{}:{}", ingested_at.timestamp_micros(), span_id);
    URL_SAFE_NO_PAD.encode(cursor_str)
}

/// Decode cursor to (ingested_at_us, span_id)
fn decode_cursor(cursor: &str) -> Result<(i64, String), ApiError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor format"))?;

    let cursor_str = String::from_utf8(decoded)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor encoding"))?;

    let parts: Vec<&str> = cursor_str.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(ApiError::bad_request(
            "INVALID_CURSOR",
            "Invalid cursor format: expected timestamp:span_id",
        ));
    }

    let timestamp_us = parts[0]
        .parse::<i64>()
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor timestamp"))?;

    Ok((timestamp_us, parts[1].to_string()))
}

/// Validate and clamp limit parameter
fn validate_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_FEED_LIMIT).clamp(1, MAX_FEED_LIMIT)
}

// ============================================================================
// Feed messages endpoint
// ============================================================================

/// GET /feed/messages - Get latest messages across the project
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/feed/messages",
    tag = "feed",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("limit" = Option<u32>, Query, description = "Max spans to return (default: 50, max: 500)"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("start_time" = Option<String>, Query, description = "Filter by event time >= (ISO 8601)"),
        ("end_time" = Option<String>, Query, description = "Filter by event time < (ISO 8601)"),
        ("role" = Option<String>, Query, description = "Filter by role (user, assistant, tool, system)")
    ),
    responses(
        (status = 200, description = "Feed messages", body = FeedMessagesResponse)
    )
)]
pub async fn get_feed_messages(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    axum::extract::Query(query): axum::extract::Query<FeedMessagesQuery>,
) -> Result<Json<FeedMessagesResponse>, ApiError> {
    let project_id = auth.project_id.clone();

    let limit = validate_limit(query.limit);
    let cursor = query
        .cursor
        .as_ref()
        .map(|c| decode_cursor(c))
        .transpose()?;
    let start_time = parse_timestamp_param(&query.start_time)?;
    let end_time = parse_timestamp_param(&query.end_time)?;

    // Query limit + 1 to detect has_more
    let query_limit = limit + 1;

    let params = FeedMessagesParams {
        project_id: project_id.clone(),
        limit: query_limit,
        cursor,
        start_time,
        end_time,
    };

    // Fetch raw span rows
    let repo = state.analytics.repository();
    let result = repo
        .get_project_messages(&params)
        .await
        .map_err(ApiError::from_data)?;

    let mut spans = result.rows;

    // Compute has_more from query results, then truncate
    let has_more = spans.len() > limit as usize;
    spans.truncate(limit as usize);

    // Compute cursor from raw query results BEFORE processing
    let next_cursor = spans
        .last()
        .map(|s| encode_cursor(s.ingested_at, &s.span_id));

    // Process spans through feed pipeline (handles grouping, dedup, sorting)
    // History filtering is automatic (duplicates are detected and filtered)
    let options = FeedOptions::new().with_role(query.role.clone());

    let processed = process_feed(spans, &options);
    let all_messages = processed.messages;
    let tool_definitions = processed.tool_definitions;
    let tool_names = processed.tool_names;

    // Compute metadata (use &str to avoid cloning span_ids)
    let mut seen_spans: HashSet<&str> = HashSet::new();
    let mut total_tokens = 0i64;
    let mut total_cost = 0.0f64;

    for block in &all_messages {
        if seen_spans.insert(&block.span_id) {
            total_tokens += block.tokens.unwrap_or(0);
            total_cost += block.cost.unwrap_or(0.0);
        }
    }

    let metadata = FeedMessagesMetadata {
        message_count: all_messages.len() as u32,
        span_count: seen_spans.len() as u32,
        total_tokens,
        total_cost,
    };

    // Build response
    let data: Vec<BlockDto> = all_messages
        .iter()
        .map(BlockDto::from_block_entry)
        .collect();

    Ok(Json(FeedMessagesResponse {
        data,
        pagination: FeedPagination {
            next_cursor,
            has_more,
        },
        metadata,
        tool_definitions,
        tool_names,
    }))
}

// ============================================================================
// Feed spans endpoint
// ============================================================================

/// GET /feed/spans - Get latest spans across the project
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/feed/spans",
    tag = "feed",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("limit" = Option<u32>, Query, description = "Max spans to return (default: 50, max: 500)"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("start_time" = Option<String>, Query, description = "Filter by event time >= (ISO 8601)"),
        ("end_time" = Option<String>, Query, description = "Filter by event time < (ISO 8601)"),
        ("is_observation" = Option<bool>, Query, description = "Filter to GenAI spans only"),
        ("include_raw_span" = Option<bool>, Query, description = "Include raw OTLP span JSON")
    ),
    responses(
        (status = 200, description = "Feed spans", body = FeedSpansResponse)
    )
)]
pub async fn get_feed_spans(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    axum::extract::Query(query): axum::extract::Query<FeedSpansQuery>,
) -> Result<Json<FeedSpansResponse>, ApiError> {
    let project_id = auth.project_id.clone();

    let limit = validate_limit(query.limit);
    let cursor = query
        .cursor
        .as_ref()
        .map(|c| decode_cursor(c))
        .transpose()?;
    let start_time = parse_timestamp_param(&query.start_time)?;
    let end_time = parse_timestamp_param(&query.end_time)?;
    let is_observation = query.is_observation;
    let include_raw_span = query.include_raw_span.unwrap_or(false);

    // Query extra for deduplication buffer, then deduplicate
    let query_limit = (limit * 2).min(1000);

    // Build query parameters with cursor support
    let params = FeedSpansParams {
        project_id: project_id.clone(),
        limit: query_limit,
        cursor,
        start_time,
        end_time,
        is_observation,
    };

    // Fetch spans with cursor applied in SQL
    let repo = state.analytics.repository();
    let mut spans = repo
        .get_feed_spans(&params)
        .await
        .map_err(ApiError::from_data)?;

    // Deduplicate spans (DuckDB is append-only, duplicates possible)
    spans = deduplicate_spans(spans);

    // Re-sort by ingested_at DESC for correct cursor-based pagination
    // (deduplicate_spans sorts by timestamp_start, but cursor uses ingested_at)
    spans.sort_by(|a, b| b.ingested_at.cmp(&a.ingested_at));

    // Compute has_more and truncate
    let has_more = spans.len() > limit as usize;
    spans.truncate(limit as usize);

    // Compute cursor from last span
    let next_cursor = spans
        .last()
        .map(|s| encode_cursor(s.ingested_at, &s.span_id));

    // Convert to DTOs
    let data: Vec<SpanSummaryDto> = spans
        .iter()
        .map(|s| SpanSummaryDto::from_row(s, 0, 0, include_raw_span))
        .collect();

    Ok(Json(FeedSpansResponse {
        data,
        pagination: FeedPagination {
            next_cursor,
            has_more,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // ========================================================================
    // Cursor encoding/decoding tests
    // ========================================================================

    #[test]
    fn test_encode_decode_cursor_roundtrip() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "abc123def456";

        let encoded = encode_cursor(timestamp, span_id);
        let (decoded_us, decoded_span_id) = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded_us, timestamp.timestamp_micros());
        assert_eq!(decoded_span_id, span_id);
    }

    #[test]
    fn test_encode_cursor_format() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "span123";

        let encoded = encode_cursor(timestamp, span_id);

        // Should be base64 URL-safe without padding
        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_decode_cursor_with_colon_in_span_id() {
        // span_id might contain colons (e.g., "trace:abc:123")
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "span:with:colons";

        let encoded = encode_cursor(timestamp, span_id);
        let (_, decoded_span_id) = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded_span_id, span_id);
    }

    #[test]
    fn test_decode_cursor_invalid_base64() {
        let result = decode_cursor("not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_cursor_invalid_format_no_colon() {
        let encoded = URL_SAFE_NO_PAD.encode("notimestamp");
        let result = decode_cursor(&encoded);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_cursor_invalid_timestamp() {
        let encoded = URL_SAFE_NO_PAD.encode("not_a_number:span123");
        let result = decode_cursor(&encoded);
        assert!(result.is_err());
    }

    // ========================================================================
    // Limit validation tests
    // ========================================================================

    #[test]
    fn test_validate_limit_default() {
        assert_eq!(validate_limit(None), DEFAULT_FEED_LIMIT);
    }

    #[test]
    fn test_validate_limit_within_range() {
        assert_eq!(validate_limit(Some(100)), 100);
        assert_eq!(validate_limit(Some(1)), 1);
        assert_eq!(validate_limit(Some(500)), 500);
    }

    #[test]
    fn test_validate_limit_clamped_to_max() {
        assert_eq!(validate_limit(Some(1000)), MAX_FEED_LIMIT);
        assert_eq!(validate_limit(Some(u32::MAX)), MAX_FEED_LIMIT);
    }

    #[test]
    fn test_validate_limit_clamped_to_min() {
        assert_eq!(validate_limit(Some(0)), 1);
    }
}
