//! The retired content-body registry: what is left is the drain. See `ContentBodyStore` in the ports.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use sideseat_ports::types::ProjectId;

use crate::SqliteError;

/// Delete up to `limit` associations. Nothing reads or writes them any more; see the port.
pub async fn retire_associations(pool: &SqlitePool, limit: usize) -> Result<u64, SqliteError> {
    Ok(sqlx::query(
        "DELETE FROM span_bodies WHERE rowid IN (SELECT rowid FROM span_bodies LIMIT ?)",
    )
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn orphans(
    pool: &SqlitePool,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, SqliteError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT b.project_id, b.body_hash FROM content_bodies b
         WHERE b.deleting_at IS NULL AND b.last_referenced_at <= ? AND NOT EXISTS (
             SELECT 1 FROM span_bodies r
             WHERE r.project_id = b.project_id AND r.body_hash = b.body_hash
         )
         ORDER BY b.last_referenced_at, b.project_id, b.body_hash LIMIT ?",
    )
    .bind(older_than.timestamp_nanos_opt().unwrap_or(0))
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project, hash)| (ProjectId::from(project), hash))
        .collect())
}

pub async fn stale_claims(
    pool: &SqlitePool,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, SqliteError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT project_id, body_hash FROM content_bodies
         WHERE deleting_at IS NOT NULL AND deleting_at <= ?
         ORDER BY deleting_at, project_id, body_hash LIMIT ?",
    )
    .bind(older_than.timestamp_micros())
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project, hash)| (ProjectId::from(project), hash))
        .collect())
}

pub async fn claim_for_deletion(
    pool: &SqlitePool,
    project_id: &ProjectId,
    body_hash: &str,
    now: DateTime<Utc>,
) -> Result<bool, SqliteError> {
    Ok(sqlx::query(
        "UPDATE content_bodies SET deleting_at = ?
         WHERE project_id = ? AND body_hash = ? AND deleting_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = ? AND body_hash = ?
           )",
    )
    .bind(now.timestamp_micros())
    .bind(project_id.as_str())
    .bind(body_hash)
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn release_deletion_claim(
    pool: &SqlitePool,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<(), SqliteError> {
    sqlx::query(
        "UPDATE content_bodies SET deleting_at = NULL
         WHERE project_id = ? AND body_hash = ?",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_claimed(
    pool: &SqlitePool,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<bool, SqliteError> {
    Ok(sqlx::query(
        "DELETE FROM content_bodies
         WHERE project_id = ? AND body_hash = ? AND deleting_at IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = ? AND body_hash = ?
           )",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete_project(
    pool: &SqlitePool,
    project_id: &ProjectId,
) -> Result<Vec<String>, SqliteError> {
    let mut tx = pool.begin().await?;
    let hashes = sqlx::query_scalar::<_, String>(
        "SELECT body_hash FROM content_bodies WHERE project_id = ? ORDER BY body_hash",
    )
    .bind(project_id.as_str())
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM content_bodies WHERE project_id = ?")
        .bind(project_id.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM content_body_backfill WHERE project_id = ?")
        .bind(project_id.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.expect("pool");
        sqlx::raw_sql(crate::schema::SCHEMA)
            .execute(&pool)
            .await
            .expect("schema");
        pool
    }

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }

    /// What an earlier version left behind: a registered object and a durable association to it.
    async fn legacy_body(pool: &SqlitePool, hash: &str) {
        sqlx::query(
            "INSERT INTO content_bodies (project_id, body_hash, logical_bytes, created_at, last_referenced_at)
             VALUES ('project', ?, 10, 0, 0)",
        )
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO span_bodies (project_id, trace_id, span_id, field, body_hash, pending_writers, durable)
             VALUES ('project', 'trace', ?, 'messages', ?, 0, 1)",
        )
        .bind(hash)
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();
    }

    /// An associated object cannot be claimed; once the associations are retired, page by page, it can,
    /// and the claim then deletes it.
    #[tokio::test]
    async fn retiring_associations_turns_objects_into_claimable_orphans() {
        let pool = pool().await;
        let project = ProjectId::from("project");
        for hash in ["a", "b", "c"] {
            legacy_body(&pool, hash).await;
        }
        assert!(
            !claim_for_deletion(&pool, &project, "a", now())
                .await
                .unwrap()
        );
        assert!(orphans(&pool, now(), 10).await.unwrap().is_empty());

        assert_eq!(retire_associations(&pool, 2).await.unwrap(), 2);
        assert_eq!(retire_associations(&pool, 2).await.unwrap(), 1);
        assert_eq!(retire_associations(&pool, 2).await.unwrap(), 0);

        let found = orphans(&pool, now(), 10).await.unwrap();
        assert_eq!(found.len(), 3);
        assert!(
            claim_for_deletion(&pool, &project, "a", now())
                .await
                .unwrap()
        );
        assert!(delete_claimed(&pool, &project, "a").await.unwrap());
        assert_eq!(orphans(&pool, now(), 10).await.unwrap().len(), 2);
    }

    /// A worker that crashed after claiming leaves a claim the stale pass reports, and releasing it makes the
    /// object an ordinary orphan again.
    #[tokio::test]
    async fn an_abandoned_claim_is_recoverable() {
        let pool = pool().await;
        let project = ProjectId::from("project");
        legacy_body(&pool, "claimed").await;
        retire_associations(&pool, 10).await.unwrap();
        assert!(
            claim_for_deletion(&pool, &project, "claimed", now())
                .await
                .unwrap()
        );
        assert!(orphans(&pool, now(), 10).await.unwrap().is_empty());
        assert_eq!(
            stale_claims(&pool, now() + chrono::Duration::seconds(1), 10)
                .await
                .unwrap(),
            vec![(project.clone(), "claimed".to_string())]
        );
        release_deletion_claim(&pool, &project, "claimed")
            .await
            .unwrap();
        assert_eq!(orphans(&pool, now(), 10).await.unwrap().len(), 1);
    }
}
