//! DuckDB retention management
//!
//! Efficient batch deletion with transaction-safe cascading deletes.
//! Note: DuckDB doesn't support data-modifying CTEs, so we use explicit transactions.

use std::collections::HashMap;

use chrono::{TimeDelta, Utc};
use duckdb::Connection;

use super::{DuckdbError, in_transaction};
use crate::core::config::RetentionConfig;
use crate::data::duckdb::repositories::query::DEDUP_SPANS;

/// Records that a batch's traces will need file and favourite cleanup, before their spans are deleted.
///
/// A parameter rather than a call, because retention is synchronous DuckDB work and the record goes to the
/// transactional store: the composition happens at the caller, which is the only place that has both.
pub type CleanupRecorder<'a> = &'a dyn Fn(&HashMap<String, Vec<String>>) -> Result<(), DuckdbError>;

/// Result of retention cleanup, including trace IDs for file cleanup
#[derive(Default)]
pub struct RetentionResult {
    /// Total spans deleted
    pub deleted_count: u64,
    /// Trace IDs grouped by project for file cleanup
    pub trace_ids_by_project: HashMap<String, Vec<String>>,
    /// The cleanup-intent token each recorded trace now carries, per project.
    ///
    /// Completion is conditional on the token, so the pass that recorded a candidate has to carry the value it
    /// wrote: guessing either fails to complete, leaving a record the sweep re-drives, or matches a *newer* row
    /// and discards work another pass recorded.
    pub cleanup_tokens: HashMap<String, Vec<(String, i64)>>,
}

/// Run retention cleanup based on config
/// Returns trace IDs for file cleanup. Runs CHECKPOINT after deletions to reclaim space.
pub fn run_retention(
    conn: &Connection,
    config: &RetentionConfig,
    record_intent: CleanupRecorder<'_>,
) -> Result<RetentionResult, DuckdbError> {
    let mut result = RetentionResult::default();

    if let Some(max_age_minutes) = config.max_age_minutes {
        // Span cleanup
        let (deleted, trace_ids) = cleanup_by_time(conn, max_age_minutes, record_intent)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based span retention cleanup"
            );
            result.deleted_count += deleted;
            merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
        }

        // Metrics cleanup (same time threshold)
        let deleted = cleanup_metrics_by_time(conn, max_age_minutes)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based metrics retention cleanup"
            );
            result.deleted_count += deleted;
        }
    }

    if let Some(max_spans) = config.max_spans {
        let (deleted, trace_ids) = cleanup_by_count(conn, max_spans, record_intent)?;
        if deleted > 0 {
            tracing::debug!(deleted, max_spans, "Count-based retention cleanup");
            result.deleted_count += deleted;
            merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
        }
    }

    if result.deleted_count > 0 {
        // CHECKPOINT ensures deleted data is flushed
        // Note: DuckDB doesn't shrink the file - freed space is reused internally
        conn.execute("CHECKPOINT", [])?;
        tracing::debug!(
            deleted = result.deleted_count,
            projects = result.trace_ids_by_project.len(),
            "Retention cleanup completed, checkpoint done"
        );
    } else {
        tracing::debug!("Retention check complete, nothing to delete");
    }

    Ok(result)
}

/// Merge trace IDs from a batch into the cumulative result
fn merge_trace_ids(
    target: &mut HashMap<String, Vec<String>>,
    source: HashMap<String, Vec<String>>,
) {
    for (project_id, trace_ids) in source {
        target.entry(project_id).or_default().extend(trace_ids);
    }
}

/// Max spans to delete per batch (prevents long transactions)
const RETENTION_BATCH_SIZE: i64 = 100_000;

/// Max batches per time-based cleanup cycle (prevents unbounded blocking)
const MAX_TIME_CLEANUP_BATCHES: usize = 10;

/// Cap on trace IDs collected from one batch, as a guard against a pathological batch rather than a
/// working limit.
///
/// It must stay **at or above [`RETENTION_BATCH_SIZE`]**, because in the worst case every span in a
/// batch belongs to its own trace. A lower value silently truncated the cleanup list: a batch could
/// delete 100 000 spans while only 10 000 traces were handed to file and favourite cleanup, and the
/// omitted traces never reappeared in a later pass - their spans were already gone - so their file
/// associations, ref-counted bytes and favourites were orphaned permanently. The memory this bounds
/// is a few megabytes at the full batch size, which is not the explosion the old value implied.
const MAX_TRACE_IDS_PER_CYCLE: usize = RETENTION_BATCH_SIZE as usize;

/// Execute retention based on time limit (delete spans older than N minutes)
/// Iterates in batches with a limit to prevent unbounded blocking
/// Returns (spans_deleted, trace_ids_by_project) for file cleanup
pub fn cleanup_by_time(
    conn: &Connection,
    minutes: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = Utc::now() - TimeDelta::minutes(minutes_i64);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    tracing::debug!(%cutoff_str, minutes, "Time-based retention check");

    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();

    for _ in 0..MAX_TIME_CLEANUP_BATCHES {
        let batch = delete_spans_before(conn, &cutoff_str, RETENTION_BATCH_SIZE, record_intent)?;
        if batch.identities == 0 {
            break;
        }
        tracing::debug!(
            identities = batch.identities,
            rows = batch.rows,
            "Deleted batch of expired spans"
        );
        total_deleted += batch.rows;
        merge_trace_ids(&mut all_trace_ids, batch.trace_ids_by_project);
    }
    Ok((total_deleted, all_trace_ids))
}

/// Max batches per count-based cleanup cycle (prevents unbounded blocking)
const MAX_COUNT_CLEANUP_BATCHES: usize = 10;

/// Ceiling on span identities one count-retention **cycle** may delete, across every project.
///
/// `MAX_COUNT_CLEANUP_BATCHES` bounds one project; without a cycle-wide ceiling the total was that bound
/// multiplied by the number of over-limit projects, which is unbounded. One pass then holds the single
/// DuckDB connection for as long as it takes - stalling ingestion, since writes take the same connection -
/// and accumulates every deleted trace id for the file sweep in memory.
const MAX_COUNT_IDENTITIES_PER_CYCLE: i64 = RETENTION_BATCH_SIZE * MAX_COUNT_CLEANUP_BATCHES as i64;

/// Ceiling on *physical rows* one retention cycle may delete, across every project.
///
/// The identity ceiling above is not a bound on work, and that gap is the whole reason this exists. Selection
/// is by winning identity but the delete removes **every revision** of each selected identity, so a project one
/// identity over its limit whose oldest identity carries five million revisions is charged `1` against the
/// identity budget and does five million rows of work - under the single DuckDB connection, which is the same
/// one writes take. Bounding identities bounds the *logical* progress the sweep needs to report; bounding rows
/// is what bounds the time the connection is held.
///
/// **Two stated residuals, because this bounds rows *deleted* and not work performed.**
///
/// One identity is atomic, so a single pathological identity can exceed the ceiling by itself. It cannot be
/// split: deleting some of an identity's revisions but not its winner leaves the identity in place, so the count
/// does not fall and the sweep makes no progress; deleting the winner but not the older revisions promotes an
/// obsolete revision to winner, which is corruption rather than slow retention. So the ceiling is enforced
/// *between* identities and the irreducible unit is one identity's revision count.
///
/// And the **selection** is not bounded by it at all. Choosing which identities to delete windows `DEDUP_SPANS`
/// and groups the raw table over the whole project, so a project with a hundred million rows pays a scan
/// proportional to that whether one identity is being deleted or a million - and the connection is held for the
/// duration, which is what the ceiling was reached for. Bounding the *scan* needs an index that orders
/// identities by age, which DuckDB will not serve from an ART index over an expression. Stated rather than
/// implied: this ceiling bounds how much is removed, not how long the connection is occupied.
const MAX_COUNT_ROWS_PER_CYCLE: u64 = (RETENTION_BATCH_SIZE as u64) * 4;

/// Execute retention based on max span count, **per project**.
///
/// `max_spans` is a limit on each project, not on the deployment. Counting the whole table made it a
/// shared budget that one noisy tenant exhausted on everyone's behalf, and the spans deleted to make
/// room belonged to whichever project happened to hold the globally oldest rows - so a quiet tenant
/// lost data because a busy one was over.
///
/// The count is over the **winning** relation. Counting raw rows double-counts an identity that has
/// been corrected, so a project one span over the limit appeared two over, the sweep deleted both
/// identities, and it finished *below* the limit.
///
/// Returns (spans_deleted, trace_ids_by_project). Iterates in batches with a limit to prevent
/// unbounded blocking.
pub fn cleanup_by_count(
    conn: &Connection,
    max_spans: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    cleanup_by_count_within(
        conn,
        max_spans,
        MAX_COUNT_IDENTITIES_PER_CYCLE,
        MAX_COUNT_ROWS_PER_CYCLE,
        record_intent,
    )
}

/// [`cleanup_by_count`] with the cycle budget as a parameter.
///
/// Separate for the same reason [`trim_project_to_limit`] is: at the production value the budget is a
/// million identities, so a test that reached it would have to build a million rows and no unit test is
/// going to. The split is what makes the budget's effect assertable rather than merely argued for.
fn cleanup_by_count_within(
    conn: &Connection,
    max_spans: u64,
    identity_budget_for_cycle: i64,
    row_budget_for_cycle: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let max_spans_i64 = i64::try_from(max_spans).unwrap_or(i64::MAX);

    let over_limit = match projects_over_limit(conn, max_spans_i64) {
        Ok(projects) => projects,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to query per-project span counts");
            return Ok((0, HashMap::new()));
        }
    };

    if over_limit.is_empty() {
        tracing::debug!(max_spans, "Every project within its span limit");
        return Ok((0, HashMap::new()));
    }

    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();

    // Bounded per **cycle**, not merely per project. Each project may run `MAX_COUNT_CLEANUP_BATCHES`
    // batches of `RETENTION_BATCH_SIZE`, so with the limit applied per project a deployment with a thousand
    // over-limit projects performed a thousand times that work in one pass - holding the single DuckDB
    // connection throughout, which stalls ingestion, and accumulating every deleted trace id for the file
    // sweep, which is where the memory goes. Whatever is left over is simply the next cycle's work: the
    // sweep is periodic and the projects that remain over their limit are found again.
    //
    // The budget is charged the *requested* overage rather than the rows the delete reported. Those are
    // different numbers - one identity can carry several revisions - and charging the request is the
    // conservative direction: it can only end the cycle sooner, never let it run longer than the ceiling.
    //
    // Two budgets, because one identity is not one row's worth of work: identities bound the logical progress
    // and rows bound how long the connection is held. Charging only identities let a project one identity over
    // its limit delete every revision of an identity carrying millions of them, inside a cycle that reported
    // itself bounded.
    let mut identity_budget = identity_budget_for_cycle;
    let mut row_budget = row_budget_for_cycle;
    let mut projects_deferred = 0usize;

    for (project_id, span_count) in over_limit {
        if identity_budget <= 0 || row_budget == 0 {
            projects_deferred += 1;
            continue;
        }
        let overage = (span_count - max_spans_i64).min(identity_budget);
        tracing::debug!(
            %project_id,
            span_count,
            max_spans,
            to_delete = overage,
            "Project exceeds its span limit, cleaning up"
        );
        let (deleted, trace_ids) = trim_project_to_limit(
            conn,
            &project_id,
            overage,
            RETENTION_BATCH_SIZE,
            row_budget,
            record_intent,
        )?;
        total_deleted += deleted;
        identity_budget -= overage;
        row_budget = row_budget.saturating_sub(deleted);
        merge_trace_ids(&mut all_trace_ids, trace_ids);
    }

    if projects_deferred > 0 {
        tracing::debug!(
            projects_deferred,
            identity_budget = identity_budget_for_cycle,
            row_budget = row_budget_for_cycle,
            "Count retention hit a per-cycle budget; the rest are next cycle's work"
        );
    }

    Ok((total_deleted, all_trace_ids))
}

/// Delete `overage` of one project's oldest span identities, in batches of `batch_size`.
///
/// Separate from [`cleanup_by_count`] so the loop's progress arithmetic is reachable in a test with a
/// small `batch_size`; at the production value an overage large enough to need two batches is 100 000
/// identities, which no unit test is going to build.
fn trim_project_to_limit(
    conn: &Connection,
    project_id: &str,
    overage: i64,
    batch_size: i64,
    row_budget: u64,
    record_intent: CleanupRecorder<'_>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut remaining = overage;

    for _ in 0..MAX_COUNT_CLEANUP_BATCHES {
        if remaining <= 0 {
            break;
        }
        // Checked between batches, not inside one: an identity's revisions are deleted together, for the
        // reason `MAX_COUNT_ROWS_PER_CYCLE` states. So the budget stops the *next* batch rather than
        // truncating the current one.
        if total_deleted >= row_budget {
            tracing::debug!(
                %project_id,
                rows = total_deleted,
                row_budget,
                "Stopping this project's trim at the cycle's row budget"
            );
            break;
        }
        // The row bound goes to the *statement*, which computes a running revision total and stops there.
        // Scaling this batch's identity limit from the previous batch's average was the first attempt and is
        // not a bound: an average says nothing about the next batch, so one batch of shallow identities
        // followed by one of deep ones overshot by orders of magnitude.
        let limit = remaining.min(batch_size);
        let batch = delete_oldest_spans_for_project(
            conn,
            project_id,
            limit,
            row_budget.saturating_sub(total_deleted),
            total_deleted == 0,
            record_intent,
        )?;
        if batch.identities == 0 {
            break;
        }
        tracing::debug!(
            %project_id,
            identities = batch.identities,
            rows = batch.rows,
            "Deleted batch of excess spans"
        );
        total_deleted += batch.rows;
        merge_trace_ids(&mut all_trace_ids, batch.trace_ids_by_project);
        // Identities, not rows. One identity can take several revisions with it, so subtracting the
        // row count overshoots the remainder and stops the loop while the project is still over its
        // limit.
        remaining = remaining.saturating_sub(batch.identities as i64);
    }

    Ok((total_deleted, all_trace_ids))
}

/// Projects whose winning span count exceeds `max_spans`, with that count.
fn projects_over_limit(
    conn: &Connection,
    max_spans: i64,
) -> Result<Vec<(String, i64)>, DuckdbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT project_id, COUNT(*) AS span_count FROM {DEDUP_SPANS}
         GROUP BY project_id
         HAVING COUNT(*) > ?1
         ORDER BY project_id"
    ))?;
    let rows = stmt.query_map([max_spans], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Outcome of one retention batch.
struct BatchOutcome {
    /// Span *identities* selected for deletion. Progress is counted in these, never in deleted
    /// rows: deleting one identity removes every revision of it, so a row count overshoots the
    /// caller's remaining budget and stops a count-based sweep while it is still over the limit.
    identities: u64,
    /// Physical rows removed, which is what the caller reports.
    rows: u64,
    trace_ids_by_project: HashMap<String, Vec<String>>,
}

/// Delete spans before cutoff timestamp (for time-based retention)
fn delete_spans_before(
    conn: &Connection,
    cutoff: &str,
    limit: i64,
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    delete_spans_with_query(
        conn,
        &format!(
            "INSERT INTO _retention_batch
             SELECT project_id, trace_id, span_id FROM {DEDUP_SPANS}
             WHERE timestamp_start < ?1
             ORDER BY timestamp_start ASC
             LIMIT ?2"
        ),
        &[&cutoff as &dyn duckdb::ToSql, &limit],
        record_intent,
    )
}

/// Delete the oldest N spans **of one project** (for count-based retention).
/// The oldest `limit` identities of one project, **and at most `row_budget` physical rows**.
///
/// The row bound is inside the selection rather than applied to the loop around it, and that is the whole
/// point: selection is by winning identity while the delete removes every revision of each selected identity,
/// so an identity count does not bound rows. Scaling the next batch from the previous batch's *average*
/// revision depth was the first attempt and is not a bound either - a batch of one-revision identities
/// followed by a batch of hundred-revision ones overshoots by two orders of magnitude, because an average
/// says nothing about the next batch.
///
/// So the running total is computed in SQL: identities are ranked oldest-first, each carries the cumulative
/// revision count up to and including itself, and the batch takes those whose cumulative count is within
/// budget.
///
/// `allow_overshoot` keeps the first identity **unconditionally**, which is the stated residual: one identity
/// is atomic, because dropping some of its revisions leaves the identity in place and makes no progress while
/// dropping its winner promotes an obsolete revision to winner. It is passed only when nothing has been
/// deleted yet in this trim, and that condition is load-bearing rather than tidy - applied on every batch it
/// re-opens the hole it exists to plug: a batch fills the budget with shallow identities, the next batch finds
/// a deep one at rank 1, takes it unconditionally, and the *cycle* overshoots by that identity's whole depth
/// even though it had already made progress. Progress needs the escape once, not repeatedly.
fn delete_oldest_spans_for_project(
    conn: &Connection,
    project_id: &str,
    limit: i64,
    row_budget: u64,
    allow_overshoot: bool,
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    let budget = i64::try_from(row_budget).unwrap_or(i64::MAX);
    let overshoot = if allow_overshoot {
        " OR row_rank = 1"
    } else {
        ""
    };
    delete_spans_with_query(
        conn,
        &format!(
            "INSERT INTO _retention_batch
             WITH winners AS (
                 SELECT project_id, trace_id, span_id, timestamp_start
                 FROM {DEDUP_SPANS}
                 WHERE project_id = ?1
             ),
             ranked AS (
                 SELECT w.project_id, w.trace_id, w.span_id,
                        ROW_NUMBER() OVER (ORDER BY w.timestamp_start ASC, w.trace_id, w.span_id)
                            AS row_rank,
                        SUM(r.revisions) OVER (
                            ORDER BY w.timestamp_start ASC, w.trace_id, w.span_id
                        ) AS cumulative_rows
                 FROM winners w
                 JOIN (
                     SELECT project_id, trace_id, span_id, COUNT(*) AS revisions
                     FROM otel_spans WHERE project_id = ?1
                     GROUP BY project_id, trace_id, span_id
                 ) r
                 ON r.project_id = w.project_id
                    AND r.trace_id = w.trace_id
                    AND r.span_id = w.span_id
             )
             SELECT project_id, trace_id, span_id FROM ranked
             WHERE row_rank <= ?2 AND (cumulative_rows <= ?3{overshoot})"
        ),
        &[&project_id as &dyn duckdb::ToSql, &limit, &budget],
        record_intent,
    )
}

/// Common delete logic using a temp table for efficiency.
///
/// Two properties this must not lose:
///
/// **The batch is keyed by `(project_id, trace_id, span_id)`.** A span id is unique only within a
/// trace and a trace id only within a project, and both come from the client - so a batch keyed by
/// `(trace_id, span_id)` alone let expiring one tenant's span delete another tenant's span, or a
/// held one, whenever two projects presented the same trace id.
///
/// **Candidates come from the deduplicated relation, not the raw table.** `otel_spans` is
/// append-only, so an expired *old* revision would otherwise select an identity whose winning
/// correction is recent, and the delete - which removes every revision of the identity - would take
/// the correction with it. Reads already go through [`DEDUP_SPANS`]; retention has to agree with them.
fn delete_spans_with_query(
    conn: &Connection,
    insert_sql: &str,
    params: &[&dyn duckdb::ToSql],
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    in_transaction(conn, |conn| {
        conn.execute(
            "CREATE TEMP TABLE IF NOT EXISTS _retention_batch (
                project_id VARCHAR NOT NULL,
                trace_id VARCHAR NOT NULL,
                span_id VARCHAR NOT NULL,
                PRIMARY KEY (project_id, trace_id, span_id)
            )",
            [],
        )?;
        conn.execute("DELETE FROM _retention_batch", [])?;

        let identities = conn.execute(insert_sql, params)? as u64;

        // Collected BEFORE the deletion: afterwards the rows naming these traces are gone.
        let trace_ids_by_project = collect_trace_ids_for_cleanup(conn)?;

        // And **recorded** before the deletion, which is the ordering that makes the record useful. The
        // cleanup runs after the delete commits, asynchronously, and a crash in between otherwise loses the
        // only knowledge that it is owed - the spans are already gone, so nothing can rediscover the traces,
        // and their associations, bytes and favourites are orphaned permanently.
        //
        // Recorded first, the two failure directions are not symmetric, which is the whole argument: if the
        // record commits and the delete does not, the cleanup is a no-op (survivor reconciliation releases
        // only what no surviving span references); if the record fails, this returns and nothing is deleted.
        // Recording *after* the delete would leave the one case that loses data.
        //
        // No transaction spans the two stores - that is this system's standing constraint - so the choice is
        // only which side of it to take.
        record_intent(&trace_ids_by_project)?;

        // Removes every revision of each selected identity (events, links and messages are
        // embedded in the span row).
        let rows = conn.execute(
            "DELETE FROM otel_spans
             WHERE (project_id, trace_id, span_id)
                   IN (SELECT project_id, trace_id, span_id FROM _retention_batch)",
            [],
        )? as u64;

        Ok(BatchOutcome {
            identities,
            rows,
            trace_ids_by_project,
        })
    })
}

/// Collect distinct `(project_id, trace_id)` pairs from the retention batch for file cleanup.
///
/// Reads the batch directly - the project is a column there now, so the join back to `otel_spans`
/// that previously recovered it is gone.
fn collect_trace_ids_for_cleanup(
    conn: &Connection,
) -> Result<HashMap<String, Vec<String>>, DuckdbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT project_id, trace_id FROM _retention_batch
         ORDER BY project_id, trace_id
         LIMIT ?1",
    )?;

    let limit = MAX_TRACE_IDS_PER_CYCLE as i64;
    let rows = stmt.query_map([limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (project_id, trace_id) = row?;
        result.entry(project_id).or_default().push(trace_id);
    }

    Ok(result)
}

// ============================================================================
// METRICS RETENTION
// ============================================================================

/// Max batches per metrics cleanup cycle (prevents unbounded blocking)
const MAX_METRICS_CLEANUP_BATCHES: usize = 10;

/// Execute retention based on time limit (delete metrics older than N minutes)
/// Iterates in batches with a limit to prevent unbounded blocking.
pub fn cleanup_metrics_by_time(conn: &Connection, minutes: u64) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = Utc::now() - TimeDelta::minutes(minutes_i64);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    tracing::debug!(%cutoff_str, minutes, "Time-based metrics retention check");

    let mut total_deleted = 0u64;
    for _ in 0..MAX_METRICS_CLEANUP_BATCHES {
        let deleted = delete_metrics_before(conn, &cutoff_str, RETENTION_BATCH_SIZE)?;
        if deleted == 0 {
            break;
        }
        tracing::debug!(deleted, "Deleted batch of expired metrics");
        total_deleted += deleted;
    }
    Ok(total_deleted)
}

/// Delete metrics before cutoff timestamp (for time-based retention)
fn delete_metrics_before(conn: &Connection, cutoff: &str, limit: i64) -> Result<u64, DuckdbError> {
    let deleted = conn.execute(
        "DELETE FROM otel_metrics
         WHERE rowid IN (
             SELECT rowid FROM otel_metrics
             WHERE timestamp < ?1
             ORDER BY timestamp ASC
             LIMIT ?2
         )",
        duckdb::params![cutoff, limit],
    )?;
    Ok(deleted as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;
    use crate::data::duckdb::DuckdbService;
    use chrono::Utc;
    use tempfile::TempDir;

    /// A recorder that records nothing, for the tests that are about deletion rather than about the intent.
    ///
    /// Named rather than an inline closure at every call site, so `no_intent` reads as "this test does not
    /// exercise the record" instead of as noise.
    fn no_intent(_: &HashMap<String, Vec<String>>) -> Result<(), DuckdbError> {
        Ok(())
    }

    async fn create_test_service() -> (TempDir, DuckdbService) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = DuckdbService::init(&storage)
            .await
            .expect("Failed to init analytics service");
        (temp_dir, service)
    }

    fn insert_test_span(conn: &Connection, trace_id: &str, span_id: &str, timestamp: &str) {
        insert_span_for_project(conn, "default", trace_id, span_id, timestamp);
    }

    fn insert_span_for_project(
        conn: &Connection,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO otel_spans (trace_id, span_id, span_name, timestamp_start, project_id)
             VALUES (?1, ?2, 'test', ?3, ?4)",
            [trace_id, span_id, timestamp, project_id],
        )
        .expect("Failed to insert test span");
    }

    /// A second delivery of an existing span. `otel_spans` is append-only, so this leaves both rows
    /// and `ingested_at` decides which one reads see.
    fn redeliver_span(
        conn: &Connection,
        project_id: &str,
        trace_id: &str,
        span_id: &str,
        timestamp: &str,
        ingested_at: &str,
    ) {
        conn.execute(
            "INSERT INTO otel_spans
                 (trace_id, span_id, span_name, timestamp_start, project_id, ingested_at)
             VALUES (?1, ?2, 'test', ?3, ?4, ?5)",
            [trace_id, span_id, timestamp, project_id, ingested_at],
        )
        .expect("Failed to re-deliver test span");
    }

    fn span_count(conn: &Connection, project_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM otel_spans WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .expect("Should query")
    }

    #[tokio::test]
    async fn test_cleanup_by_time_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let (deleted, trace_ids) = cleanup_by_time(&conn, 60, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_by_time_removes_old_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert spans: 2 old, 1 recent
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_test_span(&conn, "trace3", "span3", &recent);

        // Cleanup spans older than 1 minute
        let (deleted, _trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 2);

        // Verify only recent span remains
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_cleanup_by_count_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let (deleted, trace_ids) =
            cleanup_by_count(&conn, 100, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_by_count_under_limit() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert a few spans (under the limit)
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");

        // 100 span limit - should not delete anything (only 2 spans)
        let (deleted, trace_ids) =
            cleanup_by_count(&conn, 100, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());

        // Verify spans still exist
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_cleanup_by_count_exceeds_limit() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert 5 spans
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        insert_test_span(&conn, "trace3", "span3", "2020-01-03 00:00:00");
        insert_test_span(&conn, "trace4", "span4", "2020-01-04 00:00:00");
        insert_test_span(&conn, "trace5", "span5", "2020-01-05 00:00:00");

        // Limit to 2 spans - should delete 3 oldest
        let (deleted, _trace_ids) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 3);

        // Verify only 2 spans remain (the newest ones)
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);

        // Verify the newest spans were kept
        let trace4_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace4'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(trace4_exists, 1);

        let trace5_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'trace5'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(trace5_exists, 1);
    }

    #[tokio::test]
    async fn test_cleanup_preserves_recent_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert only recent spans
        let now = Utc::now();
        let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let recent2 = (now - TimeDelta::seconds(30))
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();

        insert_test_span(&conn, "trace1", "span1", &recent1);
        insert_test_span(&conn, "trace2", "span2", &recent2);

        // Cleanup with 1 minute retention - should preserve both
        let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 0);
        assert!(trace_ids.is_empty());

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_cleanup_deletes_oldest_first() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert spans with different timestamps
        insert_test_span(&conn, "oldest", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "middle", "span2", "2020-06-01 00:00:00");
        insert_test_span(&conn, "newest", "span3", "2020-12-01 00:00:00");

        // Use the batch primitive directly to verify ordering
        let batch =
            delete_oldest_spans_for_project(&conn, "default", 1, u64::MAX, true, &no_intent)
                .expect("Should delete");
        assert_eq!(batch.rows, 1);

        // Verify oldest was deleted
        let oldest_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE trace_id = 'oldest'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(oldest_exists, 0);

        // Middle and newest should still exist
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(remaining, 2);
    }

    #[tokio::test]
    async fn test_cleanup_mixed_old_and_new_spans() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert mix of old and new spans
        insert_test_span(&conn, "old1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "old2", "span2", "2020-01-02 00:00:00");
        insert_test_span(&conn, "old3", "span3", "2020-01-03 00:00:00");

        let now = Utc::now();
        insert_test_span(
            &conn,
            "new1",
            "span4",
            &now.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
        );
        insert_test_span(
            &conn,
            "new2",
            "span5",
            &(now - TimeDelta::seconds(10))
                .format("%Y-%m-%d %H:%M:%S%.6f")
                .to_string(),
        );

        // Cleanup old spans
        let (deleted, _trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 3);

        // Only new spans should remain
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    // ========================================================================
    // TENANT ISOLATION AND REVISION AWARENESS
    //
    // Every test below fails against the pre-fix implementation. The existing cases above cannot:
    // they use one project and one revision per span, which is exactly the shape in which both
    // defects are invisible.
    // ========================================================================

    /// A trace id comes from the client, so two projects can present the same one - and a span id is
    /// unique only within a trace. Expiring one tenant's span must not touch the other's.
    #[tokio::test]
    async fn expiring_one_project_leaves_a_colliding_span_of_another_project() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Same trace id AND same span id in two projects: the realistic collision, since both
        // values are client-supplied.
        insert_span_for_project(
            &conn,
            "alice",
            "shared-trace",
            "shared-span",
            "2020-01-01 00:00:00",
        );
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_span_for_project(&conn, "bob", "shared-trace", "shared-span", &recent);

        let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");
        assert_eq!(deleted, 1, "only alice's expired span should go");

        assert_eq!(
            span_count(&conn, "alice"),
            0,
            "alice's expired span is deleted"
        );
        assert_eq!(
            span_count(&conn, "bob"),
            1,
            "bob's recent span must survive: a batch keyed only by (trace_id, span_id) deleted it"
        );

        // And the cleanup list must attribute the trace to alice alone, or bob's files are reclaimed.
        assert_eq!(trace_ids.get("alice").map(Vec::len), Some(1));
        assert!(
            !trace_ids.contains_key("bob"),
            "bob's trace must not be handed to file cleanup"
        );
    }

    /// `max_spans` is a per-project limit. A shared budget let one busy tenant's overage delete a
    /// quiet tenant's data, because the globally oldest rows are not necessarily the offender's.
    #[tokio::test]
    async fn max_spans_is_per_project_and_spends_only_the_offender() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // The *innocent* project owns the globally oldest spans. Without that arrangement a global
        // algorithm passes by luck, deleting the offender's rows because they happen to be oldest.
        // At the limit, not over it, so the only reason to touch these is a shared budget.
        for i in 0..2 {
            insert_span_for_project(
                &conn,
                "quiet",
                &format!("q{i}"),
                &format!("qs{i}"),
                &format!("2019-01-0{} 00:00:00", i + 1),
            );
        }
        for i in 0..5 {
            insert_span_for_project(
                &conn,
                "noisy",
                &format!("n{i}"),
                &format!("ns{i}"),
                &format!("2021-01-0{} 00:00:00", i + 1),
            );
        }

        let (deleted, _) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");

        assert_eq!(
            span_count(&conn, "noisy"),
            2,
            "the over-limit project must end at exactly max_spans"
        );
        assert_eq!(
            span_count(&conn, "quiet"),
            2,
            "the at-limit project is untouched even though it owns the globally oldest spans"
        );
        assert_eq!(deleted, 3, "only noisy's three excess spans");
    }

    /// One cycle's work is bounded across every project, not merely within each.
    ///
    /// `MAX_COUNT_CLEANUP_BATCHES` bounds one project, so with no cycle-wide ceiling the total was that
    /// bound times the number of over-limit projects - unbounded. That matters because the whole pass holds
    /// the single DuckDB connection, which is the same one writes take, and accumulates every deleted trace
    /// id in memory for the file sweep.
    ///
    /// The remainder is not lost: the deferred projects are still over their limit, so the next cycle finds
    /// them. This asserts both halves - the cycle stops, *and* a second cycle finishes the job - because a
    /// budget that dropped the remainder would pass the first assertion alone.
    #[tokio::test]
    async fn count_retention_is_bounded_per_cycle_across_projects_not_only_per_project() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Three projects, each two identities over a limit of one: six identities of work in total.
        for project in ["a", "b", "c"] {
            for i in 0..3 {
                insert_span_for_project(
                    &conn,
                    project,
                    &format!("{project}t{i}"),
                    &format!("{project}s{i}"),
                    &format!("2021-01-0{} 00:00:00", i + 1),
                );
            }
        }

        // A budget of three identities cannot cover all six, so at least one project must be deferred.
        let (deleted, _) =
            cleanup_by_count_within(&conn, 1, 3, u64::MAX, &no_intent).expect("Should cleanup");
        assert_eq!(
            deleted, 3,
            "the cycle stops at its budget rather than at the work available"
        );

        let remaining: i64 = ["a", "b", "c"].iter().map(|p| span_count(&conn, p)).sum();
        assert_eq!(
            remaining, 6,
            "nine identities less the three the budget allowed"
        );
        assert!(
            ["a", "b", "c"].iter().any(|p| span_count(&conn, p) > 1),
            "at least one project is deferred, still over its limit"
        );

        // The remainder is next cycle's work, not lost work.
        let (deleted_again, _) =
            cleanup_by_count_within(&conn, 1, 3, u64::MAX, &no_intent).expect("Should cleanup");
        assert_eq!(
            deleted_again, 3,
            "the second cycle takes the deferred remainder"
        );
        for project in ["a", "b", "c"] {
            assert_eq!(
                span_count(&conn, project),
                1,
                "{project} reaches its limit once the cycles have run"
            );
        }
    }

    /// The trim is bounded by **rows** as well as identities, because those are different amounts of work.
    ///
    /// Selection is by winning identity but the delete removes every revision of each selected identity. So an
    /// identity budget alone is not a bound on work: a project one identity over its limit whose oldest identity
    /// carries a large number of revisions is charged `1` against that budget and does that many rows of work,
    /// holding the single DuckDB connection - the same one writes take - for the duration. The cycle reported
    /// itself bounded and was not.
    ///
    /// The fixture makes the two numbers diverge, which is exactly what the identity-budget test does not: every
    /// identity carries four revisions, so five identities are twenty rows behind five winners.
    ///
    /// **What is asserted is the bound the mechanism actually delivers**, not a stronger one. The row budget is
    /// checked between batches and the batch's identity limit is scaled by the revision ratio measured from the
    /// previous batch - so the *first* batch is unbounded (no ratio exists yet) and the overshoot is at most one
    /// batch. Asserting an exact row count would be asserting that an identity can be split, which it cannot:
    /// dropping some revisions leaves the identity in place and makes no progress, dropping the winner promotes
    /// an obsolete revision. So the assertion is "it stopped early", with the residual named.
    #[tokio::test]
    async fn count_retention_is_bounded_by_rows_not_only_by_identities() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Five identities, four revisions each: 20 physical rows behind 5 winners.
        for i in 0..5 {
            let ts = format!("2020-01-0{} 00:00:00", i + 1);
            for rev in 0..4 {
                redeliver_span(
                    &conn,
                    "default",
                    &format!("t{i}"),
                    &format!("s{i}"),
                    &ts,
                    &format!("2020-0{}-01 00:00:00", rev + 1),
                );
            }
        }

        // Batch size 1, so batches are per identity and the ratio is learned after the first. A row budget of 5
        // then permits the first identity (4 rows), finds 4 < 5 and takes a second (8 rows), and stops - rather
        // than taking all four identities and 16 rows, which is what an identity-only budget allows.
        let (deleted, _) =
            trim_project_to_limit(&conn, "default", 4, 1, 5, &no_intent).expect("Should trim");

        assert!(
            deleted < 16,
            "the trim deleted every revision of every over-limit identity ({deleted} rows) - the row budget \
             is not bounding anything, so one identity's revision depth is unbounded work inside a cycle that \
             reports itself bounded"
        );
        assert!(
            deleted >= 4,
            "it must still make progress, got {deleted} rows"
        );

        let winners: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert!(
            winners > 1,
            "identities remain, still over the limit, and are the next cycle's work - got {winners}"
        );
    }

    /// The cleanup intent is recorded **before** the spans are deleted, which is what survives a crash.
    ///
    /// The cleanup runs after the delete commits, asynchronously, with failures only logged. A crash or a
    /// transactional-store outage in between loses the only knowledge that the cleanup is owed - the spans are
    /// gone, so nothing can rediscover the traces, and their file associations, ref-counted bytes and
    /// favourites are orphaned permanently.
    ///
    /// Two assertions, and the ordering one is the point. A recorder that observes what it was handed *and*
    /// what the store contains at that moment shows the record is durable while the spans are still there - so
    /// a crash immediately after the delete leaves the record behind. Asserting only that the record exists
    /// afterwards would pass just as well with the recording done last, which is the version that loses data.
    #[tokio::test]
    async fn the_cleanup_intent_is_recorded_before_the_spans_are_deleted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        insert_test_span(&conn, "t1", "s1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "t2", "s2", "2020-01-02 00:00:00");

        // What the recorder saw, and how many spans still existed when it saw it.
        let observed: std::sync::Mutex<Vec<(String, usize, i64)>> =
            std::sync::Mutex::new(Vec::new());
        let recorder = |by_project: &HashMap<String, Vec<String>>| {
            let remaining: i64 = conn
                .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
                .expect("count spans");
            let mut seen = observed.lock().unwrap();
            for (project_id, trace_ids) in by_project {
                seen.push((project_id.clone(), trace_ids.len(), remaining));
            }
            Ok(())
        };

        let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &recorder).expect("Should cleanup");
        assert_eq!(deleted, 2, "both expired spans are deleted");

        let seen = observed.into_inner().unwrap();
        assert_eq!(seen.len(), 1, "one project was recorded, got {seen:?}");
        assert_eq!(seen[0].0, "default");
        assert_eq!(seen[0].1, 2, "both traces were handed to the recorder");
        assert_eq!(
            seen[0].2, 2,
            "the recorder ran with the spans still present, so the record is durable before the delete. It \
             saw {} spans, which means recording happens after the deletion and a crash in between loses the \
             cleanup entirely",
            seen[0].2
        );

        // And what it was handed is what the caller gets, so the record and the work cannot disagree.
        assert_eq!(trace_ids.get("default").map(|t| t.len()), Some(2));
    }

    /// A recorder that fails stops the deletion, because deleting without a record is the loss.
    ///
    /// The two failure directions are not symmetric and this is the one that matters: if the record commits and
    /// the delete does not, the cleanup is a harmless no-op; if the delete commits and the record does not, the
    /// spans are gone and nothing knows their files are owed. So a failure here must abort rather than proceed.
    #[tokio::test]
    async fn a_failed_intent_record_leaves_the_spans_in_place() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        insert_test_span(&conn, "t1", "s1", "2020-01-01 00:00:00");

        let failing = |_: &HashMap<String, Vec<String>>| {
            Err(DuckdbError::Io(std::io::Error::other(
                "the transactional store is unavailable",
            )))
        };
        let result = cleanup_by_time(&conn, 1, &failing);
        assert!(result.is_err(), "a failed record must fail the batch");

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("count spans");
        assert_eq!(
            remaining, 1,
            "the span was deleted although its cleanup could not be recorded, so its files are orphaned with \
             nothing able to find them"
        );
    }

    /// The row bound holds when revision depth is **not uniform**, which is when an average lies.
    ///
    /// The bound was first implemented by scaling the next batch's identity limit from the previous batch's
    /// average revisions-per-identity. That is not a bound: an average describes what has happened, not what
    /// the next batch contains. A batch of one-revision identities followed by a batch of hundred-revision ones
    /// overshoots by two orders of magnitude, and the earlier test could not see it because every identity in
    /// its fixture carried the same depth.
    ///
    /// This fixture is deliberately skewed - shallow identities first, then a deep one - so an average taken
    /// over the shallow ones is wrong about the deep one by 20x. The bound is now computed inside the delete
    /// statement as a running revision total, so it does not depend on any prediction.
    #[tokio::test]
    async fn the_row_bound_holds_when_revision_depth_is_not_uniform() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Four shallow identities (one revision each), then one carrying twenty.
        for i in 0..4 {
            insert_span_for_project(
                &conn,
                "default",
                &format!("shallow{i}"),
                &format!("s{i}"),
                &format!("2020-01-0{} 00:00:00", i + 1),
            );
        }
        for rev in 0..20 {
            redeliver_span(
                &conn,
                "default",
                "deep",
                "deep-span",
                "2020-01-05 00:00:00",
                &format!("2020-01-{:02} 00:00:00", rev + 1),
            );
        }

        // Every identity is over a limit of zero, so nothing but the budget decides where it stops. A budget of
        // six covers the four shallow identities (4 rows) and must **not** reach the deep one, which would take
        // the total to 24.
        let (deleted, _) =
            trim_project_to_limit(&conn, "default", 5, 100, 6, &no_intent).expect("Should trim");

        assert!(
            deleted <= 6,
            "the trim deleted {deleted} rows against a budget of 6 - the bound is predicted from an average \
             rather than computed, so a batch of deep identities blows through it"
        );
        assert!(
            deleted >= 4,
            "it must still make progress on what fits, got {deleted}"
        );

        // The deep identity survives to the next cycle rather than being dropped.
        let deep_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE project_id = 'default' AND trace_id = 'deep'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(
            deep_rows, 20,
            "the identity that did not fit the budget is left intact for the next cycle"
        );
    }

    /// The overshoot escape fires **once per trim**, not once per batch.
    ///
    /// `allow_overshoot` exists so a deep identity larger than the whole budget still makes progress, and it is
    /// passed only when nothing has been deleted yet. Passed on every batch it re-opens the hole it plugs: a
    /// batch fills the budget with shallow identities, the next batch finds a deep one at rank 1 and takes it
    /// unconditionally, and the trim overshoots by that identity's entire depth *after* having already made
    /// progress.
    ///
    /// The fixture needs both a **multi-batch** trim and **skewed** depth, which is what the two earlier tests
    /// each half-had: shallow identities first so batch one succeeds within budget, then a deep one so batch
    /// two's rank-1 escape would be visible.
    #[tokio::test]
    async fn the_overshoot_escape_applies_once_per_trim_not_once_per_batch() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Four shallow identities (one row each), then one carrying twenty.
        for i in 0..4 {
            insert_span_for_project(
                &conn,
                "default",
                &format!("shallow{i}"),
                &format!("s{i}"),
                &format!("2020-01-0{} 00:00:00", i + 1),
            );
        }
        for rev in 0..20 {
            redeliver_span(
                &conn,
                "default",
                "deep",
                "deep-span",
                "2020-01-05 00:00:00",
                &format!("2020-01-{:02} 00:00:00", rev + 1),
            );
        }

        // Batch size 2, so the trim takes two batches to reach the deep identity: batch one takes two shallow
        // identities (2 rows), batch two takes the other two (4 rows total), batch three reaches the deep one.
        // With a budget of 6 the deep identity does not fit, and because progress has already been made the
        // escape must not apply.
        let (deleted, _) =
            trim_project_to_limit(&conn, "default", 5, 2, 6, &no_intent).expect("Should trim");

        assert!(
            deleted <= 6,
            "the trim deleted {deleted} rows against a budget of 6 - the rank-1 escape fired on a later batch, \
             so a deep identity is taken unconditionally even once the trim has made progress"
        );

        let deep_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM otel_spans WHERE project_id = 'default' AND trace_id = 'deep'",
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(
            deep_rows, 20,
            "the deep identity is left for the next cycle rather than taken past the budget"
        );
    }

    /// A single identity larger than the whole budget is still deleted, because it cannot be split.
    ///
    /// This is the stated residual, asserted rather than described: with a budget of one row and an identity
    /// carrying twenty, the sweep must take all twenty. Dropping some of an identity's revisions would leave
    /// the identity in place and make no progress; dropping its winner would promote an obsolete revision,
    /// which is corruption rather than slow retention. So the batch overshoots by exactly one identity, and
    /// a mechanism that instead made *no* progress here would be worse.
    #[tokio::test]
    async fn one_identity_larger_than_the_budget_is_still_deleted() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        for rev in 0..20 {
            redeliver_span(
                &conn,
                "default",
                "deep",
                "deep-span",
                "2020-01-05 00:00:00",
                &format!("2020-01-{:02} 00:00:00", rev + 1),
            );
        }

        let (deleted, _) =
            trim_project_to_limit(&conn, "default", 1, 100, 1, &no_intent).expect("Should trim");

        assert_eq!(
            deleted, 20,
            "an identity cannot be split, so a budget of 1 still takes its 20 revisions - the alternative is \
             a sweep that never makes progress on a deep identity"
        );
    }

    /// Retention selects from the deduplicated relation. Selecting raw rows let an expired *old*
    /// revision nominate an identity whose winning correction is recent - and the delete, which
    /// removes every revision of an identity, took the correction with it.
    #[tokio::test]
    async fn an_expired_revision_does_not_delete_its_recent_correction() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let now = Utc::now();
        let recent = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();

        // First delivery is expired; the correction moves the span into the retention window.
        redeliver_span(
            &conn,
            "default",
            "trace1",
            "span1",
            "2020-01-01 00:00:00",
            "2020-01-01 00:00:00",
        );
        redeliver_span(&conn, "default", "trace1", "span1", &recent, &recent);

        let (deleted, trace_ids) = cleanup_by_time(&conn, 1, &no_intent).expect("Should cleanup");

        assert_eq!(
            deleted, 0,
            "the winning revision is recent, so nothing is expired"
        );
        assert_eq!(
            span_count(&conn, "default"),
            2,
            "both revisions survive: deleting the identity would have destroyed the correction"
        );
        assert!(trace_ids.is_empty());
    }

    /// Counting raw rows double-counts a corrected span, so a project one identity over the limit
    /// looked two over, both were deleted, and the sweep finished below the limit.
    #[tokio::test]
    async fn count_retention_counts_identities_not_revisions() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Two identities, one of which has been re-delivered: three physical rows, two winners.
        insert_span_for_project(&conn, "default", "t1", "s1", "2020-01-01 00:00:00");
        redeliver_span(
            &conn,
            "default",
            "t1",
            "s1",
            "2020-01-01 00:00:00",
            "2020-06-01 00:00:00",
        );
        insert_span_for_project(&conn, "default", "t2", "s2", "2020-01-02 00:00:00");

        // A limit of 2 is satisfied by two winners: nothing should be deleted.
        let (deleted, trace_ids) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");
        assert_eq!(
            deleted, 0,
            "two winning identities are within a limit of two, whatever the revision count"
        );
        assert_eq!(span_count(&conn, "default"), 3);
        assert!(trace_ids.is_empty());
    }

    /// Progress is counted in identities. Counting deleted *rows* let one batch report more progress
    /// than it made, zeroing the remainder while the project was still over its limit.
    ///
    /// Driven through [`trim_project_to_limit`] with `batch_size = 1`, because at the production
    /// batch size an overage needing two batches is 100 000 identities. With a batch per identity,
    /// each batch deletes three rows for one identity - so row-counted progress finishes after the
    /// first batch and leaves the project two identities over.
    #[tokio::test]
    async fn count_retention_progress_is_identities_not_deleted_rows() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Five identities, each carrying three revisions.
        for i in 0..5 {
            let ts = format!("2020-01-0{} 00:00:00", i + 1);
            for rev in 0..3 {
                redeliver_span(
                    &conn,
                    "default",
                    &format!("t{i}"),
                    &format!("s{i}"),
                    &ts,
                    &format!("2020-0{}-01 00:00:00", rev + 1),
                );
            }
        }

        // Three identities over a limit of two, one identity per batch.
        let (_deleted, _) = trim_project_to_limit(&conn, "default", 3, 1, u64::MAX, &no_intent)
            .expect("Should trim");

        let winners: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(
            winners, 2,
            "the loop must land exactly on max_spans; row-counted progress stops early"
        );
    }

    /// Counting raw rows also makes a corrected span look like two, so the sweep overshoots.
    #[tokio::test]
    async fn count_retention_reaches_the_limit_when_identities_carry_many_revisions() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        for i in 0..2 {
            let ts = format!("2020-01-0{} 00:00:00", i + 1);
            for rev in 0..3 {
                redeliver_span(
                    &conn,
                    "default",
                    &format!("t{i}"),
                    &format!("s{i}"),
                    &ts,
                    &format!("2020-0{}-01 00:00:00", rev + 1),
                );
            }
        }
        insert_span_for_project(&conn, "default", "t2", "s2", "2020-02-01 00:00:00");
        insert_span_for_project(&conn, "default", "t3", "s3", "2020-02-02 00:00:00");

        let (_deleted, _) = cleanup_by_count(&conn, 2, &no_intent).expect("Should cleanup");

        let winners: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {DEDUP_SPANS} WHERE project_id = 'default'"),
                [],
                |row| row.get(0),
            )
            .expect("Should query");
        assert_eq!(winners, 2, "the sweep must land exactly on max_spans");
    }

    // ========================================================================
    // METRICS RETENTION TESTS
    // ========================================================================

    fn insert_test_metric(conn: &Connection, name: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO otel_metrics (datapoint_id, metric_name, metric_type, timestamp)
             VALUES (?1, ?1, 'gauge', ?2)",
            [name, timestamp],
        )
        .expect("Failed to insert test metric");
    }

    #[tokio::test]
    async fn test_cleanup_metrics_by_time_empty_table() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        let deleted = cleanup_metrics_by_time(&conn, 60).expect("Should cleanup");
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_cleanup_metrics_by_time_removes_old_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert metrics: 2 old, 1 recent
        insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
        insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");
        let recent = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        insert_test_metric(&conn, "metric3", &recent);

        // Cleanup metrics older than 1 minute
        let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 2);

        // Verify only recent metric remains
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_cleanup_metrics_preserves_recent_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert only recent metrics
        let now = Utc::now();
        let recent1 = now.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let recent2 = (now - TimeDelta::seconds(30))
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();

        insert_test_metric(&conn, "metric1", &recent1);
        insert_test_metric(&conn, "metric2", &recent2);

        // Cleanup with 1 minute retention - should preserve both
        let deleted = cleanup_metrics_by_time(&conn, 1).expect("Should cleanup");
        assert_eq!(deleted, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_run_retention_cleans_both_spans_and_metrics() {
        let (_temp_dir, analytics) = create_test_service().await;
        let conn = analytics.conn();

        // Insert old spans and metrics
        insert_test_span(&conn, "trace1", "span1", "2020-01-01 00:00:00");
        insert_test_span(&conn, "trace2", "span2", "2020-01-02 00:00:00");
        insert_test_metric(&conn, "metric1", "2020-01-01 00:00:00");
        insert_test_metric(&conn, "metric2", "2020-01-02 00:00:00");

        // Run retention with 1 minute limit
        let config = RetentionConfig {
            max_age_minutes: Some(1),
            max_spans: None,
        };
        let result = run_retention(&conn, &config, &no_intent).expect("Should run retention");
        assert!(result.deleted_count > 0);

        // Verify both spans and metrics are deleted
        let span_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_spans", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(span_count, 0);

        let metric_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM otel_metrics", [], |row| row.get(0))
            .expect("Should query");
        assert_eq!(metric_count, 0);
    }
}
