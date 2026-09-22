use std::collections::BTreeMap;

use duckdb::{Connection, params};
use sideseat_ports::types::{
    ListLogsParams, ListSpansParams, NormalizedLog, NormalizedSpan, SearchCandidate, SearchCursor,
    SearchDocument, SearchField, SearchFieldTerms, SearchPage, SearchQuery, SearchRecord,
    SearchSignal,
};

use crate::DuckdbError;
use crate::repositories::{log, query};

struct SpanSource {
    messages: Option<String>,
    tool_definitions: Option<String>,
    tool_names: Option<String>,
    input_preview: Option<String>,
    output_preview: Option<String>,
    status_message: Option<String>,
    exception_message: Option<String>,
    exception_stacktrace: Option<String>,
    span_name: Option<String>,
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
    let mut params = ListSpansParams {
        project_id: request.project_id.clone(),
        page: 1,
        limit: 1,
        from_timestamp: request.from_timestamp,
        to_timestamp: request.to_timestamp,
        ..Default::default()
    };
    let (_, total) = query::list_spans(conn, &params)?;
    params.limit = u32::try_from(total).unwrap_or(u32::MAX).max(1);
    let (mut rows, _) = query::list_spans(conn, &params)?;
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
            conn,
            request.project_id.as_str(),
            &row.trace_id,
            &row.span_id,
        )?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Span(row),
            document,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    let limit_reached = examined >= request.max_examined;
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: limit_reached,
        arrivals_detected: arrivals_detected(conn, request.project_id.as_str(), started_at_us)?,
        index_lag_us: 0,
    })
}

fn search_logs(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    let mut params = ListLogsParams {
        project_id: request.project_id.clone(),
        page: 1,
        limit: 1,
        from_timestamp: request.from_timestamp,
        to_timestamp: request.to_timestamp,
        ..Default::default()
    };
    let (_, total) = log::list_logs(conn, &params)?;
    params.limit = u32::try_from(total).unwrap_or(u32::MAX).max(1);
    let (mut rows, _) = log::list_logs(conn, &params)?;
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
            conn,
            request.project_id.as_str(),
            &row.log_digest,
            row.ordinal,
        )?;
        next_cursor = Some(cursor.clone());
        candidates.push(SearchCandidate {
            record: SearchRecord::Log(row),
            document,
            cursor: cursor.clone(),
            indeterminate: false,
        });
    }
    let limit_reached = examined >= request.max_examined;
    Ok(SearchPage {
        candidates,
        next_cursor,
        examined,
        examination_limit_reached: limit_reached,
        arrivals_detected: arrivals_detected(conn, request.project_id.as_str(), started_at_us)?,
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

fn arrivals_detected(
    conn: &Connection,
    project_id: &str,
    started_at_us: i64,
) -> Result<bool, DuckdbError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) > 0 FROM (\
           SELECT ingested_at FROM otel_spans WHERE project_id = ? \
           UNION ALL SELECT ingested_at FROM otel_logs WHERE project_id = ?\
         ) WHERE EPOCH_US(ingested_at) > ?",
        params![project_id, project_id, started_at_us],
        |row| row.get(0),
    )?)
}

fn span_document(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
) -> Result<SearchDocument, DuckdbError> {
    let source = conn.query_row(
        "SELECT messages, tool_definitions, tool_names, input_preview, output_preview, \
                    status_message, exception_message, exception_stacktrace, span_name \
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
                status_message: row.get(5)?,
                exception_message: row.get(6)?,
                exception_stacktrace: row.get(7)?,
                span_name: row.get(8)?,
            })
        },
    )?;
    let mut texts = BTreeMap::new();
    texts.insert(
        SearchField::Prompt,
        join([source.messages.as_deref(), source.input_preview.as_deref()]),
    );
    texts.insert(
        SearchField::Completion,
        join([source.messages.as_deref(), source.output_preview.as_deref()]),
    );
    texts.insert(SearchField::ToolName, source.tool_names.unwrap_or_default());
    texts.insert(
        SearchField::ToolArgs,
        source.tool_definitions.unwrap_or_default(),
    );
    texts.insert(
        SearchField::Error,
        join([
            source.status_message.as_deref(),
            source.exception_message.as_deref(),
            source.exception_stacktrace.as_deref(),
        ]),
    );
    texts.insert(SearchField::SpanName, source.span_name.unwrap_or_default());
    load_span_terms(conn, project_id, trace_id, span_id, texts)
}

fn log_document(
    conn: &Connection,
    project_id: &str,
    digest: &str,
    ordinal: u32,
) -> Result<SearchDocument, DuckdbError> {
    let source: (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn.query_row(
        "SELECT body_text, event_name, severity_text, attributes FROM otel_logs \
         WHERE project_id = ? AND log_digest = ? AND ordinal = ?",
        params![project_id, digest, ordinal],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let texts = BTreeMap::from([
        (SearchField::Body, source.0.unwrap_or_default()),
        (SearchField::EventName, source.1.unwrap_or_default()),
        (SearchField::Severity, source.2.unwrap_or_default()),
        (SearchField::Attributes, source.3.unwrap_or_default()),
    ]);
    load_log_terms(conn, project_id, digest, ordinal, texts)
}

fn load_span_terms(
    conn: &Connection,
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    texts: BTreeMap<SearchField, String>,
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
    rows_to_document(rows.collect::<Result<Vec<_>, _>>()?, texts)
}

fn load_log_terms(
    conn: &Connection,
    project_id: &str,
    digest: &str,
    ordinal: u32,
    texts: BTreeMap<SearchField, String>,
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
    rows_to_document(rows.collect::<Result<Vec<_>, _>>()?, texts)
}

fn rows_to_document(
    rows: Vec<(String, String, bool)>,
    texts: BTreeMap<SearchField, String>,
) -> Result<SearchDocument, DuckdbError> {
    let mut grouped: BTreeMap<SearchField, (Vec<String>, bool)> = BTreeMap::new();
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
    Ok(SearchDocument {
        fields: texts
            .into_iter()
            .map(|(field, text)| {
                let (terms, truncated) = grouped.remove(&field).unwrap_or_default();
                SearchFieldTerms {
                    field,
                    terms,
                    truncated,
                    text,
                }
            })
            .collect(),
    })
}

fn join<const N: usize>(values: [Option<&str>; N]) -> String {
    values.into_iter().flatten().collect::<Vec<_>>().join("\n")
}
