//! ClickHouse messages repository.
//!
//! Provides message aggregation and query operations for the ClickHouse backend.

use chrono::DateTime;
use clickhouse::{Client, Row};
use serde::Deserialize;

use crate::ClickhouseError;
use sideseat_ports::types::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
    RequestContextParams, RequestContextRows,
};
use sideseat_query_sql::{Backend, analytics, messages, request_context};

use super::query::bind_analytics_values;

/// ClickHouse row for message span queries
#[derive(Row, Deserialize)]
struct ChMessageSpanRow {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    span_timestamp_us: i64,
    span_end_timestamp_us: Option<i64>,
    messages: String,
    model: Option<String>,
    provider: Option<String>,
    status_code: Option<String>,
    exception_type: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cost_total: f64,
    tool_definitions: String,
    tool_names: String,
    observation_type: Option<String>,
    session_id: Option<String>,
    ingested_at_us: i64,
    scope_name: Option<String>,
    scope_version: Option<String>,
    span_name: Option<String>,
    framework: Option<String>,
    response_model: Option<String>,
    response_id: Option<String>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<i64>,
    finish_reasons: Option<String>,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    cost_input: f64,
    cost_output: f64,
    log_messages: String,
    request_thread: String,
    span_marks: u16,
}

impl From<ChMessageSpanRow> for MessageSpanRow {
    fn from(row: ChMessageSpanRow) -> Self {
        Self {
            trace_id: row.trace_id,
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            span_timestamp: DateTime::from_timestamp_micros(row.span_timestamp_us)
                .unwrap_or(DateTime::UNIX_EPOCH),
            span_end_timestamp: row
                .span_end_timestamp_us
                .and_then(DateTime::from_timestamp_micros),
            messages_json: row.messages,
            model: row.model,
            provider: row.provider,
            status_code: row.status_code,
            exception_type: row.exception_type,
            exception_message: row.exception_message,
            exception_stacktrace: row.exception_stacktrace,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            total_tokens: row.total_tokens,
            cost_total: row.cost_total,
            tool_definitions_json: row.tool_definitions,
            tool_names_json: row.tool_names,
            log_messages_json: row.log_messages,
            body_cache_key: None,
            observation_type: row.observation_type,
            session_id: row.session_id,
            ingested_at: DateTime::from_timestamp_micros(row.ingested_at_us)
                .unwrap_or(DateTime::UNIX_EPOCH),
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            span_name: row.span_name,
            framework: row.framework,
            response_model: row.response_model,
            response_id: row.response_id,
            temperature: row.temperature,
            top_p: row.top_p,
            max_tokens: row.max_tokens,
            finish_reasons: row.finish_reasons,
            request_thread: row.request_thread,
            span_marks: row.span_marks,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            reasoning_tokens: row.reasoning_tokens,
            cost_input: row.cost_input,
            cost_output: row.cost_output,
        }
    }
}

async fn execute_message_query(
    client: &Client,
    query: &analytics::ParameterizedQuery,
) -> Result<MessageQueryResult, ClickhouseError> {
    let rows: Vec<ChMessageSpanRow> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?;
    Ok(MessageQueryResult {
        rows: rows.into_iter().map(MessageSpanRow::from).collect(),
    })
}

/// Get span rows for a span, trace, or session (unified query).
///
/// Priority: span_id > session_id > trace_id > trace_ids
pub async fn get_messages(
    client: &Client,
    params: &MessageQueryParams,
) -> Result<MessageQueryResult, ClickhouseError> {
    let query = messages::get_messages(params, Backend::Clickhouse);
    execute_message_query(client, &query).await
}

/// Get span rows for entire project (feed API).
///
/// Uses cursor-based pagination on (ingested_at, span_id, trace_id).
pub async fn get_project_messages(
    client: &Client,
    params: &FeedMessagesParams,
) -> Result<MessageQueryResult, ClickhouseError> {
    let query = messages::get_project_messages(params, Backend::Clickhouse);
    execute_message_query(client, &query).await
}

/// A thread's rows, as the composed-request reads project them: the messages and the facts that place them.
///
/// Its own row type, not `ChMessageSpanRow`: the projection is a tenth of a span's columns, and a row type that
/// named the rest would make ClickHouse read them.
#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct ChThreadRow {
    trace_id: String,
    span_id: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    timestamp_start: time::OffsetDateTime,
    status_code: Option<String>,
    messages: String,
    observation_type: Option<String>,
    span_name: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    session_id: Option<String>,
}

impl From<ChThreadRow> for MessageSpanRow {
    fn from(row: ChThreadRow) -> Self {
        let span_timestamp = DateTime::from_timestamp_micros(
            i64::try_from(row.timestamp_start.unix_timestamp_nanos() / 1_000).unwrap_or_default(),
        )
        .unwrap_or(DateTime::UNIX_EPOCH);
        MessageSpanRow {
            trace_id: row.trace_id,
            span_id: row.span_id,
            span_timestamp,
            ingested_at: span_timestamp,
            status_code: row.status_code,
            messages_json: row.messages,
            observation_type: row.observation_type,
            span_name: row.span_name,
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            session_id: row.session_id,
            ..thread_row_defaults()
        }
    }
}

/// The columns a thread read does not project, as a row that carries nothing.
///
/// Spelled out rather than `Default`, because `MessageSpanRow` has none: the full reads fill every field, and a
/// default there would let a new field silently arrive empty on a path that does read it.
fn thread_row_defaults() -> MessageSpanRow {
    MessageSpanRow {
        trace_id: String::new(),
        span_id: String::new(),
        parent_span_id: None,
        span_timestamp: DateTime::UNIX_EPOCH,
        span_end_timestamp: None,
        messages_json: "[]".to_string(),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
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
        observation_type: None,
        session_id: None,
        ingested_at: DateTime::UNIX_EPOCH,
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
        request_thread: String::new(),
        span_marks: 0,
    }
}

/// The rows a request span's view is composed from: its thread's earlier requests, and the tool spans holding the
/// calls those requests' deltas answer. Two keyed reads, in one call so both see one instant.
pub async fn get_request_context(
    client: &Client,
    params: &RequestContextParams,
) -> Result<RequestContextRows, ClickhouseError> {
    let ids: Vec<&str> = params.call_ids.iter().map(String::as_str).collect();
    let traces: Vec<&str> = params.call_trace_ids.iter().map(String::as_str).collect();
    Ok(RequestContextRows {
        thread: fetch_thread(
            client,
            &request_context::thread_requests(
                &params.thread,
                params.before_us,
                params.ingested_before_us,
                Backend::Clickhouse,
            ),
        )
        .await?,
        calls: match ids.is_empty() || traces.is_empty() {
            true => Vec::new(),
            false => {
                fetch_thread(
                    client,
                    &request_context::calls_in_traces(
                        &traces,
                        &ids,
                        params.ingested_before_us,
                        Backend::Clickhouse,
                    ),
                )
                .await?
            }
        },
    })
}

async fn fetch_thread(
    client: &Client,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<MessageSpanRow>, ClickhouseError> {
    let rows: Vec<ChThreadRow> = bind_analytics_values(client.query(query.sql()), query.params())
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(MessageSpanRow::from).collect())
}

/// Repository-level regression tests.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_query_result() {
        let result = MessageQueryResult { rows: vec![] };
        assert!(result.rows.is_empty());
    }
}
