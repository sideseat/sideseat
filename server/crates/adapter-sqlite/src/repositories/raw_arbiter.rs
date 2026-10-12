//! The raw-record arbiter's statements: owners, pending writers, the project epoch, and each collected record's
//! collection state. The protocol is `server/specs/RawRecordOwnership.tla`; each function names the action it
//! implements.
//!
//! SQLite has one writer at a time. Every fence below opens its transaction with a write, which takes the write
//! lock before anything is read: a deferred transaction that read first could not upgrade after a concurrent
//! writer committed (`SQLITE_BUSY_SNAPSHOT`), so the fence, the decision and the journalled deletions
//! serialize on the lock exactly as the model's single steps do.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::SqliteError;
use sideseat_ports::types::{
    ProjectId, RawCollection, RawConfirmOutcome, RawCutoff, RawOwnerClaim, raw_record_key,
    raw_trace_key,
};

/// Take the write lock for this transaction and make sure the project's epoch row exists.
async fn lock_arbiter(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &ProjectId,
) -> Result<i64, SqliteError> {
    sqlx::query("INSERT INTO raw_arbiter (project_id) VALUES (?) ON CONFLICT DO NOTHING")
        .bind(project_id.as_str())
        .execute(&mut **tx)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT epoch FROM raw_arbiter WHERE project_id = ?")
            .bind(project_id.as_str())
            .fetch_one(&mut **tx)
            .await?,
    )
}

/// The project epoch new versions are keyed under. A project with no row has decided nothing: epoch 0.
pub async fn epoch(pool: &SqlitePool, project_id: &ProjectId) -> Result<i64, SqliteError> {
    let epoch: Option<i64> =
        sqlx::query_scalar("SELECT epoch FROM raw_arbiter WHERE project_id = ?")
            .bind(project_id.as_str())
            .fetch_optional(pool)
            .await?;
    Ok(epoch.unwrap_or(0))
}

/// Each collected record's cutoff (`LiveStore`); records never collected are absent.
pub async fn cutoffs(
    pool: &SqlitePool,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashMap<String, RawCutoff>, SqliteError> {
    let mut cutoffs = HashMap::new();
    for chunk in raw_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT raw_id, high_water, pending_epoch FROM raw_collections \
             WHERE project_id = ? AND raw_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(project_id.as_str());
        for raw_id in chunk {
            query = query.bind(raw_id);
        }
        for row in query.fetch_all(pool).await? {
            cutoffs.insert(
                row.try_get::<String, _>(0)?,
                RawCutoff {
                    high_water: row.try_get(1)?,
                    pending: row.try_get(2)?,
                },
            );
        }
    }
    Ok(cutoffs)
}

/// The confirm fence (`Confirm`): grant each claim whose version epoch is above its record's cutoff; a granted
/// claim's traces become owners, except traces the deletion journal tombstoned meanwhile. One transaction for
/// the batch, under the write lock, so no decision or deletion can slip between the fence test and the grant.
pub async fn confirm_owners(
    pool: &SqlitePool,
    project_id: &ProjectId,
    claims: &[RawOwnerClaim],
) -> Result<Vec<RawConfirmOutcome>, SqliteError> {
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    let epoch = lock_arbiter(&mut tx, project_id).await?;
    let grant_seq: i64 = sqlx::query_scalar(
        "UPDATE raw_arbiter SET grant_seq = grant_seq + 1 WHERE project_id = ? RETURNING grant_seq",
    )
    .bind(project_id.as_str())
    .fetch_one(&mut *tx)
    .await?;
    let mut outcomes = Vec::with_capacity(claims.len());
    for claim in claims {
        let cutoff: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT high_water, pending_epoch FROM raw_collections WHERE project_id = ? AND raw_id = ?",
        )
        .bind(project_id.as_str())
        .bind(&claim.raw_id)
        .fetch_optional(&mut *tx)
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
        let record_key = raw_record_key(&claim.raw_id);
        for trace_id in &claim.trace_ids {
            sqlx::query(
                "INSERT INTO raw_owners (project_id, record_key, trace_key, grant_seq)
                 SELECT ?1, ?2, ?3, ?4
                 WHERE NOT EXISTS (
                     SELECT 1 FROM deleted_traces WHERE project_id = ?1 AND trace_id = ?5
                 )
                 ON CONFLICT (project_id, record_key, trace_key) DO UPDATE SET grant_seq = excluded.grant_seq",
            )
            .bind(project_id.as_str())
            .bind(record_key)
            .bind(raw_trace_key(trace_id))
            .bind(grant_seq)
            .bind(trace_id)
            .execute(&mut *tx)
            .await?;
        }
        outcomes.push(RawConfirmOutcome::Granted);
    }
    tx.commit().await?;
    Ok(outcomes)
}

/// The project's grant sequence; 0 before any confirm.
pub async fn grant_seq(pool: &SqlitePool, project_id: &ProjectId) -> Result<i64, SqliteError> {
    let seq: Option<i64> =
        sqlx::query_scalar("SELECT grant_seq FROM raw_arbiter WHERE project_id = ?")
            .bind(project_id.as_str())
            .fetch_optional(pool)
            .await?;
    Ok(seq.unwrap_or(0))
}

/// The `(record_key, trace_key)` owners of these traces' records.
pub async fn owners_of_traces(
    pool: &SqlitePool,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<(i64, Vec<u8>)>, SqliteError> {
    let mut owners = Vec::new();
    for chunk in trace_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT record_key, trace_key FROM raw_owners WHERE project_id = ? AND trace_key IN ({placeholders}) \
             ORDER BY record_key, trace_key"
        );
        let mut query = sqlx::query_as::<_, (i64, Vec<u8>)>(&sql).bind(project_id.as_str());
        for trace_id in chunk {
            query = query.bind(raw_trace_key(trace_id));
        }
        owners.extend(query.fetch_all(pool).await?);
    }
    Ok(owners)
}

/// Which of these pairs are owners, by primary key.
pub async fn owned_pairs(
    pool: &SqlitePool,
    project_id: &ProjectId,
    pairs: &[(i64, Vec<u8>)],
) -> Result<Vec<(i64, Vec<u8>)>, SqliteError> {
    let mut owned = Vec::new();
    for chunk in pairs.chunks(250) {
        let placeholders = vec!["(?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT record_key, trace_key FROM raw_owners WHERE project_id = ? \
             AND (record_key, trace_key) IN (VALUES {placeholders}) ORDER BY record_key, trace_key"
        );
        let mut query = sqlx::query_as::<_, (i64, Vec<u8>)>(&sql).bind(project_id.as_str());
        for (record_key, trace_key) in chunk {
            query = query.bind(*record_key).bind(trace_key.as_slice());
        }
        owned.extend(query.fetch_all(pool).await?);
    }
    Ok(owned)
}

/// The trace keys owning this record.
pub async fn owner_traces(
    pool: &SqlitePool,
    project_id: &ProjectId,
    raw_id: &str,
) -> Result<Vec<Vec<u8>>, SqliteError> {
    Ok(sqlx::query_scalar(
        "SELECT trace_key FROM raw_owners WHERE project_id = ? AND record_key = ? ORDER BY trace_key",
    )
    .bind(project_id.as_str())
    .bind(raw_record_key(raw_id))
    .fetch_all(pool)
    .await?)
}

/// Release these owners (`ExpireSweep`), only those granted at or below `granted_up_to`.
pub async fn release_owners(
    pool: &SqlitePool,
    project_id: &ProjectId,
    owners: &[(i64, Vec<u8>)],
    granted_up_to: i64,
) -> Result<(), SqliteError> {
    if owners.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (record_key, trace_key) in owners {
        sqlx::query(
            "DELETE FROM raw_owners \
             WHERE project_id = ? AND record_key = ? AND trace_key = ? AND grant_seq <= ?",
        )
        .bind(project_id.as_str())
        .bind(record_key)
        .bind(trace_key)
        .bind(granted_up_to)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The collection decision (`RAct`): per record, one guarded upsert - no owner, no pending writer, no active
/// legal hold, no decision already pending - recording the project epoch as the pending decision, then the
/// epoch bump. All under the write lock, so a concurrent confirm sees either no decision or the bumped epoch.
pub async fn collect_records(
    pool: &SqlitePool,
    project_id: &ProjectId,
    raw_ids: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<RawCollection>, SqliteError> {
    if raw_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    lock_arbiter(&mut tx, project_id).await?;
    let mut decided = Vec::new();
    for raw_id in raw_ids {
        let epoch: Option<i64> = sqlx::query_scalar(
            "INSERT INTO raw_collections (project_id, raw_id, pending_epoch, decided_at)
             SELECT ?1, ?2, (SELECT epoch FROM raw_arbiter WHERE project_id = ?1), ?3
             WHERE NOT EXISTS (SELECT 1 FROM raw_owners  WHERE project_id = ?1 AND record_key = ?4)
               AND NOT EXISTS (SELECT 1 FROM raw_writers WHERE project_id = ?1 AND raw_id = ?2)
               AND NOT EXISTS (SELECT 1 FROM project_holds WHERE project_id = ?1 AND hold_until >= ?3)
             ON CONFLICT (project_id, raw_id) DO UPDATE
                 SET pending_epoch = excluded.pending_epoch, decided_at = excluded.decided_at
                 WHERE raw_collections.pending_epoch IS NULL
             RETURNING pending_epoch",
        )
        .bind(project_id.as_str())
        .bind(raw_id)
        .bind(now.timestamp_micros())
        .bind(raw_record_key(raw_id))
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(epoch) = epoch {
            sqlx::query("UPDATE raw_arbiter SET epoch = epoch + 1 WHERE project_id = ?")
                .bind(project_id.as_str())
                .execute(&mut *tx)
                .await?;
            decided.push(RawCollection {
                project_id: project_id.clone(),
                raw_id: raw_id.clone(),
                epoch,
                decided_at: now,
            });
        }
    }
    tx.commit().await?;
    Ok(decided)
}

/// The purge done, fold the pending decision into the record's high-water mark (`RDrop`). A drop of a decision
/// already folded - by a takeover that got there first - changes nothing.
pub async fn drop_collection(
    pool: &SqlitePool,
    project_id: &ProjectId,
    raw_id: &str,
    epoch: i64,
) -> Result<(), SqliteError> {
    sqlx::query(
        "UPDATE raw_collections
         SET high_water = pending_epoch, pending_epoch = NULL, decided_at = NULL
         WHERE project_id = ? AND raw_id = ? AND pending_epoch = ?",
    )
    .bind(project_id.as_str())
    .bind(raw_id)
    .bind(epoch)
    .execute(pool)
    .await?;
    Ok(())
}

/// Pending decisions whose purge may have stalled, oldest first (`RResume`).
pub async fn stale_collections(
    pool: &SqlitePool,
    decided_before: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<RawCollection>, SqliteError> {
    let rows = sqlx::query(
        "SELECT project_id, raw_id, pending_epoch, decided_at FROM raw_collections
         WHERE pending_epoch IS NOT NULL AND decided_at < ?
         ORDER BY decided_at ASC LIMIT ?",
    )
    .bind(decided_before.timestamp_micros())
    .bind(limit as i64)
    .fetch_all(pool)
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
    pool: &SqlitePool,
    project_id: &ProjectId,
    raw_id: &str,
) -> Result<bool, SqliteError> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM raw_writers WHERE project_id = ? AND raw_id = ? LIMIT 1")
            .bind(project_id.as_str())
            .bind(raw_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Remove a deleted project's owners, collection state and epoch. Pending writers go with their registrations
/// (`staging::delete_project`), and only those: a registration that lands after the project's cleanup keeps
/// its writer entry, so the record it writes is never collectable under it.
pub async fn delete_project(pool: &SqlitePool, project_id: &ProjectId) -> Result<(), SqliteError> {
    let mut tx = pool.begin().await?;
    for table in ["raw_owners", "raw_collections", "raw_arbiter"] {
        let sql = format!("DELETE FROM {table} WHERE project_id = ?");
        sqlx::query(&sql)
            .bind(project_id.as_str())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
#[path = "raw_arbiter_tests.rs"]
mod raw_arbiter_tests;
