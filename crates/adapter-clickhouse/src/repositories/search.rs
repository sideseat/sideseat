use clickhouse::{Client, Row};
use serde::Deserialize;
use sideseat_ports::types::{
    LogSearchSource as PortLogSearchSource, SearchBackfillDocument, SearchBackfillSource,
    SearchCandidate, SearchCursor, SearchDocument, SearchLogRecord, SearchPage, SearchQuery,
    SearchRecord, SearchRecordId, SearchSignal, SearchSource, SearchSpanRecord,
    SpanSearchSource as PortSpanSearchSource,
};
use sideseat_query_sql::{Backend, dml, search as search_sql};

use crate::ClickhouseError;
use crate::repositories::query;

#[derive(Row, Deserialize)]
struct SpanBackfillSource {
    trace_id: String,
    span_id: String,
    content_digest: String,
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
}

#[derive(Row, Deserialize)]
struct LogBackfillSource {
    log_digest: String,
    ordinal: u32,
    body_text: Option<String>,
    body: Option<String>,
    event_name: Option<String>,
    severity_text: Option<String>,
    attributes: Option<String>,
}

#[derive(Row, Deserialize)]
struct SpanCandidate {
    trace_id: String,
    span_id: String,
    span_name: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    messages: String,
    tool_definitions: String,
    tool_names: String,
    gen_ai_tool_name: Option<String>,
    status_message: Option<String>,
    exception_type: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    timestamp_us: i64,
    #[serde(rename = "match_state")]
    _match_state: i16,
    search_indexed: u8,
}

#[derive(Row, Deserialize)]
struct LogCandidate {
    log_digest: String,
    ordinal: u32,
    severity_text: Option<String>,
    body_text: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    body: Option<String>,
    event_name: Option<String>,
    attributes: Option<String>,
    timestamp_us: i64,
    #[serde(rename = "match_state")]
    _match_state: i16,
    search_indexed: u8,
}

pub async fn search(client: &Client, request: &SearchQuery) -> Result<SearchPage, ClickhouseError> {
    match request.signal {
        SearchSignal::Spans => search_spans(client, request).await,
        SearchSignal::Logs => search_logs(client, request).await,
    }
}

pub async fn backfill_page(
    client: &Client,
    project_id: &str,
    signal: SearchSignal,
    limit: usize,
) -> Result<Vec<SearchBackfillSource>, ClickhouseError> {
    let plan = search_sql::backfill_sources(project_id, signal, limit, Backend::Clickhouse);
    match signal {
        SearchSignal::Spans => {
            let rows: Vec<SpanBackfillSource> =
                query::bind_analytics_values(client.query(plan.sql()), plan.params())
                    .fetch_all()
                    .await?;
            Ok(rows
                .into_iter()
                .map(|row| SearchBackfillSource {
                    id: SearchRecordId::Span {
                        trace_id: row.trace_id,
                        span_id: row.span_id,
                    },
                    expected_content_digest: Some(row.content_digest),
                    source: SearchSource::Span(PortSpanSearchSource {
                        messages: Some(row.messages),
                        tool_definitions: Some(row.tool_definitions),
                        tool_names: Some(row.tool_names),
                        input_preview: row.input_preview,
                        output_preview: row.output_preview,
                        gen_ai_tool_name: row.gen_ai_tool_name,
                        status_message: row.status_message,
                        exception_type: row.exception_type,
                        exception_message: row.exception_message,
                        exception_stacktrace: row.exception_stacktrace,
                        span_name: row.span_name,
                    }),
                })
                .collect())
        }
        SearchSignal::Logs => {
            let rows: Vec<LogBackfillSource> =
                query::bind_analytics_values(client.query(plan.sql()), plan.params())
                    .fetch_all()
                    .await?;
            Ok(rows
                .into_iter()
                .map(|row| SearchBackfillSource {
                    id: SearchRecordId::Log {
                        log_digest: row.log_digest,
                        ordinal: row.ordinal,
                    },
                    expected_content_digest: None,
                    source: SearchSource::Log(PortLogSearchSource {
                        body_text: row.body_text,
                        body: row.body,
                        event_name: row.event_name,
                        severity_text: row.severity_text,
                        attributes: row.attributes,
                    }),
                })
                .collect())
        }
    }
}

pub async fn write_backfill(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &str,
    signal: SearchSignal,
    documents: &[SearchBackfillDocument],
) -> Result<(), ClickhouseError> {
    let target = dml::MutationTarget::clickhouse(table, on_cluster);
    for document in documents {
        let statement = dml::search_backfill_update(target, project_id, signal, document);
        query::bind_analytics_values(client.query(statement.sql()), statement.params())
            .execute()
            .await?;
    }
    Ok(())
}

async fn search_spans(
    client: &Client,
    request: &SearchQuery,
) -> Result<SearchPage, ClickhouseError> {
    let started_at_us = traversal_watermark(client, request).await?;
    let search_indexing_complete = indexing_complete(client, request).await?;
    let plan = search_sql::candidates(request, Backend::Clickhouse);
    let mut rows: Vec<SpanCandidate> =
        query::bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
            .fetch_all()
            .await?;
    let limit_reached = rows.len() > request.max_examined as usize;
    rows.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for row in rows {
        let cursor = SearchCursor {
            timestamp_us: row.timestamp_us,
            tie_breaker: format!("{}\0{}", row.trace_id, row.span_id),
            ordinal: 0,
            started_at_us,
        };
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Span(SearchSpanRecord {
                trace_id: row.trace_id,
                span_id: row.span_id,
                timestamp: chrono::DateTime::from_timestamp_micros(row.timestamp_us)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH),
                span_name: row.span_name.clone(),
                input_preview: row.input_preview.clone(),
                output_preview: row.output_preview.clone(),
            }),
            document: SearchDocument {
                indexed: row.search_indexed != 0,
                fields: Vec::new(),
            },
            source: SearchSource::Span(PortSpanSearchSource {
                messages: Some(row.messages),
                tool_definitions: Some(row.tool_definitions),
                tool_names: Some(row.tool_names),
                input_preview: row.input_preview,
                output_preview: row.output_preview,
                gen_ai_tool_name: row.gen_ai_tool_name,
                status_message: row.status_message,
                exception_type: row.exception_type,
                exception_message: row.exception_message,
                exception_stacktrace: row.exception_stacktrace,
                span_name: row.span_name,
            }),
            cursor,
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
        search_indexing_complete,
    })
}

async fn search_logs(
    client: &Client,
    request: &SearchQuery,
) -> Result<SearchPage, ClickhouseError> {
    let started_at_us = traversal_watermark(client, request).await?;
    let search_indexing_complete = indexing_complete(client, request).await?;
    let plan = search_sql::candidates(request, Backend::Clickhouse);
    let mut rows: Vec<LogCandidate> =
        query::bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
            .fetch_all()
            .await?;
    let limit_reached = rows.len() > request.max_examined as usize;
    rows.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for row in rows {
        let cursor = SearchCursor {
            timestamp_us: row.timestamp_us,
            tie_breaker: row.log_digest.clone(),
            ordinal: row.ordinal,
            started_at_us,
        };
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Log(SearchLogRecord {
                log_digest: row.log_digest,
                ordinal: row.ordinal,
                timestamp: chrono::DateTime::from_timestamp_micros(row.timestamp_us)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH),
                severity_text: row.severity_text.clone(),
                body_text: row.body_text.clone(),
                trace_id: row.trace_id.clone(),
                span_id: row.span_id.clone(),
            }),
            document: SearchDocument {
                indexed: row.search_indexed != 0,
                fields: Vec::new(),
            },
            source: SearchSource::Log(PortLogSearchSource {
                body_text: row.body_text,
                body: row.body,
                event_name: row.event_name,
                severity_text: row.severity_text,
                attributes: row.attributes,
            }),
            cursor,
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
        search_indexing_complete,
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

async fn indexing_complete(
    client: &Client,
    request: &SearchQuery,
) -> Result<bool, ClickhouseError> {
    let query_plan = search_sql::indexing_complete(request, Backend::Clickhouse);
    let value: u8 =
        query::bind_analytics_values(client.query(query_plan.sql()), query_plan.params())
            .fetch_one()
            .await?;
    Ok(value != 0)
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
