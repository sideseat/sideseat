use duckdb::{Connection, params};
use sideseat_ports::types::{
    LogSearchSource, SearchBackfillDocument, SearchBackfillSource, SearchCandidate, SearchCursor,
    SearchDocument, SearchLogRecord, SearchPage, SearchQuery, SearchRecord, SearchRecordId,
    SearchSignal, SearchSource, SearchSourceKey, SearchSpanRecord, SpanSearchSource,
};
use sideseat_query_sql::{Backend, analytics::QueryValue, search as search_sql};

use std::collections::HashMap;

use super::keyed::{self, LogIdentity, SpanIdentity};
use crate::error::DuckdbError;

pub fn search(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    match request.signal {
        SearchSignal::Spans | SearchSignal::Logs => search_page(conn, request),
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
                    raw_log: row.get(7)?,
                    noted_references: sideseat_ports::types::noted_references_from_column(
                        row.get::<_, Option<String>>(8)?.as_deref(),
                    ),
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
        // The terms go on the winner's row, with the bits that say which fields it indexed: a search reads them
        // there, and a later revision carries its own.
        let identity: SpanIdentity = (project_id.to_string(), trace_id.clone(), span_id.clone());
        let revisions = keyed::span_revisions(conn, std::slice::from_ref(&identity), i64::MAX)?;
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
        let terms = search_sql::duckdb_term_columns(SearchSignal::Spans, &document.document);
        let (fields, truncated) =
            search_sql::duckdb_document_bits(SearchSignal::Spans, &document.document);
        conn.execute(
            search_sql::duckdb_search_bits_update(SearchSignal::Spans),
            params![
                terms[0].as_deref(),
                terms[1].as_deref(),
                terms[2].as_deref(),
                terms[3].as_deref(),
                terms[4].as_deref(),
                terms[5].as_deref(),
                fields,
                truncated,
                winner.rowid
            ],
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
        let rows = keyed::log_rows(conn, &[(identity, None)])?;
        // A log identity has one stored row: a redelivery replaces it. Its terms are on the row, written with the
        // bits that say which fields it indexed.
        let Some(&(rowid, _, _)) = rows.iter().max_by_key(|(_, _, ingested_us)| *ingested_us)
        else {
            continue;
        };
        let terms = search_sql::duckdb_term_columns(SearchSignal::Logs, &document.document);
        let (fields, truncated) =
            search_sql::duckdb_document_bits(SearchSignal::Logs, &document.document);
        conn.execute(
            search_sql::duckdb_search_bits_update(SearchSignal::Logs),
            params![
                terms[0].as_deref(),
                terms[1].as_deref(),
                terms[2].as_deref(),
                terms[3].as_deref(),
                fields,
                truncated,
                rowid
            ],
        )?;
    }
    Ok(())
}

/// One page of light candidates: identity, short columns, row id and text length, in page order.
fn search_page(conn: &Connection, request: &SearchQuery) -> Result<SearchPage, DuckdbError> {
    let started_at_us = traversal_watermark(conn, request)?;
    let search_indexing_complete = indexing_complete(conn, request)?;
    let plan = search_sql::candidates(request, Backend::Duckdb);
    let values = duckdb_values(plan.query.params());
    let mut statement = conn.prepare(plan.query.sql())?;
    let mut result = statement.query(values.as_slice())?;
    let mut candidates = Vec::new();
    let mut limit_reached = false;
    while let Some(row) = result.next()? {
        if candidates.len() == request.max_examined as usize {
            limit_reached = true;
            break;
        }
        // row_id, four light columns, revision_us, content_digest, source_bytes, timestamp_us, match_state,
        // search_indexed.
        let row_id: i64 = row.get(0)?;
        let revision_us: i64 = row.get(5)?;
        let content_digest: String = row.get(6)?;
        let source_bytes: u64 = row.get(7)?;
        let timestamp_us: i64 = row.get(8)?;
        let indexed: bool = row.get(10)?;
        let timestamp = chrono::DateTime::from_timestamp_micros(timestamp_us)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        let (record, id, tie_breaker, ordinal) = match request.signal {
            SearchSignal::Spans => {
                let (trace_id, span_id): (String, String) = (row.get(1)?, row.get(2)?);
                (
                    SearchRecord::Span(SearchSpanRecord {
                        trace_id: trace_id.clone(),
                        span_id: span_id.clone(),
                        timestamp,
                        span_name: None,
                        input_preview: row.get(3)?,
                        output_preview: row.get(4)?,
                    }),
                    SearchRecordId::Span {
                        trace_id: trace_id.clone(),
                        span_id: span_id.clone(),
                    },
                    format!("{trace_id}\0{span_id}"),
                    0,
                )
            }
            SearchSignal::Logs => {
                let (log_digest, ordinal): (String, u32) = (row.get(1)?, row.get(2)?);
                (
                    SearchRecord::Log(SearchLogRecord {
                        log_digest: log_digest.clone(),
                        ordinal,
                        timestamp,
                        severity_text: None,
                        body_text: None,
                        trace_id: row.get(3)?,
                        span_id: row.get(4)?,
                    }),
                    SearchRecordId::Log {
                        log_digest: log_digest.clone(),
                        ordinal,
                    },
                    log_digest,
                    ordinal,
                )
            }
        };
        candidates.push(SearchCandidate {
            record,
            document: SearchDocument {
                indexed,
                fields: Vec::new(),
            },
            key: SearchSourceKey {
                id,
                row_id: Some(row_id),
                timestamp_us,
                revision_us,
                content_digest,
            },
            source_bytes,
            cursor: SearchCursor {
                timestamp_us,
                tie_breaker,
                ordinal,
                started_at_us,
            },
            indeterminate: false,
        });
    }
    let next_cursor = candidates.last().map(|candidate| candidate.cursor.clone());
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

/// The source text of `keys`, in order: a key's row read by row id, and kept only while it is still that
/// revision of that identity and its winner in the project - a row deleted, superseded, or its number reused for
/// another row since, answers `None`.
pub fn sources(
    conn: &Connection,
    request: &SearchQuery,
    keys: &[SearchSourceKey],
) -> Result<Vec<Option<SearchSource>>, DuckdbError> {
    let Some(query) = search_sql::sources(
        request.project_id.as_str(),
        request.signal,
        keys,
        Backend::Duckdb,
    ) else {
        return Ok(vec![None; keys.len()]);
    };
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let mut rows = statement.query(values.as_slice())?;
    let mut found: HashMap<i64, (SearchRecordId, i64, String, SearchSource)> = HashMap::new();
    while let Some(row) = rows.next()? {
        let row_id: i64 = row.get(0)?;
        let revision_us: i64 = row.get(3)?;
        let content_digest: String = row.get(4)?;
        let (id, source) = match request.signal {
            SearchSignal::Spans => (
                SearchRecordId::Span {
                    trace_id: row.get(1)?,
                    span_id: row.get(2)?,
                },
                SearchSource::Span(SpanSearchSource {
                    messages: row.get(5)?,
                    tool_definitions: row.get(6)?,
                    tool_names: row.get(7)?,
                    input_preview: row.get(8)?,
                    output_preview: row.get(9)?,
                    gen_ai_tool_name: row.get(10)?,
                    status_message: row.get(11)?,
                    exception_type: row.get(12)?,
                    exception_message: row.get(13)?,
                    exception_stacktrace: row.get(14)?,
                    span_name: row.get(15)?,
                }),
            ),
            SearchSignal::Logs => (
                SearchRecordId::Log {
                    log_digest: row.get(1)?,
                    ordinal: row.get(2)?,
                },
                SearchSource::Log(LogSearchSource {
                    body_text: row.get(5)?,
                    body: row.get(6)?,
                    event_name: row.get(7)?,
                    severity_text: row.get(8)?,
                    attributes: row.get(9)?,
                    raw_log: row.get(10)?,
                    noted_references: sideseat_ports::types::noted_references_from_column(
                        row.get::<_, Option<String>>(11)?.as_deref(),
                    ),
                }),
            ),
        };
        found.insert(row_id, (id, revision_us, content_digest, source));
    }
    Ok(keys
        .iter()
        .map(|key| {
            let (id, revision_us, content_digest, source) = found.get(&key.row_id?)?;
            (*id == key.id
                && *revision_us == key.revision_us
                && *content_digest == key.content_digest)
                .then(|| source.clone())
        })
        .collect())
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
