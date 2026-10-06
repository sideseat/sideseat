//! Stored raw records (`otel_raw`). See `schema::raw` for the table.

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};

use crate::ClickhouseError;
use sideseat_ports::types::{ProjectId, RawRecordRow, StagedSignal};

#[derive(Row, Serialize, Deserialize)]
struct RawRow {
    project_id: String,
    raw_id: String,
    signal: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    received_at: time::OffsetDateTime,
    rewritten: u8,
    version: u32,
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
    fn into_record(self) -> RawRecordRow {
        RawRecordRow {
            project_id: ProjectId::from(self.project_id.as_str()),
            signal: StagedSignal::from_stored(&self.signal).unwrap_or(StagedSignal::Traces),
            raw_id: self.raw_id,
            received_at: from_time(self.received_at),
            rewritten: self.rewritten != 0,
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

const COLUMNS: &str = "project_id, raw_id, signal, received_at, rewritten, version, record";

pub async fn insert(client: &Client, records: &[RawRecordRow]) -> Result<(), ClickhouseError> {
    if records.is_empty() {
        return Ok(());
    }
    let mut insert = client.insert::<RawRow>("otel_raw").await?;
    for record in records {
        insert
            .write(&RawRow {
                project_id: record.project_id.to_string(),
                raw_id: record.raw_id.clone(),
                signal: record.signal.as_str().to_string(),
                received_at: to_time(record.received_at),
                rewritten: u8::from(record.rewritten),
                version: 0,
                record: record.record.clone(),
            })
            .await?;
    }
    insert.end().await?;
    Ok(())
}

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

/// A higher version of the same `(project_id, raw_id)`, which `ReplacingMergeTree` keeps.
pub async fn rewrite(
    client: &Client,
    project_id: &ProjectId,
    raw_id: &str,
    record: &[u8],
) -> Result<(), ClickhouseError> {
    let current: Vec<RawRow> = client
        .query(&format!(
            "SELECT {COLUMNS} FROM otel_raw FINAL WHERE project_id = ? AND raw_id = ?"
        ))
        .bind(project_id.as_str())
        .bind(raw_id)
        .fetch_all()
        .await?;
    let Some(current) = current.into_iter().next() else {
        return Ok(());
    };
    let mut insert = client.insert::<RawRow>("otel_raw").await?;
    insert
        .write(&RawRow {
            rewritten: 1,
            version: current.version.saturating_add(1),
            record: record.to_vec(),
            ..current
        })
        .await?;
    insert.end().await?;
    Ok(())
}

pub async fn delete_unreferenced(
    client: &Client,
    project_id: &ProjectId,
    received_before: DateTime<Utc>,
) -> Result<u64, ClickhouseError> {
    let ids: Vec<String> = client
        .query(
            "SELECT DISTINCT raw_id FROM otel_raw WHERE project_id = ? \
             AND received_at < fromUnixTimestamp64Micro(?) \
             AND raw_id NOT IN (SELECT raw_id FROM otel_spans WHERE project_id = ? AND raw_id IS NOT NULL)",
        )
        .bind(project_id.as_str())
        .bind(received_before.timestamp_micros())
        .bind(project_id.as_str())
        .fetch_all()
        .await?;
    if ids.is_empty() {
        return Ok(0);
    }
    client
        .query("DELETE FROM otel_raw WHERE project_id = ? AND raw_id IN ?")
        .bind(project_id.as_str())
        .bind(&ids)
        .execute()
        .await?;
    Ok(ids.len() as u64)
}
