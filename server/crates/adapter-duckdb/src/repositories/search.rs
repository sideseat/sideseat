use duckdb::{Connection, params};
use sideseat_ports::types::{
    LogSearchSource, NormalizedLog, NormalizedSpan, SearchBackfillDocument, SearchBackfillSource,
    SearchCandidate, SearchCursor, SearchDocument, SearchLogRecord, SearchPage, SearchQuery,
    SearchRecord, SearchRecordId, SearchSignal, SearchSource, SearchSpanRecord, SpanSearchSource,
};
use sideseat_query_sql::{Backend, analytics::QueryValue, search as search_sql};

use std::collections::HashMap;

use sideseat_core::utils::time::micros_to_datetime;

use super::keyed::{self, LogIdentity, SpanIdentity};
use crate::error::DuckdbError;
use crate::sql_types::SqlTimestamp;

/// Identities per batched delete. A row-value `IN` list binds three values each, and a statement with tens of
/// thousands of parameters costs more to plan than it saves.
const DELETE_CHUNK: usize = 500;

/// The last item of each identity, in input order: what writing the items one at a time would have left.
fn last_per_identity<'a, T, K: Eq + std::hash::Hash>(
    items: &'a [T],
    key: impl Fn(&'a T) -> K,
) -> Vec<&'a T> {
    let mut last = std::collections::HashMap::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        last.insert(key(item), index);
    }
    items
        .iter()
        .enumerate()
        .filter(|(index, item)| last.get(&key(item)) == Some(index))
        .map(|(_, item)| item)
        .collect()
}

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

/// Replace the term rows of these spans: one delete for the batch, then the rows appended.
///
/// A span carries about 35 term rows, so the row-at-a-time `INSERT` this used to do was the write path's largest
/// cost by a wide margin - measured at 3.3 ms per span against 129 microseconds for the span row itself, which
/// the appender already wrote. The rows go through the same appender now, and the deletes that make a
/// redelivery idempotent are one statement for the whole batch.
///
/// A batch can hold several revisions of one span - a correction and the export it corrects. Writing them one
/// at a time left the last revision's terms, so only the last revision of each identity is written here: the
/// batched delete would otherwise leave every revision's vocabulary behind.
///
/// A span's terms are its **winner's** - the revision a read answers with, the latest by `ingested_at` - so a
/// search matches what the span shows. `stored` is the winner before this write, by its instant
/// (`keyed::span_revisions`, read before the new rows were appended), and that instant is exactly where the terms
/// are: one value per identity whatever the span's history, so a correction deletes one revision's terms and
/// reads only the row groups they went to. A revision older than the stored winner - a write whose instant was
/// set earlier, by another instance's clock or a redelivery - does not win, so it leaves the winner's terms
/// alone and writes none of its own. A new span, almost every span, deletes nothing: a delete for every span of
/// every batch read the whole term table, about 35 rows per span ever stored.
pub fn replace_span_terms(
    conn: &Connection,
    spans: &[NormalizedSpan],
    stored: &HashMap<SpanIdentity, i64>,
) -> Result<(), DuckdbError> {
    if spans.is_empty() {
        return Ok(());
    }
    let instant = super::supersession::instant_of;
    let identity_of = |span: &NormalizedSpan| -> SpanIdentity {
        (
            span.project_id.clone().unwrap_or_default(),
            span.trace_id.clone(),
            span.span_id.clone(),
        )
    };
    // The batch's own winner per identity: the latest instant, and of equal instants the later span, which is
    // what the rows' `rowid` order makes the winner on read.
    let mut winners: HashMap<SpanIdentity, usize> = HashMap::new();
    for (index, span) in spans.iter().enumerate() {
        match winners.get(&identity_of(span)) {
            Some(&previous) if instant(&spans[previous]) > instant(span) => {}
            _ => {
                winners.insert(identity_of(span), index);
            }
        }
    }
    // In batch order, not the map's: a hash map's order differs from one process to the next, and the term
    // table's bytes must not.
    let mut superseded: Vec<(SpanIdentity, i64)> = Vec::new();
    let mut writing: Vec<&NormalizedSpan> = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let identity = identity_of(span);
        if winners.get(&identity) != Some(&index) {
            continue;
        }
        match stored.get(&identity) {
            Some(&stored_us) if instant(span) < stored_us => {}
            Some(&stored_us) => {
                superseded.push((identity, stored_us));
                writing.push(span);
            }
            None => writing.push(span),
        }
    }
    for chunk in superseded.chunks(DELETE_CHUNK) {
        let identities: Vec<SpanIdentity> =
            chunk.iter().map(|(identity, _)| identity.clone()).collect();
        let revisions: Vec<i64> = chunk.iter().map(|(_, stored_us)| *stored_us).collect();
        if let Some(query) =
            search_sql::duckdb_span_term_delete_of_revisions(&identities, &revisions)
        {
            conn.execute(query.sql(), duckdb_values(query.params()).as_slice())?;
        }
    }
    let spans = writing;
    let mut appender = conn.appender("span_terms")?;
    for span in &spans {
        let project_id = span.project_id.as_deref().unwrap_or_default();
        let ingested_at = SqlTimestamp(span.ingested_at.unwrap_or(chrono::DateTime::UNIX_EPOCH));
        // Only terms: which fields the span indexed, an empty one too, and which it truncated, its row says
        // (`search_fields`, `search_truncated`).
        for field in &span.search.fields {
            for term in &field.terms {
                appender.append_row(params![
                    project_id,
                    span.trace_id.as_str(),
                    span.span_id.as_str(),
                    field.field.as_str(),
                    term.as_str(),
                    ingested_at,
                ])?;
            }
        }
    }
    appender.flush()?;
    Ok(())
}

/// Replace the term rows of these logs, the same way: the stored revisions' terms deleted, then the rows appended.
pub fn replace_log_terms(
    conn: &Connection,
    logs: &[NormalizedLog],
    stored: &HashMap<LogIdentity, Vec<i64>>,
) -> Result<(), DuckdbError> {
    if logs.is_empty() {
        return Ok(());
    }
    let logs = last_per_identity(logs, |log| {
        (
            log.project_id.as_deref().unwrap_or_default(),
            log.log_digest.as_str(),
            log.ordinal,
        )
    });
    let superseded: Vec<LogIdentity> = logs
        .iter()
        .map(|log| {
            (
                log.project_id.clone().unwrap_or_default(),
                log.log_digest.clone(),
                log.ordinal,
            )
        })
        .filter(|identity| stored.contains_key(identity))
        .collect();
    for chunk in superseded.chunks(DELETE_CHUNK) {
        let revisions: Vec<i64> = chunk
            .iter()
            .flat_map(|identity| stored[identity].iter().copied())
            .collect();
        if let Some(query) = search_sql::duckdb_log_term_delete_of_revisions(chunk, &revisions) {
            conn.execute(query.sql(), duckdb_values(query.params()).as_slice())?;
        }
    }
    let mut appender = conn.appender("log_terms")?;
    for log in &logs {
        let project_id = log.project_id.as_deref().unwrap_or_default();
        let ingested_at = SqlTimestamp(log.ingested_at.unwrap_or(log.timestamp));
        for field in &log.search.fields {
            for term in &field.terms {
                appender.append_row(params![
                    project_id,
                    log.log_digest.as_str(),
                    log.ordinal,
                    field.field.as_str(),
                    term.as_str(),
                    ingested_at,
                ])?;
            }
        }
    }
    appender.flush()?;
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
    let identities: Vec<(String, String, String)> = spans
        .iter()
        .map(|(trace_id, span_id)| (project_id.to_string(), trace_id.clone(), span_id.clone()))
        .collect();
    for chunk in identities.chunks(DELETE_CHUNK) {
        if let Some(query) = search_sql::duckdb_span_term_delete(chunk) {
            conn.execute(query.sql(), duckdb_values(query.params()).as_slice())?;
        }
    }
    let mut log_terms = conn.prepare(search_sql::DUCKDB_LOG_TERMS_DELETE_SPAN_SQL)?;
    for (trace_id, span_id) in spans {
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
    for document in documents {
        let SearchRecordId::Span { trace_id, span_id } = &document.id else {
            panic!("span search backfill received a log identity");
        };
        let expected_content_digest = document
            .expected_content_digest
            .as_deref()
            .expect("span search backfill requires its observed content digest");
        let still_current_and_unindexed: bool = conn.query_row(
            &search_sql::duckdb_span_backfill_cas_sql(),
            params![project_id, trace_id, span_id, expected_content_digest],
            |row| row.get(0),
        )?;
        if !still_current_and_unindexed {
            continue;
        }
        // The terms carry the winner's instant, as a write's do, so a later correction finds them.
        let identity: SpanIdentity = (project_id.to_string(), trace_id.clone(), span_id.clone());
        let revisions = keyed::span_revisions(conn, std::slice::from_ref(&identity))?;
        let Some(winner) = revisions
            .get(&identity)
            .and_then(|stored| {
                stored
                    .iter()
                    .find(|revision| revision.superseded_us.is_none())
            })
            .copied()
        else {
            continue;
        };
        if let Some(query) = search_sql::duckdb_span_term_delete_of_revisions(
            std::slice::from_ref(&identity),
            &[winner.ingested_us],
        ) {
            conn.execute(query.sql(), duckdb_values(query.params()).as_slice())?;
        }
        let ingested_at = SqlTimestamp(micros_to_datetime(winner.ingested_us));
        let mut appender = conn.appender("span_terms")?;
        write_document_terms(&document.document, |field, term| {
            appender.append_row(params![
                project_id,
                trace_id,
                span_id,
                field,
                term,
                ingested_at
            ])?;
            Ok(())
        })?;
        appender.flush()?;
        drop(appender);
        set_search_bits(
            conn,
            SearchSignal::Spans,
            winner.rowid,
            search_sql::duckdb_document_bits(SearchSignal::Spans, &document.document),
        )?;
    }
    Ok(())
}

fn write_log_backfill(
    conn: &Connection,
    project_id: &str,
    documents: &[SearchBackfillDocument],
) -> Result<(), DuckdbError> {
    for document in documents {
        let SearchRecordId::Log {
            log_digest,
            ordinal,
        } = &document.id
        else {
            panic!("log search backfill received a span identity");
        };
        let identity: LogIdentity = (project_id.to_string(), log_digest.clone(), *ordinal);
        let rows = keyed::log_rows(conn, &[(identity.clone(), None)])?;
        let revisions: Vec<i64> = rows
            .iter()
            .map(|(_, _, ingested_us)| *ingested_us)
            .collect();
        if let Some(query) = search_sql::duckdb_log_term_delete_of_revisions(
            std::slice::from_ref(&identity),
            &revisions,
        ) {
            conn.execute(query.sql(), duckdb_values(query.params()).as_slice())?;
        }
        // A log identity has one stored row: a redelivery replaces it.
        let Some(&(rowid, _, current_us)) =
            rows.iter().max_by_key(|(_, _, ingested_us)| *ingested_us)
        else {
            continue;
        };
        let ingested_at = SqlTimestamp(micros_to_datetime(current_us));
        let mut appender = conn.appender("log_terms")?;
        write_document_terms(&document.document, |field, term| {
            appender.append_row(params![
                project_id,
                log_digest,
                ordinal,
                field,
                term,
                ingested_at
            ])?;
            Ok(())
        })?;
        appender.flush()?;
        drop(appender);
        set_search_bits(
            conn,
            SearchSignal::Logs,
            rowid,
            search_sql::duckdb_document_bits(SearchSignal::Logs, &document.document),
        )?;
    }
    Ok(())
}

/// Every term of a document, by field. Which fields it indexed - an empty one too - and which it truncated, the
/// record's own row says (`set_search_bits`, and the writers' `search_fields`).
fn write_document_terms(
    document: &SearchDocument,
    mut write: impl FnMut(&str, &str) -> Result<(), DuckdbError>,
) -> Result<(), DuckdbError> {
    for field in &document.fields {
        for term in &field.terms {
            write(field.field.as_str(), term)?;
        }
    }
    Ok(())
}

/// Record on a backfilled row which fields it now indexes and which it truncated.
fn set_search_bits(
    conn: &Connection,
    signal: SearchSignal,
    rowid: i64,
    (fields, truncated): (u8, u8),
) -> Result<(), DuckdbError> {
    conn.execute(
        search_sql::duckdb_search_bits_update(signal),
        params![fields, truncated, rowid],
    )?;
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

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
