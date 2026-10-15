//! Stored raw records (`otel_raw`) and their reconciliation queue (`otel_raw_pending`).
//!
//! Append-only: each write of a record is a version, and readers take the latest by `version`, then by
//! insertion - the same answer ClickHouse's `ReplacingMergeTree(version)` gives.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use duckdb::{Connection, params};

use crate::DuckdbError;
use crate::sql_types::SqlTimestamp;
use sideseat_core::utils::time::micros_to_datetime;
use sideseat_ports::types::{
    ProjectId, RAW_VERSION_EPOCH_SHIFT, RawOrigin, RawPending, RawRecordRow, RawRewrite,
    StagedSignal,
};
use sideseat_query_sql::Backend;
use sideseat_query_sql::analytics;
use sideseat_query_sql::dml;
use sideseat_query_sql::keyed::{KEYED_CHUNK, distinct_keys, duckdb_keyed};

use sideseat_query_sql::analytics::QueryValue;

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

const COLUMNS: &str = "raw_id, signal, EPOCH_US(received_at), origin, version_key, EPOCH_US(signal_until), \
                       EPOCH_US(hold_until), record";

/// Whether the records being appended were found to have no version stored in their project.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// None of them has a version: by the trace index's invariant (`index_traces`) none has a trace row either.
    Absent,
    /// Some may: a repair or a rewrite appends a further version of a record that is there.
    Unknown,
}

fn append(conn: &Connection, records: &[&RawRecordRow], stored: Stored) -> Result<(), DuckdbError> {
    let mut appender = conn.appender("otel_raw")?;
    for record in records {
        appender.append_row(params![
            record.project_id.as_str(),
            record.raw_id.as_str(),
            record.signal.as_str(),
            SqlTimestamp(record.received_at),
            record.origin.as_str(),
            record.version,
            SqlTimestamp(record.signal_until),
            record.hold_until.map(SqlTimestamp),
            record.record.as_slice(),
        ])?;
    }
    appender.flush()?;
    drop(appender);
    index_traces(conn, records, stored)
}

/// Add the trace-index rows these versions need and the index does not have.
///
/// **The invariant the write rests on: a trace row of a record exists only while some version of that record does,
/// in its project.** Every writer adds trace rows only beside a version - here, after appending it -
/// and every remover of versions removes the rows with the last of them, in its transaction: the collection purge
/// (`purge_versions`) and the supersede purge (`delete_superseded`) once no version remains, and a project's
/// deletion with all of both. So records the insert
/// found with no version (`Stored::Absent`) have no trace rows, and theirs are appended without reading the index;
/// only a further version of a stored record reads which rows it already has.
fn index_traces(
    conn: &Connection,
    records: &[&RawRecordRow],
    stored: Stored,
) -> Result<(), DuckdbError> {
    let raw_ids = distinct_keys(records.iter().map(|record| record.raw_id.as_str()));
    let mut present: HashSet<(String, String, String)> = HashSet::new();
    let reads = if stored == Stored::Absent {
        &[][..]
    } else {
        &raw_ids[..]
    };
    for chunk in reads.chunks(KEYED_CHUNK) {
        let keyed = duckdb_keyed(
            "otel_raw_traces",
            "raw_id",
            "project_id, trace_id, raw_id",
            chunk.len(),
        );
        let mut statement =
            conn.prepare(&format!("SELECT project_id, trace_id, raw_id FROM {keyed}"))?;
        let rows = statement.query_map(duckdb::params_from_iter(chunk.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
            present.insert(row?);
        }
    }
    let mut wanted = Vec::new();
    let mut seen = HashSet::new();
    for record in records {
        for trace_id in &record.trace_ids {
            let key = (
                record.project_id.to_string(),
                trace_id.clone(),
                record.raw_id.clone(),
            );
            if !present.contains(&key) && seen.insert(key) {
                wanted.push((*record, trace_id));
            }
        }
    }
    let mut appender = conn.appender("otel_raw_traces")?;
    for (record, trace_id) in wanted {
        appender.append_row(params![
            record.project_id.as_str(),
            trace_id.as_str(),
            record.raw_id.as_str(),
            SqlTimestamp(record.signal_until),
            record.hold_until.map(SqlTimestamp),
        ])?;
    }
    appender.flush()?;
    Ok(())
}

/// Store the records not already stored: a redelivered body, a redrive and a re-derivation all present a
/// `raw_id` the table has, and a second row for it would be a second copy of raw content.
///
/// In one transaction: a version is never stored without its trace rows, which a crash between the two appends
/// left behind - and which a redelivery, finding the record stored, never wrote - and the write commits once.
pub fn insert(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    if records.is_empty() {
        return Ok(());
    }
    crate::in_transaction(conn, |conn| {
        let mut present = HashSet::new();
        let raw_ids = distinct_keys(records.iter().map(|record| record.raw_id.as_str()));
        for chunk in raw_ids.chunks(KEYED_CHUNK) {
            let keyed = duckdb_keyed("otel_raw", "raw_id", "project_id, raw_id", chunk.len());
            let mut statement =
                conn.prepare(&format!("SELECT DISTINCT project_id, raw_id FROM {keyed}"))?;
            let rows = statement.query_map(duckdb::params_from_iter(chunk.iter()), |row| {
                Ok((
                    ProjectId::from(row.get::<_, String>(0)?.as_str()),
                    row.get::<_, String>(1)?,
                ))
            })?;
            for row in rows {
                present.insert(row?);
            }
        }
        let fresh: Vec<&RawRecordRow> = records
            .iter()
            .filter(|record| present.insert((record.project_id.clone(), record.raw_id.clone())))
            .collect();
        append(conn, &fresh, Stored::Absent)
    })
}

/// Store these versions whatever is already there.
pub fn append_versions(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    append(conn, &records.iter().collect::<Vec<_>>(), Stored::Unknown)
}

/// Store these versions and queue their records for the reconciler, in one transaction.
pub fn append_queued(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    crate::in_transaction(conn, |conn| {
        append_versions(conn, records)?;
        let mut by_project: BTreeMap<&ProjectId, Vec<String>> = BTreeMap::new();
        for record in records {
            by_project
                .entry(&record.project_id)
                .or_default()
                .push(record.raw_id.clone());
        }
        for (project_id, raw_ids) in by_project {
            enqueue(conn, project_id, &raw_ids)?;
        }
        Ok(())
    })
}

fn row(project_id: &ProjectId, row: &duckdb::Row<'_>) -> duckdb::Result<RawRecordRow> {
    let signal: String = row.get(1)?;
    let origin: String = row.get(3)?;
    let hold: Option<i64> = row.get(6)?;
    Ok(RawRecordRow {
        project_id: project_id.clone(),
        raw_id: row.get(0)?,
        signal: StagedSignal::from_stored(&signal).unwrap_or(StagedSignal::Traces),
        received_at: micros_to_datetime(row.get(2)?),
        origin: RawOrigin::from_stored(&origin).unwrap_or(RawOrigin::Deleted),
        version: row.get(4)?,
        signal_until: micros_to_datetime(row.get(5)?),
        hold_until: hold.map(micros_to_datetime),
        trace_ids: Vec::new(),
        record: row.get(7)?,
    })
}

/// The latest version of each requested record that exists.
pub fn get(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<Vec<RawRecordRow>, DuckdbError> {
    if raw_ids.is_empty() {
        return Ok(Vec::new());
    }
    // Read by id through its index, then the project kept and the latest version chosen: the choice is per id.
    // The keys are sorted, so the chunks come back in `raw_id` order too.
    let keys = distinct_keys(raw_ids.iter().map(String::as_str));
    let mut records = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(KEYED_CHUNK) {
        let keyed = duckdb_keyed("otel_raw", "raw_id", "*", chunk.len());
        let sql = format!(
            "SELECT {COLUMNS} FROM (SELECT * FROM {keyed} WHERE project_id = ? \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY version_key DESC, keyed_rowid DESC) = 1) \
             ORDER BY raw_id"
        );
        let values: Vec<duckdb::types::Value> = chunk
            .iter()
            .map(|key| duckdb::types::Value::Text((*key).to_string()))
            .chain(std::iter::once(duckdb::types::Value::Text(
                project_id.to_string(),
            )))
            .collect();
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(duckdb::params_from_iter(values), |r| row(project_id, r))?;
        for row in rows {
            records.push(row?);
        }
    }
    Ok(records)
}

/// One page of latest versions in `(received_at, raw_id)` order: the order a re-derivation replays.
pub fn page(
    conn: &Connection,
    project_id: &ProjectId,
    after: Option<(DateTime<Utc>, String)>,
    limit: usize,
) -> Result<Vec<RawRecordRow>, DuckdbError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let (after_us, after_id) = after
        .map(|(at, id)| (at.timestamp_micros(), id))
        .unwrap_or((i64::MIN, String::new()));
    let sql = format!(
        "SELECT {COLUMNS} FROM (SELECT *, rowid AS position FROM otel_raw WHERE project_id = ? \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY version_key DESC, rowid DESC) = 1) \
         WHERE (EPOCH_US(received_at), raw_id) > (?, ?) \
         ORDER BY received_at, raw_id LIMIT ?"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![project_id.as_str(), after_us, after_id, limit],
        |r| row(project_id, r),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Row ids of a record's rows in `table`, found through its `raw_id` index.
fn record_rowids(
    conn: &Connection,
    table: &str,
    project_id: &ProjectId,
    raw_id: &str,
    extra: &str,
    extra_value: Option<i64>,
) -> Result<Vec<i64>, DuckdbError> {
    let keyed = duckdb_keyed(table, "raw_id", "*", 1);
    let mut statement = conn.prepare(&format!(
        "SELECT keyed_rowid FROM {keyed} WHERE project_id = ? {extra}"
    ))?;
    let mut values = vec![
        duckdb::types::Value::Text(raw_id.to_string()),
        duckdb::types::Value::Text(project_id.to_string()),
    ];
    if let Some(value) = extra_value {
        values.push(duckdb::types::Value::BigInt(value));
    }
    let rows = statement.query_map(duckdb::params_from_iter(values), |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The collection purge (`RPurge`): the record's versions keyed at or below `up_to_epoch`, and its trace-index
/// rows once no version remains - in one transaction, which the trace index's invariant needs (`index_traces`). DuckDB has no time-to-live to take the index rows later,
/// and every statement runs on the one connection, so a version landing after the purge commits after it and
/// indexes its traces afresh: nothing it needs can be taken with the purged ones.
pub fn purge_versions(
    conn: &Connection,
    project_id: &ProjectId,
    raw_id: &str,
    up_to_epoch: i64,
) -> Result<(), DuckdbError> {
    crate::in_transaction(conn, |conn| {
        let doomed = record_rowids(
            conn,
            "otel_raw",
            project_id,
            raw_id,
            &format!("AND (version_key >> {RAW_VERSION_EPOCH_SHIFT}) <= ?"),
            Some(up_to_epoch),
        )?;
        super::keyed::delete_rows(conn, "otel_raw", &doomed)?;
        if record_rowids(conn, "otel_raw", project_id, raw_id, "", None)?.is_empty() {
            let index = record_rowids(conn, "otel_raw_traces", project_id, raw_id, "", None)?;
            super::keyed::delete_rows(conn, "otel_raw_traces", &index)?;
        }
        Ok(())
    })
}

/// The supersede purge: the record's versions strictly below `below_version`.
pub fn delete_superseded(
    conn: &Connection,
    project_id: &ProjectId,
    raw_id: &str,
    below_version: i64,
) -> Result<(), DuckdbError> {
    crate::in_transaction(conn, |conn| {
        // Below the key read, and at that key every version but the last stored: two rewrites can key their
        // versions alike, and readers take the last of equal keys (`get`), so the earlier one is superseded too -
        // left in place, it kept whatever deleted content the later rewrite removed, with no key below the latest
        // to find it by.
        let keyed = duckdb_keyed("otel_raw", "raw_id", "project_id, version_key", 1);
        let mut statement = conn.prepare(&format!(
            "SELECT keyed_rowid, version_key FROM {keyed} WHERE project_id = ? AND version_key <= ?"
        ))?;
        let versions: Vec<(i64, i64)> = statement
            .query_map(params![raw_id, project_id.as_str(), below_version], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let last_at_key = versions
            .iter()
            .filter(|(_, key)| *key == below_version)
            .map(|(rowid, _)| *rowid)
            .max();
        let superseded: Vec<i64> = versions
            .into_iter()
            .filter(|(rowid, key)| *key < below_version || Some(*rowid) != last_at_key)
            .map(|(rowid, _)| rowid)
            .collect();
        super::keyed::delete_rows(conn, "otel_raw", &superseded)?;
        // The version at the key read can be gone - collected since it was read - so this can take the last
        // version, and its trace rows go with it, as the purge takes them (`index_traces`).
        if !superseded.is_empty()
            && record_rowids(conn, "otel_raw", project_id, raw_id, "", None)?.is_empty()
        {
            let index = record_rowids(conn, "otel_raw_traces", project_id, raw_id, "", None)?;
            super::keyed::delete_rows(conn, "otel_raw_traces", &index)?;
        }
        Ok(())
    })
}

/// The traces the trace index holds under one record, through its `raw_id` index (`idx_raw_traces_raw`).
pub fn traces_indexed_under(
    conn: &Connection,
    project_id: &ProjectId,
    raw_id: &str,
) -> Result<Vec<String>, DuckdbError> {
    let keyed = duckdb_keyed("otel_raw_traces", "raw_id", "project_id, trace_id", 1);
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT trace_id FROM {keyed} WHERE project_id = ? ORDER BY trace_id"
    ))?;
    let rows = statement.query_map(params![raw_id, project_id.as_str()], |r| {
        r.get::<_, String>(0)
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The records the trace index holds under these traces.
pub fn indexed_under_traces(
    conn: &Connection,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<String>, DuckdbError> {
    let mut records = std::collections::BTreeSet::new();
    for chunk in trace_ids.chunks(KEYED_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let mut statement = conn.prepare(&format!(
            "SELECT DISTINCT raw_id FROM otel_raw_traces WHERE project_id = ? AND trace_id IN ({placeholders})"
        ))?;
        let values: Vec<duckdb::types::Value> = std::iter::once(project_id.to_string())
            .chain(chunk.iter().cloned())
            .map(duckdb::types::Value::Text)
            .collect();
        let rows =
            statement.query_map(duckdb::params_from_iter(values), |r| r.get::<_, String>(0))?;
        for row in rows {
            records.insert(row?);
        }
    }
    Ok(records.into_iter().collect())
}

/// The `(raw_id, trace_id)` pairs these traces' span rows still carry, through the span trace index.
pub fn named_in_traces(
    conn: &Connection,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<HashSet<(String, String)>, DuckdbError> {
    let keys = distinct_keys(trace_ids.iter().map(String::as_str));
    let mut named = HashSet::new();
    for chunk in keys.chunks(KEYED_CHUNK) {
        let keyed = duckdb_keyed(
            "otel_spans",
            "trace_id",
            "project_id, trace_id, raw_id",
            chunk.len(),
        );
        let values: Vec<duckdb::types::Value> = chunk
            .iter()
            .map(|key| duckdb::types::Value::Text((*key).to_string()))
            .chain(std::iter::once(duckdb::types::Value::Text(
                project_id.to_string(),
            )))
            .collect();
        let mut statement = conn.prepare(&format!(
            "SELECT DISTINCT raw_id, trace_id FROM {keyed} WHERE project_id = ? AND raw_id IS NOT NULL"
        ))?;
        let rows = statement.query_map(duckdb::params_from_iter(values), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            named.insert(row?);
        }
    }
    Ok(named)
}

/// Append these rewritten versions of the records still stored, and queue those appended, in one transaction: a
/// record collected since its rewrite was read is not stored again, and one stored again is looked at again
/// whatever becomes of the caller. Returns the records appended.
pub fn append_rewrites(
    conn: &Connection,
    rewrites: &[RawRewrite],
) -> Result<HashSet<String>, DuckdbError> {
    crate::in_transaction(conn, |conn| {
        let mut appended = HashSet::new();
        // One at a time, each judged against the latest as the ones before it left it: two rewrites of one record
        // in one call cannot both pass against one read.
        for rewrite in rewrites {
            let row = &rewrite.row;
            // The record's latest version, as readers take it. A rewrite lands only while the version it was
            // prepared from is still the latest: once another has landed over it - a newer rewrite, a repair, a
            // restore - this one, prepared from older content, would bring back spans and media the newer version
            // no longer holds, whose files a reconciliation may already have released. Whatever landed queued the
            // record as it did, so the pass it leads to rewrites what is left from current state. A record no
            // longer stored was collected, and stays so.
            let latest = get(conn, &row.project_id, std::slice::from_ref(&row.raw_id))?;
            if !latest
                .first()
                .is_some_and(|latest| rewrite.based_on_latest(latest))
            {
                continue;
            }
            append(conn, &[row], Stored::Unknown)?;
            enqueue(conn, &row.project_id, std::slice::from_ref(&row.raw_id))?;
            appended.insert(row.raw_id.clone());
        }
        Ok(appended)
    })
}

/// Enqueue these records for reconciliation.
pub fn enqueue(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<(), DuckdbError> {
    let Some(statement) =
        dml::raw::enqueue_raw_records(Backend::Duckdb, project_id.as_str(), raw_ids)
    else {
        return Ok(());
    };
    conn.execute(
        statement.sql(),
        duckdb_values(statement.params()).as_slice(),
    )?;
    Ok(())
}

/// Up to `limit` entries enqueued before the landing horizon, oldest first.
pub fn pending(
    conn: &Connection,
    limit: usize,
    enqueued_before: Option<DateTime<Utc>>,
) -> Result<Vec<RawPending>, DuckdbError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT project_id, raw_id, token FROM otel_raw_pending \
         WHERE ? IS NULL OR enqueued_at < CAST(? AS TIMESTAMP) \
         ORDER BY enqueued_at, project_id, raw_id, token LIMIT ?",
    )?;
    let before = enqueued_before.map(SqlTimestamp);
    let rows = statement.query_map(params![before, before, limit], |r| {
        Ok(RawPending {
            project_id: ProjectId::from(r.get::<_, String>(0)?.as_str()),
            raw_id: r.get(1)?,
            token: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Remove exactly these entries; one enqueued since they were read stays.
pub fn clear(conn: &Connection, entries: &[RawPending]) -> Result<(), DuckdbError> {
    let mut by_project: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for entry in entries {
        by_project
            .entry(entry.project_id.as_str())
            .or_default()
            .push((entry.raw_id.clone(), entry.token.clone()));
    }
    for (project_id, entries) in by_project {
        if let Some(statement) = dml::raw::clear_raw_pending(
            dml::MutationTarget::duckdb("otel_raw_pending"),
            project_id,
            &entries,
        ) {
            conn.execute(
                statement.sql(),
                duckdb_values(statement.params()).as_slice(),
            )?;
        }
    }
    Ok(())
}

/// The record each of these spans was derived from, for the winning row of each identity.
pub fn span_raw_ids(
    conn: &Connection,
    project_id: &ProjectId,
    spans: &[(String, String)],
) -> Result<HashMap<(String, String), String>, DuckdbError> {
    let mut named = HashMap::with_capacity(spans.len());
    for chunk in spans.chunks(KEYED_CHUNK) {
        let Some(query) = analytics::span_raw_ids(project_id.as_str(), chunk, Backend::Duckdb)
        else {
            continue;
        };
        let mut statement = conn.prepare(query.sql())?;
        let rows = statement.query_map(duckdb_values(query.params()).as_slice(), |row| {
            Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
        })?;
        for row in rows {
            let (identity, raw_id) = row?;
            named.insert(identity, raw_id);
        }
    }
    Ok(named)
}

/// The latest records the surviving winning spans of these traces name.
pub fn survivor_records(
    conn: &Connection,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<Vec<u8>>, DuckdbError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    // The traces' rows through the trace index, then their records through the record index (`keyed`).
    let keys = distinct_keys(trace_ids.iter().map(String::as_str));
    let mut raw_ids = HashSet::new();
    for chunk in keys.chunks(KEYED_CHUNK) {
        let keyed = duckdb_keyed("otel_spans", "trace_id", "project_id, raw_id", chunk.len());
        let values: Vec<duckdb::types::Value> = chunk
            .iter()
            .map(|key| duckdb::types::Value::Text((*key).to_string()))
            .chain(std::iter::once(duckdb::types::Value::Text(
                project_id.to_string(),
            )))
            .collect();
        let mut statement = conn.prepare(&format!(
            "SELECT DISTINCT raw_id FROM {keyed} WHERE project_id = ? AND raw_id IS NOT NULL"
        ))?;
        let rows =
            statement.query_map(duckdb::params_from_iter(values), |r| r.get::<_, String>(0))?;
        for row in rows {
            raw_ids.insert(row?);
        }
    }
    let raw_ids: Vec<String> = raw_ids.into_iter().collect();
    Ok(get(conn, project_id, &raw_ids)?
        .into_iter()
        .map(|record| record.record)
        .collect())
}
