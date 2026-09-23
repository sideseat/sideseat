//! Query repository for OTEL API queries (ClickHouse backend).
//!
//! Provides the same interface as DuckDB query repository but uses ClickHouse SQL.

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::Deserialize;

use sideseat_query_sql::{Backend, analytics, confirmations, dml};

pub(super) fn bind_analytics_values(
    mut query: clickhouse::query::Query,
    values: &[analytics::QueryValue],
) -> clickhouse::query::Query {
    for value in values {
        query = match value {
            analytics::QueryValue::String(value) => query.bind(value),
            analytics::QueryValue::Int64(value) => query.bind(value),
            analytics::QueryValue::Float64(value) => query.bind(value),
        };
    }
    query
}

pub async fn spans_match_content(
    client: &Client,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<bool, ClickhouseError> {
    let Some(plan) = confirmations::spans(project_id, records, Backend::Clickhouse) else {
        return Ok(true);
    };
    let found: u64 = bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
        .fetch_one()
        .await?;
    Ok(found == plan.expected)
}

/// Builder for constructing parameterized SQL WHERE clauses.
///
/// Collects conditions and their parameter values, then allows binding
/// all parameters to a ClickHouse query in order.
///
/// # SQL Injection Safety
/// All values that could potentially come from user input are parameterized.
use crate::ClickhouseError;
use sideseat_core::utils::time::parse_iso_timestamp;
use sideseat_ports::types::{
    EventRow, FeedSpansParams, LinkRow, ListSessionsParams, ListSpansParams, ListTracesParams,
    ProjectId, SessionRow, SpanRow, TraceRow, parse_finish_reasons, parse_tags,
};

/// ClickHouse row for trace queries
#[derive(Row, Deserialize)]
struct ChTraceRow {
    trace_id: String,
    trace_name: Option<String>,
    start_time: i64,
    end_time: i64,
    duration_ms: i64,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    span_count: u64,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
    tags: Option<String>,
    observation_count: u64,
    metadata: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    has_error: u8,
}

impl From<ChTraceRow> for TraceRow {
    fn from(row: ChTraceRow) -> Self {
        Self {
            trace_id: row.trace_id,
            trace_name: row.trace_name,
            start_time: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            end_time: Some(
                DateTime::from_timestamp_micros(row.end_time).unwrap_or(DateTime::UNIX_EPOCH),
            ),
            duration_ms: Some(row.duration_ms),
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            span_count: row.span_count as i64,
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
            tags: parse_tags(&row.tags),
            observation_count: row.observation_count as i64,
            metadata: row.metadata,
            input_preview: row.input_preview,
            output_preview: row.output_preview,
            has_error: row.has_error != 0,
        }
    }
}

/// ClickHouse row for span queries
#[derive(Row, Deserialize)]
struct ChSpanRow {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    span_name: Option<String>,
    span_kind: Option<String>,
    span_category: Option<String>,
    observation_type: Option<String>,
    framework: Option<String>,
    status_code: Option<String>,
    start_time: i64,
    end_time: Option<i64>,
    duration_ms: Option<i64>,
    environment: Option<String>,
    resource_attributes: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    gen_ai_system: Option<String>,
    gen_ai_request_model: Option<String>,
    gen_ai_agent_name: Option<String>,
    gen_ai_finish_reasons: Option<String>,
    gen_ai_usage_input_tokens: i64,
    gen_ai_usage_output_tokens: i64,
    gen_ai_usage_total_tokens: i64,
    gen_ai_usage_cache_read_tokens: i64,
    gen_ai_usage_cache_write_tokens: i64,
    gen_ai_usage_reasoning_tokens: i64,
    gen_ai_cost_input: f64,
    gen_ai_cost_output: f64,
    gen_ai_cost_cache_read: f64,
    gen_ai_cost_cache_write: f64,
    gen_ai_cost_reasoning: f64,
    gen_ai_cost_total: f64,
    gen_ai_usage_details: Option<String>,
    metadata: Option<String>,
    attributes: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    raw_span: Option<String>,
    ingested_at_us: i64,
    scope_name: Option<String>,
    scope_version: Option<String>,
}

impl From<ChSpanRow> for SpanRow {
    fn from(row: ChSpanRow) -> Self {
        Self {
            trace_id: row.trace_id,
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            span_name: row.span_name,
            span_kind: row.span_kind,
            span_category: row.span_category,
            observation_type: row.observation_type,
            framework: row.framework,
            status_code: row.status_code,
            timestamp_start: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            timestamp_end: row.end_time.and_then(DateTime::from_timestamp_micros),
            duration_ms: row.duration_ms,
            environment: row.environment,
            resource_attributes: row.resource_attributes,
            session_id: row.session_id,
            user_id: row.user_id,
            gen_ai_system: row.gen_ai_system,
            gen_ai_request_model: row.gen_ai_request_model,
            gen_ai_agent_name: row.gen_ai_agent_name,
            gen_ai_finish_reasons: parse_finish_reasons(&row.gen_ai_finish_reasons),
            gen_ai_usage_input_tokens: row.gen_ai_usage_input_tokens,
            gen_ai_usage_output_tokens: row.gen_ai_usage_output_tokens,
            gen_ai_usage_total_tokens: row.gen_ai_usage_total_tokens,
            gen_ai_usage_cache_read_tokens: row.gen_ai_usage_cache_read_tokens,
            gen_ai_usage_cache_write_tokens: row.gen_ai_usage_cache_write_tokens,
            gen_ai_usage_reasoning_tokens: row.gen_ai_usage_reasoning_tokens,
            gen_ai_cost_input: row.gen_ai_cost_input,
            gen_ai_cost_output: row.gen_ai_cost_output,
            gen_ai_cost_cache_read: row.gen_ai_cost_cache_read,
            gen_ai_cost_cache_write: row.gen_ai_cost_cache_write,
            gen_ai_cost_reasoning: row.gen_ai_cost_reasoning,
            gen_ai_cost_total: row.gen_ai_cost_total,
            gen_ai_usage_details: row.gen_ai_usage_details,
            metadata: row.metadata,
            attributes: row.attributes,
            input_preview: row.input_preview,
            output_preview: row.output_preview,
            raw_span: row.raw_span,
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            ingested_at: DateTime::from_timestamp_micros(row.ingested_at_us)
                .unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}

/// ClickHouse row for session queries
#[derive(Row, Deserialize)]
struct ChSessionRow {
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    start_time: i64,
    end_time: i64,
    trace_count: u64,
    span_count: u64,
    observation_count: u64,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
}

impl From<ChSessionRow> for SessionRow {
    fn from(row: ChSessionRow) -> Self {
        Self {
            session_id: row.session_id.unwrap_or_default(),
            user_id: row.user_id,
            environment: row.environment,
            start_time: DateTime::from_timestamp_micros(row.start_time)
                .unwrap_or(DateTime::UNIX_EPOCH),
            end_time: Some(
                DateTime::from_timestamp_micros(row.end_time).unwrap_or(DateTime::UNIX_EPOCH),
            ),
            trace_count: row.trace_count as i64,
            span_count: row.span_count as i64,
            observation_count: row.observation_count as i64,
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
        }
    }
}

/// ClickHouse row for event queries
#[derive(Row, Deserialize)]
struct ChEventRow {
    span_id: String,
    event_index: i32,
    event_timestamp: String,
    event_name: Option<String>,
    attributes: Option<String>,
}

impl From<ChEventRow> for EventRow {
    fn from(row: ChEventRow) -> Self {
        Self {
            span_id: row.span_id,
            event_index: row.event_index,
            event_time: parse_iso_timestamp(&row.event_timestamp),
            event_name: row.event_name,
            attributes: row.attributes,
        }
    }
}

/// ClickHouse row for link queries
#[derive(Row, Deserialize)]
struct ChLinkRow {
    span_id: String,
    linked_trace_id: String,
    linked_span_id: String,
    attributes: Option<String>,
}

impl From<ChLinkRow> for LinkRow {
    fn from(row: ChLinkRow) -> Self {
        Self {
            span_id: row.span_id,
            linked_trace_id: row.linked_trace_id,
            linked_span_id: row.linked_span_id,
            attributes: row.attributes,
        }
    }
}

/// List traces with pagination and filtering
pub async fn list_traces(
    client: &Client,
    params: &ListTracesParams,
) -> Result<(Vec<TraceRow>, u64), ClickhouseError> {
    let page = analytics::list_traces(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChTraceRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(TraceRow::from).collect(), total))
}

/// Get spans for a specific trace
pub async fn get_spans_for_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    limit: usize,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    let query = analytics::spans_for_trace(project_id, trace_id, limit, Backend::Clickhouse);
    let rows: Vec<ChSpanRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;

    let spans: Vec<SpanRow> = rows.into_iter().map(SpanRow::from).collect();
    Ok(spans)
}

/// Get a single span
pub async fn get_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Option<SpanRow>, ClickhouseError> {
    let query = analytics::span_by_id().render(Backend::Clickhouse);

    let row: Option<ChSpanRow> = client
        .query(query.sql())
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_optional()
        .await?;

    Ok(row.map(SpanRow::from))
}

/// List spans with pagination and filtering
pub async fn list_spans(
    client: &Client,
    params: &ListSpansParams,
) -> Result<(Vec<SpanRow>, u64), ClickhouseError> {
    let page = analytics::list_spans(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChSpanRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;

    Ok((rows.into_iter().map(SpanRow::from).collect(), total))
}

/// Get feed spans (cursor-based pagination for real-time updates)
pub async fn get_feed_spans(
    client: &Client,
    params: &FeedSpansParams,
) -> Result<Vec<SpanRow>, ClickhouseError> {
    let query = analytics::feed_spans(params, Backend::Clickhouse);
    let rows: Vec<ChSpanRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(SpanRow::from).collect())
}

/// List sessions with pagination and filtering
pub async fn list_sessions(
    client: &Client,
    params: &ListSessionsParams,
) -> Result<(Vec<SessionRow>, u64), ClickhouseError> {
    let page = analytics::list_sessions(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChSessionRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(SessionRow::from).collect(), total))
}

/// Get session details
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, ClickhouseError> {
    let query = analytics::session_by_id(project_id, session_id, Backend::Clickhouse);
    let row: Option<ChSessionRow> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_optional()
            .await?;
    Ok(row.map(SessionRow::from))
}

/// Get events for a span (extracted from raw_span JSON)
pub async fn get_events_for_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<EventRow>, ClickhouseError> {
    let query = analytics::events_for_span(project_id, trace_id, span_id, Backend::Clickhouse);
    let rows: Vec<ChEventRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;

    Ok(rows.into_iter().map(EventRow::from).collect())
}

/// Get links for a span (extracted from raw_span JSON)
pub async fn get_links_for_span(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<Vec<LinkRow>, ClickhouseError> {
    let query = analytics::links_for_span(project_id, trace_id, span_id, Backend::Clickhouse);
    let rows: Vec<ChLinkRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;

    Ok(rows.into_iter().map(LinkRow::from).collect())
}

/// Get a single trace by ID
pub async fn get_trace(
    client: &Client,
    project_id: &str,
    trace_id: &str,
) -> Result<Option<TraceRow>, ClickhouseError> {
    let query = analytics::trace_by_id(project_id, trace_id, Backend::Clickhouse);
    let row: Option<ChTraceRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_optional()
        .await?;
    Ok(row.map(TraceRow::from))
}

/// Get traces for a session
/// session_id is only on root spans; uses session_traces CTE to find all traces,
/// then queries all spans from those traces.
pub async fn get_traces_for_session(
    client: &Client,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<TraceRow>, ClickhouseError> {
    let query = analytics::traces_for_session(project_id, session_id, Backend::Clickhouse);
    let rows: Vec<ChTraceRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(TraceRow::from).collect())
}

/// Which session each of the given traces belongs to; traces with none are absent.
///
/// The DuckDB twin. `FINAL` for the same reason it deduplicates there, and `argMin` over
/// `(timestamp_start, span_id)` so the session is the one on the trace's earliest span - which is what the
/// trace and session views display. `min(session_id)` picked the lexicographically smallest instead, so a
/// trace could be displayed under one session and grouped under another.
/// The relation a membership query reads: deduplicated as of a watermark, or plain `FINAL`.
///
/// The three membership methods used to accept `as_of_us` and ignore it, on the grounds that "`FINAL` has no
/// as-of form" - which stopped being true when `ch_dedup_spans_as_of_watermark` was written for the message
/// rows. Ignoring it made a feed traversal read watermark-era *rows* and current *membership*: a trace
/// re-delivered into another session mid-traversal was reconstructed as its old version while its session,
/// and therefore the context loaded around it, came from the new one - so the traversal was not a view of one
/// instant, which is the whole point of the watermark. The residual documented on
/// `ch_dedup_spans_as_of_watermark` applies here too: exact only while the pre-watermark version has not been
/// merged away.
pub async fn get_trace_session_pairs(
    client: &Client,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<(String, String)>, ClickhouseError> {
    let Some(statement) =
        analytics::trace_session_pairs(project_id, trace_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct PairRow {
        trace_id: String,
        session: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<PairRow> = query.fetch_all().await?;

    let mut pairs: Vec<(String, String)> =
        rows.into_iter().map(|r| (r.trace_id, r.session)).collect();
    pairs.sort();
    Ok(pairs)
}

/// Get trace IDs for given session IDs
/// The distinct sessions the given traces belong to.
pub async fn get_session_ids_for_traces(
    client: &Client,
    project_id: &str,
    trace_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::session_ids_for_traces(project_id, trace_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct SessionIdRow {
        canonical_session: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<SessionIdRow> = query.fetch_all().await?;

    let mut session_ids: Vec<String> = rows.into_iter().map(|r| r.canonical_session).collect();
    session_ids.sort();
    session_ids.dedup();
    Ok(session_ids)
}

pub async fn get_trace_ids_for_sessions(
    client: &Client,
    project_id: &str,
    session_ids: &[String],
    as_of_us: Option<i64>,
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::trace_ids_for_sessions(project_id, session_ids, as_of_us, Backend::Clickhouse)
    else {
        return Ok(vec![]);
    };

    #[derive(Row, Deserialize)]
    struct TraceIdRow {
        trace_id: String,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<TraceIdRow> = query.fetch_all().await?;

    Ok(rows.into_iter().map(|r| r.trace_id).collect())
}

/// Get span counts (events and links) in bulk
pub async fn get_span_counts_bulk(
    client: &Client,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<
    std::collections::HashMap<(String, String), sideseat_ports::types::SpanCounts>,
    ClickhouseError,
> {
    use sideseat_ports::types::SpanCounts;
    use std::collections::HashMap;

    let Some(statement) = analytics::span_counts_bulk(project_id, spans, Backend::Clickhouse)
    else {
        return Ok(HashMap::new());
    };

    let mut counts: HashMap<(String, String), SpanCounts> = HashMap::with_capacity(spans.len());

    #[derive(Row, Deserialize)]
    struct CountRow {
        trace_id: String,
        span_id: String,
        event_count: u64,
        link_count: u64,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<CountRow> = query.fetch_all().await?;

    for row in rows {
        counts.insert(
            (row.trace_id, row.span_id),
            SpanCounts {
                event_count: row.event_count as i64,
                link_count: row.link_count as i64,
            },
        );
    }

    Ok(counts)
}

/// Delete traces by IDs
///
/// In distributed mode, `table` should be the local table name (e.g., `otel_spans_local`)
/// and `on_cluster` should be the ON CLUSTER clause (e.g., ` ON CLUSTER cluster_name`).
/// Which of these traces have no winning spans left. `FINAL`, for the reason the field query gives.
pub async fn traces_without_spans(
    client: &clickhouse::Client,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::surviving_trace_ids(project_id, trace_ids, Backend::Clickhouse)
    else {
        return Ok(Vec::new());
    };
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let alive: Vec<String> = query.fetch_all().await?;
    Ok(trace_ids
        .iter()
        .filter(|t| !alive.contains(t))
        .cloned()
        .collect())
}

/// The text of every field that can hold a `#!B64!#` reference, for the surviving winning spans of these
/// traces.
///
/// `FINAL`, so an expired revision's text does not keep an association alive on behalf of a span that is no
/// longer current - the DuckDB side reads `DEDUP_SPANS` for the same reason.
pub async fn file_reference_fields_for_traces(
    client: &clickhouse::Client,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    let Some(statement) =
        analytics::file_reference_fields(project_id, trace_ids, Backend::Clickhouse)
    else {
        return Ok(Vec::new());
    };
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<(String, String, String, String)> = query.fetch_all().await?;
    Ok(rows
        .into_iter()
        .flat_map(|(a, b, c, d)| [a, b, c, d])
        .filter(|text| !text.is_empty())
        .collect())
}

pub async fn span_body_fields_for_traces(
    client: &clickhouse::Client,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<sideseat_ports::types::SpanBodySource>, ClickhouseError> {
    let Some(statement) =
        analytics::span_body_fields(project_id.as_str(), trace_ids, Backend::Clickhouse)
    else {
        return Ok(Vec::new());
    };
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<(String, String, String, String, String, String)> = query.fetch_all().await?;
    Ok(rows
        .into_iter()
        .map(
            |(trace_id, span_id, messages, tool_definitions, tool_names, raw_span)| {
                sideseat_ports::types::SpanBodySource {
                    trace_id,
                    span_id,
                    messages: (!messages.is_empty()).then_some(messages),
                    tool_definitions: (!tool_definitions.is_empty()).then_some(tool_definitions),
                    tool_names: (!tool_names.is_empty()).then_some(tool_names),
                    raw_span: (!raw_span.is_empty()).then_some(raw_span),
                }
            },
        )
        .collect())
}

pub async fn span_body_backfill_page(
    client: &clickhouse::Client,
    project_id: &ProjectId,
    after: Option<(String, String)>,
    limit: usize,
) -> Result<Vec<sideseat_ports::types::SpanBodySource>, ClickhouseError> {
    let statement = analytics::span_body_backfill_page(
        project_id.as_str(),
        after
            .as_ref()
            .map(|(trace, span)| (trace.as_str(), span.as_str())),
        limit,
        Backend::Clickhouse,
    );
    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<(String, String, String, String, String, String)> = query.fetch_all().await?;
    Ok(rows
        .into_iter()
        .map(
            |(trace_id, span_id, messages, tool_definitions, tool_names, raw_span)| {
                sideseat_ports::types::SpanBodySource {
                    trace_id,
                    span_id,
                    messages: (!messages.is_empty()).then_some(messages),
                    tool_definitions: (!tool_definitions.is_empty()).then_some(tool_definitions),
                    tool_names: (!tool_names.is_empty()).then_some(tool_names),
                    raw_span: (!raw_span.is_empty()).then_some(raw_span),
                }
            },
        )
        .collect())
}

pub async fn delete_traces(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    trace_ids: &[String],
) -> Result<u64, ClickhouseError> {
    let Some(statement) = dml::delete_traces(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        trace_ids,
    ) else {
        return Ok(0);
    };

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    query.execute().await?;
    let logs = dml::delete_logs_for_traces(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        trace_ids,
    )
    .expect("non-empty trace set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;

    // Return count - mutations are async in ClickHouse so we estimate
    Ok(trace_ids.len() as u64)
}

/// Delete spans by (trace_id, span_id) pairs
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_spans(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<u64, ClickhouseError> {
    let Some(statement) = dml::delete_spans(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        spans,
    ) else {
        return Ok(0);
    };

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    query.execute().await?;
    let logs = dml::delete_logs_for_spans(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        spans,
    )
    .expect("non-empty span set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;

    Ok(spans.len() as u64)
}

/// Delete sessions (all spans in the sessions)
///
/// In distributed mode, `table` should be the local table name and
/// `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_sessions(
    client: &Client,
    table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    session_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    if session_ids.is_empty() {
        return Ok(vec![]);
    }

    // Resolve the sessions to their traces and delete those, exactly as the DuckDB backend does.
    //
    // Deleting the rows that *name* a session leaves the rest of its spans behind: a session id is
    // recorded on the spans that know it, often the root alone, so a root-only session lost its root
    // and kept its children - and the call returned success. The orphaned spans then show up as a
    // trace with no session and cannot be deleted by session again, because nothing names it any
    // more. Every read path already resolves a session through its traces; deletion has to agree.
    // No watermark: a deletion acts on what is stored *now*, not as of some past instant.
    let trace_ids = get_trace_ids_for_sessions(client, project_id, session_ids, None).await?;
    if trace_ids.is_empty() {
        return Ok(vec![]);
    }
    let statement = dml::delete_session_traces(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id,
        &trace_ids,
    )
    .expect("non-empty trace set produces a delete");
    bind_analytics_values(client.query(statement.sql()), statement.params())
        .execute()
        .await?;
    let logs = dml::delete_logs_for_traces(
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        &trace_ids,
    )
    .expect("non-empty trace set produces a log delete");
    bind_analytics_values(client.query(logs.sql()), logs.params())
        .execute()
        .await?;
    Ok(trace_ids)
}

/// Delete all data for a project
///
/// In distributed mode, `spans_table` and `metrics_table` should be local table names
/// and `on_cluster` should be the ON CLUSTER clause.
pub async fn delete_project_data(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
) -> Result<u64, ClickhouseError> {
    let plan = dml::delete_project_data(
        dml::MutationTarget::clickhouse(spans_table, on_cluster),
        dml::MutationTarget::clickhouse(metrics_table, on_cluster),
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
    );
    let count_statement = plan
        .count_spans
        .as_ref()
        .expect("ClickHouse project deletion has a count statement");
    let count: u64 = bind_analytics_values(
        client.query(count_statement.sql()),
        count_statement.params(),
    )
    .fetch_one()
    .await?;

    bind_analytics_values(
        client.query(plan.delete_spans.sql()),
        plan.delete_spans.params(),
    )
    .execute()
    .await?;

    // Metrics too, and a failure here is reported rather than swallowed.
    //
    // This used to log at debug and continue, on the reasoning that the table "may not exist in all
    // deployments" - but the schema in this repository always creates it, so the only thing that
    // rationale bought was hiding real failures. A project's metrics are not reachable through the
    // project row either, so metrics left behind by a swallowed error are the same class of orphan as
    // spans left behind, and the caller's verification would never look at them.
    //
    // The one case the old comment was right about is asked directly instead of inferred from an error:
    // if the table is genuinely absent there is nothing to delete.
    let table_check = plan
        .metrics_table_exists
        .as_ref()
        .expect("ClickHouse project deletion checks the metrics table");
    let table_exists: u64 =
        bind_analytics_values(client.query(table_check.sql()), table_check.params())
            .fetch_one()
            .await?;
    if table_exists > 0 {
        bind_analytics_values(
            client.query(plan.delete_metrics.sql()),
            plan.delete_metrics.params(),
        )
        .execute()
        .await?;
    }

    bind_analytics_values(
        client.query(plan.delete_logs.sql()),
        plan.delete_logs.params(),
    )
    .execute()
    .await?;

    Ok(count)
}

/// Count spans grouped by project for a set of project IDs.
/// Count every row a project still owns, spans and metrics together.
///
/// `FINAL` on both, because a `ReplacingMergeTree` may still hold superseded parts - and because this is
/// read to decide whether a deleted project's data is really gone, an approximate answer is the wrong
/// kind of answer. The metrics table is asked only if it exists, for the same reason its delete is.
pub async fn count_project_rows(
    client: &Client,
    metrics_table: &str,
    project_id: &str,
) -> Result<u64, ClickhouseError> {
    let plan = analytics::project_row_count(project_id, Backend::Clickhouse, Some(metrics_table));
    let spans: u64 = bind_analytics_values(client.query(plan.spans.sql()), plan.spans.params())
        .fetch_one()
        .await?;

    let table_check = plan
        .metrics_table_exists
        .as_ref()
        .expect("ClickHouse count plan checks metric table");
    let table_exists: u64 =
        bind_analytics_values(client.query(table_check.sql()), table_check.params())
            .fetch_one()
            .await?;
    let metrics: u64 = if table_exists > 0 {
        bind_analytics_values(client.query(plan.metrics.sql()), plan.metrics.params())
            .fetch_one()
            .await?
    } else {
        0
    };
    let logs: u64 = bind_analytics_values(client.query(plan.logs.sql()), plan.logs.params())
        .fetch_one()
        .await?;
    Ok(spans + metrics + logs)
}

pub async fn patch_project_hold(
    client: &Client,
    spans_table: &str,
    metrics_table: &str,
    logs_table: &str,
    on_cluster: &str,
    project_id: &str,
    hold_until: chrono::DateTime<Utc>,
) -> Result<(), ClickhouseError> {
    let statements = dml::patch_project_hold(
        dml::MutationTarget::clickhouse(spans_table, on_cluster),
        dml::MutationTarget::clickhouse(metrics_table, on_cluster),
        dml::MutationTarget::clickhouse(logs_table, on_cluster),
        project_id,
        hold_until,
    );
    for statement in &statements {
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .execute()
            .await?;
    }
    Ok(())
}

pub async fn project_logical_bytes(
    client: &Client,
    project_id: &str,
    held_at: Option<chrono::DateTime<Utc>>,
) -> Result<u64, ClickhouseError> {
    let plan = analytics::project_logical_bytes(project_id, Backend::Clickhouse, held_at);
    let spans: u64 = bind_analytics_values(client.query(plan.spans.sql()), plan.spans.params())
        .fetch_one()
        .await?;
    let metrics: u64 =
        bind_analytics_values(client.query(plan.metrics.sql()), plan.metrics.params())
            .fetch_one()
            .await?;
    let logs: u64 = bind_analytics_values(client.query(plan.logs.sql()), plan.logs.params())
        .fetch_one()
        .await?;
    Ok(spans.saturating_add(metrics).saturating_add(logs))
}

pub async fn oldest_reclaimable_spans(
    client: &Client,
    project_id: &str,
    target_bytes: u64,
    now: chrono::DateTime<Utc>,
    limit: usize,
) -> Result<Vec<sideseat_ports::types::PressureSpanCandidate>, ClickhouseError> {
    let statement = analytics::oldest_reclaimable_spans(
        project_id,
        Backend::Clickhouse,
        target_bytes,
        now,
        limit,
    );
    let rows: Vec<(String, String, u64)> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_all()
            .await?;
    Ok(rows
        .into_iter()
        .map(
            |(trace_id, span_id, logical_bytes)| sideseat_ports::types::PressureSpanCandidate {
                trace_id,
                span_id,
                logical_bytes,
            },
        )
        .collect())
}

/// The newest committed ingestion time for a project, in microseconds.
pub async fn max_ingested_at_us(
    client: &Client,
    project_id: &str,
) -> Result<Option<i64>, ClickhouseError> {
    let statement = analytics::max_ingested_at_us(project_id, Backend::Clickhouse);
    let value: Option<i64> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_optional()
            .await?;
    // ClickHouse answers max() over an empty set with zero rather than null.
    Ok(value.filter(|value| *value > 0))
}

pub async fn analytics_project_ids(
    client: &Client,
    limit: usize,
) -> Result<Vec<ProjectId>, ClickhouseError> {
    #[derive(Row, Deserialize)]
    struct ProjectRow {
        project_id: String,
    }

    let statement = analytics::analytics_project_ids(Backend::Clickhouse, limit);
    let rows: Vec<ProjectRow> =
        bind_analytics_values(client.query(statement.sql()), statement.params())
            .fetch_all()
            .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectId::from(row.project_id))
        .collect())
}

pub async fn count_spans_by_project(
    client: &Client,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, ClickhouseError> {
    use std::collections::HashMap;

    let Some(statement) = analytics::span_counts_by_project(project_ids, Backend::Clickhouse)
    else {
        return Ok(HashMap::new());
    };

    #[derive(Row, Deserialize)]
    struct CountRow {
        project_id: String,
        cnt: u64,
    }

    let query = bind_analytics_values(client.query(statement.sql()), statement.params());
    let rows: Vec<CountRow> = query.fetch_all().await?;

    let mut result = HashMap::new();
    for row in rows {
        result.insert(row.project_id, row.cnt);
    }

    Ok(result)
}

/// ClickHouse row for filter options
#[derive(Row, Deserialize)]
struct ChFilterOptionRow {
    value: Option<String>,
    count: u64,
}

async fn fetch_filter_option_rows(
    client: &Client,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<ChFilterOptionRow>, ClickhouseError> {
    Ok(
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?,
    )
}

fn plain_filter_options(
    rows: Vec<ChFilterOptionRow>,
) -> Vec<sideseat_ports::traits::FilterOptionRow> {
    rows.into_iter()
        .filter_map(|row| {
            row.value
                .map(|value| sideseat_ports::traits::FilterOptionRow {
                    value,
                    count: row.count,
                })
        })
        .collect()
}

/// Get trace filter options
pub async fn get_trace_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::trace_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Get trace tags options
pub async fn get_trace_tags_options(
    client: &Client,
    project_id: &str,
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<Vec<sideseat_ports::traits::FilterOptionRow>, ClickhouseError> {
    let query = analytics::trace_tag_options(
        project_id,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    );
    let rows = fetch_filter_option_rows(client, &query).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.value
                .map(|raw| sideseat_ports::traits::FilterOptionRow {
                    value: serde_json::from_str::<String>(&raw)
                        .unwrap_or_else(|_| raw.trim_matches('"').to_string()),
                    count: row.count,
                })
        })
        .collect())
}

/// Get span filter options
pub async fn get_span_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
    observations_only: bool,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::span_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        observations_only,
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Get session filter options
pub async fn get_session_filter_options(
    client: &Client,
    project_id: &str,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<
    std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>,
    ClickhouseError,
> {
    let mut results = std::collections::HashMap::new();
    for option in analytics::session_filter_options(
        project_id,
        columns,
        from_timestamp.as_ref(),
        to_timestamp.as_ref(),
        Backend::Clickhouse,
    ) {
        let rows = fetch_filter_option_rows(client, &option.query).await?;
        results.insert(option.column, plain_filter_options(rows));
    }
    Ok(results)
}

/// Repository-level regression tests.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_traces_params_default() {
        let params = ListTracesParams::default();
        assert_eq!(params.page, 0);
        assert_eq!(params.limit, 0);
    }
}
