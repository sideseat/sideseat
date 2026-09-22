use clickhouse::{Client, Row};
use serde::Deserialize;
use sideseat_ports::types::{
    ListLogsParams, ListSpansParams, SearchCandidate, SearchCursor, SearchDocument, SearchField,
    SearchFieldTerms, SearchPage, SearchQuery, SearchRecord, SearchSignal,
};

use crate::ClickhouseError;
use crate::repositories::{log, query};

#[derive(Row, Deserialize)]
struct SpanSearchSource {
    messages: String,
    tool_definitions: String,
    tool_names: String,
    input_preview: Option<String>,
    output_preview: Option<String>,
    status_message: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    span_name: Option<String>,
    search_prompt: Vec<String>,
    search_prompt_truncated: u8,
    search_completion: Vec<String>,
    search_completion_truncated: u8,
    search_tool_name: Vec<String>,
    search_tool_name_truncated: u8,
    search_tool_args: Vec<String>,
    search_tool_args_truncated: u8,
    search_error: Vec<String>,
    search_error_truncated: u8,
    search_span_name: Vec<String>,
    search_span_name_truncated: u8,
}

#[derive(Row, Deserialize)]
struct LogSearchSource {
    body_text: Option<String>,
    event_name: Option<String>,
    severity_text: Option<String>,
    attributes: Option<String>,
    search_body: Vec<String>,
    search_body_truncated: u8,
    search_event_name: Vec<String>,
    search_event_name_truncated: u8,
    search_severity: Vec<String>,
    search_severity_truncated: u8,
    search_attributes: Vec<String>,
    search_attributes_truncated: u8,
}

pub async fn search(client: &Client, request: &SearchQuery) -> Result<SearchPage, ClickhouseError> {
    match request.signal {
        SearchSignal::Spans => search_spans(client, request).await,
        SearchSignal::Logs => search_logs(client, request).await,
    }
}

async fn search_spans(
    client: &Client,
    request: &SearchQuery,
) -> Result<SearchPage, ClickhouseError> {
    let mut params = ListSpansParams {
        project_id: request.project_id.clone(),
        page: 1,
        limit: 1,
        from_timestamp: request.from_timestamp,
        to_timestamp: request.to_timestamp,
        ..Default::default()
    };
    let (_, total) = query::list_spans(client, &params).await?;
    params.limit = u32::try_from(total).unwrap_or(u32::MAX).max(1);
    let (mut rows, _) = query::list_spans(client, &params).await?;
    rows.sort_by(|left, right| {
        right
            .timestamp_start
            .cmp(&left.timestamp_start)
            .then_with(|| left.trace_id.cmp(&right.trace_id))
            .then_with(|| left.span_id.cmp(&right.span_id))
    });
    let started_at_us = request
        .cursor
        .as_ref()
        .map_or_else(now_us, |cursor| cursor.started_at_us);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    let mut examined = 0;
    for row in rows {
        let cursor = SearchCursor {
            timestamp_us: row.timestamp_start.timestamp_micros(),
            tie_breaker: format!("{}\0{}", row.trace_id, row.span_id),
            ordinal: 0,
            started_at_us,
        };
        if !after_cursor(&cursor, request.cursor.as_ref()) {
            continue;
        }
        if examined >= request.max_examined {
            break;
        }
        examined += 1;
        let document = span_document(
            client,
            request.project_id.as_str(),
            &row.trace_id,
            &row.span_id,
        )
        .await?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Span(row),
            document,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: examined >= request.max_examined,
        arrivals_detected: arrivals_detected(client, request.project_id.as_str(), started_at_us)
            .await?,
        index_lag_us: 0,
    })
}

async fn search_logs(
    client: &Client,
    request: &SearchQuery,
) -> Result<SearchPage, ClickhouseError> {
    let mut params = ListLogsParams {
        project_id: request.project_id.clone(),
        page: 1,
        limit: 1,
        from_timestamp: request.from_timestamp,
        to_timestamp: request.to_timestamp,
        ..Default::default()
    };
    let (_, total) = log::list_logs(client, &params).await?;
    params.limit = u32::try_from(total).unwrap_or(u32::MAX).max(1);
    let (mut rows, _) = log::list_logs(client, &params).await?;
    rows.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.log_digest.cmp(&right.log_digest))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });
    let started_at_us = request
        .cursor
        .as_ref()
        .map_or_else(now_us, |cursor| cursor.started_at_us);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    let mut examined = 0;
    for row in rows {
        let cursor = SearchCursor {
            timestamp_us: row.timestamp.timestamp_micros(),
            tie_breaker: row.log_digest.clone(),
            ordinal: row.ordinal,
            started_at_us,
        };
        if !after_cursor(&cursor, request.cursor.as_ref()) {
            continue;
        }
        if examined >= request.max_examined {
            break;
        }
        examined += 1;
        let document = log_document(
            client,
            request.project_id.as_str(),
            &row.log_digest,
            row.ordinal,
        )
        .await?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Log(row),
            document,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: examined >= request.max_examined,
        arrivals_detected: arrivals_detected(client, request.project_id.as_str(), started_at_us)
            .await?,
        index_lag_us: 0,
    })
}

fn after_cursor(candidate: &SearchCursor, cursor: Option<&SearchCursor>) -> bool {
    cursor.is_none_or(|cursor| {
        candidate.timestamp_us < cursor.timestamp_us
            || (candidate.timestamp_us == cursor.timestamp_us
                && (candidate.tie_breaker.as_str(), candidate.ordinal)
                    > (cursor.tie_breaker.as_str(), cursor.ordinal))
    })
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

async fn arrivals_detected(
    client: &Client,
    project_id: &str,
    started_at_us: i64,
) -> Result<bool, ClickhouseError> {
    let value: u8 = client
        .query(
            "SELECT count() > 0 FROM (\
             SELECT ingested_at FROM otel_spans WHERE project_id = ? \
             UNION ALL SELECT ingested_at FROM otel_logs WHERE project_id = ?) \
             WHERE toInt64(toUnixTimestamp64Micro(ingested_at)) > ?",
        )
        .bind(project_id)
        .bind(project_id)
        .bind(started_at_us)
        .fetch_one()
        .await?;
    Ok(value != 0)
}

async fn span_document(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<SearchDocument, ClickhouseError> {
    let source: SpanSearchSource = client
        .query(
            "SELECT messages, tool_definitions, tool_names, input_preview, output_preview, \
             status_message, exception_message, exception_stacktrace, span_name, \
             search_prompt, search_prompt_truncated, search_completion, \
             search_completion_truncated, search_tool_name, search_tool_name_truncated, \
             search_tool_args, search_tool_args_truncated, search_error, \
             search_error_truncated, search_span_name, search_span_name_truncated \
             FROM otel_spans FINAL WHERE project_id = ? AND trace_id = ? AND span_id = ? \
             ORDER BY ingested_at DESC LIMIT 1",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(span_id)
        .fetch_one()
        .await?;
    Ok(SearchDocument {
        fields: vec![
            field(
                SearchField::Prompt,
                source.search_prompt,
                source.search_prompt_truncated,
                join([
                    Some(source.messages.as_str()),
                    source.input_preview.as_deref(),
                ]),
            ),
            field(
                SearchField::Completion,
                source.search_completion,
                source.search_completion_truncated,
                join([
                    Some(source.messages.as_str()),
                    source.output_preview.as_deref(),
                ]),
            ),
            field(
                SearchField::ToolName,
                source.search_tool_name,
                source.search_tool_name_truncated,
                source.tool_names,
            ),
            field(
                SearchField::ToolArgs,
                source.search_tool_args,
                source.search_tool_args_truncated,
                source.tool_definitions,
            ),
            field(
                SearchField::Error,
                source.search_error,
                source.search_error_truncated,
                join([
                    source.status_message.as_deref(),
                    source.exception_message.as_deref(),
                    source.exception_stacktrace.as_deref(),
                ]),
            ),
            field(
                SearchField::SpanName,
                source.search_span_name,
                source.search_span_name_truncated,
                source.span_name.unwrap_or_default(),
            ),
        ],
    })
}

async fn log_document(
    client: &Client,
    project_id: &str,
    digest: &str,
    ordinal: u32,
) -> Result<SearchDocument, ClickhouseError> {
    let source: LogSearchSource = client
        .query(
            "SELECT body_text, event_name, severity_text, attributes, search_body, \
             search_body_truncated, search_event_name, search_event_name_truncated, \
             search_severity, search_severity_truncated, search_attributes, \
             search_attributes_truncated FROM otel_logs FINAL \
             WHERE project_id = ? AND log_digest = ? AND ordinal = ? \
             ORDER BY ingested_at DESC LIMIT 1",
        )
        .bind(project_id)
        .bind(digest)
        .bind(ordinal)
        .fetch_one()
        .await?;
    Ok(SearchDocument {
        fields: vec![
            field(
                SearchField::Body,
                source.search_body,
                source.search_body_truncated,
                source.body_text.unwrap_or_default(),
            ),
            field(
                SearchField::EventName,
                source.search_event_name,
                source.search_event_name_truncated,
                source.event_name.unwrap_or_default(),
            ),
            field(
                SearchField::Severity,
                source.search_severity,
                source.search_severity_truncated,
                source.severity_text.unwrap_or_default(),
            ),
            field(
                SearchField::Attributes,
                source.search_attributes,
                source.search_attributes_truncated,
                source.attributes.unwrap_or_default(),
            ),
        ],
    })
}

fn field(field: SearchField, terms: Vec<String>, truncated: u8, text: String) -> SearchFieldTerms {
    SearchFieldTerms {
        field,
        terms,
        truncated: truncated != 0,
        text,
    }
}

fn join<const N: usize>(values: [Option<&str>; N]) -> String {
    values.into_iter().flatten().collect::<Vec<_>>().join("\n")
}
