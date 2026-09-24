//! Feed API endpoints for project-wide message and span feeds
//!
//! Provides cursor-based pagination for real-time activity feeds.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::OtelApiState;
use super::json_stream;
use super::types::{
    BlockDto, FeedMessagesMetadata, FeedMessagesResponse, FeedPagination, FeedSpansResponse,
    SpanSummaryDto,
};
use crate::auth::ProjectRead;
use crate::types::{ApiError, parse_timestamp_param};
use sideseat_domain::sideml::{
    BlockEntry, FeedOptions, apply_time_window, extract_tools_from_rows, process_feed_cached,
};
use sideseat_ports::types::{FeedMessagesParams, FeedSpansParams, MessageQueryParams};

// ============================================================================
// Constants
// ============================================================================

const DEFAULT_FEED_LIMIT: u32 = 50;
const MAX_FEED_LIMIT: u32 = 500;
const FEED_CURSOR_VERSION: &str = "v1";

// ============================================================================
// Query parameters
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct FeedMessagesQuery {
    /// Maximum number of spans to return (default: 50, max: 500)
    pub limit: Option<u32>,
    /// Opaque URL-safe base64 pagination cursor
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
    /// Opaque URL-safe base64 pagination cursor
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

/// Encode a feed cursor.
///
/// The trace id disambiguates span ids, which are unique only within a trace. The complete ordering
/// key prevents a page boundary from skipping one of two equal span ids ingested in the same
/// microsecond.
fn encode_cursor(
    watermark_us: i64,
    ingested_at: DateTime<Utc>,
    span_id: &str,
    trace_id: &str,
) -> String {
    // The watermark leads, because it is the traversal's identity rather than the page's position: a
    // traversal is a view of one instant, and the same watermark bounds page selection *and* the
    // reconstruction context loaded around each page.
    //
    // Trace id before span id, because the span id goes last and is the only field allowed to
    // contain a colon - `test_decode_cursor_with_colon_in_span_id` pins that. A trace id is hex.
    let cursor_str = format!(
        "{FEED_CURSOR_VERSION}:{}:{}:{}:{}",
        watermark_us,
        ingested_at.timestamp_micros(),
        trace_id,
        span_id
    );
    URL_SAFE_NO_PAD.encode(cursor_str)
}

/// A decoded feed cursor: the traversal's watermark and the page's position within it.
struct FeedCursor {
    /// The traversal watermark: rows ingested at or after this are invisible for the whole traversal.
    ///
    /// `None` for a legacy cursor without a watermark. Such a traversal remains unbounded so pagination
    /// already in progress stays compatible across an upgrade.
    watermark_us: Option<i64>,
    position: (i64, String, String),
}

/// Decode a feed cursor, accepting every form this server has issued.
///
/// Current cursors carry an explicit version. Legacy four-, three- and two-part cursors remain valid so
/// pagination already in progress survives an upgrade. Trace-bearing legacy forms are recognised by their
/// canonical 32-hex-character trace id; otherwise the complete suffix belongs to the two-part cursor's span
/// id. This preserves colons in legacy span ids despite the old format's ambiguous delimiter.
fn decode_cursor(cursor: &str) -> Result<FeedCursor, ApiError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor format"))?;

    let cursor_str = String::from_utf8(decoded)
        .map_err(|_| ApiError::bad_request("INVALID_CURSOR", "Invalid cursor encoding"))?;

    let invalid_format = || ApiError::bad_request("INVALID_CURSOR", "Invalid cursor format");
    let invalid = || ApiError::bad_request("INVALID_CURSOR", "Invalid cursor timestamp");

    if let Some(versioned) = cursor_str.strip_prefix("v1:") {
        let parts: Vec<&str> = versioned.splitn(4, ':').collect();
        if parts.len() != 4 {
            return Err(invalid_format());
        }
        return Ok(FeedCursor {
            watermark_us: Some(parts[0].parse::<i64>().map_err(|_| invalid())?),
            position: (
                parts[1].parse::<i64>().map_err(|_| invalid())?,
                parts[3].to_string(),
                parts[2].to_string(),
            ),
        });
    }

    let (first, suffix) = cursor_str.split_once(':').ok_or_else(invalid_format)?;
    let first_us = first.parse::<i64>().map_err(|_| invalid())?;

    if let Some((second, legacy_suffix)) = suffix.split_once(':')
        && let Ok(timestamp_us) = second.parse::<i64>()
        && let Some((candidate, span_id)) = legacy_suffix.split_once(':')
        && is_canonical_trace_id(candidate)
    {
        return Ok(FeedCursor {
            watermark_us: Some(first_us),
            position: (timestamp_us, span_id.to_string(), candidate.to_string()),
        });
    }

    let (trace_id, span_id) = match suffix.split_once(':') {
        Some((candidate, span_id)) if is_canonical_trace_id(candidate) => (candidate, span_id),
        _ => ("", suffix),
    };
    Ok(FeedCursor {
        watermark_us: None,
        position: (first_us, span_id.to_string(), trace_id.to_string()),
    })
}

fn is_canonical_trace_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Validate and clamp the limit parameter.
fn validate_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_FEED_LIMIT).clamp(1, MAX_FEED_LIMIT)
}

/// Keep the blocks belonging to the spans a feed page holds.
///
/// The pipeline is given whole traces so that reconstruction does not depend on where the page
/// boundary fell; this is what narrows the answer back to the page. The same shape as the trace
/// view's scoping of a session-loaded feed.
fn scope_feed_to_page(
    messages: Vec<sideseat_domain::sideml::BlockEntry>,
    page_spans: &HashSet<(String, String)>,
) -> Vec<sideseat_domain::sideml::BlockEntry> {
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
) -> Result<Response, ApiError> {
    let project_id = auth.project_id.clone();

    let limit = validate_limit(query.limit);
    let decoded = query
        .cursor
        .as_ref()
        .map(|c| decode_cursor(c))
        .transpose()?;
    let start_time = parse_timestamp_param(&query.start_time)?;
    let end_time = parse_timestamp_param(&query.end_time)?;

    // Establish one traversal watermark on the first page and carry it in every subsequent cursor.
    //
    // Applying the same watermark to page selection and reconstruction context makes the traversal a view
    // of one instant. Otherwise a row arriving between pages could participate in deduplication before it is
    // selected and suppress both itself and the row it replaces. Legacy cursors carry no watermark and keep
    // their unbounded traversal semantics.
    //
    // Derive the watermark from the **store**, not the reader's clock, so committed rows and page boundaries
    // share one time domain. `max_ingested_at_us` documents the remaining write-commit race.
    let repo = state.analytics.as_ref();
    let watermark_us = match decoded.as_ref().and_then(|c| c.watermark_us) {
        Some(carried) => carried,
        None => repo
            .max_ingested_at_us(&project_id)
            .await
            .map_err(ApiError::from_data)?
            // An empty project has nothing to bound, and a watermark of zero would match nothing at all.
            .map_or(i64::MAX, |newest| newest + 1),
    };
    let cursor = decoded.map(|c| c.position);

    // Query limit + 1 to detect has_more
    let query_limit = limit + 1;

    let params = FeedMessagesParams {
        project_id: project_id.clone(),
        limit: query_limit,
        cursor,
        start_time,
        end_time,
        ingested_before_us: Some(watermark_us),
    };

    // Fetch raw span rows
    let mut result = repo
        .get_project_messages(&params)
        .await
        .map_err(ApiError::from_data)?;
    state
        .content_bodies
        .hydrate_message_rows(&project_id, &mut result.rows)
        .await;

    let mut spans = result.rows;

    // Compute has_more from query results, then truncate
    let has_more = spans.len() > limit as usize;
    spans.truncate(limit as usize);

    // Compute cursor from raw query results BEFORE processing
    let next_cursor = spans
        .last()
        .map(|s| encode_cursor(watermark_us, s.ingested_at, &s.span_id, &s.trace_id));

    // Reconstruct over complete sessions, then narrow the result to the selected page.
    //
    // Replay detection and tool-call matching depend on neighbouring spans and traces. Session-wide context
    // makes those decisions independent of page boundaries; `scope_feed_to_page` then keeps only blocks
    // owned by the page's spans.
    //
    // What remains, and cannot be otherwise on a cursor-paginated endpoint: pages are selected by
    // *ingestion* time while each page's messages are ordered by *message* time, because the ordering key
    // is computed by the pipeline and does not exist in SQL to page by. So each page is a correct window
    // and concatenating pages is not a globally ordered transcript - which is what `session_scoped` in the
    // response metadata says out loud, rather than leaving a client to assume otherwise. The trace and
    // session views are where a conversation is read in order.
    // An empty page loads nothing. `trace_ids` empty means "selector unused" to the message
    // queries, so passing it on would ask for the whole project with no content filter - a future
    // time window or an exhausted cursor turning into an unbounded read.
    if spans.is_empty() {
        return stream_feed_messages_response(
            Vec::new(),
            FeedPagination {
                next_cursor,
                has_more,
            },
            FeedMessagesMetadata {
                // An empty page collapsed nothing, so it hid nothing.
                replay_matching_complete: true,
                message_count: 0,
                span_count: 0,
                total_tokens: 0,
                total_cost: 0.0,
                // An empty page touched no session, so it saw all of every session it touched.
                session_scoped: true,
                pages_are_globally_ordered: false,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
    }

    let page_spans: HashSet<(String, String)> = spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();
    // Envelopes for the page's own spans, before the context load widens the row set - the same
    // scope the totals use, and for the same reason: the pipeline sees more than the page shows.
    let mut envelope_seen: HashSet<(&str, &str)> = HashSet::new();
    let envelopes: Vec<super::types::SpanEnvelopeDto> = spans
        .iter()
        .filter(|row| envelope_seen.insert((row.trace_id.as_str(), row.span_id.as_str())))
        .map(super::types::SpanEnvelopeDto::from_row)
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

    // Whole *sessions*, not merely whole traces.
    //
    // Resolve sessions from the page's **traces**, not from session ids on the selected spans. Frameworks
    // commonly attach the session only to a root span, while a page can begin or end on child spans.
    //
    // Resolve membership at the traversal watermark as well. This keeps the content and its session graph
    // in the same snapshot when a trace is re-delivered into another session. DuckDB honours this bound;
    // ClickHouse has the same current-snapshot limitation documented for the page query.
    let mut trace_ids: Vec<String> = spans.iter().map(|s| s.trace_id.clone()).collect();
    trace_ids.sort();
    trace_ids.dedup();

    let session_ids = repo
        .get_session_ids_for_traces(&project_id, &trace_ids, Some(watermark_us))
        .await
        .map_err(ApiError::from_data)?;

    if !session_ids.is_empty() {
        let session_traces = repo
            .get_trace_ids_for_sessions(&project_id, &session_ids, Some(watermark_us))
            .await
            .map_err(ApiError::from_data)?;
        trace_ids.extend(session_traces);
        trace_ids.sort();
        trace_ids.dedup();
    }

    let context_trace_ids = trace_ids.clone();

    let mut context = repo
        .get_messages(&MessageQueryParams {
            project_id: project_id.clone(),
            trace_ids: Some(trace_ids),
            // The traversal watermark, so the context and the page describe the same instant.
            ingested_before_us: Some(watermark_us),
            // Bounded above by the window the request asked for, and deliberately not below it.
            //
            // Context is what came *before*, so the lower bound belongs on the reconstructed answer rather
            // than this query. The upper bound keeps reconstruction deterministic as a trace continues after
            // the requested window.
            to_timestamp: end_time,
            ..Default::default()
        })
        .await
        .map_err(ApiError::from_data)?;
    state
        .content_bodies
        .hydrate_message_rows(&project_id, &mut context.rows)
        .await;

    // Which trace is in which session, from the store rather than from the rows below.
    //
    // `MESSAGE_CONTENT_FILTER` may remove the root span that carries the session id. Store-derived membership
    // therefore supplies the complete conversation grouping required for cross-trace replay detection.
    let session_of_trace: std::collections::HashMap<String, String> = repo
        .get_trace_session_pairs(&project_id, &context_trace_ids, Some(watermark_us))
        .await
        .map_err(ApiError::from_data)?
        .into_iter()
        .collect();

    let options = FeedOptions::new()
        .with_role(query.role.clone())
        .with_session_of_trace(session_of_trace);

    // The window is a filter on the answer, here as in the other three views.
    //
    // Queries bound `timestamp_start`, while a completed response is timestamped at *span end*. Filtering
    // reconstructed blocks enforces the requested window on message time rather than span-selection time.
    //
    // A page whose every block is filtered out still reports `has_more` and a cursor, because both
    // are properties of the row page rather than of the answer. Clients can therefore continue past
    // a page emptied by a filter.
    let reconstructed = process_feed_cached(&state.reconstruction, context.rows, &options);
    let processed = apply_time_window(&reconstructed, start_time, end_time);
    // Cloned here rather than moved, because the source may be the shared memo: the page scoping consumes the
    // block vector, and consuming a cached answer is not available to one of several readers of it.
    let all_messages = scope_feed_to_page(processed.messages.clone(), &page_spans);
    let tool_definitions = page_tools.tool_definitions;
    let tool_names = page_tools.tool_names;

    // The page's totals come from its selected rows rather than the session-wide reconstruction context.
    //
    // Sum over spans, not returned blocks, so replay and role filtering cannot remove billed usage.
    let metadata = FeedMessagesMetadata {
        // Carried from the pipeline: a page whose replay matching was cut short may repeat history, and
        // the caller has no other way to know.
        replay_matching_complete: processed.metadata.replay_matching_complete,
        message_count: all_messages.len() as u32,
        span_count: page_span_count,
        total_tokens: page_tokens,
        total_cost: page_cost,
        // Every page trace contributes its resolved session; a trace without a session has no wider context.
        session_scoped: true,
        // Always false, and said out loud: pages are selected by ingestion time while their messages are
        // ordered by message time.
        pages_are_globally_ordered: false,
    };

    stream_feed_messages_response(
        all_messages,
        FeedPagination {
            next_cursor,
            has_more,
        },
        metadata,
        tool_definitions,
        tool_names,
        envelopes,
    )
}

fn stream_feed_messages_response(
    messages: Vec<BlockEntry>,
    pagination: FeedPagination,
    metadata: FeedMessagesMetadata,
    tool_definitions: Vec<serde_json::Value>,
    tool_names: Vec<String>,
    envelopes: Vec<super::types::SpanEnvelopeDto>,
) -> Result<Response, ApiError> {
    let trailing = [
        ("pagination", serde_json::to_string(&pagination)),
        ("metadata", serde_json::to_string(&metadata)),
        ("tool_definitions", serde_json::to_string(&tool_definitions)),
        ("tool_names", serde_json::to_string(&tool_names)),
        ("envelopes", serde_json::to_string(&envelopes)),
    ]
    .into_iter()
    .map(|(name, value)| {
        value
            .map(|value| (name, value))
            .map_err(|error| ApiError::internal(format!("Could not serialize response: {error}")))
    })
    .collect::<Result<Vec<_>, _>>()?;
    let messages = Arc::new(messages);
    let len = messages.len();
    Ok(json_stream::object_array(
        "data",
        len,
        move |index| serde_json::to_string(&BlockDto::from_block_entry(&messages[index])),
        trailing,
    ))
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
    let decoded = query
        .cursor
        .as_ref()
        .map(|c| decode_cursor(c))
        .transpose()?;
    let start_time = parse_timestamp_param(&query.start_time)?;
    let end_time = parse_timestamp_param(&query.end_time)?;
    let is_observation = query.is_observation;
    let include_raw_span = query.include_raw_span.unwrap_or(false);

    // The same traversal watermark the message feed uses, and taken from the store for the same reason.
    // This endpoint returns raw spans rather than a reconstruction, so there is no context load to keep
    // consistent - but a span ingested mid-traversal still shifts every subsequent page's boundary, and a
    // client concatenating pages would see one twice or not at all.
    let repo = state.analytics.as_ref();
    let watermark_us = match decoded.as_ref().and_then(|c| c.watermark_us) {
        Some(carried) => carried,
        None => repo
            .max_ingested_at_us(&project_id)
            .await
            .map_err(ApiError::from_data)?
            .map_or(i64::MAX, |newest| newest + 1),
    };
    let cursor = decoded.map(|c| c.position);

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
        ingested_before_us: Some(watermark_us),
    };

    // Fetch spans with cursor applied in SQL
    let mut spans = repo
        .get_feed_spans(&params)
        .await
        .map_err(ApiError::from_data)?;

    // Compute has_more and truncate
    let has_more = spans.len() > limit as usize;
    spans.truncate(limit as usize);
    if include_raw_span {
        state
            .content_bodies
            .hydrate_raw_spans(&project_id, &mut spans)
            .await;
    }

    // Compute cursor from last span
    let next_cursor = spans
        .last()
        .map(|s| encode_cursor(watermark_us, s.ingested_at, &s.span_id, &s.trace_id));

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
    use sideseat_domain::sideml::process_feed;
    use sideseat_ports::types::MessageSpanRow;

    // ========================================================================
    // Cursor encoding/decoding tests
    // ========================================================================

    #[test]
    fn test_encode_decode_cursor_roundtrip() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "abc123def456";

        let trace_id = "0123456789abcdef";

        let watermark = 1_700_000_000_000_000i64;
        let encoded = encode_cursor(watermark, timestamp, span_id, trace_id);
        let decoded = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded.watermark_us, Some(watermark));
        assert_eq!(decoded.position.0, timestamp.timestamp_micros());
        assert_eq!(decoded.position.1, span_id);
        assert_eq!(decoded.position.2, trace_id);
    }

    #[test]
    fn test_encode_cursor_format() {
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "span123";

        let encoded = encode_cursor(1, timestamp, span_id, "trace123");

        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        let wire = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
        assert!(wire.starts_with("v1:"));
    }

    #[test]
    fn test_decode_cursor_with_colon_in_span_id() {
        // span_id might contain colons (e.g., "trace:abc:123")
        let timestamp = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let span_id = "span:with:colons";

        let encoded = encode_cursor(1, timestamp, span_id, "tracewithoutcolons");
        let decoded = decode_cursor(&encoded).unwrap();

        assert_eq!(decoded.position.1, span_id);
        assert_eq!(decoded.position.2, "tracewithoutcolons");
    }

    /// Legacy cursors remain readable without weakening the current cursor's total ordering.
    ///
    /// A two- or three-part cursor carries no watermark, so its traversal remains unbounded. The two-part
    /// format keeps its complete span-id suffix, including colons; trace-bearing forms recognise a canonical
    /// trace id.
    #[test]
    fn test_decode_legacy_cursors() {
        let two_part = URL_SAFE_NO_PAD.encode("1736937000000000:abc123");
        let decoded = decode_cursor(&two_part).expect("legacy two-part cursor");
        assert_eq!(decoded.watermark_us, None);
        assert_eq!(decoded.position.0, 1_736_937_000_000_000);
        assert_eq!(decoded.position.1, "abc123");
        assert_eq!(
            decoded.position.2, "",
            "an absent trace id must order before every real one, not become the span id"
        );

        let two_part_with_colons = URL_SAFE_NO_PAD.encode("1736937000000000:123:with:colons");
        let decoded =
            decode_cursor(&two_part_with_colons).expect("legacy two-part cursor with colons");
        assert_eq!(decoded.watermark_us, None);
        assert_eq!(decoded.position.1, "123:with:colons");
        assert_eq!(decoded.position.2, "");

        let three_part = URL_SAFE_NO_PAD
            .encode("1736937000000000:0123456789abcdef0123456789abcdef:span:with:colons");
        let decoded = decode_cursor(&three_part).expect("legacy three-part cursor");
        assert_eq!(
            decoded.watermark_us, None,
            "a pre-watermark cursor must not have its trace id read as a watermark"
        );
        assert_eq!(decoded.position.0, 1_736_937_000_000_000);
        assert_eq!(decoded.position.1, "span:with:colons");
        assert_eq!(decoded.position.2, "0123456789abcdef0123456789abcdef");

        let four_part = URL_SAFE_NO_PAD.encode(
            "1700000000000000:1736937000000000:0123456789abcdef0123456789abcdef:span:with:colons",
        );
        let decoded = decode_cursor(&four_part).expect("legacy four-part cursor");
        assert_eq!(decoded.watermark_us, Some(1_700_000_000_000_000));
        assert_eq!(decoded.position.0, 1_736_937_000_000_000);
        assert_eq!(decoded.position.1, "span:with:colons");
        assert_eq!(decoded.position.2, "0123456789abcdef0123456789abcdef");
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
            body_cache_key: None,
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
            scope_name: None,
            scope_version: None,
            span_name: None,
            framework: None,
            response_model: None,
            response_id: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            finish_reasons: None,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_input: 0.0,
            cost_output: 0.0,
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

        let unwindowed = process_feed(vec![row], &options);
        let windowed = apply_time_window(&unwindowed, None, Some(window_end));
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
