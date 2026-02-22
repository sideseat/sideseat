//! ClickHouse messages repository
//!
//! Provides message aggregation and query operations for the ClickHouse backend.

use chrono::DateTime;
use clickhouse::{Client, Row};
use serde::Deserialize;

use crate::data::clickhouse::ClickhouseError;
use crate::data::types::{
    FeedMessagesParams, MessageQueryParams, MessageQueryResult, MessageSpanRow,
};

/// Shared SELECT columns for all message queries.
const CH_MESSAGE_SELECT_COLUMNS: &str = r#"
    trace_id,
    span_id,
    parent_span_id,
    toInt64(toUnixTimestamp64Micro(timestamp_start)) AS span_timestamp_us,
    if(timestamp_end IS NULL, NULL, toInt64(toUnixTimestamp64Micro(timestamp_end))) AS span_end_timestamp_us,
    messages,
    gen_ai_request_model AS model,
    gen_ai_system AS provider,
    status_code,
    exception_type,
    exception_message,
    exception_stacktrace,
    gen_ai_usage_input_tokens AS input_tokens,
    gen_ai_usage_output_tokens AS output_tokens,
    gen_ai_usage_total_tokens AS total_tokens,
    toFloat64(gen_ai_cost_total) AS cost_total,
    tool_definitions,
    tool_names,
    observation_type,
    session_id,
    toInt64(toUnixTimestamp64Micro(ingested_at)) AS ingested_at_us"#;

/// Shared content filter for message queries.
const CH_MESSAGE_CONTENT_FILTER: &str =
    "(messages != '[]' OR tool_definitions != '[]' OR tool_names != '[]' OR status_code = 'ERROR')";

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
            observation_type: row.observation_type,
            session_id: row.session_id,
            ingested_at: DateTime::from_timestamp_micros(row.ingested_at_us)
                .unwrap_or(DateTime::UNIX_EPOCH),
        }
    }
}

/// Get span rows for a span, trace, or session (unified query).
///
/// Priority: span_id > session_id > trace_id
pub async fn get_messages(
    client: &Client,
    params: &MessageQueryParams,
) -> Result<MessageQueryResult, ClickhouseError> {
    let mut conditions = vec!["project_id = ?".to_string()];
    let mut string_binds: Vec<String> = vec![params.project_id.clone()];
    let mut time_params: Vec<i64> = Vec::new();

    if let Some(span_id) = &params.span_id {
        conditions.push("span_id = ?".to_string());
        string_binds.push(span_id.clone());
    } else if let Some(session_id) = &params.session_id {
        conditions.push(
            "trace_id IN (SELECT DISTINCT trace_id FROM otel_spans FINAL WHERE project_id = ? AND session_id = ?)".to_string()
        );
        string_binds.push(params.project_id.clone());
        string_binds.push(session_id.clone());
        conditions.push(CH_MESSAGE_CONTENT_FILTER.to_string());
    } else if let Some(trace_id) = &params.trace_id {
        conditions.push("trace_id = ?".to_string());
        string_binds.push(trace_id.clone());
        conditions.push(CH_MESSAGE_CONTENT_FILTER.to_string());
    }

    if let Some(from) = &params.from_timestamp {
        conditions.push("timestamp_start >= fromUnixTimestamp64Micro(?)".to_string());
        time_params.push(from.timestamp_micros());
    }
    if let Some(to) = &params.to_timestamp {
        conditions.push("timestamp_start < fromUnixTimestamp64Micro(?)".to_string());
        time_params.push(to.timestamp_micros());
    }

    let sql = format!(
        "SELECT {CH_MESSAGE_SELECT_COLUMNS} FROM otel_spans FINAL WHERE {} ORDER BY timestamp_start ASC",
        conditions.join(" AND ")
    );

    let mut query = client.query(&sql);
    for s in &string_binds {
        query = query.bind(s);
    }
    for ts in &time_params {
        query = query.bind(ts);
    }
    let rows: Vec<ChMessageSpanRow> = query.fetch_all().await?;

    Ok(MessageQueryResult {
        rows: rows.into_iter().map(MessageSpanRow::from).collect(),
    })
}

/// Parameter type for mixed binding in get_project_messages
enum BindParam {
    String(String),
    Int64(i64),
}

/// Get span rows for entire project (feed API).
///
/// Uses cursor-based pagination on (ingested_at, span_id) for stable pagination.
pub async fn get_project_messages(
    client: &Client,
    params: &FeedMessagesParams,
) -> Result<MessageQueryResult, ClickhouseError> {
    let mut conditions = vec![
        "project_id = ?".to_string(),
        CH_MESSAGE_CONTENT_FILTER.to_string(),
    ];
    let mut bind_params: Vec<BindParam> = vec![BindParam::String(params.project_id.clone())];

    // Cursor condition - cursor_time_us is safe (derived integer), cursor_span_id uses placeholder
    if let Some((cursor_time_us, cursor_span_id)) = &params.cursor {
        conditions.push(format!(
            "(toInt64(toUnixTimestamp64Micro(ingested_at)), span_id) < ({}, ?)",
            cursor_time_us
        ));
        bind_params.push(BindParam::String(cursor_span_id.clone()));
    }

    // Event time filters - use parameterized timestamps
    if let Some(start) = &params.start_time {
        conditions.push("timestamp_start >= fromUnixTimestamp64Micro(?)".to_string());
        bind_params.push(BindParam::Int64(start.timestamp_micros()));
    }
    if let Some(end) = &params.end_time {
        conditions.push("timestamp_start < fromUnixTimestamp64Micro(?)".to_string());
        bind_params.push(BindParam::Int64(end.timestamp_micros()));
    }

    let sql = format!(
        "SELECT {CH_MESSAGE_SELECT_COLUMNS} FROM otel_spans FINAL WHERE {} ORDER BY ingested_at DESC, span_id DESC LIMIT {}",
        conditions.join(" AND "),
        params.limit
    );

    let mut query = client.query(&sql);
    for param in &bind_params {
        query = match param {
            BindParam::String(s) => query.bind(s),
            BindParam::Int64(i) => query.bind(i),
        };
    }

    let rows: Vec<ChMessageSpanRow> = query.fetch_all().await?;

    Ok(MessageQueryResult {
        rows: rows.into_iter().map(MessageSpanRow::from).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_query_result() {
        let result = MessageQueryResult { rows: vec![] };
        assert!(result.rows.is_empty());
    }
}
