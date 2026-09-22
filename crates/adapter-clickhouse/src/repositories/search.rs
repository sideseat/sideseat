use clickhouse::{Client, Row};
use serde::Deserialize;
use sideseat_ports::types::{
    LogSearchSource as PortLogSearchSource, SearchCandidate, SearchCursor, SearchDocument,
    SearchField, SearchFieldTerms, SearchPage, SearchQuery, SearchRecord, SearchSignal,
    SearchSource, SpanSearchSource as PortSpanSearchSource,
};
use sideseat_query_sql::{Backend, search as search_sql};

use crate::ClickhouseError;
use crate::repositories::{log, query};

#[derive(Row, Deserialize)]
struct SpanSearchSource {
    messages: String,
    tool_definitions: String,
    tool_names: String,
    input_preview: Option<String>,
    output_preview: Option<String>,
    gen_ai_tool_name: Option<String>,
    status_message: Option<String>,
    exception_type: Option<String>,
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
    body: Option<String>,
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

#[derive(Row, Deserialize)]
struct SpanCandidateRef {
    trace_id: String,
    span_id: String,
    timestamp_us: i64,
    match_state: i16,
}

#[derive(Row, Deserialize)]
struct LogCandidateRef {
    log_digest: String,
    ordinal: u32,
    timestamp_us: i64,
    match_state: i16,
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
    let started_at_us = traversal_watermark(client, request).await?;
    let plan = search_sql::candidates(request, Backend::Clickhouse);
    let mut references: Vec<SpanCandidateRef> =
        query::bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
            .fetch_all()
            .await?;
    let limit_reached = references.len() > request.max_examined as usize;
    references.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for reference in references {
        let _ = reference.match_state;
        let cursor = SearchCursor {
            timestamp_us: reference.timestamp_us,
            tie_breaker: format!("{}\0{}", reference.trace_id, reference.span_id),
            ordinal: 0,
            started_at_us,
        };
        let Some(row) = query::get_span(
            client,
            request.project_id.as_str(),
            &reference.trace_id,
            &reference.span_id,
        )
        .await?
        else {
            next_cursor = Some(cursor);
            continue;
        };
        let (document, source) = span_document(
            client,
            request.project_id.as_str(),
            &reference.trace_id,
            &reference.span_id,
        )
        .await?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Span(row),
            document,
            source,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    let examined = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: limit_reached,
        arrivals_detected: false,
        index_lag_us: 0,
    })
}

async fn search_logs(
    client: &Client,
    request: &SearchQuery,
) -> Result<SearchPage, ClickhouseError> {
    let started_at_us = traversal_watermark(client, request).await?;
    let plan = search_sql::candidates(request, Backend::Clickhouse);
    let mut references: Vec<LogCandidateRef> =
        query::bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
            .fetch_all()
            .await?;
    let limit_reached = references.len() > request.max_examined as usize;
    references.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for reference in references {
        let _ = reference.match_state;
        let cursor = SearchCursor {
            timestamp_us: reference.timestamp_us,
            tie_breaker: reference.log_digest.clone(),
            ordinal: reference.ordinal,
            started_at_us,
        };
        let Some(row) = log::get_log(
            client,
            &request.project_id,
            &reference.log_digest,
            reference.ordinal,
        )
        .await?
        else {
            next_cursor = Some(cursor);
            continue;
        };
        let (document, source) = log_document(
            client,
            request.project_id.as_str(),
            &reference.log_digest,
            reference.ordinal,
        )
        .await?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Log(row),
            document,
            source,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    let examined = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: limit_reached,
        arrivals_detected: false,
        index_lag_us: 0,
    })
}

async fn traversal_watermark(
    client: &Client,
    request: &SearchQuery,
) -> Result<i64, ClickhouseError> {
    if let Some(cursor) = request.cursor.as_ref() {
        return Ok(cursor.started_at_us);
    }
    let query_plan = search_sql::watermark(request, Backend::Clickhouse);
    Ok(
        query::bind_analytics_values(client.query(query_plan.sql()), query_plan.params())
            .fetch_one()
            .await?,
    )
}

pub async fn arrivals_detected(
    client: &Client,
    request: &SearchQuery,
    through: &SearchCursor,
) -> Result<bool, ClickhouseError> {
    let query_plan = search_sql::arrivals(request, through, Backend::Clickhouse);
    let value: u8 =
        query::bind_analytics_values(client.query(query_plan.sql()), query_plan.params())
            .fetch_one()
            .await?;
    Ok(value != 0)
}

async fn span_document(
    client: &Client,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<(SearchDocument, SearchSource), ClickhouseError> {
    let source: SpanSearchSource = client
        .query(
            "SELECT messages, tool_definitions, tool_names, input_preview, output_preview, \
             gen_ai_tool_name, status_message, exception_type, exception_message, \
             exception_stacktrace, span_name, \
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
    let document = SearchDocument {
        fields: vec![
            field(
                SearchField::Prompt,
                source.search_prompt,
                source.search_prompt_truncated,
            ),
            field(
                SearchField::Completion,
                source.search_completion,
                source.search_completion_truncated,
            ),
            field(
                SearchField::ToolName,
                source.search_tool_name,
                source.search_tool_name_truncated,
            ),
            field(
                SearchField::ToolArgs,
                source.search_tool_args,
                source.search_tool_args_truncated,
            ),
            field(
                SearchField::Error,
                source.search_error,
                source.search_error_truncated,
            ),
            field(
                SearchField::SpanName,
                source.search_span_name,
                source.search_span_name_truncated,
            ),
        ],
    };
    Ok((
        document,
        SearchSource::Span(PortSpanSearchSource {
            messages: Some(source.messages),
            tool_definitions: Some(source.tool_definitions),
            tool_names: Some(source.tool_names),
            input_preview: source.input_preview,
            output_preview: source.output_preview,
            gen_ai_tool_name: source.gen_ai_tool_name,
            status_message: source.status_message,
            exception_type: source.exception_type,
            exception_message: source.exception_message,
            exception_stacktrace: source.exception_stacktrace,
            span_name: source.span_name,
        }),
    ))
}

async fn log_document(
    client: &Client,
    project_id: &str,
    digest: &str,
    ordinal: u32,
) -> Result<(SearchDocument, SearchSource), ClickhouseError> {
    let source: LogSearchSource = client
        .query(
            "SELECT body_text, body, event_name, severity_text, attributes, search_body, \
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
    let document = SearchDocument {
        fields: vec![
            field(
                SearchField::Body,
                source.search_body,
                source.search_body_truncated,
            ),
            field(
                SearchField::EventName,
                source.search_event_name,
                source.search_event_name_truncated,
            ),
            field(
                SearchField::Severity,
                source.search_severity,
                source.search_severity_truncated,
            ),
            field(
                SearchField::Attributes,
                source.search_attributes,
                source.search_attributes_truncated,
            ),
        ],
    };
    Ok((
        document,
        SearchSource::Log(PortLogSearchSource {
            body_text: source.body_text,
            body: source.body,
            event_name: source.event_name,
            severity_text: source.severity_text,
            attributes: source.attributes,
        }),
    ))
}

fn field(field: SearchField, terms: Vec<String>, truncated: u8) -> SearchFieldTerms {
    SearchFieldTerms {
        field,
        terms,
        truncated: truncated != 0,
        text: String::new(),
    }
}
