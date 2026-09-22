use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use sideseat_ports::types::{
    ContentBodyBackfillProgress, ContentBodyObject, ProjectId, SpanBodyAssociation, SpanBodyField,
};

use crate::PostgresError;

fn unique(associations: &[SpanBodyAssociation]) -> Vec<&SpanBodyAssociation> {
    let mut rows = associations.iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        (
            a.project_id.as_str(),
            a.trace_id.as_str(),
            a.span_id.as_str(),
            a.field.as_str(),
            a.body_hash.as_str(),
        )
            .cmp(&(
                b.project_id.as_str(),
                b.trace_id.as_str(),
                b.span_id.as_str(),
                b.field.as_str(),
                b.body_hash.as_str(),
            ))
    });
    rows.dedup_by(|a, b| {
        a.project_id == b.project_id
            && a.trace_id == b.trace_id
            && a.span_id == b.span_id
            && a.field == b.field
            && a.body_hash == b.body_hash
    });
    rows
}

pub async fn register(
    pool: &PgPool,
    objects: &[ContentBodyObject],
    now: DateTime<Utc>,
) -> Result<Vec<ContentBodyObject>, PostgresError> {
    let mut rows = objects.iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        (a.project_id.as_str(), a.body_hash.as_str())
            .cmp(&(b.project_id.as_str(), b.body_hash.as_str()))
    });
    rows.dedup_by(|a, b| a.project_id == b.project_id && a.body_hash == b.body_hash);
    let mut tx = pool.begin().await?;
    let now_nanos = now.timestamp_nanos_opt().unwrap_or(0);
    let mut inserted = HashSet::new();
    for chunk in rows.chunks(300) {
        let mut insert = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "INSERT INTO content_bodies
                 (project_id, body_hash, logical_bytes, created_at, last_referenced_at) ",
        );
        insert.push_values(chunk, |mut values, row| {
            values
                .push_bind(row.project_id.as_str())
                .push_bind(&row.body_hash)
                .push_bind(i64::try_from(row.logical_bytes).unwrap_or(i64::MAX))
                .push_bind(now_nanos)
                .push_bind(now_nanos);
        });
        insert.push(
            " ON CONFLICT(project_id, body_hash) DO NOTHING
              RETURNING project_id, body_hash",
        );
        let new_rows: Vec<(String, String)> = insert.build_query_as().fetch_all(&mut *tx).await?;
        inserted.extend(new_rows);

        let mut touch =
            sqlx::QueryBuilder::<sqlx::Postgres>::new("WITH input(project_id, body_hash) AS (");
        touch.push_values(chunk, |mut values, row| {
            values
                .push_bind(row.project_id.as_str())
                .push_bind(&row.body_hash);
        });
        touch.push(
            ") UPDATE content_bodies body
             SET last_referenced_at = ",
        );
        touch.push_bind(now_nanos);
        touch.push(
            " FROM input
             WHERE body.deleting_at IS NULL
               AND input.project_id = body.project_id
               AND input.body_hash = body.body_hash",
        );
        touch.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .filter(|row| inserted.contains(&(row.project_id.to_string(), row.body_hash.clone())))
        .cloned()
        .collect())
}

pub async fn unresolved(
    pool: &PgPool,
    associations: &[SpanBodyAssociation],
) -> Result<Vec<SpanBodyAssociation>, PostgresError> {
    let rows = unique(associations);
    let mut unresolved = Vec::new();
    for chunk in rows.chunks(500) {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "WITH input(ordinal, project_id, trace_id, span_id, field, body_hash) AS (",
        );
        query.push_values(
            chunk.iter().enumerate(),
            |mut row, (ordinal, association)| {
                row.push_bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
                    .push_bind(association.project_id.as_str())
                    .push_bind(&association.trace_id)
                    .push_bind(&association.span_id)
                    .push_bind(association.field.as_str())
                    .push_bind(&association.body_hash);
            },
        );
        query.push(
            ") SELECT ordinal FROM input
             WHERE NOT EXISTS (
                 SELECT 1 FROM span_bodies durable
                 WHERE durable.project_id = input.project_id
                   AND durable.trace_id = input.trace_id
                   AND durable.span_id = input.span_id
                   AND durable.field = input.field
                   AND durable.body_hash = input.body_hash
                   AND durable.durable = TRUE
             )
             ORDER BY ordinal",
        );
        let ordinals: Vec<i64> = query.build_query_scalar().fetch_all(pool).await?;
        unresolved.extend(ordinals.into_iter().filter_map(|ordinal| {
            usize::try_from(ordinal)
                .ok()
                .and_then(|index| chunk.get(index))
                .map(|association| (*association).clone())
        }));
    }
    Ok(unresolved)
}

pub async fn stage(
    pool: &PgPool,
    associations: &[SpanBodyAssociation],
) -> Result<u64, PostgresError> {
    let rows = unique(associations);
    let mut tx = pool.begin().await?;
    let mut staged = 0;
    for chunk in rows.chunks(400) {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "WITH input(project_id, trace_id, span_id, field, body_hash) AS (",
        );
        query.push_values(chunk, |mut values, row| {
            values
                .push_bind(row.project_id.as_str())
                .push_bind(&row.trace_id)
                .push_bind(&row.span_id)
                .push_bind(row.field.as_str())
                .push_bind(&row.body_hash);
        });
        query.push(
            ") INSERT INTO span_bodies
                 (project_id, trace_id, span_id, field, body_hash, pending_writers, durable)
             SELECT input.project_id, input.trace_id, input.span_id, input.field, input.body_hash,
                    1, FALSE
             FROM input
             JOIN content_bodies body
               ON body.project_id = input.project_id AND body.body_hash = input.body_hash
             WHERE body.deleting_at IS NULL
             ON CONFLICT(project_id, trace_id, span_id, field, body_hash)
             DO UPDATE SET pending_writers = span_bodies.pending_writers + 1",
        );
        staged += query.build().execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;
    Ok(staged)
}

pub async fn confirm(
    pool: &PgPool,
    associations: &[SpanBodyAssociation],
) -> Result<u64, PostgresError> {
    let rows = unique(associations);
    let mut tx = pool.begin().await?;
    let mut confirmed = 0;
    for chunk in rows.chunks(400) {
        let mut update = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "WITH input(project_id, trace_id, span_id, field, body_hash) AS (",
        );
        update.push_values(chunk, |mut values, row| {
            values
                .push_bind(row.project_id.as_str())
                .push_bind(&row.trace_id)
                .push_bind(&row.span_id)
                .push_bind(row.field.as_str())
                .push_bind(&row.body_hash);
        });
        update.push(
            ") UPDATE span_bodies body
             SET durable = TRUE, pending_writers = GREATEST(body.pending_writers - 1, 0)
             FROM input
             WHERE input.project_id = body.project_id
               AND input.trace_id = body.trace_id
               AND input.span_id = body.span_id
               AND input.field = body.field
               AND input.body_hash = body.body_hash",
        );
        confirmed += update.build().execute(&mut *tx).await?.rows_affected();

        let mut delete = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "WITH input(project_id, trace_id, span_id, field, body_hash) AS (",
        );
        delete.push_values(chunk, |mut values, row| {
            values
                .push_bind(row.project_id.as_str())
                .push_bind(&row.trace_id)
                .push_bind(&row.span_id)
                .push_bind(row.field.as_str())
                .push_bind(&row.body_hash);
        });
        delete.push(
            ") DELETE FROM span_bodies body
             USING input
             WHERE body.durable = TRUE AND body.pending_writers = 0
               AND input.project_id = body.project_id
               AND input.trace_id = body.trace_id
               AND input.span_id = body.span_id
               AND input.field = body.field
               AND input.body_hash <> body.body_hash",
        );
        delete.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(confirmed)
}

pub async fn release(pool: &PgPool, row: &SpanBodyAssociation) -> Result<bool, PostgresError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE span_bodies SET pending_writers = GREATEST(pending_writers - 1, 0)
         WHERE project_id = $1 AND trace_id = $2 AND span_id = $3
           AND field = $4 AND body_hash = $5",
    )
    .bind(row.project_id.as_str())
    .bind(&row.trace_id)
    .bind(&row.span_id)
    .bind(row.field.as_str())
    .bind(&row.body_hash)
    .execute(&mut *tx)
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM span_bodies
         WHERE project_id = $1 AND trace_id = $2 AND span_id = $3
           AND field = $4 AND body_hash = $5
           AND durable = FALSE AND pending_writers = 0",
    )
    .bind(row.project_id.as_str())
    .bind(&row.trace_id)
    .bind(&row.span_id)
    .bind(row.field.as_str())
    .bind(&row.body_hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    tx.commit().await?;
    Ok(deleted)
}

pub async fn get_hash(
    pool: &PgPool,
    project_id: &ProjectId,
    trace_id: &str,
    span_id: &str,
    field: SpanBodyField,
) -> Result<Option<String>, PostgresError> {
    Ok(sqlx::query_scalar(
        "SELECT body_hash FROM span_bodies
         WHERE project_id = $1 AND trace_id = $2 AND span_id = $3
           AND field = $4 AND durable = TRUE
         ORDER BY body_hash LIMIT 1",
    )
    .bind(project_id.as_str())
    .bind(trace_id)
    .bind(span_id)
    .bind(field.as_str())
    .fetch_optional(pool)
    .await?)
}

pub async fn orphans(
    pool: &PgPool,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, PostgresError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT b.project_id, b.body_hash FROM content_bodies b
         WHERE b.deleting_at IS NULL AND b.last_referenced_at <= $1 AND NOT EXISTS (
             SELECT 1 FROM span_bodies r
             WHERE r.project_id = b.project_id AND r.body_hash = b.body_hash
         )
         ORDER BY b.last_referenced_at, b.project_id, b.body_hash LIMIT $2",
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
    pool: &PgPool,
    older_than: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<(ProjectId, String)>, PostgresError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT project_id, body_hash FROM content_bodies
         WHERE deleting_at IS NOT NULL AND deleting_at <= $1
         ORDER BY deleting_at, project_id, body_hash LIMIT $2",
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
    pool: &PgPool,
    project_id: &ProjectId,
    body_hash: &str,
    now: DateTime<Utc>,
) -> Result<bool, PostgresError> {
    Ok(sqlx::query(
        "UPDATE content_bodies SET deleting_at = $1
         WHERE project_id = $2 AND body_hash = $3 AND deleting_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = $2 AND body_hash = $3
           )",
    )
    .bind(now.timestamp_micros())
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn release_deletion_claim(
    pool: &PgPool,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "UPDATE content_bodies SET deleting_at = NULL
         WHERE project_id = $1 AND body_hash = $2",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_claimed(
    pool: &PgPool,
    project_id: &ProjectId,
    body_hash: &str,
) -> Result<bool, PostgresError> {
    Ok(sqlx::query(
        "DELETE FROM content_bodies
         WHERE project_id = $1 AND body_hash = $2 AND deleting_at IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM span_bodies
               WHERE project_id = $1 AND body_hash = $2
           )",
    )
    .bind(project_id.as_str())
    .bind(body_hash)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete_spans(
    pool: &PgPool,
    project_id: &ProjectId,
    spans: &[(String, String)],
) -> Result<Vec<String>, PostgresError> {
    let mut hashes = Vec::new();
    let mut tx = pool.begin().await?;
    for (trace_id, span_id) in spans {
        hashes.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT body_hash FROM span_bodies
                 WHERE project_id = $1 AND trace_id = $2 AND span_id = $3",
            )
            .bind(project_id.as_str())
            .bind(trace_id)
            .bind(span_id)
            .fetch_all(&mut *tx)
            .await?,
        );
        sqlx::query(
            "DELETE FROM span_bodies
             WHERE project_id = $1 AND trace_id = $2 AND span_id = $3",
        )
        .bind(project_id.as_str())
        .bind(trace_id)
        .bind(span_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    hashes.sort_unstable();
    hashes.dedup();
    Ok(hashes)
}

pub async fn delete_traces(
    pool: &PgPool,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<String>, PostgresError> {
    let mut hashes = Vec::new();
    let mut tx = pool.begin().await?;
    for trace_id in trace_ids {
        hashes.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT body_hash FROM span_bodies
                 WHERE project_id = $1 AND trace_id = $2",
            )
            .bind(project_id.as_str())
            .bind(trace_id)
            .fetch_all(&mut *tx)
            .await?,
        );
        sqlx::query("DELETE FROM span_bodies WHERE project_id = $1 AND trace_id = $2")
            .bind(project_id.as_str())
            .bind(trace_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    hashes.sort_unstable();
    hashes.dedup();
    Ok(hashes)
}

pub async fn delete_project(
    pool: &PgPool,
    project_id: &ProjectId,
) -> Result<Vec<String>, PostgresError> {
    let mut tx = pool.begin().await?;
    let hashes = sqlx::query_scalar::<_, String>(
        "SELECT body_hash FROM content_bodies WHERE project_id = $1 ORDER BY body_hash",
    )
    .bind(project_id.as_str())
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM content_bodies WHERE project_id = $1")
        .bind(project_id.as_str())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM content_body_backfill WHERE project_id = $1")
        .bind(project_id.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(hashes)
}

pub async fn reconcile(
    pool: &PgPool,
    project_id: &ProjectId,
    trace_ids: &[String],
    keep: &[SpanBodyAssociation],
) -> Result<Vec<String>, PostgresError> {
    let keep_set = keep
        .iter()
        .map(|row| {
            (
                row.trace_id.as_str(),
                row.span_id.as_str(),
                row.field.as_str(),
                row.body_hash.as_str(),
            )
        })
        .collect::<std::collections::HashSet<_>>();
    let mut removed = Vec::new();
    let mut tx = pool.begin().await?;
    let mut locked_hashes = std::collections::HashSet::new();
    for row in unique(keep) {
        if !locked_hashes.insert(row.body_hash.as_str()) {
            continue;
        }
        let locked = sqlx::query(
            "UPDATE content_bodies SET last_referenced_at = last_referenced_at
             WHERE project_id = $1 AND body_hash = $2 AND deleting_at IS NULL",
        )
        .bind(project_id.as_str())
        .bind(&row.body_hash)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if !locked {
            return Err(PostgresError::Conflict(format!(
                "content body {} is missing or being deleted",
                row.body_hash
            )));
        }
    }
    for trace_id in trace_ids {
        let rows = sqlx::query_as::<_, (String, String, String, String, i64)>(
            "SELECT trace_id, span_id, field, body_hash, pending_writers::bigint
             FROM span_bodies WHERE project_id = $1 AND trace_id = $2",
        )
        .bind(project_id.as_str())
        .bind(trace_id)
        .fetch_all(&mut *tx)
        .await?;
        for (trace, span, field, hash, pending) in rows {
            if pending == 0
                && !keep_set.contains(&(
                    trace.as_str(),
                    span.as_str(),
                    field.as_str(),
                    hash.as_str(),
                ))
            {
                let deleted = sqlx::query(
                    "DELETE FROM span_bodies
                     WHERE project_id = $1 AND trace_id = $2 AND span_id = $3
                       AND field = $4 AND body_hash = $5 AND pending_writers = 0",
                )
                .bind(project_id.as_str())
                .bind(&trace)
                .bind(&span)
                .bind(&field)
                .bind(&hash)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                    > 0;
                if deleted {
                    removed.push(hash);
                }
            }
        }
    }
    for row in unique(keep) {
        sqlx::query(
            "INSERT INTO span_bodies
                 (project_id, trace_id, span_id, field, body_hash, pending_writers, durable)
             VALUES ($1, $2, $3, $4, $5, 0, TRUE)
             ON CONFLICT(project_id, trace_id, span_id, field, body_hash)
             DO UPDATE SET durable = TRUE",
        )
        .bind(project_id.as_str())
        .bind(&row.trace_id)
        .bind(&row.span_id)
        .bind(row.field.as_str())
        .bind(&row.body_hash)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    removed.sort_unstable();
    removed.dedup();
    Ok(removed)
}

pub async fn backfill_progress(
    pool: &PgPool,
    project_id: &ProjectId,
) -> Result<Option<ContentBodyBackfillProgress>, PostgresError> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>, bool, i64)>(
        "SELECT cursor_trace_id, cursor_span_id, complete, updated_at
         FROM content_body_backfill WHERE project_id = $1",
    )
    .bind(project_id.as_str())
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(cursor_trace_id, cursor_span_id, complete, updated_at)| {
            ContentBodyBackfillProgress {
                project_id: project_id.clone(),
                cursor_trace_id,
                cursor_span_id,
                complete,
                updated_at: DateTime::from_timestamp_micros(updated_at)
                    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            }
        }),
    )
}

pub async fn save_backfill_progress(
    pool: &PgPool,
    progress: &ContentBodyBackfillProgress,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO content_body_backfill
             (project_id, cursor_trace_id, cursor_span_id, complete, updated_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT(project_id) DO UPDATE SET
             cursor_trace_id = excluded.cursor_trace_id,
             cursor_span_id = excluded.cursor_span_id,
             complete = excluded.complete,
             updated_at = excluded.updated_at",
    )
    .bind(progress.project_id.as_str())
    .bind(&progress.cursor_trace_id)
    .bind(&progress.cursor_span_id)
    .bind(progress.complete)
    .bind(progress.updated_at.timestamp_micros())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_backfill(
    pool: &PgPool,
    project_id: &ProjectId,
    now: DateTime<Utc>,
) -> Result<(), PostgresError> {
    save_backfill_progress(
        pool,
        &ContentBodyBackfillProgress {
            project_id: project_id.clone(),
            cursor_trace_id: None,
            cursor_span_id: None,
            complete: false,
            updated_at: now,
        },
    )
    .await
}
