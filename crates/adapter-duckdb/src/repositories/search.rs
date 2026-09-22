use duckdb::{Connection, params};
use sideseat_ports::types::{
    LogSearchSource, NormalizedLog, NormalizedSpan, SearchCandidate, SearchCursor, SearchDocument,
    SearchField, SearchFieldTerms, SearchPage, SearchQuery, SearchRecord, SearchSignal,
    SearchSource, SpanSearchSource,
};
use sideseat_query_sql::{Backend, analytics::QueryValue, search as search_sql};

use crate::DuckdbError;
use crate::repositories::query;

struct SpanSource {
    messages: Option<String>,
    tool_definitions: Option<String>,
    tool_names: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    gen_ai_tool_name: Option<String>,
    status_message: Option<String>,
    exception_type: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    span_name: Option<String>,
}

struct LogSource {
    body_text: Option<String>,
    body: Option<String>,
    event_name: Option<String>,
    severity_text: Option<String>,
    attributes: Option<String>,
}

struct SpanCandidateRef {
    trace_id: String,
    span_id: String,
    timestamp_us: i64,
}

struct LogCandidateRef {
    log_digest: String,
    ordinal: u32,
    timestamp_us: i64,
}

pub fn replace_span_terms(conn: &Connection, spans: &[NormalizedSpan]) -> Result<(), DuckdbError> {
    let mut delete = conn
        .prepare("DELETE FROM span_terms WHERE project_id = ? AND trace_id = ? AND span_id = ?")?;
    let mut insert = conn.prepare(
        "INSERT INTO span_terms(project_id, trace_id, span_id, field, term, truncated) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )?;
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
    let mut delete = conn
        .prepare("DELETE FROM log_terms WHERE project_id = ? AND log_digest = ? AND ordinal = ?")?;
    let mut insert = conn.prepare(
        "INSERT INTO log_terms(project_id, log_digest, ordinal, field, term, truncated) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )?;
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
    let mut span_terms =
        conn.prepare("DELETE FROM span_terms WHERE project_id = ? AND trace_id = ?")?;
    let mut log_terms = conn.prepare(
        "DELETE FROM log_terms WHERE project_id = ? AND EXISTS (\
         SELECT 1 FROM otel_logs l WHERE l.project_id = log_terms.project_id \
         AND l.log_digest = log_terms.log_digest AND l.ordinal = log_terms.ordinal \
         AND l.trace_id = ?)",
    )?;
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
    let mut span_terms = conn
        .prepare("DELETE FROM span_terms WHERE project_id = ? AND trace_id = ? AND span_id = ?")?;
    let mut log_terms = conn.prepare(
        "DELETE FROM log_terms WHERE project_id = ? AND EXISTS (\
         SELECT 1 FROM otel_logs l WHERE l.project_id = log_terms.project_id \
         AND l.log_digest = log_terms.log_digest AND l.ordinal = log_terms.ordinal \
         AND l.trace_id = ? AND l.span_id = ?)",
    )?;
    for (trace_id, span_id) in spans {
        span_terms.execute(params![project_id, trace_id, span_id])?;
        log_terms.execute(params![project_id, trace_id, span_id])?;
    }
    Ok(())
}

pub fn delete_for_project(conn: &Connection, project_id: &str) -> Result<(), DuckdbError> {
    conn.execute(
        "DELETE FROM span_terms WHERE project_id = ?",
        params![project_id],
    )?;
    conn.execute(
        "DELETE FROM log_terms WHERE project_id = ?",
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

fn search_spans(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    let started_at_us = traversal_watermark(conn, request)?;
    let search_indexing_complete = indexing_complete(conn, request)?;
    let plan = search_sql::candidates(request, Backend::Duckdb);
    let values = duckdb_values(plan.query.params());
    let mut statement = conn.prepare(plan.query.sql())?;
    let mut result = statement.query(values.as_slice())?;
    let mut references = Vec::new();
    while let Some(row) = result.next()? {
        references.push(SpanCandidateRef {
            trace_id: row.get(0)?,
            span_id: row.get(1)?,
            timestamp_us: row.get(2)?,
        });
    }
    let limit_reached = references.len() > request.max_examined as usize;
    references.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for reference in references {
        let cursor = SearchCursor {
            timestamp_us: reference.timestamp_us,
            tie_breaker: format!("{}\0{}", reference.trace_id, reference.span_id),
            ordinal: 0,
            started_at_us,
        };
        let Some(row) = query::get_span(
            conn,
            request.project_id.as_str(),
            &reference.trace_id,
            &reference.span_id,
        )?
        else {
            next_cursor = Some(cursor);
            continue;
        };
        let (document, source) = span_document(
            conn,
            request.project_id.as_str(),
            &reference.trace_id,
            &reference.span_id,
        )?;
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
    let mut references = Vec::new();
    while let Some(row) = result.next()? {
        references.push(LogCandidateRef {
            log_digest: row.get(0)?,
            ordinal: row.get(1)?,
            timestamp_us: row.get(2)?,
        });
    }
    let limit_reached = references.len() > request.max_examined as usize;
    references.truncate(request.max_examined as usize);
    let mut candidates = Vec::new();
    let mut next_cursor = None;
    for reference in references {
        let cursor = SearchCursor {
            timestamp_us: reference.timestamp_us,
            tie_breaker: reference.log_digest.clone(),
            ordinal: reference.ordinal,
            started_at_us,
        };
        let Some(row) = crate::repositories::log::get_log(
            conn,
            &request.project_id,
            &reference.log_digest,
            reference.ordinal,
        )?
        else {
            next_cursor = Some(cursor);
            continue;
        };
        let (document, source) = log_document(
            conn,
            request.project_id.as_str(),
            &reference.log_digest,
            reference.ordinal,
        )?;
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

fn span_document(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<(SearchDocument, SearchSource), DuckdbError> {
    let source = conn.query_row(
        "SELECT messages, tool_definitions, tool_names, input_preview, output_preview, \
                    gen_ai_tool_name, status_message, exception_type, exception_message, \
                    exception_stacktrace, span_name \
             FROM otel_spans WHERE project_id = ? AND trace_id = ? AND span_id = ? \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                        ORDER BY ingested_at DESC, rowid DESC) = 1",
        params![project_id, trace_id, span_id],
        |row| {
            Ok(SpanSource {
                messages: row.get(0)?,
                tool_definitions: row.get(1)?,
                tool_names: row.get(2)?,
                input_preview: row.get(3)?,
                output_preview: row.get(4)?,
                gen_ai_tool_name: row.get(5)?,
                status_message: row.get(6)?,
                exception_type: row.get(7)?,
                exception_message: row.get(8)?,
                exception_stacktrace: row.get(9)?,
                span_name: row.get(10)?,
            })
        },
    )?;
    let document = load_span_terms(conn, project_id, trace_id, span_id)?;
    Ok((
        document,
        SearchSource::Span(SpanSearchSource {
            messages: source.messages,
            tool_definitions: source.tool_definitions,
            tool_names: source.tool_names,
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

fn log_document(
    conn: &Connection,
    project_id: &str,
    digest: &str,
    ordinal: u32,
) -> Result<(SearchDocument, SearchSource), DuckdbError> {
    let source = conn.query_row(
        "SELECT body_text, body, event_name, severity_text, attributes FROM otel_logs \
         WHERE project_id = ? AND log_digest = ? AND ordinal = ?",
        params![project_id, digest, ordinal],
        |row| {
            Ok(LogSource {
                body_text: row.get(0)?,
                body: row.get(1)?,
                event_name: row.get(2)?,
                severity_text: row.get(3)?,
                attributes: row.get(4)?,
            })
        },
    )?;
    let document = load_log_terms(conn, project_id, digest, ordinal)?;
    Ok((
        document,
        SearchSource::Log(LogSearchSource {
            body_text: source.body_text,
            body: source.body,
            event_name: source.event_name,
            severity_text: source.severity_text,
            attributes: source.attributes,
        }),
    ))
}

fn load_span_terms(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<SearchDocument, DuckdbError> {
    let mut statement = conn.prepare(
        "SELECT field, term, truncated FROM span_terms \
         WHERE project_id = ? AND trace_id = ? AND span_id = ? ORDER BY field, term",
    )?;
    let rows = statement.query_map(params![project_id, trace_id, span_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })?;
    rows_to_document(
        rows.collect::<Result<Vec<_>, _>>()?,
        &[
            SearchField::Prompt,
            SearchField::Completion,
            SearchField::ToolName,
            SearchField::ToolArgs,
            SearchField::Error,
            SearchField::SpanName,
        ],
    )
}

fn load_log_terms(
    conn: &Connection,
    project_id: &str,
    digest: &str,
    ordinal: u32,
) -> Result<SearchDocument, DuckdbError> {
    let mut statement = conn.prepare(
        "SELECT field, term, truncated FROM log_terms \
         WHERE project_id = ? AND log_digest = ? AND ordinal = ? ORDER BY field, term",
    )?;
    let rows = statement.query_map(params![project_id, digest, ordinal], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })?;
    rows_to_document(
        rows.collect::<Result<Vec<_>, _>>()?,
        &[
            SearchField::Body,
            SearchField::EventName,
            SearchField::Severity,
            SearchField::Attributes,
        ],
    )
}

fn rows_to_document(
    rows: Vec<(String, String, bool)>,
    fields: &[SearchField],
) -> Result<SearchDocument, DuckdbError> {
    let mut grouped: std::collections::BTreeMap<SearchField, (Vec<String>, bool)> =
        std::collections::BTreeMap::new();
    for (field, term, truncated) in rows {
        let Some(field) = SearchField::parse(&field) else {
            continue;
        };
        let entry = grouped.entry(field).or_default();
        if !term.is_empty() {
            entry.0.push(term);
        }
        entry.1 |= truncated;
    }
    let indexed = fields.iter().all(|field| grouped.contains_key(field));
    Ok(SearchDocument {
        indexed,
        fields: fields
            .iter()
            .copied()
            .map(|field| {
                let (terms, truncated) = grouped.remove(&field).unwrap_or_default();
                SearchFieldTerms {
                    field,
                    terms,
                    truncated,
                    text: String::new(),
                }
            })
            .collect(),
    })
}
