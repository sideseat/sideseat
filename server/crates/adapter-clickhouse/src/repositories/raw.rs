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
use sideseat_query_sql::keyed::KEYED_CHUNK;

#[derive(Row, Serialize, Deserialize)]
struct RawRow {
    project_id: String,
    raw_id: String,
    signal: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    received_at: time::OffsetDateTime,
    origin: String,
    version_key: i64,
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
            version_key: record.version,
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
            version: self.version_key,
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

const COLUMNS: &str = "project_id, raw_id, signal, received_at, origin, version_key, signal_until, hold_until, record";

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
/// The trace index first, then the versions: two inserts with no transaction between them, so a failure after the
/// first leaves index rows naming a record that may not exist - harmless, a deletion's enqueue of an absent record
/// settles at once - and never a version its traces cannot find. A redelivery skips a record already stored, so
/// in the other order an index insert that failed was never written again, and a deletion or a collection could
/// not reach the record's rows.
async fn write(client: &Client, records: &[&RawRecordRow]) -> Result<(), ClickhouseError> {
    if records.is_empty() {
        return Ok(());
    }
    if records.iter().any(|record| !record.trace_ids.is_empty()) {
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
    }
    let mut insert = client.insert::<RawRow>("otel_raw").await?;
    for record in records {
        insert.write(&RawRow::from_record(record)).await?;
    }
    insert.end().await?;
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
/// ordering tie-break is the same as DuckDB's: the highest version, the last written. The table has one
/// partition, so `FINAL` sees every version of a record together (`crate::schema::raw_tables`).
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

/// The collection purge: the record's versions keyed at or below `up_to_epoch`. The record's trace-index
/// rows are left to their time-to-live: deleting them after a read that found no version could race a landing
/// that wrote fresh ones, and a deletion finds records through them.
pub async fn purge_versions(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &ProjectId,
    raw_id: &str,
    up_to_epoch: i64,
) -> Result<(), ClickhouseError> {
    let statement = dml::raw::purge_raw_versions(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id.as_str(),
        raw_id,
        up_to_epoch,
    );
    execute(client, &statement).await
}

/// The supersede purge: the record's versions strictly below `below_version`.
pub async fn delete_superseded(
    client: &Client,
    table: &str,
    on_cluster: &str,
    project_id: &ProjectId,
    raw_id: &str,
    below_version: i64,
) -> Result<(), ClickhouseError> {
    let statement = dml::raw::delete_superseded_raw_versions(
        dml::MutationTarget::clickhouse(table, on_cluster),
        project_id.as_str(),
        raw_id,
        below_version,
    );
    execute(client, &statement).await
}

/// The records the trace index holds under these traces, narrowed by its `(project_id, trace_id, ...)` key.
pub async fn indexed_under_traces(
    client: &Client,
    traces: &str,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<String>, ClickhouseError> {
    // A chunk at a time: the driver writes the bound list into the statement, which `max_query_size` bounds.
    let mut raw_ids: Vec<String> = Vec::new();
    for chunk in sideseat_query_sql::keyed::bounded_chunks(trace_ids, KEYED_CHUNK) {
        raw_ids.extend(
            client
                .query(&format!(
                    "SELECT DISTINCT raw_id FROM {traces} WHERE project_id = ? AND trace_id IN ? \
                     SETTINGS skip_unavailable_shards = 0"
                ))
                .bind(project_id.as_str())
                .bind(chunk)
                .fetch_all::<String>()
                .await?,
        );
    }
    raw_ids.sort_unstable();
    raw_ids.dedup();
    Ok(raw_ids)
}

/// The `(raw_id, trace_id)` pairs these traces' span rows still carry, narrowed by the span table's sort key.
pub async fn named_in_traces(
    client: &Client,
    spans: &str,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<HashSet<(String, String)>, ClickhouseError> {
    // `skip_unavailable_shards = 0` pinned: a replica that cannot be read fails the answer, never shortens it. A
    // chunk at a time, as `indexed_under_traces` reads.
    let mut named = HashSet::new();
    for chunk in sideseat_query_sql::keyed::bounded_chunks(trace_ids, KEYED_CHUNK) {
        let rows: Vec<(Option<String>, String)> = client
            .query(&format!(
                "SELECT DISTINCT raw_id, trace_id FROM {spans} \
                 WHERE project_id = ? AND trace_id IN ? AND raw_id IS NOT NULL \
                 SETTINGS skip_unavailable_shards = 0"
            ))
            .bind(project_id.as_str())
            .bind(chunk)
            .fetch_all()
            .await?;
        named.extend(
            rows.into_iter()
                .filter_map(|(raw_id, trace_id)| raw_id.map(|raw_id| (raw_id, trace_id))),
        );
    }
    Ok(named)
}

/// A collection's removal of a record's rows written at or below `up_to_epoch` (`dml::raw::remove_collected_rows`):
/// the record's traces from the trace index, read on every replica (`traces`) so none a lagging replica misses is
/// skipped, then a delete narrowed by the span table's `(project_id, trace_id, ...)` sort key. Returns the traces,
/// whose files the caller reconciles.
/// Enqueue these records for reconciliation.
pub async fn enqueue(
    client: &Client,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<(), ClickhouseError> {
    // A chunk at a time: a long-lived trace sits in a record per export that carried it. A record enqueued twice,
    // by a retry after a chunk failed, is reconciled once.
    for chunk in sideseat_query_sql::keyed::bounded_chunks(raw_ids, KEYED_CHUNK) {
        if let Some(statement) =
            dml::raw::enqueue_raw_records(Backend::Clickhouse, project_id.as_str(), chunk)
        {
            execute(client, &statement).await?;
        }
    }
    Ok(())
}

#[derive(Row, Deserialize)]
struct PendingRow {
    project_id: String,
    raw_id: String,
    token: String,
}

/// Every project's queue: read with the maintenance client, which the tenant row policy lets through.
pub async fn pending(
    client: &Client,
    limit: usize,
    enqueued_before: Option<DateTime<Utc>>,
) -> Result<Vec<RawPending>, ClickhouseError> {
    let (filtered, before) = match enqueued_before {
        Some(at) => (1u8, at.timestamp_micros()),
        None => (0u8, 0),
    };
    let rows: Vec<PendingRow> = client
        .query(
            "SELECT project_id, raw_id, token FROM otel_raw_pending \
             WHERE ? = 0 OR enqueued_at < fromUnixTimestamp64Micro(?) \
             ORDER BY enqueued_at, project_id, raw_id, token LIMIT ?",
        )
        .bind(filtered)
        .bind(before)
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
    #[derive(Row, Deserialize)]
    struct Found {
        trace_id: String,
        span_id: String,
        // Nullable on the table; the query excludes nulls, but the driver checks the column's type.
        raw_id: Option<String>,
    }
    // A chunk at a time: a settlement wave asks for every matched span of a project at once.
    let mut found = HashMap::with_capacity(spans.len());
    for chunk in sideseat_query_sql::keyed::bounded_chunks(spans, KEYED_CHUNK) {
        let Some(query) = analytics::span_raw_ids(project_id.as_str(), chunk, Backend::Clickhouse)
        else {
            continue;
        };
        let mut request = client.query(query.sql());
        for value in query.params() {
            request = match value {
                QueryValue::String(value) => request.bind(value),
                QueryValue::Int64(value) => request.bind(value),
                QueryValue::Float64(value) => request.bind(value),
            };
        }
        let rows: Vec<Found> = request.fetch_all().await?;
        found.extend(rows.into_iter().filter_map(|row| {
            row.raw_id
                .map(|raw_id| ((row.trace_id, row.span_id), raw_id))
        }));
    }
    Ok(found)
}

/// The records a survivor read asks for at once: the bound list stays small, and a trace with a long history
/// costs more reads rather than one statement past the server's limits.
const SURVIVOR_RECORD_CHUNK: usize = 512;

/// The latest records the surviving span rows of these traces name - every physical row, superseded
/// revisions included, as the DuckDB adapter answers: a record lives while any row names it.
///
/// Both reads are over every replica (`spans`, `raw` - `ClickhouseService::every_replica_table`), since a release
/// keeps only the media of the records this returns (`SurvivorReferences`): a row or a record an acknowledged
/// insert committed on one replica is found however far the others lag, and an unreachable replica fails the
/// read. Each replica answers its own latest version of a record, and every distinct one is kept: two
/// deliveries of one record can share a version and still differ - one inline, one with its media stored apart -
/// so neither stands for the other.
pub async fn survivor_records(
    client: &Client,
    spans: &str,
    raw: &str,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<Vec<u8>>, ClickhouseError> {
    let mut raw_ids: Vec<String> = Vec::new();
    for chunk in sideseat_query_sql::keyed::bounded_chunks(trace_ids, SURVIVOR_RECORD_CHUNK) {
        raw_ids.extend(
            client
                .query(&format!(
                    "SELECT DISTINCT assumeNotNull(raw_id) FROM {spans} \
                     WHERE project_id = ? AND trace_id IN ? AND raw_id IS NOT NULL \
                     SETTINGS skip_unavailable_shards = 0"
                ))
                .bind(project_id.as_str())
                .bind(chunk)
                .fetch_all::<String>()
                .await?,
        );
    }
    raw_ids.sort_unstable();
    raw_ids.dedup();
    #[derive(Row, Deserialize)]
    struct RecordOnly {
        #[serde(with = "binary_string")]
        record: Vec<u8>,
    }
    let mut records = Vec::new();
    for chunk in sideseat_query_sql::keyed::bounded_chunks(&raw_ids, SURVIVOR_RECORD_CHUNK) {
        let rows: Vec<RecordOnly> = client
            .query(&format!(
                "SELECT record FROM {raw} FINAL WHERE project_id = ? AND raw_id IN ? \
                 SETTINGS skip_unavailable_shards = 0"
            ))
            .bind(project_id.as_str())
            .bind(chunk)
            .fetch_all()
            .await?;
        records.extend(rows.into_iter().map(|row| row.record));
    }
    records.sort_unstable();
    records.dedup();
    Ok(records)
}
