//! SQLite repositories.
//!
//! Shared rows belong to `sideseat_ports::types`; this module contains SQLite implementations.

pub mod api_key;
pub mod auth_method;
pub mod credential_permissions;
pub mod credentials;
pub mod favorite;
pub mod file;
pub mod governance;
pub mod journal;
pub mod membership;
pub mod organization;
pub mod process_lease;
pub mod project;
pub mod raw_arbiter;
pub mod restore;
pub mod retention_cleanup;
pub mod staging;
pub mod user;

/// A transaction that will write, holding SQLite's write lock from its first statement.
///
/// A deferred transaction takes the lock only at its first write. One that reads first and then writes cannot
/// wait for the lock: if another connection committed after its read, SQLite answers `SQLITE_BUSY` or
/// `SQLITE_BUSY_SNAPSHOT` at once, without the busy timeout, since waiting could not make its read current. So
/// a read-then-write transaction failed whenever another writer committed in between - a file association
/// failing its whole ingest batch, which was then refused and redelivered.
///
/// The lock is taken by a write that changes nothing, as `raw_arbiter`'s fences take it by their first write,
/// waiting under the busy timeout like any other writer, so the reads that follow are current. Not
/// `BEGIN IMMEDIATE` through `begin_with`: sqlx 0.8 awaits again after that statement and before the
/// transaction exists to roll back on drop, so a cancelled caller could return a connection to the pool still
/// inside a transaction holding the lock.
pub(crate) async fn begin_write(
    pool: &sqlx::SqlitePool,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE schema_version SET version = version WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
