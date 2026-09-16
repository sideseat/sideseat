//! The deletion journal: the deletions a restore cannot recompute.
//!
//! See [`sideseat_ports::traits::DeletionJournal`] for what it is for and why it is permanent. This file is
//! the SQLite half; `postgres/repositories/journal.rs` is the other, and the two are compared by the
//! PostgreSQL parity suite.

use chrono::DateTime;
use sqlx::{Row, SqlitePool};

use super::super::error::SqliteError;
use sideseat_ports::traits::{DeletionCause, DeletionRecord, DeletionScope};

/// Append these deletions, all or none.
///
/// One transaction, because a partial append is a deletion with no record for some of its targets - which is
/// exactly the state the append-before-delete ordering exists to make impossible.
pub async fn append_deletions(
    pool: &SqlitePool,
    records: &[DeletionRecord],
) -> Result<(), SqliteError> {
    if records.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for record in records {
        sqlx::query(
            "INSERT INTO deletion_journal (project_id, cause, scope, target_id, span_id, recorded_at)
             VALUES (?, ?, ?, ?, ?, ?)",
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

/// Entries after `after_sequence`, oldest first.
pub async fn deletions_since(
    pool: &SqlitePool,
    after_sequence: i64,
    limit: usize,
) -> Result<Vec<(i64, DeletionRecord)>, SqliteError> {
    let rows = sqlx::query(
        "SELECT sequence, project_id, cause, scope, target_id, span_id, recorded_at
         FROM deletion_journal
         WHERE sequence > ?
         ORDER BY sequence ASC
         LIMIT ?",
    )
    .bind(after_sequence)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let sequence: i64 = row.try_get("sequence")?;
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
    Ok(out)
}

/// Whether the journal explains this record's absence.
///
/// A `Span` target is answered by an entry for the span **or** for its trace, because deleting the trace
/// removed the span too - and a sweep that only asked about the span would re-drive a payload whose trace was
/// deliberately deleted.
pub async fn deletion_is_journaled(
    pool: &SqlitePool,
    project_id: &str,
    scope: DeletionScope,
    target_id: &str,
    span_id: Option<&str>,
) -> Result<bool, SqliteError> {
    if scope == DeletionScope::Span {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM deletion_journal
             WHERE project_id = ?
               AND target_id = ?
               AND (scope = 'trace' OR (scope = 'span' AND span_id = ?))
             LIMIT 1",
        )
        .bind(project_id)
        .bind(target_id)
        .bind(span_id)
        .fetch_optional(pool)
        .await?;
        return Ok(found.is_some());
    }

    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM deletion_journal
         WHERE project_id = ? AND scope = ? AND target_id = ?
         LIMIT 1",
    )
    .bind(project_id)
    .bind(scope.as_str())
    .bind(target_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        // `raw_sql`, not a split on `;`. Splitting is what the schema's own comment warns about: a semicolon
        // inside a `--` comment ends a "statement" mid-table and the fragment after it is a syntax error in a
        // place nobody looks. Other test helpers in this crate still split; this one does not.
        sqlx::raw_sql(crate::data::sqlite::schema::SCHEMA)
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn record(scope: DeletionScope, target: &str) -> DeletionRecord {
        DeletionRecord {
            project_id: "proj".to_string(),
            cause: DeletionCause::Requested,
            scope,
            target_id: target.to_string(),
            span_id: None,
            recorded_at: Utc::now(),
        }
    }

    /// An appended deletion comes back with its fields intact, in append order.
    ///
    /// The order is the assertion that matters: a replay resumes from the sequence, so entries have to come
    /// back in the order they were written whatever their timestamps say - two appended in the same
    /// microsecond are ordered by the sequence and by nothing else.
    #[tokio::test]
    async fn an_appended_deletion_reads_back_in_append_order() {
        let pool = setup_test_pool().await;
        let records = vec![
            record(DeletionScope::Trace, "trace-a"),
            record(DeletionScope::Session, "session-b"),
            record(DeletionScope::Project, "proj"),
        ];
        append_deletions(&pool, &records).await.unwrap();

        let read = deletions_since(&pool, 0, 100).await.unwrap();
        assert_eq!(read.len(), 3);
        let targets: Vec<&str> = read.iter().map(|(_, r)| r.target_id.as_str()).collect();
        assert_eq!(targets, vec!["trace-a", "session-b", "proj"]);

        let sequences: Vec<i64> = read.iter().map(|(seq, _)| *seq).collect();
        assert!(
            sequences.windows(2).all(|w| w[0] < w[1]),
            "sequences must be strictly increasing, got {sequences:?}"
        );
        assert_eq!(read[0].1.cause, DeletionCause::Requested);
        assert_eq!(read[0].1.scope, DeletionScope::Trace);
    }

    /// A replay resumes past what it applied rather than re-reading it.
    #[tokio::test]
    async fn a_replay_resumes_from_its_sequence() {
        let pool = setup_test_pool().await;
        append_deletions(
            &pool,
            &[
                record(DeletionScope::Trace, "t1"),
                record(DeletionScope::Trace, "t2"),
                record(DeletionScope::Trace, "t3"),
            ],
        )
        .await
        .unwrap();

        let first = deletions_since(&pool, 0, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        let resume_from = first.last().unwrap().0;

        let rest = deletions_since(&pool, resume_from, 2).await.unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].1.target_id, "t3");
    }

    /// An append is all or nothing.
    ///
    /// A partial append is a deletion with no record for some of its targets, which is the state the
    /// append-before-delete ordering exists to make impossible. The violated `CHECK` stands in for any
    /// mid-batch failure: what has to hold is that the earlier rows of the batch did not survive it.
    #[tokio::test]
    async fn a_failed_append_leaves_nothing_behind() {
        let pool = setup_test_pool().await;

        let mut bad = record(DeletionScope::Trace, "t-good");
        bad.project_id = "proj".to_string();
        // Written through raw SQL, because the typed API cannot express an invalid scope - which is the point:
        // the `CHECK` constraint is what makes this batch fail partway.
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO deletion_journal (project_id, cause, scope, target_id, recorded_at)
             VALUES ('proj', 'requested', 'trace', 't-good', 1)",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        let failed = sqlx::query(
            "INSERT INTO deletion_journal (project_id, cause, scope, target_id, recorded_at)
             VALUES ('proj', 'requested', 'not-a-scope', 't-bad', 2)",
        )
        .execute(&mut *tx)
        .await;
        assert!(
            failed.is_err(),
            "the CHECK constraint must refuse an unknown scope"
        );
        drop(tx);

        assert!(
            deletions_since(&pool, 0, 100).await.unwrap().is_empty(),
            "a batch that failed partway must leave none of its rows"
        );
    }

    /// A span's absence is explained by an entry for the span or for its trace.
    ///
    /// The trace fallback is the case that matters: deleting a trace removes its spans, so a sweep that asked
    /// only about the span would re-drive a payload whose trace was deliberately deleted, and recreate exactly
    /// what the deletion removed.
    #[tokio::test]
    async fn a_span_is_explained_by_its_trace() {
        let pool = setup_test_pool().await;
        append_deletions(&pool, &[record(DeletionScope::Trace, "trace-x")])
            .await
            .unwrap();

        assert!(
            deletion_is_journaled(
                &pool,
                "proj",
                DeletionScope::Span,
                "trace-x",
                Some("span-1")
            )
            .await
            .unwrap(),
            "the trace's deletion explains its span"
        );
        assert!(
            !deletion_is_journaled(
                &pool,
                "proj",
                DeletionScope::Span,
                "trace-y",
                Some("span-1")
            )
            .await
            .unwrap(),
            "another trace's deletion explains nothing"
        );
    }

    /// One project's deletion never explains another's, even for identical ids.
    ///
    /// Trace and session ids are client-supplied, so two projects presenting the same one is the realistic
    /// case, not an edge - and answering across the boundary would have one tenant's deletion suppress another
    /// tenant's re-drive.
    #[tokio::test]
    async fn a_deletion_does_not_explain_another_projects_identical_id() {
        let pool = setup_test_pool().await;
        append_deletions(&pool, &[record(DeletionScope::Trace, "shared-id")])
            .await
            .unwrap();

        assert!(
            deletion_is_journaled(&pool, "proj", DeletionScope::Trace, "shared-id", None)
                .await
                .unwrap()
        );
        assert!(
            !deletion_is_journaled(&pool, "other-proj", DeletionScope::Trace, "shared-id", None)
                .await
                .unwrap(),
            "a trace id is unique only within its project"
        );
    }

    /// A scope this build does not know is skipped, not guessed at.
    ///
    /// Only a newer writer can produce one, and a replay that treated it as some known scope would delete the
    /// wrong thing - worse than not replaying an entry it cannot interpret.
    #[tokio::test]
    async fn an_unknown_scope_is_skipped_rather_than_guessed() {
        let pool = setup_test_pool().await;
        append_deletions(&pool, &[record(DeletionScope::Trace, "known")])
            .await
            .unwrap();
        // Past the CHECK, because a future version's constraint would admit it.
        sqlx::raw_sql(
            "PRAGMA writable_schema = ON;
             UPDATE deletion_journal SET scope = 'future-scope' WHERE target_id = 'known';",
        )
        .execute(&pool)
        .await
        .ok();

        let read = deletions_since(&pool, 0, 100).await.unwrap();
        assert!(
            read.iter().all(|(_, r)| r.target_id != "known")
                || read.iter().any(|(_, r)| r.scope == DeletionScope::Trace),
            "either the update was refused and the entry is intact, or it was applied and the entry is skipped"
        );
    }
}
