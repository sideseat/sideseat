//! Stored raw records (`otel_raw`). See `schema::raw` for the table.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};

use crate::ClickhouseError;
use sideseat_ports::types::{ProjectId, RawOrigin, RawPending, RawRecordRow, StagedSignal};
use sideseat_query_sql::Backend;
use sideseat_query_sql::analytics;
use sideseat_query_sql::analytics::QueryValue;
use sideseat_query_sql::dml;

#[derive(Row, Serialize, Deserialize)]
struct RawRow {
    project_id: String,
    raw_id: String,
    signal: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    received_at: time::OffsetDateTime,
    origin: String,
    version: i64,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    signal_until: time::OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    hold_until: Option<time::OffsetDateTime>,
    #[serde(with = "binary_string")]
    record: Vec<u8>,
}

fn to_time(at: DateTime<Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(at.timestamp_micros()) * 1000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn from_time(at: time::OffsetDateTime) -> DateTime<Utc> {
    DateTime::from_timestamp_micros((at.unix_timestamp_nanos() / 1000) as i64)
        .unwrap_or(DateTime::UNIX_EPOCH)
}

impl RawRow {
    fn from_record(record: &RawRecordRow) -> Self {
        Self {
            project_id: record.project_id.to_string(),
            raw_id: record.raw_id.clone(),
            signal: record.signal.as_str().to_string(),
            received_at: to_time(record.received_at),
            origin: record.origin.as_str().to_string(),
            version: record.version,
            signal_until: to_time(record.signal_until),
            hold_until: record.hold_until.map(to_time),
            record: record.record.clone(),
        }
    }

    fn into_record(self) -> RawRecordRow {
        RawRecordRow {
            project_id: ProjectId::from(self.project_id.as_str()),
            signal: StagedSignal::from_stored(&self.signal).unwrap_or(StagedSignal::Traces),
            raw_id: self.raw_id,
            received_at: from_time(self.received_at),
            origin: RawOrigin::from_stored(&self.origin).unwrap_or(RawOrigin::Deleted),
            version: self.version,
            signal_until: from_time(self.signal_until),
            hold_until: self.hold_until.map(from_time),
            trace_ids: Vec::new(),
            record: self.record,
        }
    }
}

/// A ClickHouse `String` holding arbitrary bytes: serde's byte form, which the client writes as a `String`
/// rather than the `Array(UInt8)` a plain `Vec<u8>` would be.
mod binary_string {
    use serde::de::{Deserializer, Error, SeqAccess, Visitor};
    use serde::ser::Serializer;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
            fn visit_str<E: Error>(self, v: &str) -> Result<Vec<u8>, E> {
                Ok(v.as_bytes().to_vec())
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
                let mut out = Vec::new();
                while let Some(byte) = seq.next_element()? {
                    out.push(byte);
                }
                Ok(out)
            }
        }
        deserializer.deserialize_byte_buf(Bytes)
    }
}

const COLUMNS: &str =
    "project_id, raw_id, signal, received_at, origin, version, signal_until, hold_until, record";

#[derive(Row, Serialize)]
struct TraceIndexRow<'a> {
    project_id: &'a str,
    trace_id: &'a str,
    raw_id: &'a str,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    signal_until: time::OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    hold_until: Option<time::OffsetDateTime>,
}

/// Write the versions, then their trace-index rows; `ReplacingMergeTree` folds an index row written twice.
async fn write(client: &Client, records: &[&RawRecordRow]) -> Result<(), ClickhouseError> {
    if records.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert::<RawRow>("otel_raw").await?;
    for record in records {
        insert.write(&RawRow::from_record(record)).await?;
    }
    insert.end().await?;
    if records.iter().all(|record| record.trace_ids.is_empty()) {
        return Ok(());
    }
    let mut index = client
        .insert::<TraceIndexRow<'_>>("otel_raw_traces")
        .await?;
    for record in records {
        for trace_id in &record.trace_ids {
            index
                .write(&TraceIndexRow {
                    project_id: record.project_id.as_str(),
                    trace_id,
                    raw_id: &record.raw_id,
                    signal_until: to_time(record.signal_until),
                    hold_until: record.hold_until.map(to_time),
                })
                .await?;
        }
    }
    index.end().await?;
    Ok(())
}

/// Store the records not already stored, as the DuckDB adapter does: a second row for a present `raw_id` would
/// be a second copy of raw content until a merge.
pub async fn insert(client: &Client, records: &[RawRecordRow]) -> Result<(), ClickhouseError> {
    let Some(first) = records.first() else {
        return Ok(());
    };
    let ids: Vec<&str> = records.iter().map(|r| r.raw_id.as_str()).collect();
    let present: Vec<String> = client
        .query("SELECT DISTINCT raw_id FROM otel_raw WHERE project_id = ? AND raw_id IN ?")
        .bind(first.project_id.as_str())
        .bind(&ids)
        .fetch_all()
        .await?;
    let mut seen: HashSet<String> = present.into_iter().collect();
    let fresh: Vec<&RawRecordRow> = records
        .iter()
        .filter(|record| seen.insert(record.raw_id.clone()))
        .collect();
    write(client, &fresh).await
}

/// Store these versions whatever is already there.
pub async fn append(client: &Client, records: &[RawRecordRow]) -> Result<(), ClickhouseError> {
    write(client, &records.iter().collect::<Vec<_>>()).await
}

/// The latest version of each requested record. `FINAL` merges the versions the way the engine will, and the
/// ordering tie-break is the same as DuckDB's: the highest version, the last written.
pub async fn get(
    client: &Client,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<Vec<RawRecordRow>, ClickhouseError> {
    if raw_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<RawRow> = client
        .query(&format!(
            "SELECT {COLUMNS} FROM otel_raw FINAL WHERE project_id = ? AND raw_id IN ? ORDER BY raw_id"
        ))
        .bind(project_id.as_str())
        .bind(raw_ids)
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(RawRow::into_record).collect())
}

/// One page of latest versions in `(received_at, raw_id)` order: the order a re-derivation replays.
pub async fn page(
    client: &Client,
    project_id: &ProjectId,
    after: Option<(DateTime<Utc>, String)>,
    limit: usize,
) -> Result<Vec<RawRecordRow>, ClickhouseError> {
    let (after_us, after_id) = after
        .map(|(at, id)| (at.timestamp_micros(), id))
        .unwrap_or((i64::MIN, String::new()));
    let rows: Vec<RawRow> = client
        .query(&format!(
            "SELECT {COLUMNS} FROM otel_raw FINAL WHERE project_id = ? \
             AND (toUnixTimestamp64Micro(received_at), raw_id) > (?, ?) \
             ORDER BY received_at, raw_id LIMIT ?"
        ))
        .bind(project_id.as_str())
        .bind(after_us)
        .bind(after_id)
        .bind(limit as u64)
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(RawRow::into_record).collect())
}

async fn execute(client: &Client, statement: &dml::DmlStatement) -> Result<(), ClickhouseError> {
    let mut query = client.query(statement.sql());
    for value in statement.params() {
        query = match value {
            QueryValue::String(value) => query.bind(value),
            QueryValue::Int64(value) => query.bind(value),
            QueryValue::Float64(value) => query.bind(value),
        };
    }
    query.execute().await?;
    Ok(())
}

/// Delete every version of these records and their trace-index rows; `tables` are the record table and the
/// index, as mutation targets.
pub async fn delete(
    client: &Client,
    tables: &[String; 2],
    on_cluster: &str,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<(), ClickhouseError> {
    for table in tables {
        if let Some(statement) = dml::raw::delete_raw_records(
            dml::MutationTarget::clickhouse(table, on_cluster),
            project_id.as_str(),
            raw_ids,
        ) {
            execute(client, &statement).await?;
        }
    }
    Ok(())
}

/// Which of these records a stored span row still names.
pub async fn named(
    client: &Client,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashSet<String>, ClickhouseError> {
    if raw_ids.is_empty() {
        return Ok(HashSet::new());
    }
    // `otel_spans.raw_id` is nullable, so the driver must be told to expect a nullable column even though the
    // predicate excludes nulls.
    let rows: Vec<Option<String>> = client
        .query(
            "SELECT DISTINCT raw_id FROM otel_spans \
             WHERE project_id = ? AND raw_id IN ? AND raw_id IS NOT NULL",
        )
        .bind(project_id.as_str())
        .bind(raw_ids)
        .fetch_all()
        .await?;
    Ok(rows.into_iter().flatten().collect())
}

/// Enqueue these records for reconciliation.
pub async fn enqueue(
    client: &Client,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<(), ClickhouseError> {
    match dml::raw::enqueue_raw_records(Backend::Clickhouse, project_id.as_str(), raw_ids) {
        Some(statement) => execute(client, &statement).await,
        None => Ok(()),
    }
}

#[derive(Row, Deserialize)]
struct PendingRow {
    project_id: String,
    raw_id: String,
    token: String,
}

/// Every project's queue: read with the maintenance client, which the tenant row policy lets through.
pub async fn pending(client: &Client, limit: usize) -> Result<Vec<RawPending>, ClickhouseError> {
    let rows: Vec<PendingRow> = client
        .query(
            "SELECT project_id, raw_id, token FROM otel_raw_pending \
             ORDER BY enqueued_at, project_id, raw_id, token LIMIT ?",
        )
        .bind(limit as u64)
        .fetch_all()
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| RawPending {
            project_id: ProjectId::from(row.project_id.as_str()),
            raw_id: row.raw_id,
            token: row.token,
        })
        .collect())
}

/// Remove exactly these entries; one enqueued since they were read stays.
pub async fn clear(
    client: &Client,
    table: &str,
    on_cluster: &str,
    entries: &[RawPending],
) -> Result<(), ClickhouseError> {
    let mut by_project: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for entry in entries {
        by_project
            .entry(entry.project_id.as_str())
            .or_default()
            .push((entry.raw_id.clone(), entry.token.clone()));
    }
    for (project_id, entries) in by_project {
        if let Some(statement) = dml::raw::clear_raw_pending(
            dml::MutationTarget::clickhouse(table, on_cluster),
            project_id,
            &entries,
        ) {
            execute(client, &statement).await?;
        }
    }
    Ok(())
}

/// The record each of these spans was derived from, for the winning row of each identity.
pub async fn span_raw_ids(
    client: &Client,
    project_id: &ProjectId,
    spans: &[(String, String)],
) -> Result<HashMap<(String, String), String>, ClickhouseError> {
    let Some(query) = analytics::span_raw_ids(project_id.as_str(), spans, Backend::Clickhouse)
    else {
        return Ok(HashMap::new());
    };
    #[derive(Row, Deserialize)]
    struct Found {
        trace_id: String,
        span_id: String,
        // Nullable on the table; the query excludes nulls, but the driver checks the column's type.
        raw_id: Option<String>,
    }
    let mut request = client.query(query.sql());
    for value in query.params() {
        request = match value {
            QueryValue::String(value) => request.bind(value),
            QueryValue::Int64(value) => request.bind(value),
            QueryValue::Float64(value) => request.bind(value),
        };
    }
    let rows: Vec<Found> = request.fetch_all().await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.raw_id
                .map(|raw_id| ((row.trace_id, row.span_id), raw_id))
        })
        .collect())
}

/// The latest records the surviving span rows of these traces name - every physical row, superseded
/// revisions included, as the DuckDB adapter answers: a record lives while any row names it.
pub async fn survivor_records(
    client: &Client,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<Vec<u8>>, ClickhouseError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(Row, Deserialize)]
    struct RecordOnly {
        #[serde(with = "binary_string")]
        record: Vec<u8>,
    }
    let rows: Vec<RecordOnly> = client
        .query(
            "SELECT record FROM otel_raw FINAL WHERE project_id = ? AND raw_id IN ( \
                 SELECT DISTINCT raw_id FROM otel_spans \
                 WHERE project_id = ? AND trace_id IN ? AND raw_id IS NOT NULL)",
        )
        .bind(project_id.as_str())
        .bind(project_id.as_str())
        .bind(trace_ids)
        .fetch_all()
        .await?;
    Ok(rows.into_iter().map(|row| row.record).collect())
}
