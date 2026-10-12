//! The raw-record arbiter's statements for PostgreSQL. The protocol is
//! `server/specs/RawRecordOwnership.tla`; each function names the action it implements.
//!
//! PostgreSQL runs writers concurrently under READ COMMITTED, so the model's single steps are rebuilt from
//! locks, always taken in one order - the project's `raw_arbiter` row, then trace locks in sorted order:
//!
//! - the confirm fence and the collection decision both lock the project's `raw_arbiter` row first, so a
//!   grant and a decision serialize - neither can test what the other is about to change;
//! - the confirm fence and the journalled trace deletions both take a transaction-scoped advisory lock per
//!   trace ([`lock_traces`]), so a grant can never insert an owner for a trace whose uncommitted tombstone it
//!   cannot see, and a deletion's owner release never misses an uncommitted grant.
//!
//! Every function runs inside the caller's transaction (the repository macros own it).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use crate::PostgresError;
use sideseat_ports::types::{
    ProjectId, RawCollection, RawConfirmOutcome, RawCutoff, RawOwnerClaim, raw_record_key,
    raw_trace_key,
};

/// Ensure the project's epoch row exists and lock it for this transaction; returns the epoch.
async fn lock_arbiter(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<i64, PostgresError> {
    sqlx::query("INSERT INTO raw_arbiter (project_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(project_id.as_str())
        .execute(&mut *connection)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT epoch FROM raw_arbiter WHERE project_id = $1 FOR UPDATE")
            .bind(project_id.as_str())
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// Take the transaction-scoped lock of each of these traces, in sorted order so two holders of overlapping
/// sets cannot deadlock. Shared by the confirm fence and the journalled trace deletions.
pub async fn lock_traces(
    connection: &mut PgConnection,
    project_id: &str,
    trace_ids: &[String],
) -> Result<(), PostgresError> {
    let mut sorted: Vec<&String> = trace_ids.iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return Ok(());
    }
    // One statement per lock: a set-returning form gives no guarantee about the order its target list is
    // evaluated in, and the order is what rules out deadlock.
    for trace_id in sorted {
        sqlx::query(
            // A two-key lock: the project and the trace hashed apart, so neither can bleed into the other's
            // text (PostgreSQL text holds no separator byte it cannot also hold in an id).
            "SELECT pg_advisory_xact_lock(hashtext('sideseat-raw-owner:' || $1), hashtext($2))",
        )
        .bind(project_id)
        .bind(trace_id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

pub async fn epoch(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<i64, PostgresError> {
    let epoch: Option<i64> =
        sqlx::query_scalar("SELECT epoch FROM raw_arbiter WHERE project_id = $1")
            .bind(project_id.as_str())
            .fetch_optional(&mut *connection)
            .await?;
    Ok(epoch.unwrap_or(0))
}

/// Each collected record's cutoff (`LiveStore`); records never collected are absent.
pub async fn cutoffs(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashMap<String, RawCutoff>, PostgresError> {
    if raw_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT raw_id, high_water, pending_epoch FROM raw_collections
         WHERE project_id = $1 AND raw_id = ANY($2)",
    )
    .bind(project_id.as_str())
    .bind(raw_ids)
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>(0)?,
                RawCutoff {
                    high_water: row.try_get(1)?,
                    pending: row.try_get(2)?,
                },
            ))
        })
        .collect()
}

/// The confirm fence (`Confirm`), serialized against decisions by the arbiter row lock and against
/// deletions by the trace locks.
pub async fn confirm_owners(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    claims: &[RawOwnerClaim],
) -> Result<Vec<RawConfirmOutcome>, PostgresError> {
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let epoch = lock_arbiter(connection, project_id).await?;
    let grant_seq: i64 = sqlx::query_scalar(
        "UPDATE raw_arbiter SET grant_seq = grant_seq + 1 WHERE project_id = $1 RETURNING grant_seq",
    )
    .bind(project_id.as_str())
    .fetch_one(&mut *connection)
    .await?;
    let traces: Vec<String> = claims
        .iter()
        .flat_map(|claim| claim.trace_ids.iter().cloned())
        .collect();
    lock_traces(connection, project_id.as_str(), &traces).await?;
    let mut outcomes = Vec::with_capacity(claims.len());
    for claim in claims {
        let cutoff: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT high_water, pending_epoch FROM raw_collections WHERE project_id = $1 AND raw_id = $2",
        )
        .bind(project_id.as_str())
        .bind(&claim.raw_id)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some((high_water, pending)) = cutoff {
            if pending.is_some() {
                outcomes.push(RawConfirmOutcome::Collecting);
                continue;
            }
            if claim.version_epoch <= high_water {
                outcomes.push(RawConfirmOutcome::Refused { fresh_epoch: epoch });
                continue;
            }
        }
        let trace_keys: Vec<Vec<u8>> = claim
            .trace_ids
            .iter()
            .map(|trace_id| raw_trace_key(trace_id))
            .collect();
        sqlx::query(
            "INSERT INTO raw_owners (project_id, record_key, trace_key, grant_seq)
             SELECT $1, $2, u.trace_key, $5 FROM UNNEST($3::text[], $4::bytea[]) AS u(trace_id, trace_key)
             WHERE NOT EXISTS (
                 SELECT 1 FROM deleted_traces WHERE project_id = $1 AND trace_id = u.trace_id
             )
             ON CONFLICT (project_id, record_key, trace_key) DO UPDATE SET grant_seq = excluded.grant_seq",
        )
        .bind(project_id.as_str())
        .bind(raw_record_key(&claim.raw_id))
        .bind(&claim.trace_ids)
        .bind(&trace_keys)
        .bind(grant_seq)
        .execute(&mut *connection)
        .await?;
        outcomes.push(RawConfirmOutcome::Granted);
    }
    Ok(outcomes)
}

pub async fn grant_seq(
    connection: &mut PgConnection,
    project_id: &ProjectId,
) -> Result<i64, PostgresError> {
    let seq: Option<i64> =
        sqlx::query_scalar("SELECT grant_seq FROM raw_arbiter WHERE project_id = $1")
            .bind(project_id.as_str())
            .fetch_optional(&mut *connection)
            .await?;
    Ok(seq.unwrap_or(0))
}

pub async fn owners_of_traces(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<(i64, Vec<u8>)>, PostgresError> {
    if trace_ids.is_empty() {
        return Ok(Vec::new());
    }
    let trace_keys: Vec<Vec<u8>> = trace_ids
        .iter()
        .map(|trace_id| raw_trace_key(trace_id))
        .collect();
    Ok(sqlx::query_as(
        "SELECT record_key, trace_key FROM raw_owners WHERE project_id = $1 AND trace_key = ANY($2)
         ORDER BY record_key, trace_key",
    )
    .bind(project_id.as_str())
    .bind(&trace_keys)
    .fetch_all(&mut *connection)
    .await?)
}

/// Which of these pairs are owners, by primary key.
pub async fn owned_pairs(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    pairs: &[(i64, Vec<u8>)],
) -> Result<Vec<(i64, Vec<u8>)>, PostgresError> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let records: Vec<i64> = pairs.iter().map(|(record_key, _)| *record_key).collect();
    let traces: Vec<Vec<u8>> = pairs
        .iter()
        .map(|(_, trace_key)| trace_key.clone())
        .collect();
    Ok(sqlx::query_as(
        "SELECT o.record_key, o.trace_key FROM raw_owners o \
         JOIN UNNEST($2::bigint[], $3::bytea[]) AS p(record_key, trace_key) \
           ON o.record_key = p.record_key AND o.trace_key = p.trace_key \
         WHERE o.project_id = $1 ORDER BY o.record_key, o.trace_key",
    )
    .bind(project_id.as_str())
    .bind(&records)
    .bind(&traces)
    .fetch_all(&mut *connection)
    .await?)
}

pub async fn owner_traces(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    raw_id: &str,
) -> Result<Vec<Vec<u8>>, PostgresError> {
    Ok(sqlx::query_scalar(
        "SELECT trace_key FROM raw_owners WHERE project_id = $1 AND record_key = $2 ORDER BY trace_key",
    )
    .bind(project_id.as_str())
    .bind(raw_record_key(raw_id))
    .fetch_all(&mut *connection)
    .await?)
}

/// Release these owners (`ExpireSweep`), only those granted at or below `granted_up_to`. A concurrent confirm
/// either stamps a grant above the bound before this statement reads the row - and it stays - or waits on the
/// row this statement deletes and inserts it afresh, above the bound.
pub async fn release_owners(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    owners: &[(i64, Vec<u8>)],
    granted_up_to: i64,
) -> Result<(), PostgresError> {
    if owners.is_empty() {
        return Ok(());
    }
    let (records, traces): (Vec<i64>, Vec<Vec<u8>>) = owners.iter().cloned().unzip();
    sqlx::query(
        "DELETE FROM raw_owners o USING UNNEST($2::bigint[], $3::bytea[]) AS u(record_key, trace_key)
         WHERE o.project_id = $1 AND o.record_key = u.record_key AND o.trace_key = u.trace_key
           AND o.grant_seq <= $4",
    )
    .bind(project_id.as_str())
    .bind(&records)
    .bind(&traces)
    .bind(granted_up_to)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// The collection decision (`RAct`), serialized against grants by the arbiter row lock.
pub async fn collect_records(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    raw_ids: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<RawCollection>, PostgresError> {
    if raw_ids.is_empty() {
        return Ok(Vec::new());
    }
    lock_arbiter(connection, project_id).await?;
    let mut decided = Vec::new();
    for raw_id in raw_ids {
        let epoch: Option<i64> = sqlx::query_scalar(
            "INSERT INTO raw_collections (project_id, raw_id, pending_epoch, decided_at)
             SELECT $1, $2, (SELECT epoch FROM raw_arbiter WHERE project_id = $1), $3
             WHERE NOT EXISTS (SELECT 1 FROM raw_owners  WHERE project_id = $1 AND record_key = $4)
               AND NOT EXISTS (SELECT 1 FROM raw_writers WHERE project_id = $1 AND raw_id = $2)
               AND NOT EXISTS (SELECT 1 FROM project_holds WHERE project_id = $1 AND hold_until >= $3)
             ON CONFLICT (project_id, raw_id) DO UPDATE
                 SET pending_epoch = excluded.pending_epoch, decided_at = excluded.decided_at
                 WHERE raw_collections.pending_epoch IS NULL
             RETURNING pending_epoch",
        )
        .bind(project_id.as_str())
        .bind(raw_id)
        .bind(now.timestamp_micros())
        .bind(raw_record_key(raw_id))
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(epoch) = epoch {
            sqlx::query("UPDATE raw_arbiter SET epoch = epoch + 1 WHERE project_id = $1")
                .bind(project_id.as_str())
                .execute(&mut *connection)
                .await?;
            decided.push(RawCollection {
                project_id: project_id.clone(),
                raw_id: raw_id.clone(),
                epoch,
                decided_at: now,
            });
        }
    }
    Ok(decided)
}

/// The purge done, fold the pending decision into the record's high-water mark (`RDrop`).
pub async fn drop_collection(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    raw_id: &str,
    epoch: i64,
) -> Result<(), PostgresError> {
    sqlx::query(
        "UPDATE raw_collections
         SET high_water = pending_epoch, pending_epoch = NULL, decided_at = NULL
         WHERE project_id = $1 AND raw_id = $2 AND pending_epoch = $3",
    )
    .bind(project_id.as_str())
    .bind(raw_id)
    .bind(epoch)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Pending decisions whose purge may have stalled, oldest first (`RResume`).
pub async fn stale_collections(
    connection: &mut PgConnection,
    decided_before: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<RawCollection>, PostgresError> {
    let rows = sqlx::query(
        "SELECT project_id, raw_id, pending_epoch, decided_at FROM raw_collections
         WHERE pending_epoch IS NOT NULL AND decided_at < $1
         ORDER BY decided_at ASC LIMIT $2",
    )
    .bind(decided_before.timestamp_micros())
    .bind(limit as i64)
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RawCollection {
                project_id: ProjectId::from(row.try_get::<String, _>("project_id")?),
                raw_id: row.try_get("raw_id")?,
                epoch: row.try_get("pending_epoch")?,
                decided_at: DateTime::from_timestamp_micros(row.try_get("decided_at")?)
                    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            })
        })
        .collect()
}

/// Whether a staged payload or staged rewrite is registered as this record's pending writer.
pub async fn writer_pending(
    connection: &mut PgConnection,
    project_id: &ProjectId,
    raw_id: &str,
) -> Result<bool, PostgresError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM raw_writers WHERE project_id = $1 AND raw_id = $2 LIMIT 1",
    )
    .bind(project_id.as_str())
    .bind(raw_id)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(found.is_some())
}
