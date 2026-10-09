//! Stored rows looked up through an identity index, and the deletes a write makes by their row ids.
//!
//! A write replaces what it supersedes: a correction's span terms, a redelivered log or datapoint. DuckDB
//! deletes through the same scans it reads with, so a delete filtered by identity read every row of the table;
//! these find the rows through the identity's index instead ([`sideseat_query_sql::keyed`]) and delete them by
//! row id. Rows are found and deleted inside the writer's transaction on the one connection, so a row id read is
//! still that row when it is deleted.

use std::collections::HashMap;

use duckdb::Connection;
use sideseat_query_sql::keyed::{KEYED_CHUNK, distinct_keys, duckdb_keyed};

use crate::DuckdbError;

/// A span's identity: project, trace id, span id.
pub(crate) type SpanIdentity = (String, String, String);

/// A log record's identity: project, digest, ordinal.
pub(crate) type LogIdentity = (String, String, u32);

/// A stored revision of a span identity: its row, its ingest instant and the instant of the revision that
/// follows it, both in epoch microseconds (`sideseat_query_sql::winners`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredRevision {
    pub rowid: i64,
    pub ingested_us: i64,
    pub superseded_us: Option<i64>,
}

/// The stored revisions of these span identities that a write of revisions ingested at or after `since_us` can
/// change, read through the `span_id` index: every revision followed at or after that instant, or by nothing -
/// the winner.
///
/// That is all `supersession::plan` needs, not the whole history: a new revision lands after the stored ones
/// before it, so only a revision whose follower is at or after the earliest new instant can change the instant
/// it is superseded at, and only those after it can be what the new one is superseded at. Every older revision
/// keeps its follower. So a write reads what it can change - the winner, for a revision that arrives in order -
/// however often the identity was corrected before. `i64::MAX` reads the winner alone.
pub(crate) fn span_revisions(
    conn: &Connection,
    identities: &[SpanIdentity],
    since_us: i64,
) -> Result<HashMap<SpanIdentity, Vec<StoredRevision>>, DuckdbError> {
    let mut revisions: HashMap<SpanIdentity, Vec<StoredRevision>> = HashMap::new();
    // A row read twice - its identity in two chunks - is kept once, by row id, in constant time.
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for chunk in identities.chunks(KEYED_CHUNK) {
        let keys = distinct_keys(chunk.iter().map(|(_, _, span_id)| span_id.as_str()));
        let keyed = duckdb_keyed(
            "otel_spans",
            "span_id",
            "project_id, trace_id, span_id, ingested_at, superseded_at",
            keys.len(),
        );
        let sql = format!(
            "SELECT keyed_rowid, project_id, trace_id, span_id, epoch_us(ingested_at), \
             epoch_us(superseded_at) FROM {keyed} WHERE (project_id, trace_id, span_id) IN ({}) \
             AND (superseded_at IS NULL OR epoch_us(superseded_at) >= ?::BIGINT)",
            triples(chunk.len())
        );
        let mut values: Vec<duckdb::types::Value> = keys
            .iter()
            .map(|key| duckdb::types::Value::Text((*key).to_string()))
            .collect();
        for (project_id, trace_id, span_id) in chunk {
            values.push(duckdb::types::Value::Text(project_id.clone()));
            values.push(duckdb::types::Value::Text(trace_id.clone()));
            values.push(duckdb::types::Value::Text(span_id.clone()));
        }
        values.push(duckdb::types::Value::BigInt(since_us));
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(duckdb::params_from_iter(values), |row| {
            Ok((
                (row.get(1)?, row.get(2)?, row.get(3)?),
                StoredRevision {
                    rowid: row.get(0)?,
                    ingested_us: row.get(4)?,
                    superseded_us: row.get(5)?,
                },
            ))
        })?;
        for row in rows {
            let (identity, revision) = row?;
            if seen.insert(revision.rowid) {
                revisions.entry(identity).or_default().push(revision);
            }
        }
    }
    Ok(revisions)
}

/// The ingest instant of each identity's current winner, the one revision with search terms: the latest, so the
/// greatest instant whatever order the revisions were stored in.
pub(crate) fn winner_instants(
    revisions: &HashMap<SpanIdentity, Vec<StoredRevision>>,
) -> HashMap<SpanIdentity, i64> {
    revisions
        .iter()
        .filter_map(|(identity, stored)| {
            stored
                .iter()
                .map(|revision| revision.ingested_us)
                .max()
                .map(|instant| (identity.clone(), instant))
        })
        .collect()
}

/// Set `superseded_at` on these stored rows, by row id: the stored revisions a write's new revisions follow.
///
/// Grouped by value, so the usual write - one correction per identity, all at the batch's own instant - is one
/// statement per chunk of rows, answered from their row groups like [`delete_rows`].
pub(crate) fn supersede_rows(
    conn: &Connection,
    updates: &[(i64, Option<i64>)],
) -> Result<usize, DuckdbError> {
    let mut by_value: std::collections::BTreeMap<Option<i64>, Vec<i64>> = Default::default();
    for (rowid, superseded_us) in updates {
        by_value.entry(*superseded_us).or_default().push(*rowid);
    }
    let mut updated = 0;
    for (superseded_us, rowids) in by_value {
        let value = superseded_us
            .map(|us| format!("make_timestamp({us}::BIGINT)"))
            .unwrap_or_else(|| "NULL".to_string());
        for chunk in rowids.chunks(KEYED_CHUNK) {
            let list = chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            updated += conn.execute(
                &format!("UPDATE otel_spans SET superseded_at = {value} WHERE rowid IN ({list})"),
                [],
            )?;
        }
    }
    Ok(updated)
}

/// The stored rows of these log identities: row id, identity, ingest instant in epoch microseconds.
///
/// Each identity comes with the record's own instant in epoch microseconds, when it has one: a log record's
/// instant is part of its digest, so its stored row carries it, and a read conditioned on the instants reads only
/// the row groups whose zone maps hold one (`sideseat_query_sql::confirmations::instants_condition`) - no index,
/// which on `log_digest` measured 68 bytes per record. An identity without one - a record with neither time nor
/// observed time, or the search backfill, which does not know it - is read unconditioned.
pub(crate) fn log_rows(
    conn: &Connection,
    identities: &[(LogIdentity, Option<i64>)],
) -> Result<Vec<(i64, LogIdentity, i64)>, DuckdbError> {
    let (bounded, unbounded): (Vec<_>, Vec<_>) = identities
        .iter()
        .partition(|(_, instant)| instant.is_some());
    let mut found = Vec::new();
    for chunk in bounded
        .chunks(KEYED_CHUNK)
        .chain(unbounded.chunks(KEYED_CHUNK))
    {
        let instants = sideseat_query_sql::confirmations::distinct_instants(
            chunk
                .iter()
                .filter_map(|(_, instant)| *instant)
                .filter_map(chrono::DateTime::from_timestamp_micros),
        );
        let (condition, condition_values) = match &instants {
            Some(instants) => {
                let (condition, values) =
                    sideseat_query_sql::confirmations::instants_condition(instants);
                (format!("{condition} AND "), values)
            }
            None => (String::new(), Vec::new()),
        };
        let sql = format!(
            "SELECT rowid, project_id, log_digest, ordinal, epoch_us(ingested_at) FROM otel_logs \
             WHERE {condition}(project_id, log_digest, ordinal) IN ({})",
            triples(chunk.len())
        );
        let mut values: Vec<duckdb::types::Value> = condition_values
            .into_iter()
            .map(|value| match value {
                sideseat_query_sql::analytics::QueryValue::Int64(number) => {
                    duckdb::types::Value::BigInt(number)
                }
                other => unreachable!("an instant condition binds integers, not {other:?}"),
            })
            .collect();
        for ((project_id, digest, ordinal), _) in chunk {
            values.push(duckdb::types::Value::Text(project_id.clone()));
            values.push(duckdb::types::Value::Text(digest.clone()));
            values.push(duckdb::types::Value::UInt(*ordinal));
        }
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(duckdb::params_from_iter(values), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?),
                row.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            found.push(row?);
        }
    }
    Ok(found)
}

/// Delete these rows of `table` by row id.
///
/// A list of row ids is answered from the row groups that hold them - measured on DuckDB 1.5, 3.6 ms over a
/// million rows and 4.3 ms over ten million - where a delete filtered by identity reads the whole table.
pub(crate) fn delete_rows(
    conn: &Connection,
    table: &str,
    rowids: &[i64],
) -> Result<usize, DuckdbError> {
    let mut deleted = 0;
    for chunk in rowids.chunks(KEYED_CHUNK) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        deleted += conn.execute(&format!("DELETE FROM {table} WHERE rowid IN ({list})"), [])?;
    }
    Ok(deleted)
}

fn triples(count: usize) -> String {
    std::iter::repeat_n("(?, ?, ?)", count)
        .collect::<Vec<_>>()
        .join(", ")
}
