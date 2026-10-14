//! The retention cleanup record, as `sideseat_adapter_sqlite`'s module of the same name describes it. Tokens
//! come from one sequence (`retention_cleanup_tokens`), so none is carried twice. Every statement that locks
//! cleanup rows takes them in trace order, so a record and a completion of overlapping traces never deadlock.

use sqlx::PgConnection;

use crate::PostgresError;
use sideseat_ports::traits::retention_cleanup_logical_bytes;

/// Record cleanup intent for one trace under a fresh token, due now. A row already there takes the token: new
/// work behind the same identity, which a stale completion must not discard.
pub(crate) async fn record_one(
    connection: &mut PgConnection,
    project_id: &str,
    trace_id: &str,
    now: i64,
) -> Result<i64, PostgresError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO retention_cleanup \
         (project_id, trace_id, created_at, attempts, next_attempt_at, claim_token, logical_bytes) \
         VALUES ($1, $2, $3, 0, 0, nextval('retention_cleanup_tokens'), $4) \
         ON CONFLICT (project_id, trace_id) DO UPDATE SET \
             claim_token = excluded.claim_token, next_attempt_at = 0 \
         RETURNING claim_token",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(now)
    .bind(i64::try_from(retention_cleanup_logical_bytes(project_id, trace_id)).unwrap_or(i64::MAX))
    .fetch_one(&mut *connection)
    .await?)
}

/// Record cleanup intent for these traces, each once under a fresh token, due now, in trace order: two
/// transactions recording overlapping traces then lock their rows in one order.
pub async fn record_retention_cleanup(
    connection: &mut PgConnection,
    project_id: &str,
    trace_ids: &[String],
    now: i64,
) -> Result<Vec<(String, i64)>, PostgresError> {
    let mut ordered: Vec<&String> = trace_ids.iter().collect();
    ordered.sort_unstable();
    ordered.dedup();
    let mut written = Vec::with_capacity(ordered.len());
    for trace_id in ordered {
        let token = record_one(connection, project_id, trace_id, now).await?;
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
    connection: &mut PgConnection,
    limit: i64,
    lease_secs: i64,
    now: i64,
) -> Result<Vec<(String, String, i64)>, PostgresError> {
    // The token is **returned and not changed here** - a new token is what a *re-record* means, so a worker
    // holding a token whose row was re-recorded finds no match on completion and leaves the new work alone.
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        // `FOR UPDATE SKIP LOCKED` on the inner select, which the SQLite twin does not need: with several
        // replicas a blocked one can otherwise resume against a stale subquery snapshot and return ids another
        // replica has already claimed. The same reason the file and project claims use it.
        "UPDATE retention_cleanup \
         SET attempts = attempts + 1, \
             next_attempt_at = $1 + $2 * LEAST(attempts + 1, 8) \
         WHERE (project_id, trace_id) IN ( \
             SELECT project_id, trace_id FROM retention_cleanup \
             WHERE next_attempt_at <= $3 ORDER BY next_attempt_at LIMIT $4 FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING project_id, trace_id, claim_token",
    )
    .bind(now)
    .bind(lease_secs)
    .bind(now)
    .bind(limit)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows)
}

/// Drop candidates whose cleanup completed, only if still on the claimed token.
pub async fn complete_retention_cleanup(
    connection: &mut PgConnection,
    project_id: &str,
    completed: &[(String, i64)],
) -> Result<(), PostgresError> {
    // In trace order, as records lock them: a completion and a record of overlapping traces then never wait on
    // each other's rows in opposite orders.
    let mut ordered: Vec<&(String, i64)> = completed.iter().collect();
    ordered.sort_unstable();
    for (trace_id, token) in ordered {
        sqlx::query(
            "DELETE FROM retention_cleanup \
             WHERE project_id = $1 AND trace_id = $2 AND claim_token = $3",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(token)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}
