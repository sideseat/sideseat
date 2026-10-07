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
