//! DuckDB retention management.
//!
//! Efficient batch deletion with transaction-safe cascading deletes.
//! Note: DuckDB doesn't support data-modifying CTEs, so we use explicit transactions.

use std::collections::HashMap;

use chrono::{TimeDelta, Utc};
use duckdb::Connection;

use super::{DuckdbError, in_transaction};
use sideseat_core::config::RetentionConfig;
use sideseat_query_sql::analytics::QueryValue;
use sideseat_query_sql::dml::{self, DmlStatement};

/// Records that a batch's traces will need file and favourite cleanup, before their spans are deleted.
///
/// A parameter rather than a call, because retention is synchronous DuckDB work and the record goes to the
/// transactional store: the composition happens at the caller, which is the only place that has both.
pub type CleanupRecorder<'a> = &'a dyn Fn(&HashMap<String, Vec<String>>) -> Result<(), DuckdbError>;
pub type PressureRecorder<'a> = &'a dyn Fn(&str, &[(String, String)]) -> Result<(), DuckdbError>;

#[cfg(test)]
fn ignore_pressure(_: &str, _: &[(String, String)]) -> Result<(), DuckdbError> {
    Ok(())
}

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
#[cfg(test)]
pub fn run_retention(
    conn: &Connection,
    config: &RetentionConfig,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<RetentionResult, DuckdbError> {
    let mut result = RetentionResult::default();

    if let Some(max_age_minutes) = config.max_age_minutes {
        // Span cleanup
        let (deleted, trace_ids) = cleanup_by_time(conn, max_age_minutes, record_intent, now)?;
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
        let deleted = cleanup_metrics_by_time(conn, max_age_minutes, now)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based metrics retention cleanup"
            );
            result.deleted_count += deleted;
        }

        let deleted = cleanup_logs_by_time(conn, max_age_minutes, now)?;
        if deleted > 0 {
            tracing::debug!(
                deleted,
                max_age_minutes,
                "Time-based logs retention cleanup"
            );
            result.deleted_count += deleted;
        }
    }

    if let Some(max_spans) = config.max_spans {
        let (deleted, trace_ids) = cleanup_by_count(conn, max_spans, record_intent, now)?;
        if deleted > 0 {
            tracing::debug!(deleted, max_spans, "Count-based retention cleanup");
            result.deleted_count += deleted;
            merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
        }
    }

    if result.deleted_count > 0 {
        // CHECKPOINT ensures deleted data is flushed
        // Note: DuckDB doesn't shrink the file - freed space is reused internally
        execute_statement(conn, &dml::retention::retention_checkpoint())?;
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

/// Run one project's retention while its cross-store maintenance lease is held by the caller.
///
/// Keeping the project predicate inside every mutation is essential: listing projects and then running
/// a global delete leaves a race in which a newly-created project can be deleted without ever having
/// acquired its lease.
#[cfg(test)]
pub fn run_retention_for_project(
    conn: &Connection,
    config: &RetentionConfig,
    project_id: &str,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<RetentionResult, DuckdbError> {
    run_retention_for_project_with_pressure(
        conn,
        config,
        project_id,
        record_intent,
        &ignore_pressure,
        now,
    )
}

pub fn run_retention_for_project_with_pressure(
    conn: &Connection,
    config: &RetentionConfig,
    project_id: &str,
    record_intent: CleanupRecorder<'_>,
    record_pressure: PressureRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<RetentionResult, DuckdbError> {
    let mut result = RetentionResult::default();

    if let Some(max_age_minutes) = config.max_age_minutes {
        let (deleted, trace_ids) =
            cleanup_project_by_time(conn, project_id, max_age_minutes, record_intent, now)?;
        result.deleted_count += deleted;
        merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);

        result.deleted_count +=
            cleanup_project_metrics_by_time(conn, project_id, max_age_minutes, now)?;
        result.deleted_count +=
            cleanup_project_logs_by_time(conn, project_id, max_age_minutes, now)?;
    }

    if let Some(max_spans) = config.max_spans {
        let (deleted, trace_ids) = cleanup_project_by_count(
            conn,
            project_id,
            max_spans,
            record_intent,
            record_pressure,
            now,
        )?;
        result.deleted_count += deleted;
        merge_trace_ids(&mut result.trace_ids_by_project, trace_ids);
    }

    if result.deleted_count > 0 {
        execute_statement(conn, &dml::retention::retention_checkpoint())?;
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
/// It stays at or above [`RETENTION_BATCH_SIZE`] because every selected span may belong to a distinct trace.
/// Cleanup must receive every trace whose rows are deleted; omitted identities cannot be rediscovered after
/// deletion.
const MAX_TRACE_IDS_PER_CYCLE: usize = RETENTION_BATCH_SIZE as usize;

/// Execute retention based on time limit (delete spans older than N minutes)
/// Iterates in batches with a limit to prevent unbounded blocking
/// Returns (spans_deleted, trace_ids_by_project) for file cleanup
#[cfg(test)]
pub fn cleanup_by_time(
    conn: &Connection,
    minutes: u64,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    tracing::debug!(%cutoff, minutes, "Time-based retention check");

    let mut total_deleted = 0u64;
    let mut all_trace_ids: HashMap<String, Vec<String>> = HashMap::new();

    for _ in 0..MAX_TIME_CLEANUP_BATCHES {
        let batch = delete_spans_before(conn, cutoff, now, RETENTION_BATCH_SIZE, record_intent)?;
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

fn cleanup_project_by_time(
    conn: &Connection,
    project_id: &str,
    minutes: u64,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    let mut total_deleted = 0u64;
    let mut all_trace_ids = HashMap::new();

    for _ in 0..MAX_TIME_CLEANUP_BATCHES {
        let batch = delete_project_spans_before(
            conn,
            project_id,
            cutoff,
            now,
            RETENTION_BATCH_SIZE,
            record_intent,
        )?;
        if batch.identities == 0 {
            break;
        }
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
#[cfg(test)]
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
/// And the **selection** is not bounded by it at all. Choosing which identities to delete reads the project's
/// winners (`sideseat_query_sql::winners`) and orders them by age, so a project with a hundred million rows pays
/// a scan proportional to that whether one identity is being deleted or a million - and the connection is held
/// for the duration, which is what the ceiling was reached for. Bounding the *scan* needs an index that orders
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
#[cfg(test)]
pub fn cleanup_by_count(
    conn: &Connection,
    max_spans: u64,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    cleanup_by_count_within(
        conn,
        max_spans,
        MAX_COUNT_IDENTITIES_PER_CYCLE,
        MAX_COUNT_ROWS_PER_CYCLE,
        record_intent,
        now,
    )
}

/// [`cleanup_by_count`] with the cycle budget as a parameter.
///
/// Separate for the same reason [`trim_project_to_limit`] is: at the production value the budget is a
/// million identities, so a test that reached it would have to build a million rows and no unit test is
/// going to. The split is what makes the budget's effect assertable rather than merely argued for.
#[cfg(test)]
fn cleanup_by_count_within(
    conn: &Connection,
    max_spans: u64,
    identity_budget_for_cycle: i64,
    row_budget_for_cycle: u64,
    record_intent: CleanupRecorder<'_>,
    now: chrono::DateTime<Utc>,
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
            &ignore_pressure,
            now,
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

fn cleanup_project_by_count(
    conn: &Connection,
    project_id: &str,
    max_spans: u64,
    record_intent: CleanupRecorder<'_>,
    record_pressure: PressureRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<(u64, HashMap<String, Vec<String>>), DuckdbError> {
    let max_spans = i64::try_from(max_spans).unwrap_or(i64::MAX);
    let Some((_, span_count)) = project_over_limit(conn, project_id, max_spans)? else {
        return Ok((0, HashMap::new()));
    };
    trim_project_to_limit(
        conn,
        project_id,
        span_count - max_spans,
        RETENTION_BATCH_SIZE,
        MAX_COUNT_ROWS_PER_CYCLE,
        record_intent,
        record_pressure,
        now,
    )
}

/// Delete `overage` of one project's oldest span identities, in batches of `batch_size`.
///
/// Separate from [`cleanup_by_count`] so the loop's progress arithmetic is reachable in a test with a
/// small `batch_size`; at the production value an overage large enough to need two batches is 100 000
/// identities, which no unit test is going to build.
#[allow(clippy::too_many_arguments)]
fn trim_project_to_limit(
    conn: &Connection,
    project_id: &str,
    overage: i64,
    batch_size: i64,
    row_budget: u64,
    record_intent: CleanupRecorder<'_>,
    record_pressure: PressureRecorder<'_>,
    now: chrono::DateTime<Utc>,
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
            record_pressure,
            now,
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
#[cfg(test)]
fn projects_over_limit(
    conn: &Connection,
    max_spans: i64,
) -> Result<Vec<(String, i64)>, DuckdbError> {
    let query = dml::retention::retention_projects_over_limit(max_spans);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let rows = stmt.query_map(values.as_slice(), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn project_over_limit(
    conn: &Connection,
    project_id: &str,
    max_spans: i64,
) -> Result<Option<(String, i64)>, DuckdbError> {
    let query = dml::retention::retention_project_over_limit(project_id, max_spans);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let mut rows = stmt.query(values.as_slice())?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
        None => Ok(None),
    }
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
#[cfg(test)]
fn delete_spans_before(
    conn: &Connection,
    cutoff: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    limit: i64,
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    let query = dml::retention::retention_select_expired(cutoff, now, limit);
    delete_spans_with_query(conn, &query, record_intent, None)
}

fn delete_project_spans_before(
    conn: &Connection,
    project_id: &str,
    cutoff: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    limit: i64,
    record_intent: CleanupRecorder<'_>,
) -> Result<BatchOutcome, DuckdbError> {
    let query =
        dml::retention::retention_select_expired_for_project(project_id, cutoff, now, limit);
    delete_spans_with_query(conn, &query, record_intent, None)
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
#[allow(clippy::too_many_arguments)]
fn delete_oldest_spans_for_project(
    conn: &Connection,
    project_id: &str,
    limit: i64,
    row_budget: u64,
    allow_overshoot: bool,
    record_intent: CleanupRecorder<'_>,
    record_pressure: PressureRecorder<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<BatchOutcome, DuckdbError> {
    let budget = i64::try_from(row_budget).unwrap_or(i64::MAX);
    let query =
        dml::retention::retention_select_oldest(project_id, now, limit, budget, allow_overshoot);
    delete_spans_with_query(conn, &query, record_intent, Some(record_pressure))
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
/// the correction with it. Reads take winners only (`sideseat_query_sql::winners`); retention has to agree with
/// them.
fn delete_spans_with_query(
    conn: &Connection,
    insert: &DmlStatement,
    record_intent: CleanupRecorder<'_>,
    record_pressure: Option<PressureRecorder<'_>>,
) -> Result<BatchOutcome, DuckdbError> {
    in_transaction(conn, |conn| {
        execute_statement(conn, &dml::retention::retention_prepare_batch())?;
        execute_statement(conn, &dml::retention::retention_clear_batch())?;

        let identities = execute_statement(conn, insert)?;

        // Collect every trace before deleting the rows that identify it.
        let trace_ids_by_project = collect_trace_ids_for_cleanup(conn)?;

        // Record cleanup intent before deletion. The asynchronous cleanup can then resume after a crash
        // without rediscovering traces from rows that no longer exist.
        //
        // If intent commits but deletion does not, survivor reconciliation is a no-op. If intent recording
        // fails, this transaction returns before deleting anything. No transaction spans both stores.
        if let Some(record_pressure) = record_pressure {
            for (project_id, spans) in collect_selected_spans(conn)? {
                record_pressure(&project_id, &spans)?;
            }
        } else {
            record_intent(&trace_ids_by_project)?;
        }

        // The records those rows name go to reconciliation in the same transaction as the delete.
        execute_statement(conn, &dml::raw::enqueue_raw_for_retention_batch())?;
        // Its search terms first, while the batch names it; then every revision of each selected identity
        // (events, links and messages are embedded in the span row).
        execute_statement(
            conn,
            &dml::retention::retention_delete_selected_span_terms(),
        )?;
        let rows = execute_statement(conn, &dml::retention::retention_delete_selected_spans())?;

        Ok(BatchOutcome {
            identities,
            rows,
            trace_ids_by_project,
        })
    })
}

fn collect_selected_spans(
    conn: &Connection,
) -> Result<HashMap<String, Vec<(String, String)>>, DuckdbError> {
    let mut statement = conn.prepare(
        "SELECT project_id, trace_id, span_id
         FROM _retention_batch
         ORDER BY project_id, trace_id, span_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut selected = HashMap::new();
    for row in rows {
        let (project_id, trace_id, span_id) = row?;
        selected
            .entry(project_id)
            .or_insert_with(Vec::new)
            .push((trace_id, span_id));
    }
    Ok(selected)
}

/// Collect distinct `(project_id, trace_id)` pairs from the retention batch for file cleanup.
///
/// Reads the project and trace directly from the materialised batch so collection does not depend on rows that
/// the same transaction will delete.
fn collect_trace_ids_for_cleanup(
    conn: &Connection,
) -> Result<HashMap<String, Vec<String>>, DuckdbError> {
    let limit = MAX_TRACE_IDS_PER_CYCLE as i64;
    let query = dml::retention::retention_selected_traces(limit);
    let values = duckdb_values(query.params());
    let mut stmt = conn.prepare(query.sql())?;
    let rows = stmt.query_map(values.as_slice(), |row| {
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
#[cfg(test)]
pub fn cleanup_metrics_by_time(
    conn: &Connection,
    minutes: u64,
    now: chrono::DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    tracing::debug!(%cutoff, minutes, "Time-based metrics retention check");

    let mut total_deleted = 0u64;
    for _ in 0..MAX_METRICS_CLEANUP_BATCHES {
        let deleted = delete_metrics_before(conn, cutoff, now, RETENTION_BATCH_SIZE)?;
        if deleted == 0 {
            break;
        }
        tracing::debug!(deleted, "Deleted batch of expired metrics");
        total_deleted += deleted;
    }
    Ok(total_deleted)
}

fn cleanup_project_metrics_by_time(
    conn: &Connection,
    project_id: &str,
    minutes: u64,
    now: chrono::DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    let mut total_deleted = 0u64;
    for _ in 0..MAX_METRICS_CLEANUP_BATCHES {
        let deleted = execute_statement(
            conn,
            &dml::retention::retention_delete_expired_metrics_for_project(
                project_id,
                cutoff,
                now,
                RETENTION_BATCH_SIZE,
            ),
        )?;
        if deleted == 0 {
            break;
        }
        total_deleted += deleted;
    }
    Ok(total_deleted)
}

/// Delete metrics before cutoff timestamp (for time-based retention)
#[cfg(test)]
fn delete_metrics_before(
    conn: &Connection,
    cutoff: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    limit: i64,
) -> Result<u64, DuckdbError> {
    execute_statement(
        conn,
        &dml::retention::retention_delete_expired_metrics(cutoff, now, limit),
    )
}

#[cfg(test)]
pub fn cleanup_logs_by_time(
    conn: &Connection,
    minutes: u64,
    now: chrono::DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    delete_expired_log_pages(conn, None, cutoff, now)
}

fn cleanup_project_logs_by_time(
    conn: &Connection,
    project_id: &str,
    minutes: u64,
    now: chrono::DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let minutes_i64 = i64::try_from(minutes).unwrap_or(i64::MAX);
    let cutoff = now - TimeDelta::minutes(minutes_i64);
    delete_expired_log_pages(conn, Some(project_id), cutoff, now)
}

/// Delete expired logs a page at a time, each page's search terms with it in one transaction.
fn delete_expired_log_pages(
    conn: &Connection,
    project_id: Option<&str>,
    cutoff: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
) -> Result<u64, DuckdbError> {
    let mut total_deleted = 0u64;
    for _ in 0..MAX_METRICS_CLEANUP_BATCHES {
        let deleted = in_transaction(conn, |conn| {
            execute_statement(
                conn,
                &dml::retention::retention_delete_expired_log_terms(
                    project_id,
                    cutoff,
                    now,
                    RETENTION_BATCH_SIZE,
                ),
            )?;
            execute_statement(
                conn,
                &dml::retention::retention_delete_expired_logs(
                    project_id,
                    cutoff,
                    now,
                    RETENTION_BATCH_SIZE,
                ),
            )
        })?;
        if deleted == 0 {
            break;
        }
        total_deleted += deleted;
    }
    Ok(total_deleted)
}

fn execute_statement(conn: &Connection, statement: &DmlStatement) -> Result<u64, DuckdbError> {
    let values = duckdb_values(statement.params());
    Ok(conn.execute(statement.sql(), values.as_slice())? as u64)
}

fn duckdb_values(values: &[QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            QueryValue::String(value) => value as &dyn duckdb::ToSql,
            QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
