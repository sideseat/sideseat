//! Message query operations.
//!
//! Provides queries for extracting conversation messages from spans.
//! Messages are stored as raw JSON (SideML conversion happens at query time).
//!
//! This repository only handles data retrieval. All message processing
//! (filtering, deduplication, sorting, metadata harvesting) is done by
//! the feed pipeline (process_spans) in the domain layer.

use duckdb::Connection;

use crate::error::DuckdbError;
use sideseat_core::utils::time::micros_to_datetime;
use sideseat_ports::types::{
    FeedMessagesParams, LogMessageSource, MessageQueryParams, MessageQueryResult, MessageSpanRow,
    RequestContextParams, RequestContextRows, RequestFrameRecord, RequestFramesParams,
};
use sideseat_query_sql::{Backend, analytics, messages, request_context, request_frames};

fn duckdb_values(values: &[analytics::QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            analytics::QueryValue::String(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            analytics::QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

/// A row parser's columns, by the names its query gives them, resolved to positions once per statement.
///
/// Positional parsing has to agree with its query about every column's place, and it failed the way that
/// invites: one column added to the projection moved the joined log messages along, and the parser read them
/// from the column beside them. By name, a column the query does not return is an error naming it, and where a
/// column sits stops mattering. Resolved once, from the executed statement, so each row then reads by index
/// exactly as a positional parser does - the names cost one lookup per column per statement, never per row.
macro_rules! row_columns {
    ($(#[$meta:meta])* $name:ident { $($field:ident),+ $(,)? }) => {
        $(#[$meta])*
        struct $name {
            $($field: usize),+
        }

        impl $name {
            fn resolve(statement: &duckdb::Statement<'_>) -> duckdb::Result<Self> {
                Ok(Self {
                    $($field: statement.column_index(stringify!($field))?),+
                })
            }
        }
    };
}

row_columns!(
    /// What a span read returns: `messages::MESSAGE_COLUMNS` and the joined log messages.
    SpanColumns {
        trace_id,
        span_id,
        parent_span_id,
        span_timestamp_us,
        span_end_timestamp_us,
        messages,
        model,
        provider,
        status_code,
        exception_type,
        exception_message,
        exception_stacktrace,
        input_tokens,
        output_tokens,
        total_tokens,
        cost_total,
        tool_definitions,
        tool_names,
        observation_type,
        session_id,
        ingested_at_us,
        scope_name,
        scope_version,
        span_name,
        framework,
        response_model,
        response_id,
        temperature,
        top_p,
        max_tokens,
        finish_reasons,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        cost_input,
        cost_output,
        request_thread,
        span_marks,
        request_frame,
        log_messages,
    }
);

row_columns!(
    /// What a thread or call read returns: `request_context::THREAD_COLUMNS`.
    ThreadColumns {
        trace_id,
        span_id,
        timestamp_start,
        status_code,
        messages,
        observation_type,
        span_name,
        scope_name,
        scope_version,
        session_id,
        span_marks,
    }
);

/// Every row a query returns, each parsed against the columns resolved once from the executed statement.
fn read_rows<C>(
    conn: &Connection,
    sql: &str,
    params: &[analytics::QueryValue],
    resolve: fn(&duckdb::Statement<'_>) -> duckdb::Result<C>,
    parse: fn(&duckdb::Row<'_>, &C) -> duckdb::Result<MessageSpanRow>,
) -> Result<Vec<MessageSpanRow>, DuckdbError> {
    let values = duckdb_values(params);
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(values.as_slice())?;
    let Some(statement) = rows.as_ref() else {
        return Ok(Vec::new());
    };
    let columns = resolve(statement)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(parse(row, &columns)?);
    }
    Ok(out)
}

fn execute_message_query(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<MessageQueryResult, DuckdbError> {
    Ok(MessageQueryResult {
        rows: read_rows(
            conn,
            query.sql(),
            query.params(),
            SpanColumns::resolve,
            parse_span_row,
        )?,
    })
}

// ============================================================================
// Query functions - return raw unfiltered data
// ============================================================================

/// Get span rows for a span, trace, or session (unified query).
///
/// Priority: span_id > session_id > trace_id > trace_ids
pub fn get_messages(
    conn: &Connection,
    params: &MessageQueryParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let query = messages::get_messages(params, Backend::Duckdb);
    execute_message_query(conn, &query)
}

/// Get span rows for entire project (feed API).
///
/// Uses cursor-based pagination on (ingested_at, span_id, trace_id).
pub fn get_project_messages(
    conn: &Connection,
    params: &FeedMessagesParams,
) -> Result<MessageQueryResult, DuckdbError> {
    let query = messages::get_project_messages(params, Backend::Duckdb);
    execute_message_query(conn, &query)
}

/// The rows a request span's view is composed from: its thread's earlier requests, and the tool spans holding the
/// calls those requests' deltas answer. Two keyed reads, in one call so both see one connection's instant.
pub fn get_request_context(
    conn: &Connection,
    params: &RequestContextParams,
) -> Result<RequestContextRows, DuckdbError> {
    let ids: Vec<&str> = params.call_ids.iter().map(String::as_str).collect();
    let traces: Vec<&str> = params.call_trace_ids.iter().map(String::as_str).collect();
    Ok(RequestContextRows {
        thread: thread_rows(
            conn,
            &request_context::thread_requests(
                &params.thread,
                params.before_us,
                params.ingested_before_us,
                Backend::Duckdb,
            ),
        )?,
        calls: match ids.is_empty() || traces.is_empty() {
            true => Vec::new(),
            false => {
                let mut calls = Vec::new();
                for query in request_context::calls_in_traces(
                    &traces,
                    &ids,
                    params.ingested_before_us,
                    Backend::Duckdb,
                ) {
                    calls.extend(thread_rows(conn, &query)?);
                }
                calls.sort_by(|a, b| {
                    (a.span_timestamp, &a.span_id, &a.trace_id).cmp(&(
                        b.span_timestamp,
                        &b.span_id,
                        &b.trace_id,
                    ))
                });
                calls
            }
        },
    })
}

fn thread_rows(
    conn: &Connection,
    query: &analytics::ParameterizedQuery,
) -> Result<Vec<MessageSpanRow>, DuckdbError> {
    read_rows(
        conn,
        query.sql(),
        query.params(),
        ThreadColumns::resolve,
        parse_thread_row,
    )
}

/// One row of a thread read.
///
/// The unprojected fields are left at what a span that carries none would have: a composition reads the messages
/// and the facts that place them, and a projection is what keeps the rest of a span's bytes unread.
fn parse_thread_row(
    row: &duckdb::Row,
    at: &ThreadColumns,
) -> Result<MessageSpanRow, duckdb::Error> {
    let span_timestamp = row
        .get::<_, Option<i64>>(at.timestamp_start)
        .map(|micros| micros.map(micros_to_datetime))?
        .unwrap_or(chrono::DateTime::UNIX_EPOCH);
    Ok(MessageSpanRow {
        trace_id: row.get(at.trace_id)?,
        span_id: row.get(at.span_id)?,
        span_timestamp,
        ingested_at: span_timestamp,
        status_code: row.get(at.status_code)?,
        messages_json: row
            .get::<_, Option<String>>(at.messages)?
            .unwrap_or_default(),
        observation_type: row.get(at.observation_type)?,
        span_name: row.get(at.span_name)?,
        scope_name: row.get(at.scope_name)?,
        scope_version: row.get(at.scope_version)?,
        session_id: row.get(at.session_id)?,
        span_marks: row.get(at.span_marks)?,
        parent_span_id: None,
        span_end_timestamp: None,
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        log_message_sources: Vec::new(),
        body_cache_key: None,
        model: None,
        provider: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
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
        request_frame: String::new(),
    })
}

// ============================================================================
// Helper functions
// ============================================================================

/// One row of a span read - just extracts fields, no transformation.
fn parse_span_row(row: &duckdb::Row, at: &SpanColumns) -> Result<MessageSpanRow, duckdb::Error> {
    let text = |column: usize| row.get::<_, Option<String>>(column);
    Ok(MessageSpanRow {
        trace_id: row.get(at.trace_id)?,
        span_id: row.get(at.span_id)?,
        parent_span_id: row.get(at.parent_span_id)?,
        span_timestamp: micros_to_datetime(row.get::<_, i64>(at.span_timestamp_us)?),
        span_end_timestamp: row
            .get::<_, Option<i64>>(at.span_end_timestamp_us)?
            .map(micros_to_datetime),
        messages_json: text(at.messages)?.unwrap_or_default(),
        model: row.get(at.model)?,
        provider: row.get(at.provider)?,
        status_code: row.get(at.status_code)?,
        exception_type: row.get(at.exception_type)?,
        exception_message: row.get(at.exception_message)?,
        exception_stacktrace: row.get(at.exception_stacktrace)?,
        input_tokens: row.get(at.input_tokens)?,
        output_tokens: row.get(at.output_tokens)?,
        total_tokens: row.get(at.total_tokens)?,
        cost_total: row.get(at.cost_total)?,
        tool_definitions_json: text(at.tool_definitions)?.unwrap_or_default(),
        tool_names_json: text(at.tool_names)?.unwrap_or_default(),
        // Resolved into the messages by the domain (`domain::log_messages`).
        log_messages_json: "[]".to_string(),
        log_message_sources: LogMessageSource::many_from_column(text(at.log_messages)?.as_deref()),
        body_cache_key: None,
        observation_type: row.get(at.observation_type)?,
        session_id: row.get(at.session_id)?,
        ingested_at: micros_to_datetime(row.get::<_, i64>(at.ingested_at_us)?),
        scope_name: row.get(at.scope_name)?,
        scope_version: row.get(at.scope_version)?,
        span_name: row.get(at.span_name)?,
        framework: row.get(at.framework)?,
        response_model: row.get(at.response_model)?,
        response_id: row.get(at.response_id)?,
        temperature: row.get(at.temperature)?,
        top_p: row.get(at.top_p)?,
        max_tokens: row.get(at.max_tokens)?,
        finish_reasons: row.get(at.finish_reasons)?,
        cache_read_tokens: row.get(at.cache_read_tokens)?,
        cache_write_tokens: row.get(at.cache_write_tokens)?,
        reasoning_tokens: row.get(at.reasoning_tokens)?,
        cost_input: row.get(at.cost_input)?,
        cost_output: row.get(at.cost_output)?,
        request_thread: text(at.request_thread)?.unwrap_or_default(),
        span_marks: row.get(at.span_marks)?,
        request_frame: text(at.request_frame)?.unwrap_or_default(),
    })
}

row_columns!(
    /// What a frame read returns: `request_frames::FRAME_COLUMNS`.
    FrameColumns {
        trace_id,
        span_id,
        timestamp_us,
        log_digest,
        ordinal,
        messages,
    }
);

/// The frame records a framed request span's view opens with: one keyed statement, bounded in itself.
pub fn get_request_frames(
    conn: &Connection,
    params: &RequestFramesParams,
) -> Result<Vec<RequestFrameRecord>, DuckdbError> {
    let query = request_frames::frames_in_trace(
        params.project_id.as_str(),
        &params.trace_id,
        params.key,
        params.ingested_before_us,
        Backend::Duckdb,
    );
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    let Some(statement) = rows.as_ref() else {
        return Ok(Vec::new());
    };
    let at = FrameColumns::resolve(statement)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(RequestFrameRecord {
            trace_id: row.get(at.trace_id)?,
            span_id: row.get(at.span_id)?,
            timestamp: micros_to_datetime(row.get::<_, i64>(at.timestamp_us)?),
            log_digest: row.get(at.log_digest)?,
            ordinal: row.get(at.ordinal)?,
            messages_json: None,
            source: LogMessageSource::from_column(
                row.get::<_, Option<String>>(at.messages)?.as_deref(),
            ),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
