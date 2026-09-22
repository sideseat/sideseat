//! PostgreSQL legal-hold, maintenance-lease and logical-byte accounting operations.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use crate::PostgresError;
use sideseat_ports::types::{ProjectHold, ProjectId, ProjectStorageUsage};

pub async fn active_hold(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    now: DateTime<Utc>,
) -> Result<Option<ProjectHold>, PostgresError> {
    let row = sqlx::query(
        "SELECT project_id, hold_until, updated_at
         FROM project_holds
         WHERE project_id = $1 AND hold_until >= $2",
    )
    .bind(project_id.as_str())
    .bind(now.timestamp_micros())
    .fetch_optional(&mut *connection)
    .await?;
    row.map(row_to_hold).transpose()
}

pub async fn list_active_holds(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<ProjectHold>, PostgresError> {
    let rows = sqlx::query(
        "SELECT project_id, hold_until, updated_at
         FROM project_holds
         WHERE hold_until >= $1
         ORDER BY updated_at ASC, project_id ASC
         LIMIT $2",
    )
    .bind(now.timestamp_micros())
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter().map(row_to_hold).collect()
}

pub async fn set_hold(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    hold_until: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<ProjectHold, PostgresError> {
    sqlx::query(
        "INSERT INTO project_holds (project_id, hold_until, updated_at)
         VALUES ($1, $2, $3)
         ON CONFLICT(project_id) DO UPDATE SET
             hold_until = EXCLUDED.hold_until,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(project_id.as_str())
    .bind(hold_until.timestamp_micros())
    .bind(now.timestamp_micros())
    .execute(&mut *connection)
    .await?;
    Ok(ProjectHold {
        project_id: project_id.clone(),
        hold_until,
        updated_at: now,
    })
}

pub async fn clear_hold(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<bool, PostgresError> {
    Ok(
        sqlx::query("DELETE FROM project_holds WHERE project_id = $1")
            .bind(project_id.as_str())
            .execute(&mut *connection)
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn acquire_maintenance(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    owner: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<bool, PostgresError> {
    let changed = sqlx::query(
        "INSERT INTO project_maintenance_leases (project_id, owner, lease_until)
         VALUES ($1, $2, $3)
         ON CONFLICT(project_id) DO UPDATE SET
             owner = EXCLUDED.owner,
             lease_until = EXCLUDED.lease_until
         WHERE project_maintenance_leases.lease_until <= $4
            OR project_maintenance_leases.owner = EXCLUDED.owner",
    )
    .bind(project_id.as_str())
    .bind(owner)
    .bind(lease_until.timestamp_micros())
    .bind(now.timestamp_micros())
    .execute(&mut *connection)
    .await?;
    Ok(changed.rows_affected() > 0)
}

pub async fn release_maintenance(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    owner: &str,
) -> Result<(), PostgresError> {
    sqlx::query("DELETE FROM project_maintenance_leases WHERE project_id = $1 AND owner = $2")
        .bind(project_id.as_str())
        .bind(owner)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn reserve_storage(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    additional_bytes: u64,
    ordinary_limit_bytes: u64,
    now: DateTime<Utc>,
) -> Result<Option<ProjectStorageUsage>, PostgresError> {
    let additional = i64::try_from(additional_bytes).unwrap_or(i64::MAX);
    let limit = i64::try_from(ordinary_limit_bytes).unwrap_or(i64::MAX);
    let row = sqlx::query(
        "INSERT INTO project_storage_usage (project_id, logical_bytes, updated_at)
         SELECT $1, $2, $3 WHERE $2 <= $4
         ON CONFLICT(project_id) DO UPDATE SET
             logical_bytes = project_storage_usage.logical_bytes + EXCLUDED.logical_bytes,
             updated_at = EXCLUDED.updated_at
         WHERE project_storage_usage.logical_bytes <= $4 - EXCLUDED.logical_bytes
         RETURNING logical_bytes, updated_at",
    )
    .bind(project_id.as_str())
    .bind(additional)
    .bind(now.timestamp_micros())
    .bind(limit)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(row_to_usage).transpose()
}

pub async fn usage(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<ProjectStorageUsage, PostgresError> {
    let row = sqlx::query(
        "SELECT logical_bytes, updated_at FROM project_storage_usage WHERE project_id = $1",
    )
    .bind(project_id.as_str())
    .fetch_optional(&mut *connection)
    .await?;
    row.map(row_to_usage).transpose().map(|usage| {
        usage.unwrap_or(ProjectStorageUsage {
            logical_bytes: 0,
            updated_at: DateTime::<Utc>::UNIX_EPOCH,
        })
    })
}

pub async fn replace_usage(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    logical_bytes: u64,
    now: DateTime<Utc>,
) -> Result<ProjectStorageUsage, PostgresError> {
    let logical_bytes = i64::try_from(logical_bytes).unwrap_or(i64::MAX);
    sqlx::query(
        "INSERT INTO project_storage_usage (project_id, logical_bytes, updated_at)
         VALUES ($1, $2, $3)
         ON CONFLICT(project_id) DO UPDATE SET
             logical_bytes = EXCLUDED.logical_bytes,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(project_id.as_str())
    .bind(logical_bytes)
    .bind(now.timestamp_micros())
    .execute(&mut *connection)
    .await?;
    Ok(ProjectStorageUsage {
        logical_bytes: u64::try_from(logical_bytes).unwrap_or(u64::MAX),
        updated_at: now,
    })
}

pub async fn held_transactional_bytes(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<u64, PostgresError> {
    let bytes: i64 = sqlx::query_scalar(
        "SELECT
             COALESCE((SELECT SUM(byte_len) FROM staged_payloads WHERE project_id = $1), 0)::bigint
           + COALESCE((SELECT SUM(logical_bytes) FROM deletion_journal WHERE project_id = $1), 0)::bigint
           + COALESCE((SELECT SUM(logical_bytes) FROM retention_cleanup WHERE project_id = $1), 0)::bigint",
    )
    .bind(project_id.as_str())
    .fetch_one(&mut *connection)
    .await?;
    Ok(u64::try_from(bytes).unwrap_or(0))
}

pub async fn project_ids(
    connection: &mut PgConnection,
    limit: usize,
) -> Result<Vec<ProjectId>, PostgresError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM projects
         UNION SELECT project_id FROM project_storage_usage
         UNION SELECT project_id FROM project_holds
         ORDER BY 1
         LIMIT $1",
    )
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *connection)
    .await?;
    Ok(ids.into_iter().map(ProjectId::from).collect())
}

fn row_to_hold(row: sqlx::postgres::PgRow) -> Result<ProjectHold, PostgresError> {
    Ok(ProjectHold {
        project_id: ProjectId::from(row.try_get::<String, _>("project_id")?),
        hold_until: instant(row.try_get("hold_until")?),
        updated_at: instant(row.try_get("updated_at")?),
    })
}

fn row_to_usage(row: sqlx::postgres::PgRow) -> Result<ProjectStorageUsage, PostgresError> {
    Ok(ProjectStorageUsage {
        logical_bytes: u64::try_from(row.try_get::<i64, _>("logical_bytes")?).unwrap_or(0),
        updated_at: instant(row.try_get("updated_at")?),
    })
}

fn instant(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}
