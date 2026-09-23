use duckdb::{Connection, params};
use sideseat_ports::types::{
    LogSearchSource, NormalizedLog, NormalizedSpan, SearchBackfillDocument, SearchBackfillSource,
    SearchCandidate, SearchCursor, SearchDocument, SearchLogRecord, SearchPage, SearchQuery,
    SearchRecord, SearchRecordId, SearchSignal, SearchSource, SearchSpanRecord, SpanSearchSource,
};
use sideseat_query_sql::{Backend, analytics::QueryValue, search as search_sql};

use crate::DuckdbError;
struct SpanCandidate {
    trace_id: String,
    span_id: String,
    span_name: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    messages: Option<String>,
    tool_definitions: Option<String>,
    tool_names: Option<String>,
    gen_ai_tool_name: Option<String>,
    status_message: Option<String>,
    exception_type: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    timestamp_us: i64,
    _match_state: i16,
    search_indexed: bool,
}

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
    _match_state: i16,
    search_indexed: bool,
}

pub fn replace_span_terms(conn: &Connection, spans: &[NormalizedSpan]) -> Result<(), DuckdbError> {
    let mut delete = conn.prepare(search_sql::DUCKDB_SPAN_TERM_DELETE_SQL)?;
    let mut insert = conn.prepare(search_sql::DUCKDB_SPAN_TERM_INSERT_SQL)?;
    for span in spans {
        let project_id = span.project_id.as_deref().unwrap_or_default();
        delete.execute(params![project_id, span.trace_id, span.span_id])?;
        for field in &span.search.fields {
            if field.terms.is_empty() {
                insert.execute(params![
                    project_id,
                    span.trace_id,
                    span.span_id,
                    field.field.as_str(),
                    "",
                    field.truncated,
                ])?;
            } else {
                for term in &field.terms {
                    insert.execute(params![
                        project_id,
                        span.trace_id,
                        span.span_id,
                        field.field.as_str(),
                        term,
                        field.truncated,
                    ])?;
                }
            }
        }
    }
    Ok(())
}

pub fn replace_log_terms(conn: &Connection, logs: &[NormalizedLog]) -> Result<(), DuckdbError> {
    let mut delete = conn.prepare(search_sql::DUCKDB_LOG_TERM_DELETE_SQL)?;
    let mut insert = conn.prepare(search_sql::DUCKDB_LOG_TERM_INSERT_SQL)?;
    for log in logs {
        let project_id = log.project_id.as_deref().unwrap_or_default();
        delete.execute(params![project_id, log.log_digest, log.ordinal])?;
        for field in &log.search.fields {
            if field.terms.is_empty() {
                insert.execute(params![
                    project_id,
                    log.log_digest,
                    log.ordinal,
                    field.field.as_str(),
                    "",
                    field.truncated,
                ])?;
            } else {
                for term in &field.terms {
                    insert.execute(params![
                        project_id,
                        log.log_digest,
                        log.ordinal,
                        field.field.as_str(),
                        term,
                        field.truncated,
                    ])?;
                }
            }
        }
    }
    Ok(())
}

pub fn delete_for_traces(
    conn: &Connection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<(), DuckdbError> {
    let mut span_terms = conn.prepare(search_sql::DUCKDB_SPAN_TERMS_DELETE_TRACE_SQL)?;
    let mut log_terms = conn.prepare(search_sql::DUCKDB_LOG_TERMS_DELETE_TRACE_SQL)?;
    for trace_id in trace_ids {
        span_terms.execute(params![project_id, trace_id])?;
        log_terms.execute(params![project_id, trace_id])?;
    }
    Ok(())
}

pub fn delete_for_spans(
    conn: &Connection,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<(), DuckdbError> {
    let mut span_terms = conn.prepare(search_sql::DUCKDB_SPAN_TERM_DELETE_SQL)?;
    let mut log_terms = conn.prepare(search_sql::DUCKDB_LOG_TERMS_DELETE_SPAN_SQL)?;
    for (trace_id, span_id) in spans {
        span_terms.execute(params![project_id, trace_id, span_id])?;
        log_terms.execute(params![project_id, trace_id, span_id])?;
    }
    Ok(())
}

pub fn delete_for_project(conn: &Connection, project_id: &str) -> Result<(), DuckdbError> {
    conn.execute(
        search_sql::DUCKDB_SPAN_TERMS_DELETE_PROJECT_SQL,
        params![project_id],
    )?;
    conn.execute(
        search_sql::DUCKDB_LOG_TERMS_DELETE_PROJECT_SQL,
        params![project_id],
    )?;
    Ok(())
}

pub fn search(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    match request.signal {
        SearchSignal::Spans => search_spans(conn, request),
        SearchSignal::Logs => search_logs(conn, request),
    }
}

pub fn backfill_page(
    conn: &Connection,
    project_id: &str,
    signal: SearchSignal,
    limit: usize,
) -> Result<Vec<SearchBackfillSource>, DuckdbError> {
    let query = search_sql::backfill_sources(project_id, signal, limit, Backend::Duckdb);
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let mut rows = statement.query(values.as_slice())?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        sources.push(match signal {
            SearchSignal::Spans => SearchBackfillSource {
                id: SearchRecordId::Span {
                    trace_id: row.get(0)?,
                    span_id: row.get(1)?,
                },
                expected_content_digest: Some(row.get(2)?),
                source: SearchSource::Span(SpanSearchSource {
                    messages: row.get(3)?,
                    tool_definitions: row.get(4)?,
                    tool_names: row.get(5)?,
                    input_preview: row.get(6)?,
                    output_preview: row.get(7)?,
                    gen_ai_tool_name: row.get(8)?,
                    status_message: row.get(9)?,
                    exception_type: row.get(10)?,
                    exception_message: row.get(11)?,
                    exception_stacktrace: row.get(12)?,
                    span_name: row.get(13)?,
                }),
            },
            SearchSignal::Logs => SearchBackfillSource {
                id: SearchRecordId::Log {
                    log_digest: row.get(0)?,
                    ordinal: row.get(1)?,
                },
                expected_content_digest: None,
                source: SearchSource::Log(LogSearchSource {
                    body_text: row.get(2)?,
                    body: row.get(3)?,
                    event_name: row.get(4)?,
                    severity_text: row.get(5)?,
                    attributes: row.get(6)?,
                }),
            },
        });
    }
    Ok(sources)
}

pub fn write_backfill(
    conn: &Connection,
    project_id: &str,
    signal: SearchSignal,
    documents: &[SearchBackfillDocument],
) -> Result<(), DuckdbError> {
    crate::in_transaction(conn, |conn| match signal {
        SearchSignal::Spans => write_span_backfill(conn, project_id, documents),
        SearchSignal::Logs => write_log_backfill(conn, project_id, documents),
    })
}

fn write_span_backfill(
    conn: &Connection,
    project_id: &str,
    documents: &[SearchBackfillDocument],
) -> Result<(), DuckdbError> {
    let mut delete = conn.prepare(search_sql::DUCKDB_SPAN_TERM_DELETE_SQL)?;
    let mut insert = conn.prepare(search_sql::DUCKDB_SPAN_TERM_INSERT_SQL)?;
    for document in documents {
        let SearchRecordId::Span { trace_id, span_id } = &document.id else {
            panic!("span search backfill received a log identity");
        };
        let expected_content_digest = document
            .expected_content_digest
            .as_deref()
            .expect("span search backfill requires its observed content digest");
        let still_current_and_unindexed: bool = conn.query_row(
            search_sql::DUCKDB_SPAN_BACKFILL_CAS_SQL,
            params![
                project_id,
                trace_id,
                span_id,
                expected_content_digest,
                project_id,
                trace_id,
                span_id
            ],
            |row| row.get(0),
        )?;
        if !still_current_and_unindexed {
            continue;
        }
        delete.execute(params![project_id, trace_id, span_id])?;
        write_document_fields(&document.document, |field, term, truncated| {
            insert.execute(params![
                project_id, trace_id, span_id, field, term, truncated
            ])?;
            Ok(())
        })?;
    }
    Ok(())
}

fn write_log_backfill(
    conn: &Connection,
    project_id: &str,
    documents: &[SearchBackfillDocument],
) -> Result<(), DuckdbError> {
    let mut delete = conn.prepare(search_sql::DUCKDB_LOG_TERM_DELETE_SQL)?;
    let mut insert = conn.prepare(search_sql::DUCKDB_LOG_TERM_INSERT_SQL)?;
    for document in documents {
        let SearchRecordId::Log {
            log_digest,
            ordinal,
        } = &document.id
        else {
            panic!("log search backfill received a span identity");
        };
        delete.execute(params![project_id, log_digest, ordinal])?;
        write_document_fields(&document.document, |field, term, truncated| {
            insert.execute(params![
                project_id, log_digest, ordinal, field, term, truncated
            ])?;
            Ok(())
        })?;
    }
    Ok(())
}

fn write_document_fields(
    document: &SearchDocument,
    mut write: impl FnMut(&str, &str, bool) -> Result<(), DuckdbError>,
) -> Result<(), DuckdbError> {
    for field in &document.fields {
        if field.terms.is_empty() {
            write(field.field.as_str(), "", field.truncated)?;
        } else {
            for term in &field.terms {
                write(field.field.as_str(), term, field.truncated)?;
            }
        }
    }
    Ok(())
}

fn search_spans(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    let started_at_us = traversal_watermark(conn, request)?;
    let search_indexing_complete = indexing_complete(conn, request)?;
    let plan = search_sql::candidates(request, Backend::Duckdb);
    let values = duckdb_values(plan.query.params());
    let mut statement = conn.prepare(plan.query.sql())?;
    let mut result = statement.query(values.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = result.next()? {
        rows.push(SpanCandidate {
            trace_id: row.get(0)?,
            span_id: row.get(1)?,
            span_name: row.get(2)?,
            input_preview: row.get(3)?,
            output_preview: row.get(4)?,
            messages: row.get(5)?,
            tool_definitions: row.get(6)?,
            tool_names: row.get(7)?,
            gen_ai_tool_name: row.get(8)?,
            status_message: row.get(9)?,
            exception_type: row.get(10)?,
            exception_message: row.get(11)?,
            exception_stacktrace: row.get(12)?,
            timestamp_us: row.get(13)?,
            _match_state: row.get(14)?,
            search_indexed: row.get(15)?,
        });
    }
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
                indexed: row.search_indexed,
                fields: Vec::new(),
            },
            source: SearchSource::Span(SpanSearchSource {
                messages: row.messages,
                tool_definitions: row.tool_definitions,
                tool_names: row.tool_names,
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

fn search_logs(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    let started_at_us = traversal_watermark(conn, request)?;
    let search_indexing_complete = indexing_complete(conn, request)?;
    let plan = search_sql::candidates(request, Backend::Duckdb);
    let values = duckdb_values(plan.query.params());
    let mut statement = conn.prepare(plan.query.sql())?;
    let mut result = statement.query(values.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = result.next()? {
        rows.push(LogCandidate {
            log_digest: row.get(0)?,
            ordinal: row.get(1)?,
            severity_text: row.get(2)?,
            body_text: row.get(3)?,
            trace_id: row.get(4)?,
            span_id: row.get(5)?,
            body: row.get(6)?,
            event_name: row.get(7)?,
            attributes: row.get(8)?,
            timestamp_us: row.get(9)?,
            _match_state: row.get(10)?,
            search_indexed: row.get(11)?,
        });
    }
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
                indexed: row.search_indexed,
                fields: Vec::new(),
            },
            source: SearchSource::Log(LogSearchSource {
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

fn traversal_watermark(conn: &Connection, request: &SearchQuery) -> Result<i64, DuckdbError> {
    if let Some(cursor) = request.cursor.as_ref() {
        return Ok(cursor.started_at_us);
    }
    let query = search_sql::watermark(request, Backend::Duckdb);
    let values = duckdb_values(query.params());
    Ok(conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?)
}

fn indexing_complete(conn: &Connection, request: &SearchQuery) -> Result<bool, DuckdbError> {
    let query = search_sql::indexing_complete(request, Backend::Duckdb);
    let values = duckdb_values(query.params());
    Ok(conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?)
}

pub fn arrivals_detected(
    conn: &Connection,
    request: &SearchQuery,
    through: &SearchCursor,
) -> Result<bool, DuckdbError> {
    let query = search_sql::arrivals(request, through, Backend::Duckdb);
    let values = duckdb_values(query.params());
    Ok(conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))?)
}

fn duckdb_values(values: &[QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            QueryValue::String(value) => value as &dyn duckdb::ToSql,
            QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}
