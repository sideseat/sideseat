use chrono::DateTime;
use clickhouse::Row;
use serde::Deserialize;

use sideseat_core::utils::time::parse_iso_timestamp;
use sideseat_ports::types::{
    EventRow, LinkRow, SessionRow, SpanRow, TraceRow, parse_finish_reasons, parse_tags,
};

/// ClickHouse row for trace queries
#[derive(Row, Deserialize)]
pub(super) struct ChTraceRow {
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
pub(super) struct ChSpanRow {
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
pub(super) struct ChSessionRow {
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
pub(super) struct ChEventRow {
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
pub(super) struct ChLinkRow {
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
