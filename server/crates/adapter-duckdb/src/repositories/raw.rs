//! Stored raw records (`otel_raw`).
//!
//! Append-only, like every analytics table: a rewrite after a deletion is a new row for the same `raw_id`, and
//! readers take the latest row of each `raw_id`.

use chrono::{DateTime, Utc};
use duckdb::{Connection, params};

use crate::DuckdbError;
use crate::sql_types::SqlTimestamp;
use sideseat_core::utils::time::micros_to_datetime;
use sideseat_ports::types::{ProjectId, RawRecordRow, StagedSignal};

/// The latest row of each `raw_id`.
const WINNERS: &str = "(SELECT * FROM otel_raw WHERE project_id = ? \
     QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY rowid DESC) = 1)";

pub fn insert(conn: &Connection, records: &[RawRecordRow]) -> Result<(), DuckdbError> {
    if records.is_empty() {
        return Ok(());
    }
    let mut appender = conn.appender("otel_raw")?;
    for record in records {
        appender.append_row(params![
            record.project_id.as_str(),
            record.raw_id.as_str(),
            record.signal.as_str(),
            SqlTimestamp(record.received_at),
            record.rewritten,
            record.record.as_slice(),
        ])?;
    }
    appender.flush()?;
    Ok(())
}

fn row(project_id: &ProjectId, row: &duckdb::Row<'_>) -> duckdb::Result<RawRecordRow> {
    let signal: String = row.get(1)?;
    Ok(RawRecordRow {
        project_id: project_id.clone(),
        raw_id: row.get(0)?,
        signal: StagedSignal::from_stored(&signal).unwrap_or(StagedSignal::Traces),
        received_at: micros_to_datetime(row.get(2)?),
        rewritten: row.get(3)?,
        record: row.get(4)?,
    })
}

pub fn get(
    conn: &Connection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<Vec<RawRecordRow>, DuckdbError> {
    if raw_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", raw_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    // Restricted to the requested ids before the latest row is chosen: the choice is per id.
    let sql = format!(
        "SELECT raw_id, signal, EPOCH_US(received_at), rewritten, record FROM otel_raw \
         WHERE project_id = ? AND raw_id IN ({placeholders}) \
         QUALIFY ROW_NUMBER() OVER (PARTITION BY raw_id ORDER BY rowid DESC) = 1 \
         ORDER BY raw_id"
    );
    let values: Vec<duckdb::types::Value> = std::iter::once(project_id.to_string())
        .chain(raw_ids.iter().cloned())
        .map(duckdb::types::Value::Text)
        .collect();
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(duckdb::params_from_iter(values), |r| row(project_id, r))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

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
        "SELECT raw_id, signal, EPOCH_US(received_at), rewritten, record FROM {WINNERS} \
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

/// Append the rewritten bytes as the newest row of `raw_id`, keeping its signal and receipt time.
pub fn rewrite(
    conn: &Connection,
    project_id: &ProjectId,
    raw_id: &str,
    record: &[u8],
) -> Result<(), DuckdbError> {
    conn.execute(
        "INSERT INTO otel_raw (project_id, raw_id, signal, received_at, rewritten, record) \
         SELECT project_id, raw_id, signal, received_at, true, ? FROM otel_raw \
         WHERE project_id = ? AND raw_id = ? ORDER BY rowid DESC LIMIT 1",
        params![record, project_id.as_str(), raw_id],
    )?;
    Ok(())
}

/// Delete every row of the project's records received before the cutoff that no span row names.
pub fn delete_unreferenced(
    conn: &Connection,
    project_id: &ProjectId,
    received_before: DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT DISTINCT r.raw_id FROM otel_raw r \
             WHERE r.project_id = ? AND r.received_at < ?::TIMESTAMP \
               AND NOT EXISTS (SELECT 1 FROM otel_spans s \
                               WHERE s.project_id = r.project_id AND s.raw_id = r.raw_id)",
        )?;
        statement
            .query_map(
                params![project_id.as_str(), SqlTimestamp(received_before)],
                |r| r.get(0),
            )?
            .collect::<Result<_, _>>()?
    };
    let mut deleted = 0u64;
    let mut statement = conn.prepare("DELETE FROM otel_raw WHERE project_id = ? AND raw_id = ?")?;
    for id in &ids {
        statement.execute(params![project_id.as_str(), id])?;
        deleted += 1;
    }
    Ok(deleted)
}
