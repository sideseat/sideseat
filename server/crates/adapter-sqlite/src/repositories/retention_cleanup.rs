//! The retention cleanup record: which traces still owe the cleanup that follows a deletion of their rows - the
//! journalled span deletions re-applied, owners the rows no longer justify released, files reconciled with the
//! survivors, favourites of emptied traces removed - claimed and leased by the retention sweep, completed only on
//! the token it claimed.
//!
//! **A token is never carried twice.** Tokens come from one sequence for every row, so a worker that paused past
//! its lease cannot complete an intent recorded after its own - even when the row it claimed was completed and
//! its trace recorded again, which a per-row count restarting at 1 let it do.

use sqlx::SqlitePool;

use crate::SqliteError;
use sideseat_ports::traits::retention_cleanup_logical_bytes;

/// A claim token no row has carried.
async fn fresh_cleanup_token<'e, E>(executor: E) -> Result<i64, SqliteError>
where
    E: sqlx::SqliteExecutor<'e>,
{
    Ok(sqlx::query_scalar(
        "INSERT INTO retention_cleanup_tokens (id, last) VALUES (1, 1) \
         ON CONFLICT(id) DO UPDATE SET last = last + 1 RETURNING last",
    )
    .fetch_one(executor)
    .await?)
}

/// Record cleanup intent for one trace under a fresh token, due now, in the caller's transaction. A row already
/// there takes the token: new work behind the same identity, which a stale completion must not discard.
pub(crate) async fn record_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    trace_id: &str,
    now: i64,
) -> Result<i64, SqliteError> {
    let token = fresh_cleanup_token(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO retention_cleanup \
         (project_id, trace_id, created_at, attempts, next_attempt_at, claim_token, logical_bytes) \
         VALUES (?, ?, ?, 0, 0, ?, ?) \
         ON CONFLICT(project_id, trace_id) DO UPDATE SET \
             claim_token = excluded.claim_token, next_attempt_at = 0",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(now)
    .bind(token)
    .bind(i64::try_from(retention_cleanup_logical_bytes(project_id, trace_id)).unwrap_or(i64::MAX))
    .execute(&mut **tx)
    .await?;
    Ok(token)
}

/// Record cleanup intent for these traces, each once under a fresh token, due now, in trace order - as the
/// PostgreSQL twin records them, which orders its row locks so.
pub async fn record_retention_cleanup(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
    now: i64,
) -> Result<Vec<(String, i64)>, SqliteError> {
    let mut ordered: Vec<&String> = trace_ids.iter().collect();
    ordered.sort_unstable();
    ordered.dedup();
    let mut written = Vec::with_capacity(ordered.len());
    for trace_id in ordered {
        let mut tx = pool.begin().await?;
        let token = record_in(&mut tx, project_id, trace_id, now).await?;
        tx.commit().await?;
        written.push((trace_id.clone(), token));
    }
    Ok(written)
}

/// Claim due candidates, pushing their next attempt out before returning them.
///
/// `WHERE (project_id, trace_id) IN (SELECT ... LIMIT n)` rather than a bare `LIMIT` on the update: the inner
/// select fixes the set, and the update then leases exactly those. The backoff is geometric to a ceiling, so a
/// candidate whose reconciliation keeps failing is retried more slowly rather than spinning - and it is
/// **never dropped**, because the record is the only thing that knows the cleanup is owed.
pub async fn claim_retention_cleanup(
    pool: &SqlitePool,
    limit: i64,
    lease_secs: i64,
    now: i64,
) -> Result<Vec<(String, String, i64)>, SqliteError> {
    // The token is **returned**, so completion can require it: without that a stale worker's completion deletes
    // a newer intent. The claim does not change it - a new token is what a *re-record* means - so a worker
    // holding a token whose row was re-recorded finds no match and leaves the new work alone.
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "UPDATE retention_cleanup \
         SET attempts = attempts + 1, \
             next_attempt_at = ? + ? * MIN(attempts + 1, 8) \
         WHERE (project_id, trace_id) IN ( \
             SELECT project_id, trace_id FROM retention_cleanup \
             WHERE next_attempt_at <= ? ORDER BY next_attempt_at LIMIT ? \
         ) \
         RETURNING project_id, trace_id, claim_token",
    )
    .bind(now)
    .bind(lease_secs)
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Drop candidates whose cleanup completed.
pub async fn complete_retention_cleanup(
    pool: &SqlitePool,
    project_id: &str,
    completed: &[(String, i64)],
) -> Result<(), SqliteError> {
    // Per row, because the token is per row. A single statement would need a values list; the batch is at most
    // one claim's worth.
    for (trace_id, token) in completed {
        sqlx::query(
            "DELETE FROM retention_cleanup \
             WHERE project_id = ? AND trace_id = ? AND claim_token = ?",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "retention_cleanup_tests.rs"]
mod tests;
