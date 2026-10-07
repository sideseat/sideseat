//! Faults a test needs to inject, and state it needs to read, that the ports deliberately do not offer.
//!
//! Compiled only with the `test-support` feature, so the SQL stays inside this adapter and no layer crate
//! needs a database driver to test against one.

use crate::{SqliteError, SqliteService};

/// What a power loss after a staged payload's reference was published leaves: its registry row gone and the
/// AUTOINCREMENT high-water mark rolled back with it.
pub async fn lose_staged_registration(
    service: &SqliteService,
    id: &str,
    seq: i64,
) -> Result<(), SqliteError> {
    sqlx::query("DELETE FROM staged_payloads WHERE id = ?")
        .bind(id)
        .execute(service.pool())
        .await?;
    sqlx::query("UPDATE sqlite_sequence SET seq = ? WHERE name = 'staged_payloads'")
        .bind(seq - 1)
        .execute(service.pool())
        .await?;
    Ok(())
}

/// Every recorded staging anomaly as `(id, seq, occurrences)`, in id order.
pub async fn staging_anomalies(
    service: &SqliteService,
) -> Result<Vec<(String, i64, i64)>, SqliteError> {
    Ok(
        sqlx::query_as("SELECT id, seq, occurrences FROM staged_payload_anomalies ORDER BY id")
            .fetch_all(service.pool())
            .await?,
    )
}

/// Make recording a staging anomaly fail, as a broken registry would.
pub async fn break_staging_anomalies(service: &SqliteService) -> Result<(), SqliteError> {
    sqlx::query("DROP TABLE staged_payload_anomalies")
        .execute(service.pool())
        .await?;
    Ok(())
}

/// A staging registry write that fails, as a broken or full registry would.
#[derive(Debug, Clone, Copy)]
pub enum StagingFault {
    /// Registering a payload fails.
    Register,
    /// Retiring a payload fails.
    Retire,
}

/// Make the staging registry refuse `fault` until [`clear_staging_fault`]. A trigger raises inside the real
/// statement, so the failure arrives exactly where a broken registry's would.
pub async fn inject_staging_fault(
    service: &SqliteService,
    fault: StagingFault,
) -> Result<(), SqliteError> {
    let (name, event) = match fault {
        StagingFault::Register => ("staging_fault_register", "INSERT"),
        StagingFault::Retire => ("staging_fault_retire", "DELETE"),
    };
    sqlx::query(&format!(
        "CREATE TRIGGER {name} BEFORE {event} ON staged_payloads \
         BEGIN SELECT RAISE(FAIL, 'injected staging fault'); END"
    ))
    .execute(service.pool())
    .await?;
    Ok(())
}

pub async fn clear_staging_fault(
    service: &SqliteService,
    fault: StagingFault,
) -> Result<(), SqliteError> {
    let name = match fault {
        StagingFault::Register => "staging_fault_register",
        StagingFault::Retire => "staging_fault_retire",
    };
    sqlx::query(&format!("DROP TRIGGER IF EXISTS {name}"))
        .execute(service.pool())
        .await?;
    Ok(())
}

/// How many payloads the registry holds.
pub async fn staged_payload_count(service: &SqliteService) -> Result<i64, SqliteError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM staged_payloads")
        .fetch_one(service.pool())
        .await?)
}
