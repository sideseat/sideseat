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
use sideseat_ports::types::{ProjectId, RawOrigin, RawPending, RawRecordRow, StagedSignal};
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

const COLUMNS: &str = "raw_id, signal, EPOCH_US(received_at), origin, version, EPOCH_US(signal_until), \
                       EPOCH_US(hold_until), record";

fn append(conn: &Connection, records: &[&RawRecordRow]) -> Result<(), DuckdbError> {
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
    index_traces(conn, records)
}

/// Add the trace-index rows these versions need and the index does not have.
fn index_traces(conn: &Connection, records: &[&RawRecordRow]) -> Result<(), DuckdbError> {
    let raw_ids = distinct_keys(records.iter().map(|record| record.raw_id.as_str()));
    let mut present: HashSet<(String, String, String)> = HashSet::new();
    for chunk in raw_ids.chunks(KEYED_CHUNK) {
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
pub fn insert(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    if records.is_empty() {
        return Ok(());
    }
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
    append(conn, &fresh)
}

/// Store these versions whatever is already there.
pub fn append_versions(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    append(conn, &records.iter().collect::<Vec<_>>())
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

fn text_values(project_id: &ProjectId, ids: &[String]) -> Vec<duckdb::types::Value> {
    std::iter::once(project_id.to_string())
        .chain(ids.iter().cloned())
        .map(duckdb::types::Value::Text)
        .collect()
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
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
             QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY version DESC, keyed_rowid DESC) = 1) \
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
             QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY version DESC, rowid DESC) = 1) \
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

/// Delete every version of these records, and their trace-index rows.
pub fn delete(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<(), DuckdbError> {
    // Found by id through each table's index and deleted by row id (`keyed`).
    let keys = distinct_keys(raw_ids.iter().map(String::as_str));
    for table in ["otel_raw", "otel_raw_traces"] {
        let mut rowids = Vec::new();
        for chunk in keys.chunks(KEYED_CHUNK) {
            let keyed = duckdb_keyed(table, "raw_id", "project_id", chunk.len());
            let values: Vec<duckdb::types::Value> = chunk
                .iter()
                .map(|key| duckdb::types::Value::Text((*key).to_string()))
                .chain(std::iter::once(duckdb::types::Value::Text(
                    project_id.to_string(),
                )))
                .collect();
            let mut statement = conn.prepare(&format!(
                "SELECT keyed_rowid FROM {keyed} WHERE project_id = ?"
            ))?;
            let rows =
                statement.query_map(duckdb::params_from_iter(values), |r| r.get::<_, i64>(0))?;
            for row in rows {
                rowids.push(row?);
            }
        }
        super::keyed::delete_rows(conn, table, &rowids)?;
    }
    Ok(())
}

/// Delete every version of those of these records no stored span row names, in one transaction on the one
/// connection, so no row is written between the look and the delete. Returns the records deleted.
pub fn delete_unnamed(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashSet<String>, DuckdbError> {
    crate::in_transaction(conn, |conn| {
        let named = named(conn, project_id, raw_ids)?;
        let unnamed: Vec<String> = distinct_keys(raw_ids.iter().map(String::as_str))
            .into_iter()
            .filter(|raw_id| !named.contains(*raw_id))
            .map(str::to_string)
            .collect();
        delete(conn, project_id, &unnamed)?;
        Ok(unnamed.into_iter().collect())
    })
}

/// Append these rewritten versions of the records still stored, and queue those appended, in one transaction: a
/// record collected since its rewrite was read is not stored again, and one stored again is looked at again
/// whatever becomes of the caller. Returns the records appended.
pub fn append_rewrites(
    conn: &Connection,
    records: &[RawRecordRow],
) -> Result<HashSet<String>, DuckdbError> {
    crate::in_transaction(conn, |conn| {
        let mut by_project: BTreeMap<&ProjectId, Vec<String>> = BTreeMap::new();
        for record in records {
            by_project
                .entry(&record.project_id)
                .or_default()
                .push(record.raw_id.clone());
        }
        let mut stored: HashSet<(ProjectId, String)> = HashSet::new();
        for (project_id, raw_ids) in by_project {
            for raw_id in stored_ids(conn, project_id, &raw_ids)? {
                stored.insert((project_id.clone(), raw_id));
            }
        }
        let kept: Vec<&RawRecordRow> = records
            .iter()
            .filter(|record| stored.contains(&(record.project_id.clone(), record.raw_id.clone())))
            .collect();
        if !kept.is_empty() {
            append(conn, &kept)?;
        }
        let mut queued: BTreeMap<&ProjectId, Vec<String>> = BTreeMap::new();
        for record in &kept {
            queued
                .entry(&record.project_id)
                .or_default()
                .push(record.raw_id.clone());
        }
        for (project_id, raw_ids) in queued {
            enqueue(conn, project_id, &raw_ids)?;
        }
        Ok(kept.iter().map(|record| record.raw_id.clone()).collect())
    })
}

/// Which of these records have a version stored, read through the index without their bodies.
fn stored_ids(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<std::collections::BTreeSet<String>, DuckdbError> {
    let keys = distinct_keys(raw_ids.iter().map(String::as_str));
    let mut stored = std::collections::BTreeSet::new();
    for chunk in keys.chunks(KEYED_CHUNK) {
        let keyed = duckdb_keyed("otel_raw", "raw_id", "project_id, raw_id", chunk.len());
        let values: Vec<duckdb::types::Value> = chunk
            .iter()
            .map(|key| duckdb::types::Value::Text((*key).to_string()))
            .chain(std::iter::once(duckdb::types::Value::Text(
                project_id.to_string(),
            )))
            .collect();
        let mut statement = conn.prepare(&format!(
            "SELECT DISTINCT raw_id FROM {keyed} WHERE project_id = ?"
        ))?;
        let rows =
            statement.query_map(duckdb::params_from_iter(values), |r| r.get::<_, String>(0))?;
        for row in rows {
            stored.insert(row?);
        }
    }
    Ok(stored)
}

/// Which of these records a stored span row still names.
pub fn named(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashSet<String>, DuckdbError> {
    if raw_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let sql = format!(
        "SELECT DISTINCT raw_id FROM otel_spans WHERE project_id = ? AND raw_id IN ({})",
        placeholders(raw_ids.len())
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        duckdb::params_from_iter(text_values(project_id, raw_ids)),
        |r| r.get::<_, String>(0),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
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

/// Up to `limit` queued entries, oldest first.
pub fn pending(conn: &Connection, limit: usize) -> Result<Vec<RawPending>, DuckdbError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT project_id, raw_id, token FROM otel_raw_pending \
         ORDER BY enqueued_at, project_id, raw_id, token LIMIT ?",
    )?;
    let rows = statement.query_map(params![limit], |r| {
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
