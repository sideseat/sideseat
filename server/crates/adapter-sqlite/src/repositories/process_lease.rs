//! The leases processes hold on the store: see `sideseat_ports::traits::ProcessLeases`.

use sqlx::SqliteConnection;

use crate::SqliteError;

/// The store's clock, in seconds: see `ProcessLeases::renew_process_lease`.
const NOW: &str = "CAST(strftime('%s', 'now') AS INTEGER)";

pub async fn renew(
    connection: &mut SqliteConnection,
    process_id: &str,
    role: &str,
    lease_secs: i64,
) -> Result<(), SqliteError> {
    sqlx::query(&format!(
        "INSERT INTO process_leases (process_id, role, lease_until) VALUES (?, ?, {NOW} + ?)
         ON CONFLICT(process_id) DO UPDATE SET role = excluded.role, lease_until = excluded.lease_until"
    ))
    .bind(process_id)
    .bind(role)
    .bind(lease_secs)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

pub async fn end(connection: &mut SqliteConnection, process_id: &str) -> Result<(), SqliteError> {
    sqlx::query("DELETE FROM process_leases WHERE process_id = ?")
        .bind(process_id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

pub async fn live(
    connection: &mut SqliteConnection,
    role: &str,
    except: &str,
) -> Result<Vec<String>, SqliteError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT process_id FROM process_leases
          WHERE role = ? AND process_id <> ? AND lease_until > {NOW}
          ORDER BY process_id"
    ))
    .bind(role)
    .bind(except)
    .fetch_all(&mut *connection)
    .await?)
}
