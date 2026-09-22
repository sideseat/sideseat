//! PostgreSQL staged-payload registry.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::PostgresError;
use sideseat_ports::types::{ProjectId, StagedPayload, StagedRecord, StagedSignal};

pub async fn create(pool: &PgPool, payload: &StagedPayload) -> Result<(), PostgresError> {
    let records = serde_json::to_string(&payload.records)
        .map_err(|error| PostgresError::Conflict(error.to_string()))?;
    sqlx::query(
        "INSERT INTO staged_payloads
         (id, project_id, signal, blob_hash, byte_len, created_at, redrive_attempts, unconfirmed, records_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
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
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(pool: &PgPool, id: &str) -> Result<Option<StagedPayload>, PostgresError> {
    let row = sqlx::query(
        "SELECT id, project_id, signal, blob_hash, byte_len, created_at, redrive_attempts,
                unconfirmed, records_json
         FROM staged_payloads WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_payload).transpose()
}

pub async fn pending(pool: &PgPool, limit: usize) -> Result<Vec<StagedPayload>, PostgresError> {
    let rows = sqlx::query(
        "SELECT id, project_id, signal, blob_hash, byte_len, created_at, redrive_attempts,
                unconfirmed, records_json
         FROM staged_payloads
         WHERE unconfirmed = FALSE
         ORDER BY created_at ASC, id ASC
         LIMIT $1",
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(row_to_payload).collect()
}

pub async fn increment_attempts(pool: &PgPool, id: &str) -> Result<u32, PostgresError> {
    let attempts: i64 = sqlx::query_scalar(
        "UPDATE staged_payloads
         SET redrive_attempts = redrive_attempts + 1
         WHERE id = $1
         RETURNING redrive_attempts",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(u32::try_from(attempts).unwrap_or(u32::MAX))
}

pub async fn mark_unconfirmed(pool: &PgPool, id: &str) -> Result<(), PostgresError> {
    sqlx::query("UPDATE staged_payloads SET unconfirmed = TRUE WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &PgPool, id: &str) -> Result<(), PostgresError> {
    sqlx::query("DELETE FROM staged_payloads WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
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
