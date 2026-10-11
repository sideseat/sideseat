//! PostgreSQL staged-payload registry.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use crate::PostgresError;
use sideseat_ports::types::{
    ProjectId, StagedPayload, StagedRecord, StagedSequenceState, StagedSignal,
};

pub async fn create(
    connection: &mut PgConnection,
    payload: &StagedPayload,
) -> Result<i64, PostgresError> {
    let records = serde_json::to_string(&payload.records)
        .map_err(|error| PostgresError::Conflict(error.to_string()))?;
    let seq = sqlx::query_scalar(
        "INSERT INTO staged_payloads
         (id, project_id, signal, blob_hash, byte_len, created_at, redrive_attempts, unconfirmed, records_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING seq",
    )
    .bind(&payload.id)
    .bind(payload.project_id.as_str())
    .bind(payload.signal.as_str())
    .bind(&payload.blob_hash)
    .bind(payload.byte_len as i64)
    .bind(payload.created_at.timestamp_micros())
    .bind(i64::from(payload.redrive_attempts))
    .bind(payload.unconfirmed)
    .bind(records)
    .fetch_one(&mut *connection)
    .await?;
    // The registration's pending-writer entry, in the caller's transaction: the raw arbiter's collection
    // decision must never see a registered payload without its record claim (`RawRecordOwnership.tla`,
    // `Register`).
    if let Some(raw_id) = &payload.raw_id {
        sqlx::query("INSERT INTO raw_writers (payload_id, project_id, raw_id) VALUES ($1, $2, $3)")
            .bind(&payload.id)
            .bind(payload.project_id.as_str())
            .bind(raw_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(seq)
}

pub async fn sequence_state(
    connection: &mut PgConnection,
    seq: i64,
) -> Result<StagedSequenceState, PostgresError> {
    // `last_value` is NULL until the sequence is first used.
    let high_water: i64 = sqlx::query_scalar(
        "SELECT COALESCE(last_value, 0) FROM pg_sequences
         WHERE schemaname = current_schema()
           AND sequencename = pg_get_serial_sequence('staged_payloads', 'seq')::regclass::name",
    )
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or(0);
    let holder: Option<String> =
        sqlx::query_scalar("SELECT id FROM staged_payloads WHERE seq = $1")
            .bind(seq)
            .fetch_optional(&mut *connection)
            .await?;
    Ok(StagedSequenceState { high_water, holder })
}

pub async fn record_anomaly(
    connection: &mut PgConnection,
    id: &str,
    seq: i64,
    detected_at: DateTime<Utc>,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO staged_payload_anomalies (id, seq, detected_at) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET occurrences = staged_payload_anomalies.occurrences + 1",
    )
    .bind(id)
    .bind(seq)
    .bind(detected_at.timestamp_micros())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

pub async fn get(
    connection: &mut PgConnection,
    id: &str,
) -> Result<Option<StagedPayload>, PostgresError> {
    let row = sqlx::query(
        "SELECT p.id, p.project_id, p.signal, p.blob_hash, p.byte_len, p.created_at, p.redrive_attempts,
                p.unconfirmed, p.records_json, w.raw_id
         FROM staged_payloads p LEFT JOIN raw_writers w ON w.payload_id = p.id
         WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(row_to_payload).transpose()
}

pub async fn pending(
    connection: &mut PgConnection,
    limit: usize,
) -> Result<Vec<StagedPayload>, PostgresError> {
    let rows = sqlx::query(
        "SELECT p.id, p.project_id, p.signal, p.blob_hash, p.byte_len, p.created_at, p.redrive_attempts,
                p.unconfirmed, p.records_json, w.raw_id
         FROM staged_payloads p LEFT JOIN raw_writers w ON w.payload_id = p.id
         WHERE p.unconfirmed = FALSE
         ORDER BY p.created_at ASC, p.id ASC
         LIMIT $1",
    )
    .bind(limit as i64)
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter().map(row_to_payload).collect()
}

pub async fn increment_attempts(
    connection: &mut PgConnection,
    id: &str,
) -> Result<u32, PostgresError> {
    let attempts: i64 = sqlx::query_scalar(
        "UPDATE staged_payloads
         SET redrive_attempts = redrive_attempts + 1
         WHERE id = $1
         RETURNING redrive_attempts",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await?;
    Ok(u32::try_from(attempts).unwrap_or(u32::MAX))
}

pub async fn mark_unconfirmed(
    connection: &mut PgConnection,
    id: &str,
) -> Result<(), PostgresError> {
    sqlx::query("UPDATE staged_payloads SET unconfirmed = TRUE WHERE id = $1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn delete(connection: &mut PgConnection, id: &str) -> Result<(), PostgresError> {
    // Settlement retires the registration and its pending-writer entry together, in the caller's
    // transaction (`RawRecordOwnership.tla`, `Settle`).
    sqlx::query("DELETE FROM staged_payloads WHERE id = $1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM raw_writers WHERE payload_id = $1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn delete_project(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<u64, PostgresError> {
    // The arbiter row first, the lock order the confirm fence and the collection decision take: cleanup
    // removes the state they read, and taking it later could deadlock against a confirm holding it.
    sqlx::query("SELECT 1 FROM raw_arbiter WHERE project_id = $1 FOR UPDATE")
        .bind(project_id.as_str())
        .execute(&mut *connection)
        .await?;
    // Only the writer entries of the registrations removed here, in the same statement: under READ COMMITTED
    // a registration that commits in between keeps both its row and its writer entry. A CTE, so the removed
    // ids never leave the server.
    let removed: i64 = sqlx::query_scalar(
        "WITH removed AS (DELETE FROM staged_payloads WHERE project_id = $1 RETURNING id),
              writers AS (DELETE FROM raw_writers WHERE payload_id IN (SELECT id FROM removed))
         SELECT COUNT(*) FROM removed",
    )
    .bind(project_id.as_str())
    .fetch_one(&mut *connection)
    .await?;
    for table in ["raw_owners", "raw_collections", "raw_arbiter"] {
        let sql = format!("DELETE FROM {table} WHERE project_id = $1");
        sqlx::query(&sql)
            .bind(project_id.as_str())
            .execute(&mut *connection)
            .await?;
    }
    Ok(u64::try_from(removed).unwrap_or(0))
}

fn row_to_payload(row: sqlx::postgres::PgRow) -> Result<StagedPayload, PostgresError> {
    let signal: String = row.try_get("signal")?;
    let records: String = row.try_get("records_json")?;
    Ok(StagedPayload {
        id: row.try_get("id")?,
        project_id: ProjectId::from(row.try_get::<String, _>("project_id")?),
        signal: parse_signal(&signal)?,
        blob_hash: row.try_get("blob_hash")?,
        byte_len: u64::try_from(row.try_get::<i64, _>("byte_len")?).unwrap_or(0),
        created_at: DateTime::from_timestamp_micros(row.try_get("created_at")?)
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        redrive_attempts: u32::try_from(row.try_get::<i64, _>("redrive_attempts")?)
            .unwrap_or(u32::MAX),
        unconfirmed: row.try_get("unconfirmed")?,
        records: serde_json::from_str::<Vec<StagedRecord>>(&records)
            .map_err(|error| PostgresError::Conflict(error.to_string()))?,
        raw_id: row.try_get("raw_id")?,
    })
}

fn parse_signal(value: &str) -> Result<StagedSignal, PostgresError> {
    match value {
        "traces" => Ok(StagedSignal::Traces),
        "metrics" => Ok(StagedSignal::Metrics),
        "logs" => Ok(StagedSignal::Logs),
        other => Err(PostgresError::Conflict(format!(
            "unknown staged signal {other:?}"
        ))),
    }
}
