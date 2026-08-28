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
use crate::data::types::{FeedMessagesParams, FeedSpansParams, MessageQueryParams};
use crate::domain::sideml::{
    FeedOptions, apply_time_window, extract_tools_from_rows, process_feed,
};

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
/// Encode a feed cursor.
///
/// The trace id is part of it because a span id is unique only *within* a trace. Two traces can
/// carry the same span id in the same ingestion microsecond, and a page boundary falling between
/// them made the `< cursor` predicate skip the one that had not been returned - a message missing
/// from the feed for good.
fn encode_cursor(ingested_at: DateTime<Utc>, span_id: &str, trace_id: &str) -> String {
    // Trace id before span id, because the span id goes last and is the only field allowed to
    // contain a colon - `test_decode_cursor_with_colon_in_span_id` pins that. A trace id is hex.
    let cursor_str = format!(
        "{}:{}:{}",
        ingested_at.timestamp_micros(),
        trace_id,
        span_id
    );
    URL_SAFE_NO_PAD.encode(cursor_str)
}

/// Decode cursor to (ingested_at_us, span_id, trace_id)
///
/// A two-part cursor is one this server issued before the trace id was included; it is accepted so
/// a page request in flight across an upgrade does not fail, and resolves to an empty trace id,
/// which orders before every real one.
fn decode_cursor(cursor: &str) -> Result<(i64, String, String), ApiError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor format"))?;

    let cursor_str = String::from_utf8(decoded)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor encoding"))?;

    let parts: Vec<&str> = cursor_str.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(ApiError::bad_request(
            "INVALID_CURSOR",
            "Invalid cursor format: expected timestamp:span_id:trace_id",
        ));
    }

    let timestamp_us = parts[0]
        .parse::<i64>()
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor timestamp"))?;

    // Two parts is a cursor this server issued before the trace id was in the key: the second
    // field is the span id, and the trace id resolves to empty, which orders before every real one.
    let (trace_id, span_id) = match parts.len() {
        2 => ("", parts[1]),
        _ => (parts[1], parts[2]),
    };

    Ok((timestamp_us, span_id.to_string(), trace_id.to_string()))
}

/// Validate and clamp limit parameter
fn validate_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_FEED_LIMIT).clamp(1, MAX_FEED_LIMIT)
}

/// Keep the blocks belonging to the spans a feed page holds.
///
/// The pipeline is given whole traces so that reconstruction does not depend on where the page
/// boundary fell; this is what narrows the answer back to the page. The same shape as the trace
/// view's scoping of a session-loaded feed.
fn scope_feed_to_page(
    messages: Vec<crate::domain::sideml::BlockEntry>,
    page_spans: &HashSet<(String, String)>,
) -> Vec<crate::domain::sideml::BlockEntry> {
    messages
        .into_iter()
        .filter(|b| page_spans.contains(&(b.trace_id.clone(), b.span_id.clone())))
        .collect()
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
        .map(|s| encode_cursor(s.ingested_at, &s.span_id, &s.trace_id));

    // Reconstruct over whole traces, then narrow to the page.
    //
    // The page is chosen before the pipeline runs, so anything the pipeline decides by looking
    // across spans - which copy of a re-sent turn survives, which call a result answers - used to be
    // decided from a fragment. A trace split across two pages was reconstructed twice, from half its
    // spans each time, and both halves could show the same turn.
    //
    // Loading each trace on the page in full removes that: the traces are already named by the rows
    // just selected, so it is one further query bounded by the page, and the answer for a trace no
    // longer depends on where the page boundary fell. Blocks are then kept only for the spans the
    // page actually holds, the way the trace view scopes a session-loaded feed back to one trace.
    //
    // What remains page-local, and cannot be otherwise on a cursor-paginated endpoint: a replay that
    // crosses *traces* within a session is only recognised when both traces are on the page, and
    // pages are selected by ingestion time while each is ordered by message time, so concatenating
    // them is not guaranteed to be globally ordered. The trace and session views, which are where a
    // conversation is actually read, load their whole session for that reason.
    // An empty page loads nothing. `trace_ids` empty means "selector unused" to the message
    // queries, so passing it on would ask for the whole project with no content filter - a future
    // time window or an exhausted cursor turning into an unbounded read.
    if spans.is_empty() {
        return Ok(Json(FeedMessagesResponse {
            data: Vec::new(),
            pagination: FeedPagination {
                next_cursor,
                has_more,
            },
            metadata: FeedMessagesMetadata {
                message_count: 0,
                span_count: 0,
                total_tokens: 0,
                total_cost: 0.0,
            },
            tool_definitions: Vec::new(),
            tool_names: Vec::new(),
        }));
    }

    let page_spans: HashSet<(String, String)> = spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();
    // Totals from the page's own rows, before the context load widens the row set. Counted once per
    // span: a re-ingested span is two rows on DuckDB, which reads the raw table, and one on
    // ClickHouse, which reads it with FINAL.
    let mut counted: HashSet<(&str, &str)> = HashSet::new();
    let mut page_tokens = 0i64;
    let mut page_cost = 0.0f64;
    for row in &spans {
        if counted.insert((row.trace_id.as_str(), row.span_id.as_str())) {
            page_tokens += row.total_tokens;
            page_cost += row.cost_total;
        }
    }
    let page_span_count = counted.len() as u32;

    // The tools a page offers are the tools its own spans declared. Taken from the reconstruction
    // instead, a page would list tools that exist only on spans it does not show - the trace view
    // scopes them the same way when it loads a whole session.
    let page_tools = extract_tools_from_rows(spans.iter());

    let mut trace_ids: Vec<String> = spans.iter().map(|s| s.trace_id.clone()).collect();
    trace_ids.sort();
    trace_ids.dedup();

    let context = repo
        .get_messages(&MessageQueryParams {
            project_id: project_id.clone(),
            trace_ids: Some(trace_ids),
            // Bounded above by the window the request asked for, and deliberately not below it.
            //
            // Context is what came *before*, so the lower bound must not be applied here - that is
            // the whole reason `apply_time_window` runs on the answer instead. But without the upper
            // bound the reconstruction also read spans recorded *after* the window, which changes
            // what history detection collapses: a page of yesterday's feed could come back different
            // today because the same trace has since continued.
            to_timestamp: end_time,
            ..Default::default()
        })
        .await
        .map_err(ApiError::from_data)?;

    let options = FeedOptions::new().with_role(query.role.clone());

    // The window is a filter on the answer, here as in the other three views.
    //
    // The queries bound `timestamp_start`, and a completed response is timestamped at *span end* -
    // so a span that started inside the window and finished after it returned a message dated past
    // the window the request asked for. The upper bound on the context load does not cover that: it
    // decides which spans are read, not what time their messages carry.
    //
    // A page whose every block is filtered out still reports `has_more` and a cursor, because both
    // are properties of the row page rather than of the answer. That is how the role filter has
    // always behaved here, and it is what lets a client keep paging rather than stopping at the
    // first page a filter empties.
    let processed = apply_time_window(process_feed(context.rows, &options), start_time, end_time);
    let all_messages = scope_feed_to_page(processed.messages, &page_spans);
    let tool_definitions = page_tools.tool_definitions;
    let tool_names = page_tools.tool_names;

    // The page's totals, computed from the page's rows above rather than from the pipeline's - the
    // pipeline now sees whole traces, so its totals cover more than the page shows.
    //
    // Sums over spans, not over the blocks returned: summing blocks made a billed span contribute
    // nothing whenever all of its messages were dropped as history or by the role filter, so the
    // page's reported cost fell below what was actually spent.
    let metadata = FeedMessagesMetadata {
        message_count: all_messages.len() as u32,
        span_count: page_span_count,
        total_tokens: page_tokens,
        total_cost: page_cost,
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

    // Query limit + 1 to detect has_more
    let query_limit = limit + 1;

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

    // Compute has_more and truncate
    let has_more = spans.len() > limit as usize;
    spans.truncate(limit as usize);

    // Compute cursor from last span
    let next_cursor = spans
        .last()
        .map(|s| encode_cursor(s.ingested_at, &s.span_id, &s.trace_id));

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
    use crate::data::types::MessageSpanRow;
    use chrono::TimeZone;

    // ========================================================================
    // Cursor encoding/decoding tests
    // ========================================================================

    #[test]
    fn test_encode_decode_cursor_roundtrip() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "abc123def456";

        let trace_id = "0123456789abcdef";

        let encoded = encode_cursor(timestamp, span_id, trace_id);
        let (decoded_us, decoded_span_id, decoded_trace_id) = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded_us, timestamp.timestamp_micros());
        assert_eq!(decoded_span_id, span_id);
        assert_eq!(decoded_trace_id, trace_id);
    }

    #[test]
    fn test_encode_cursor_format() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "span123";

        let encoded = encode_cursor(timestamp, span_id, "trace123");

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

        let encoded = encode_cursor(timestamp, span_id, "tracewithoutcolons");
        let (_, decoded_span_id, decoded_trace_id) = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded_span_id, span_id);
        assert_eq!(decoded_trace_id, "tracewithoutcolons");
    }

    /// A cursor issued before the trace id was part of the key must still parse.
    #[test]
    fn test_decode_legacy_two_part_cursor() {
        let legacy = URL_SAFE_NO_PAD.encode("1736937000000000:abc123");
        let (us, span_id, trace_id) = decode_cursor(&legacy).expect("legacy cursor");
        assert_eq!(us, 1_736_937_000_000_000);
        assert_eq!(span_id, "abc123");
        assert_eq!(
            trace_id, "",
            "an absent trace id must order before every real one, not become the span id"
        );
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

    // ========================================================================
    // Page-scoped reconstruction
    // ========================================================================

    fn feed_row(trace: &str, span: &str, messages: &str, second: i64) -> MessageSpanRow {
        let t = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap()
            + chrono::Duration::seconds(second);
        MessageSpanRow {
            trace_id: trace.to_string(),
            span_id: span.to_string(),
            parent_span_id: None,
            span_timestamp: t,
            span_end_timestamp: Some(t),
            messages_json: messages.to_string(),
            tool_definitions_json: "[]".to_string(),
            tool_names_json: "[]".to_string(),
            model: None,
            provider: None,
            status_code: None,
            exception_type: None,
            exception_message: None,
            exception_stacktrace: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cost_total: 0.0,
            observation_type: Some("generation".to_string()),
            session_id: None,
            ingested_at: t,
        }
    }

    /// A response that completed after the window is not in the window.
    ///
    /// The queries bound `timestamp_start`, and a completed response carries its span's *end* time -
    /// so a span that began inside the window and finished after it produced a message dated past the
    /// window the request asked for. The window has to be applied to the answer, as the span, trace
    /// and session endpoints all do.
    #[test]
    fn a_response_finishing_after_the_window_is_excluded() {
        let start = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        // The span begins inside the window and ends a minute later, outside it.
        let messages = format!(
            r#"[{{"source":{{"event":{{"name":"gen_ai.user.message","time":"{}"}}}},
                 "content":{{"role":"user","content":"the question"}}}},
                {{"source":{{"event":{{"name":"gen_ai.choice","time":"{}"}}}},
                 "content":{{"role":"assistant","content":"the answer"}}}}]"#,
            start.to_rfc3339(),
            (start + chrono::Duration::seconds(60)).to_rfc3339()
        );
        let mut row = feed_row("trace-1", "span-1", &messages, 0);
        row.span_end_timestamp = Some(start + chrono::Duration::seconds(60));

        let options = FeedOptions::new();
        let window_end = start + chrono::Duration::seconds(30);

        let unwindowed = process_feed(vec![row.clone()], &options);
        assert!(
            unwindowed.messages.iter().any(|b| b.timestamp > window_end),
            "premise: the completed response is dated after the window closes"
        );

        let windowed = apply_time_window(process_feed(vec![row], &options), None, Some(window_end));
        assert!(
            windowed.messages.iter().all(|b| b.timestamp < window_end),
            "a message dated after the window was returned: {:?}",
            windowed
                .messages
                .iter()
                .map(|b| (b.role.as_str(), b.timestamp))
                .collect::<Vec<_>>()
        );
        assert!(
            windowed.messages.len() < unwindowed.messages.len(),
            "the window removed nothing, so this test proves nothing"
        );
    }

    /// The tools a page lists are the tools its own spans declared.
    ///
    /// Reconstruction is handed whole traces, so taking the tool set from its result would expose
    /// tools that exist only on spans the page does not show.
    #[test]
    fn page_tools_come_from_the_page() {
        let mut on_page = feed_row("trace-1", "span-1", "[]", 0);
        on_page.tool_names_json = r#"["on_page_tool"]"#.to_string();
        let mut off_page = feed_row("trace-1", "span-2", "[]", 5);
        off_page.tool_names_json = r#"["off_page_tool"]"#.to_string();

        // The page holds one span; the context load would add the other.
        let page_tools = extract_tools_from_rows([on_page.clone()].iter());
        assert_eq!(
            page_tools.tool_names,
            vec!["on_page_tool".to_string()],
            "the page's tools must come from its own rows"
        );

        let context_tools = extract_tools_from_rows([on_page, off_page].iter());
        assert!(
            context_tools.tool_names.len() > page_tools.tool_names.len(),
            "the context holds more tools than the page, or this test proves nothing"
        );
    }

    /// A trace split across two pages must not show the same turn twice.
    ///
    /// Each generation span re-sends the conversation so far, which is what the pipeline collapses.
    /// Reconstructing one page at a time meant each page saw only its own half of the trace, so the
    /// re-sent turn had nothing to collapse against and both pages returned it. Reconstructing the
    /// whole trace and then narrowing to the page removes that, and the two pages together return
    /// each turn once.
    #[test]
    fn a_trace_split_across_pages_returns_each_turn_once() {
        let first_turn = r#"[{"source":{"event":{"name":"gen_ai.user.message","time":"2025-01-15T10:30:00Z"}},
             "content":{"role":"user","content":"the question"}}]"#;
        // The second span re-sends the first turn, as every framework that keeps history does.
        let with_history = r#"[{"source":{"event":{"name":"gen_ai.user.message","time":"2025-01-15T10:30:00Z"}},
             "content":{"role":"user","content":"the question"}},
            {"source":{"event":{"name":"gen_ai.choice","time":"2025-01-15T10:30:05Z"}},
             "content":{"role":"assistant","content":"the answer"}}]"#;

        let rows = vec![
            feed_row("trace-1", "span-1", first_turn, 0),
            feed_row("trace-1", "span-2", with_history, 5),
        ];

        let options = FeedOptions::new();
        let whole = process_feed(rows.clone(), &options);

        // Two pages, one span each - the boundary a cursor would fall on.
        let page_one: HashSet<(String, String)> = [("trace-1".to_string(), "span-1".to_string())]
            .into_iter()
            .collect();
        let page_two: HashSet<(String, String)> = [("trace-1".to_string(), "span-2".to_string())]
            .into_iter()
            .collect();

        let mut returned: Vec<String> = scope_feed_to_page(whole.messages.clone(), &page_one)
            .iter()
            .chain(scope_feed_to_page(whole.messages.clone(), &page_two).iter())
            .map(|b| format!("{}:{}", b.role.as_str(), b.content_hash))
            .collect();
        let before_dedup = returned.len();
        returned.sort();
        returned.dedup();
        assert_eq!(
            returned.len(),
            before_dedup,
            "a turn was returned on both pages: {returned:?}"
        );

        // And the conversation is complete across the two pages: the question and the answer.
        assert_eq!(
            before_dedup, 2,
            "the two pages together must hold the question and the answer, not {before_dedup} blocks"
        );

        // What it replaces: reconstructing each page on its own sees half the trace, so the re-sent
        // question has nothing to collapse against and comes back twice.
        let page_local: usize = [vec![rows[0].clone()], vec![rows[1].clone()]]
            .into_iter()
            .map(|page| process_feed(page, &options).messages.len())
            .sum();
        assert!(
            page_local > before_dedup,
            "the page-local reconstruction returned {page_local} blocks and the trace-complete one \
             {before_dedup}; if they agree this test no longer distinguishes them"
        );
    }
}
