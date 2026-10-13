use clickhouse::{Client, Row};
use serde::Deserialize;
use sideseat_ports::types::{
    LogSearchSource as PortLogSearchSource, SearchBackfillDocument, SearchBackfillSource,
    SearchCandidate, SearchCursor, SearchDocument, SearchLogRecord, SearchPage, SearchQuery,
    SearchRecord, SearchRecordId, SearchSignal, SearchSource, SearchSourceKey, SearchSpanRecord,
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
    raw_log: Option<String>,
    noted_references: Option<String>,
}

/// A light span candidate, in `search_sql::candidates`' column order.
#[derive(Row, Deserialize)]
struct SpanCandidate {
    trace_id: String,
    span_id: String,
    input_preview: Option<String>,
    output_preview: Option<String>,
    revision_us: i64,
    content_digest: String,
    source_bytes: u64,
    timestamp_us: i64,
    #[serde(rename = "match_state")]
    _match_state: i16,
    search_indexed: u8,
}

/// A light log candidate, in `search_sql::candidates`' column order.
#[derive(Row, Deserialize)]
struct LogCandidate {
    log_digest: String,
    ordinal: u32,
    trace_id: Option<String>,
    span_id: Option<String>,
    revision_us: i64,
    content_digest: String,
    source_bytes: u64,
    timestamp_us: i64,
    #[serde(rename = "match_state")]
    _match_state: i16,
    search_indexed: u8,
}

/// A span's source text, in `search_sql::sources`' column order.
#[derive(Row, Deserialize)]
struct SpanSourceRow {
    trace_id: String,
    span_id: String,
    revision_us: i64,
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

/// A log's source text, in `search_sql::sources`' column order.
#[derive(Row, Deserialize)]
struct LogSourceRow {
    log_digest: String,
    ordinal: u32,
    revision_us: i64,
    content_digest: String,
    body_text: Option<String>,
    body: Option<String>,
    event_name: Option<String>,
    severity_text: Option<String>,
    attributes: Option<String>,
    raw_log: Option<String>,
    noted_references: Option<String>,
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
                        raw_log: row.raw_log,
                        noted_references: sideseat_ports::types::noted_references_from_column(
                            row.noted_references.as_deref(),
                        ),
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
    let candidates: Vec<SearchCandidate> = rows
        .into_iter()
        .map(|row| SearchCandidate {
            cursor: SearchCursor {
                timestamp_us: row.timestamp_us,
                tie_breaker: format!("{}\0{}", row.trace_id, row.span_id),
                ordinal: 0,
                started_at_us,
            },
            key: SearchSourceKey {
                id: SearchRecordId::Span {
                    trace_id: row.trace_id.clone(),
                    span_id: row.span_id.clone(),
                },
                row_id: None,
                timestamp_us: row.timestamp_us,
                revision_us: row.revision_us,
                content_digest: row.content_digest,
            },
            source_bytes: row.source_bytes,
            record: SearchRecord::Span(SearchSpanRecord {
                trace_id: row.trace_id,
                span_id: row.span_id,
                timestamp: chrono::DateTime::from_timestamp_micros(row.timestamp_us)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH),
                span_name: None,
                input_preview: row.input_preview,
                output_preview: row.output_preview,
            }),
            document: SearchDocument {
                indexed: row.search_indexed != 0,
                fields: Vec::new(),
            },
            indeterminate: false,
        })
        .collect();
    Ok(page(candidates, limit_reached, search_indexing_complete))
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
    let candidates: Vec<SearchCandidate> = rows
        .into_iter()
        .map(|row| SearchCandidate {
            cursor: SearchCursor {
                timestamp_us: row.timestamp_us,
                tie_breaker: row.log_digest.clone(),
                ordinal: row.ordinal,
                started_at_us,
            },
            key: SearchSourceKey {
                id: SearchRecordId::Log {
                    log_digest: row.log_digest.clone(),
                    ordinal: row.ordinal,
                },
                row_id: None,
                timestamp_us: row.timestamp_us,
                revision_us: row.revision_us,
                content_digest: row.content_digest,
            },
            source_bytes: row.source_bytes,
            record: SearchRecord::Log(SearchLogRecord {
                log_digest: row.log_digest,
                ordinal: row.ordinal,
                timestamp: chrono::DateTime::from_timestamp_micros(row.timestamp_us)
                    .unwrap_or(chrono::DateTime::UNIX_EPOCH),
                severity_text: None,
                body_text: None,
                trace_id: row.trace_id,
                span_id: row.span_id,
            }),
            document: SearchDocument {
                indexed: row.search_indexed != 0,
                fields: Vec::new(),
            },
            indeterminate: false,
        })
        .collect();
    Ok(page(candidates, limit_reached, search_indexing_complete))
}

fn page(
    candidates: Vec<SearchCandidate>,
    limit_reached: bool,
    search_indexing_complete: bool,
) -> SearchPage {
    SearchPage {
        next_cursor: candidates.last().map(|candidate| candidate.cursor.clone()),
        examined: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
        candidates,
        examination_limit_reached: limit_reached,
        arrivals_detected: false,
        index_lag_us: 0,
        search_indexing_complete,
    }
}

/// The source text of `keys`, in order: each identity's winner, read only while it is the key's revision - one
/// deleted or superseded since the page was read is not read, and answers `None`.
pub async fn sources(
    client: &Client,
    request: &SearchQuery,
    keys: &[SearchSourceKey],
) -> Result<Vec<Option<SearchSource>>, ClickhouseError> {
    let Some(statement) = search_sql::sources(
        request.project_id.as_str(),
        request.signal,
        keys,
        Backend::Clickhouse,
    ) else {
        return Ok(vec![None; keys.len()]);
    };
    let query = query::bind_analytics_values(client.query(statement.sql()), statement.params());
    // At most a chunk of rows, so a scan per key costs less than a keyed map of them would.
    let mut found: Vec<((SearchRecordId, i64, String), SearchSource)> = Vec::new();
    match request.signal {
        SearchSignal::Spans => {
            for row in query.fetch_all::<SpanSourceRow>().await? {
                found.push((
                    (
                        SearchRecordId::Span {
                            trace_id: row.trace_id,
                            span_id: row.span_id,
                        },
                        row.revision_us,
                        row.content_digest,
                    ),
                    SearchSource::Span(PortSpanSearchSource {
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
                ));
            }
        }
        SearchSignal::Logs => {
            for row in query.fetch_all::<LogSourceRow>().await? {
                found.push((
                    (
                        SearchRecordId::Log {
                            log_digest: row.log_digest,
                            ordinal: row.ordinal,
                        },
                        row.revision_us,
                        row.content_digest,
                    ),
                    SearchSource::Log(PortLogSearchSource {
                        body_text: row.body_text,
                        body: row.body,
                        event_name: row.event_name,
                        severity_text: row.severity_text,
                        attributes: row.attributes,
                        raw_log: row.raw_log,
                        noted_references: sideseat_ports::types::noted_references_from_column(
                            row.noted_references.as_deref(),
                        ),
                    }),
                ));
            }
        }
    }
    Ok(keys
        .iter()
        .map(|key| {
            found
                .iter()
                .find(|((id, at, digest), _)| {
                    *id == key.id && *at == key.revision_us && *digest == key.content_digest
                })
                .map(|(_, source)| source.clone())
        })
        .collect())
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
