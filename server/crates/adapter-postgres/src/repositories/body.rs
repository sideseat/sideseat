//! The retired content-body registry: what is left is the drain. See `ContentBodyStore` in the ports.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use sideseat_ports::types::ProjectId;

use crate::PostgresError;

/// Delete up to `limit` associations. Nothing reads or writes them any more; see the port.
pub async fn retire_associations(
    connection: &mut PgConnection,
    limit: usize,
) -> Result<u64, PostgresError> {
    Ok(
        sqlx::query(
            "DELETE FROM span_bodies WHERE ctid IN (SELECT ctid FROM span_bodies LIMIT $1)",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await?
        .rows_affected(),
    )
}

pub async fn orphans(
    connection: &mut PgConnection,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, PostgresError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT b.project_id, b.body_hash FROM content_bodies b
         WHERE b.deleting_at IS NULL AND b.last_referenced_at <= $1 AND NOT EXISTS (
             SELECT 1 FROM span_bodies r
             WHERE r.project_id = b.project_id AND r.body_hash = b.body_hash
         )
         ORDER BY b.last_referenced_at, b.project_id, b.body_hash LIMIT $2",
    )
    .bind(older_than.timestamp_nanos_opt().unwrap_or(0))
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project, hash)| (ProjectId::from(project), hash))
        .collect())
}

pub async fn stale_claims(
    connection: &mut PgConnection,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, PostgresError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT project_id, body_hash FROM content_bodies
         WHERE deleting_at IS NOT NULL AND deleting_at <= $1
         ORDER BY deleting_at, project_id, body_hash LIMIT $2",
    )
    .bind(older_than.timestamp_micros())
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project, hash)| (ProjectId::from(project), hash))
        .collect())
}

pub async fn claim_for_deletion(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    body_hash: &str,
    now: DateTime<Utc>,
) -> Result<bool, PostgresError> {
    Ok(sqlx::query(
        "UPDATE content_bodies SET deleting_at = $1
         WHERE project_id = $2 AND body_hash = $3 AND deleting_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = $2 AND body_hash = $3
           )",
    )
    .bind(now.timestamp_micros())
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        > 0)
}

pub async fn release_deletion_claim(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "UPDATE content_bodies SET deleting_at = NULL
         WHERE project_id = $1 AND body_hash = $2",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

pub async fn delete_claimed(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<bool, PostgresError> {
    Ok(sqlx::query(
        "DELETE FROM content_bodies
         WHERE project_id = $1 AND body_hash = $2 AND deleting_at IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = $1 AND body_hash = $2
           )",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(&mut *connection)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete_project(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<Vec<String>, PostgresError> {
    let hashes = sqlx::query_scalar::<_, String>(
        "SELECT body_hash FROM content_bodies WHERE project_id = $1 ORDER BY body_hash",
    )
    .bind(project_id.as_str())
    .fetch_all(&mut *connection)
    .await?;
    sqlx::query("DELETE FROM content_bodies WHERE project_id = $1")
        .bind(project_id.as_str())
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM content_body_backfill WHERE project_id = $1")
        .bind(project_id.as_str())
        .execute(&mut *connection)
        .await?;
    Ok(hashes)
}
