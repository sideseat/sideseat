//! ClickHouse messages repository.
//!
//! Provides message aggregation and query operations for the ClickHouse backend.

use chrono::DateTime;
use clickhouse::{Client, Row};
use serde::Deserialize;

use crate::ClickhouseError;
use sideseat_ports::types::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
};
use sideseat_query_sql::{Backend, analytics, messages};

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
