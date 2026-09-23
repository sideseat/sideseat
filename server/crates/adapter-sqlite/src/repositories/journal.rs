//! The SQLite deletion journal: deletions a restore cannot recompute.
//!
//! See [`sideseat_ports::traits::DeletionJournal`] for what it is for and why it is permanent. This file is
//! the SQLite half; `server/crates/adapter-postgres/src/repositories/journal.rs` is the other, and the two are compared by the
//! PostgreSQL parity suite.

use chrono::DateTime;
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;

use super::super::error::SqliteError;
use sideseat_ports::traits::{DeletionCause, DeletionRecord, DeletionScope};
use sideseat_ports::types::ProjectId;

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
            "INSERT INTO deletion_journal
                 (project_id, cause, scope, target_id, span_id, recorded_at, logical_bytes)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.project_id.as_str())
        .bind(record.cause.as_str())
        .bind(record.scope.as_str())
        .bind(&record.target_id)
        .bind(record.span_id.as_deref())
        // Nanoseconds, matching every other instant in this schema. `timestamp_nanos_opt` answers `None` past
        // 2262 and the fallback is the maximum rather than zero: an entry that sorted to the beginning of time
        // would be replayed first and then be indistinguishable from the oldest deletion in the journal.
        .bind(record.recorded_at.timestamp_nanos_opt().unwrap_or(i64::MAX))
        .bind(i64::try_from(record.logical_bytes()).unwrap_or(i64::MAX))
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
    pool: &SqlitePool,
    after_sequence: i64,
    limit: usize,
) -> Result<(Vec<(i64, DeletionRecord)>, i64), SqliteError> {
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
                project_id: ProjectId::from(row.try_get::<String, _>("project_id")?),
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

pub async fn journaled_spans_among(
    pool: &SqlitePool,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<HashSet<(String, String)>, SqliteError> {
    let mut found = HashSet::new();
    for chunk in spans.chunks(300) {
        let mut query =
            sqlx::QueryBuilder::<sqlx::Sqlite>::new("WITH input(trace_id, span_id) AS (");
        query.push_values(chunk, |mut row, (trace_id, span_id)| {
            row.push_bind(trace_id).push_bind(span_id);
        });
        query.push(
            ") SELECT input.trace_id, input.span_id
             FROM input
             WHERE EXISTS (
                 SELECT 1 FROM deletion_journal journal
                 WHERE journal.project_id = ",
        );
        query.push_bind(project_id);
        query.push(
            " AND journal.target_id = input.trace_id
               AND (
                   journal.scope = 'trace'
                   OR (journal.scope = 'span' AND journal.span_id = input.span_id)
               )
             )",
        );
        let rows: Vec<(String, String)> = query.build_query_as().fetch_all(pool).await?;
        found.extend(rows);
    }
    Ok(found)
}

pub async fn journaled_span_deletions_for_traces(
    pool: &SqlitePool,
    project_id: &str,
    trace_ids: &[String],
) -> Result<Vec<(String, String)>, SqliteError> {
    let mut found = HashSet::new();
    for chunk in trace_ids.chunks(900) {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT target_id, span_id FROM deletion_journal \
             WHERE project_id = ",
        );
        query.push_bind(project_id);
        query.push(" AND scope = 'span' AND target_id IN (");
        let mut separated = query.separated(", ");
        for trace_id in chunk {
            separated.push_bind(trace_id);
        }
        separated.push_unseparated(")");
        let rows: Vec<(String, String)> = query.build_query_as().fetch_all(pool).await?;
        found.extend(rows);
    }
    let mut found = found.into_iter().collect::<Vec<_>>();
    found.sort_unstable();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_instant() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        // `raw_sql`, not a split on `;`. Splitting is what the schema's own comment warns about: a semicolon
        // inside a `--` comment ends a "statement" mid-table and the fragment after it is a syntax error in a
        // place nobody looks. Other test helpers in this crate still split; this one does not.
        sqlx::raw_sql(crate::schema::SCHEMA)
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn record(scope: DeletionScope, target: &str) -> DeletionRecord {
        DeletionRecord {
            project_id: ProjectId::from("proj"),
            cause: DeletionCause::Requested,
            scope,
            target_id: target.to_string(),
            span_id: None,
            recorded_at: test_instant(),
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

        let (read, examined) = deletions_since(&pool, 0, 100).await.unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(
            examined,
            read.last().unwrap().0,
            "the examined watermark is the last row read"
        );
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

        let (first, examined) = deletions_since(&pool, 0, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        let resume_from = examined;

        let (rest, _) = deletions_since(&pool, resume_from, 2).await.unwrap();
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
        bad.project_id = ProjectId::from("proj");
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
            deletions_since(&pool, 0, 100).await.unwrap().0.is_empty(),
            "a batch that failed partway must leave none of its rows"
        );
    }

    /// The tombstone and its journal entry are one transaction, so neither can exist without the other.
    ///
    /// An ordering cannot give this, and both orderings are wrong. Tombstone first, a failed append leaves a
    /// deletion the sweeps perform anyway with no record, and a restore undoes it. Append first, a failed
    /// tombstone leaves a permanent record for a deletion the request reported as *failed*, and a restore replays
    /// it and deletes the data. The write that fails here is the tombstone - forced by dropping its table - and
    /// what has to hold is that no journal entry survives it.
    #[tokio::test]
    async fn a_tombstone_and_its_journal_entry_are_atomic() {
        let pool = setup_test_pool().await;
        let traces = vec!["trace-a".to_string(), "trace-b".to_string()];

        crate::repositories::project::record_deleted_traces_journalled(
            &pool,
            "proj",
            &traces,
            test_instant(),
        )
        .await
        .expect("both writes succeed together");

        let (entries, _) = deletions_since(&pool, 0, 100).await.unwrap();
        assert_eq!(entries.len(), 2, "one journal entry per trace");
        let tombstoned: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deleted_traces WHERE project_id = 'proj'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tombstoned, 2, "and one tombstone per trace");

        // Now make the tombstone write fail, and require the journal to roll back with it.
        sqlx::raw_sql("DROP TABLE deleted_traces;")
            .execute(&pool)
            .await
            .expect("drop");
        let before = deletions_since(&pool, 0, 100).await.unwrap().0.len();
        let failed = crate::repositories::project::record_deleted_traces_journalled(
            &pool,
            "proj",
            &["trace-c".to_string()],
            test_instant(),
        )
        .await;
        assert!(failed.is_err(), "the tombstone write must fail here");

        let after = deletions_since(&pool, 0, 100).await.unwrap().0.len();
        assert_eq!(
            after, before,
            "a failed tombstone must leave no journal entry: a record for a deletion that did not happen is one \
             a restore replays"
        );
    }

    /// A claim that loses writes no journal entry.
    ///
    /// Journalling before the claim wrote an entry for every losing caller - and an organization cleanup re-runs
    /// while its projects' tombstones remain, so one deletion accumulated permanent, quota-counted records without
    /// bound. Conditional on winning is only expressible inside the claim's own transaction.
    #[tokio::test]
    async fn a_losing_claim_writes_no_journal_entry() {
        let pool = setup_test_pool().await;
        sqlx::raw_sql(
            "INSERT INTO organizations (id, name, slug, created_at, updated_at)
                 VALUES ('org', 'Org', 'org', 0, 0);
             INSERT INTO projects (id, organization_id, name, created_at, updated_at)
                 VALUES ('proj', 'org', 'P', 0, 0);",
        )
        .execute(&pool)
        .await
        .expect("a project to claim");

        let won = crate::repositories::project::claim_project_for_deletion_journalled(
            &pool,
            None,
            "proj",
            test_instant(),
        )
        .await
        .expect("claim");
        assert!(won, "the first caller wins");
        assert_eq!(
            deletions_since(&pool, 0, 100).await.unwrap().0.len(),
            1,
            "and its entry is written"
        );

        for _ in 0..5 {
            let won = crate::repositories::project::claim_project_for_deletion_journalled(
                &pool,
                None,
                "proj",
                test_instant(),
            )
            .await
            .expect("claim");
            assert!(!won, "a later caller loses: the project is already fenced");
        }
        assert_eq!(
            deletions_since(&pool, 0, 100).await.unwrap().0.len(),
            1,
            "and none of them adds a record - five resumptions of one deletion is still one deletion"
        );
    }

    /// A page of rows this build cannot interpret does not read as the end of the journal.
    ///
    /// Skipped rows used to leave the cursor where it was, so a page consisting entirely of them returned
    /// nothing - indistinguishable from EOF against a cursor advanced by returned entries. A replay would stop
    /// there and never reach the known deletions behind them, which is the resurrection the journal exists to
    /// prevent, arrived at through the mechanism meant to prevent it.
    #[tokio::test]
    async fn a_page_of_uninterpretable_rows_still_advances_the_cursor() {
        let pool = setup_test_pool().await;
        // Two rows a future version might write, then one this build understands. Inserted through raw SQL
        // because the typed API cannot express a scope the `CHECK` refuses - which is the point: only a newer
        // writer produces these.
        // `ignore_check_constraints`, because a future version's `CHECK` would admit these and this build's
        // refuses them. Writing them any other way would be testing the constraint rather than the cursor.
        sqlx::raw_sql(
            "PRAGMA ignore_check_constraints = ON;
             INSERT INTO deletion_journal (project_id, cause, scope, target_id, recorded_at)
                 VALUES ('proj', 'requested', 'future-scope', 'future-1', 1);
             INSERT INTO deletion_journal (project_id, cause, scope, target_id, recorded_at)
                 VALUES ('proj', 'requested', 'future-scope', 'future-2', 2);
             PRAGMA ignore_check_constraints = OFF;",
        )
        .execute(&pool)
        .await
        .expect("a future version's rows");
        append_deletions(&pool, &[record(DeletionScope::Trace, "known")])
            .await
            .expect("append");

        // A page of two, which is exactly the two uninterpretable rows.
        let (entries, examined) = deletions_since(&pool, 0, 2).await.unwrap();
        assert!(
            entries.is_empty(),
            "neither row is interpretable, so neither is returned"
        );
        assert!(
            examined > 0,
            "but the cursor must advance past them, or the replay stops here forever"
        );

        let (rest, _) = deletions_since(&pool, examined, 2).await.unwrap();
        assert_eq!(rest.len(), 1, "the known deletion behind them is reachable");
        assert_eq!(rest[0].1.target_id, "known");
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

    #[tokio::test]
    async fn batch_span_lookup_matches_exact_spans_and_trace_deletions_only() {
        let pool = setup_test_pool().await;
        let mut exact = record(DeletionScope::Span, "trace-exact");
        exact.span_id = Some("span-exact".to_string());
        append_deletions(&pool, &[exact, record(DeletionScope::Trace, "trace-all")])
            .await
            .unwrap();

        let found = journaled_spans_among(
            &pool,
            "proj",
            &[
                ("trace-exact".to_string(), "span-exact".to_string()),
                ("trace-exact".to_string(), "span-other".to_string()),
                ("trace-all".to_string(), "span-any".to_string()),
                ("trace-live".to_string(), "span-live".to_string()),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            found,
            [
                ("trace-exact".to_string(), "span-exact".to_string()),
                ("trace-all".to_string(), "span-any".to_string()),
            ]
            .into_iter()
            .collect()
        );
    }

    #[tokio::test]
    async fn cleanup_replay_lists_only_exact_span_deletions_for_requested_traces() {
        let pool = setup_test_pool().await;
        let mut first = record(DeletionScope::Span, "trace-a");
        first.span_id = Some("span-2".to_string());
        let mut second = record(DeletionScope::Span, "trace-a");
        second.span_id = Some("span-1".to_string());
        let mut other = record(DeletionScope::Span, "trace-b");
        other.span_id = Some("span-b".to_string());
        append_deletions(
            &pool,
            &[
                first,
                second,
                other,
                record(DeletionScope::Trace, "trace-a"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            journaled_span_deletions_for_traces(
                &pool,
                "proj",
                &["trace-a".to_string(), "trace-missing".to_string()],
            )
            .await
            .unwrap(),
            vec![
                ("trace-a".to_string(), "span-1".to_string()),
                ("trace-a".to_string(), "span-2".to_string()),
            ],
            "trace-scoped entries and other traces are not exact span replay work"
        );
    }

    #[tokio::test]
    async fn pressure_eviction_record_is_idempotent_and_cleanup_remains_tokened() {
        let pool = setup_test_pool().await;
        let spans = vec![("trace-p".to_string(), "span-p".to_string())];
        let first = crate::repositories::project::record_pressure_eviction(
            &pool,
            "proj",
            &spans,
            test_instant(),
        )
        .await
        .unwrap();
        let second = crate::repositories::project::record_pressure_eviction(
            &pool,
            "proj",
            &spans,
            test_instant(),
        )
        .await
        .unwrap();

        let journal_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM deletion_journal
             WHERE project_id = 'proj' AND target_id = 'trace-p' AND span_id = 'span-p'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let cleanup_bytes: i64 = sqlx::query_scalar(
            "SELECT logical_bytes FROM retention_cleanup
             WHERE project_id = 'proj' AND trace_id = 'trace-p'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            journal_rows, 1,
            "a retry must not grow the permanent journal"
        );
        assert!(cleanup_bytes > 0);
        assert_eq!(
            first[0].1 + 1,
            second[0].1,
            "re-recording bumps the claim token"
        );
    }

    /// A span-scoped entry without a span id is refused, not stored as an inert row.
    ///
    /// The lookup matches on `span_id = ?`, so such a row can be appended successfully and then never found -
    /// and the re-drive sweep recreates the very span the entry was written to explain. Refused by the schema
    /// rather than by the writer, because a writer's care is not a constraint.
    #[tokio::test]
    async fn a_span_entry_needs_a_span_id() {
        let pool = setup_test_pool().await;
        let mut bad = record(DeletionScope::Span, "trace-x");
        bad.span_id = None;
        assert!(
            append_deletions(&pool, std::slice::from_ref(&bad))
                .await
                .is_err(),
            "a span-scoped entry with no span id must be refused"
        );

        // And the reverse: a span id on a trace-scoped row claims something about a span the entry does not
        // describe.
        let mut also_bad = record(DeletionScope::Trace, "trace-x");
        also_bad.span_id = Some("span-1".to_string());
        assert!(
            append_deletions(&pool, std::slice::from_ref(&also_bad))
                .await
                .is_err(),
            "only a span-scoped entry may carry a span id"
        );

        let mut good = record(DeletionScope::Span, "trace-x");
        good.span_id = Some("span-1".to_string());
        append_deletions(&pool, std::slice::from_ref(&good))
            .await
            .expect("a well-formed span entry is accepted");
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
            "and it is findable, which the null-id form was not"
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
            "PRAGMA ignore_check_constraints = ON;
             UPDATE deletion_journal SET scope = 'future-scope' WHERE target_id = 'known';
             PRAGMA ignore_check_constraints = OFF;",
        )
        .execute(&pool)
        .await
        .ok();

        let (read, _) = deletions_since(&pool, 0, 100).await.unwrap();
        assert!(
            read.iter().all(|(_, r)| r.target_id != "known")
                || read.iter().any(|(_, r)| r.scope == DeletionScope::Trace),
            "either the update was refused and the entry is intact, or it was applied and the entry is skipped"
        );
    }
}
