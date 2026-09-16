//! The deletion journal: the deletions a restore cannot recompute.
//!
//! See [`sideseat_ports::traits::DeletionJournal`] for what it is for and why it is permanent. This file is
//! the PostgreSQL half; `sqlite/repositories/journal.rs` is the other, and the two are compared by the
//! PostgreSQL parity suite.

use chrono::DateTime;
use sqlx::{PgPool, Row};

use super::super::error::PostgresError;
use sideseat_ports::traits::{DeletionCause, DeletionRecord, DeletionScope};

/// Append these deletions, all or none.
///
/// One transaction, because a partial append is a deletion with no record for some of its targets - which is
/// exactly the state the append-before-delete ordering exists to make impossible.
pub async fn append_deletions(
    pool: &PgPool,
    records: &[DeletionRecord],
) -> Result<(), PostgresError> {
    if records.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for record in records {
        sqlx::query(
            "INSERT INTO deletion_journal (project_id, cause, scope, target_id, span_id, recorded_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&record.project_id)
        .bind(record.cause.as_str())
        .bind(record.scope.as_str())
        .bind(&record.target_id)
        .bind(record.span_id.as_deref())
        // Nanoseconds, matching every other instant in this schema. `timestamp_nanos_opt` answers `None` past
        // 2262 and the fallback is the maximum rather than zero: an entry that sorted to the beginning of time
        // would be replayed first and then be indistinguishable from the oldest deletion in the journal.
        .bind(
            record
                .recorded_at
                .timestamp_nanos_opt()
                .unwrap_or(i64::MAX),
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Entries after `after_sequence`, oldest first, with the highest sequence this page examined.
///
/// The second value is what keeps a replay making progress across rows it cannot interpret: skipped rows would
/// otherwise leave the cursor where it was, and a page of nothing but skipped rows reads exactly like the end of
/// the journal.
pub async fn deletions_since(
    pool: &PgPool,
    after_sequence: i64,
    limit: usize,
) -> Result<(Vec<(i64, DeletionRecord)>, i64), PostgresError> {
    let rows = sqlx::query(
        "SELECT sequence, project_id, cause, scope, target_id, span_id, recorded_at
         FROM deletion_journal
         WHERE sequence > $1
         ORDER BY sequence ASC
         LIMIT $2",
    )
    .bind(after_sequence)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    let mut examined = after_sequence;
    for row in rows {
        let sequence: i64 = row.try_get("sequence")?;
        // Advanced for every row read, before any decision about whether it is interpretable.
        examined = examined.max(sequence);
        let cause_text: String = row.try_get("cause")?;
        let scope_text: String = row.try_get("scope")?;
        // An unparseable spelling is skipped rather than guessed at. A replay that treated an unknown scope as
        // some known one would delete the wrong thing, which is worse than not replaying an entry a future
        // version wrote - and the `CHECK` constraint means this can only be reached by a newer writer.
        let (Some(cause), Some(scope)) = (
            DeletionCause::from_stored(&cause_text),
            DeletionScope::from_stored(&scope_text),
        ) else {
            tracing::warn!(
                sequence,
                cause = %cause_text,
                scope = %scope_text,
                "deletion journal entry has a cause or scope this build does not know; skipping it rather \
                 than guessing what it meant"
            );
            continue;
        };
        let nanos: i64 = row.try_get("recorded_at")?;
        out.push((
            sequence,
            DeletionRecord {
                project_id: row.try_get("project_id")?,
                cause,
                scope,
                target_id: row.try_get("target_id")?,
                span_id: row.try_get("span_id")?,
                // A nanosecond count out of `DateTime` range reads as the maximum rather than as the epoch:
                // an entry stamped at the beginning of time would be replayed first and would then be
                // indistinguishable from the journal's oldest genuine deletion.
                recorded_at: DateTime::from_timestamp_nanos(nanos),
            },
        ));
    }
    Ok((out, examined))
}

/// Whether the journal explains this record's absence.
///
/// A `Span` target is answered by an entry for the span **or** for its trace, because deleting the trace
/// removed the span too - and a sweep that only asked about the span would re-drive a payload whose trace was
/// deliberately deleted.
pub async fn deletion_is_journaled(
    pool: &PgPool,
    project_id: &str,
    scope: DeletionScope,
    target_id: &str,
    span_id: Option<&str>,
) -> Result<bool, PostgresError> {
    if scope == DeletionScope::Span {
        let found: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM deletion_journal
             WHERE project_id = $1
               AND target_id = $2
               AND (scope = 'trace' OR (scope = 'span' AND span_id = $3))
             LIMIT 1",
        )
        .bind(project_id)
        .bind(target_id)
        .bind(span_id)
        .fetch_optional(pool)
        .await?;
        return Ok(found.is_some());
    }

    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM deletion_journal
         WHERE project_id = $1 AND scope = $2 AND target_id = $3
         LIMIT 1",
    )
    .bind(project_id)
    .bind(scope.as_str())
    .bind(target_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}
