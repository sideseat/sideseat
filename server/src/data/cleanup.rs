//! Cross-database cleanup logic for organization and project deletion

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::core::constants::{
    CLAIM_RECOVERY_INTERVAL_SECS, DELETED_PROJECT_CHECK_BASE_SECS, DELETED_PROJECT_CHECK_BATCH,
    DELETED_PROJECT_CHECK_LEASE_SECS, DELETED_PROJECT_CHECK_MAX_SECS,
    DELETED_TRACE_CHECK_BASE_SECS, DELETED_TRACE_CHECK_BATCH, DELETED_TRACE_CHECK_LEASE_SECS,
    DELETED_TRACE_CHECK_MAX_SECS, FILE_DELETION_CLAIM_STALE_SECS,
    PROJECT_DELETION_CLAIM_STALE_SECS, PROJECT_TOMBSTONE_CLEAN_SWEEPS,
};
use crate::data::AnalyticsService;
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};
use crate::data::files::FileService;

/// Delete an organization: tombstone it, tombstone its projects, and let the sweep finish.
///
/// The organization row cannot go with its data, and the reason is the same one that makes a project's
/// row a tombstone: deleting it cascades its *project* rows away, and those rows are what the projects'
/// own cleanups depend on to keep running. Removing it early would therefore stop exactly the collection
/// that a late write needs.
///
/// So this claims the organization, claims every project under it, deletes what it can, and returns. From
/// that moment the organization and its projects are invisible to every read and refuse every write; the
/// sweep removes the project rows as each one's data is observed gone, and the organization row once no
/// project rows remain.
///
/// Returns `Ok(false)` when there was no live organization to delete.
pub async fn cleanup_organization(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<bool> {
    let repo = database.repository();

    if !repo
        .claim_organization_for_deletion(org_id)
        .await
        .context("Failed to claim organization for deletion")?
    {
        return Ok(false);
    }

    // API keys are org-scoped, so their caches go now: the organization is no longer usable.
    if let Some(cache) = cache {
        invalidate_org_api_key_caches(repo.as_ref(), cache, org_id).await;
    }

    finish_organization_deletion(database, analytics, file_service, cache, org_id).await?;
    Ok(true)
}

/// The resumable part of an organization deletion: everything after the claim.
///
/// Idempotent throughout, so the sweep can run it repeatedly - which it must, because the organization
/// row waits for its projects and a project row waits for repeated evidence that its data is gone.
pub async fn finish_organization_deletion(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<()> {
    let repo = database.repository();
    let mut errors: Vec<String> = Vec::new();

    // Every project fenced first. A project that is not fenced can still be written to while its data
    // is being deleted, and nothing later in this function would notice.
    for project_id in repo.list_project_ids(org_id).await? {
        if let Err(e) = repo.claim_project_for_deletion(cache, &project_id).await {
            return Err(anyhow!(
                "Failed to fence project {} of organization {}: {}",
                project_id,
                org_id,
                e
            ));
        }
        if let Err(e) =
            finish_project_deletion(database, analytics, file_service, cache, &project_id).await
        {
            errors.push(format!("project {}: {}", project_id, e));
        }
    }

    if !errors.is_empty() {
        // The organization stays tombstoned, which is to say invisible and unwritable, and the sweep
        // tries again. Deleting its row now would cascade away the project rows the retry needs.
        return Err(anyhow!(
            "Organization {} cleanup is not finished; it stays fenced for retry: {}",
            org_id,
            errors.join("; ")
        ));
    }

    // Only once no project rows are left. While one remains, its own cleanup is still relying on it.
    let remaining = repo
        .count_projects_of_organization(org_id)
        .await
        .context("Failed to count an organization's remaining projects")?;
    if remaining > 0 {
        tracing::debug!(
            org_id,
            remaining,
            "Organization data deleted; its row waits for its projects' tombstones"
        );
        return Ok(());
    }

    repo.delete_organization(None, org_id)
        .await
        .context("Failed to delete organization row")?;
    Ok(())
}

/// Delete a project: everything it owns, then the row that owns it.
///
/// # Why there is a fence at all
///
/// Deletion touches four stores with no transaction over them - analytics rows, file bytes, file rows,
/// the project row - so there is no instant at which the project simply stops existing. It used to
/// delete the *data* first and the row last, which meant the project was fully live for the whole of
/// it: readable, and ingestible. A batch arriving in that window was written after its analytics rows
/// had been deleted, survived the rest of the cleanup, and ended up attached to a project id that then
/// had no row - invisible to every read path, counted against no quota, and inherited by the next
/// project created with the same id. If a step *failed*, the row was deleted anyway, which turned a
/// recoverable failure into data nothing could ever find again.
///
/// ```mermaid
/// stateDiagram-v2
///     [*] --> Live
///     Live --> Claimed: claim (compare-and-set)
///     Claimed --> Claimed: sweep resumes an abandoned cleanup
///     Claimed --> Claimed: a sweep deletes what appeared and counts a clean pass
///     Claimed --> [*]: enough consecutive clean sweeps, row deleted
///     note right of Claimed
///         reads report absent
///         ingestion refuses
///         rename refuses
///     end note
/// ```
///
/// The claim is a **tombstone**: it outlives the data rather than the other way round. While it is set the
/// project is not live, and it survives a restart, so a crash leaves the project fenced rather than
/// half-deleted and live.
///
/// # What the barrier does and does not promise
///
/// The write path checks the fence and then writes, and the two cannot be one act: spans live in the
/// analytics store and the fence in the transactional one, so no transaction spans them. A writer can
/// therefore read "live", have the tombstone land underneath it, and commit afterwards - and *no elapsed
/// time bounds that*. A blocking insert can outlive its statement timeout, an object store can retry, a
/// container can be paused. A wall-clock grace period was a guess dressed as a guarantee, which is why
/// there is not one.
///
/// What the tombstone does promise, and it is enough:
///
/// 1. No **new** writer passes the fence, because every write path asks whether the project accepts
///    writes and a tombstoned - or absent - project does not.
/// 2. Cleanup keeps running for as long as the row exists, so a late writer's spans are deleted by the
///    next sweep.
/// 3. The row is removed only after [`PROJECT_TOMBSTONE_CLEAN_SWEEPS`] consecutive sweeps have found
///    nothing, and a sweep that finds something starts that count over.
///
/// So a late write is *collected* rather than stranded, and the residual is stated rather than hidden: a
/// writer whose first commit lands after that many consecutive quiet sweeps would leave rows nothing
/// collects. That needs a writer stalled for ten minutes of wall clock while the request that started it
/// is long gone.
///
/// The consequence is that deletion is asynchronous. This returns once the data is deleted and the
/// project is invisible to every read and every write; [`start_claim_recovery_task`] removes the row once
/// it has watched it stay empty.
///
/// It also needs nothing from the instance that started it, which is what makes it correct in a
/// horizontally scaled deployment: the tombstone is a row, every instance's write path consults it, and
/// every instance's sweep advances it. Concurrent sweeps duplicate work rather than corrupt state.
///
/// Returns `Ok(false)` when there was no live project to delete.
pub async fn cleanup_project(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    project_id: &str,
) -> Result<bool> {
    let repo = database.repository();

    // The compare-and-set decides who owns this deletion. Losing it means the project was already
    // claimed or already gone - either way there is nothing for this caller to do. The cache goes with
    // it, so the project stops being readable at the same instant it stops being live.
    if !repo
        .claim_project_for_deletion(cache, project_id)
        .await
        .context("Failed to claim project for deletion")?
    {
        return Ok(false);
    }

    finish_project_deletion(database, analytics, file_service, cache, project_id).await?;
    Ok(true)
}

/// The part of a project deletion that is safe to run again: everything after the claim.
///
/// Every step is idempotent, which is what makes resumption possible rather than merely hopeful -
/// deleting rows that are already deleted and bytes that are already gone are both no-ops. Called by
/// [`cleanup_project`] once it holds the claim, and by the sweep for a claim whose owner died.
pub async fn finish_project_deletion(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    project_id: &str,
) -> Result<()> {
    let repo = database.repository();
    let analytics_repo = analytics.repository();
    let mut errors: Vec<String> = Vec::new();

    if let Err(e) = analytics_repo.delete_project_data(project_id).await {
        errors.push(format!("Analytics delete failed: {}", e));
    }

    // Files: bytes then rows, and the `files` rows are not reached by any cascade - `files.project_id`
    // has no foreign key to `projects`, so nothing else would ever remove them.
    if let Err(e) = file_service.delete_project(project_id).await {
        errors.push(format!("File delete failed: {}", e));
    }

    // API keys are org-scoped, so there is nothing project-scoped to remove.

    if !errors.is_empty() {
        // The row stays, claimed. That is the whole point: the project is already fenced, so nothing
        // new can arrive, and the sweep will find the claim and try again. Deleting the row here would
        // strand whatever survived with no project to find it by.
        return Err(anyhow!(
            "Project {} cleanup failed with {} errors, leaving it claimed for retry: {}",
            project_id,
            errors.len(),
            errors.join("; ")
        ));
    }

    // Verified, not assumed. A delete that reported success can still leave rows behind: ClickHouse
    // applies `ALTER TABLE ... DELETE` as an asynchronous mutation, and a batch that read the fence
    // before the claim can commit after it.
    let remaining = analytics_repo
        .count_project_rows(project_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(project_id, error = %e, "Could not verify a project's data is gone");
            u64::MAX
        });
    if remaining > 0 {
        // Not an error: this is the case the barrier exists for. A writer that read the fence before the
        // tombstone has committed, its rows were just deleted again, and the sweep count below starts
        // over - which is what "collected by continued cleanup" means in practice.
        tracing::info!(
            project_id,
            remaining,
            "A late writer's spans were collected; the project's tombstone stays"
        );
    }

    // The row goes on the strength of repeated observation, never on elapsed time.
    //
    // This is the barrier, and it is worth being precise about what it does and does not promise. It does
    // not promise that no writer commits after the count above: the fence and the spans are in different
    // stores, so a writer that read the fence before the tombstone can commit arbitrarily later - a
    // blocking insert that outlived its statement timeout, an object store retrying, a container that was
    // paused - and no elapsed time bounds that. A wall-clock grace period was therefore a guess dressed
    // as a guarantee.
    //
    // What it does promise: while the tombstone exists, no *new* writer passes the fence, and every sweep
    // deletes whatever has appeared. So a late write is collected by the next sweep, and the row is
    // removed only after `PROJECT_TOMBSTONE_CLEAN_SWEEPS` consecutive sweeps have found nothing - a sweep
    // that finds data resets the count and starts it over. The residual is stated rather than hidden: a
    // writer that first commits after that many quiet sweeps leaves rows nothing collects, which needs a
    // writer stalled for `CLAIM_RECOVERY_INTERVAL_SECS` times that many while its request is long gone.
    // Counting, deciding and removing are one statement, so a decision cannot be acted on after another
    // instance's sweep invalidated it - see `record_project_sweep`.
    let removed = repo
        .record_project_sweep(
            project_id,
            remaining == 0,
            PROJECT_TOMBSTONE_CLEAN_SWEEPS,
            // A window, not a sweep: every instance sweeps, and without this N instances would reach the
            // required count in one interval. Half the interval, so a slightly early tick still counts.
            (CLAIM_RECOVERY_INTERVAL_SECS / 2) as i64,
        )
        .await
        .context("Failed to record a project cleanup sweep")?;
    if removed {
        // The caches the row's disappearance invalidates. `delete_project` did this; the delete is inside
        // the sweep statement now, so it is done here.
        if let Some(cache) = cache {
            for key in [
                CacheKey::project(project_id),
                CacheKey::project_org(project_id),
            ] {
                if let Err(e) = cache.delete(&key).await {
                    tracing::warn!(project_id, error = %e, "Cache invalidation error");
                }
            }
        }
        tracing::debug!(project_id, "Project tombstone removed");
    } else {
        tracing::debug!(
            project_id,
            "Project data deleted; its row waits for repeated evidence that it stays deleted"
        );
    }
    Ok(())
}

/// Advance every tombstoned project and organization: delete what has appeared, remove what is finished.
///
/// Called on a timer and at startup, and it is not only a recovery path. A tombstone is *meant* to be
/// revisited: it is removed on repeated evidence that the data stays gone, and a sweep that finds a late
/// writer's spans deletes them and starts that evidence over. So this is the mechanism that makes
/// deletion complete, not a fallback for when something went wrong.
///
/// `stale_after_secs` keeps a deletion that is actively in progress from being picked up in parallel by
/// this sweep; concurrent runs would be harmless - every step is idempotent and the claims are
/// compare-and-set - but doing the same work twice on every instance of a horizontally scaled deployment
/// is worth avoiding.
pub async fn advance_pending_deletions(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    stale_after_secs: i64,
) -> Result<usize> {
    let repo = database.repository();
    let mut advanced = 0;

    let projects = repo
        .get_stale_claimed_projects(stale_after_secs)
        .await
        .context("Failed to look for tombstoned projects")?;
    for project_id in &projects {
        match finish_project_deletion(database, analytics, file_service, None, project_id).await {
            Ok(()) => advanced += 1,
            // Still tombstoned, so still fenced and still found next time.
            Err(e) => {
                tracing::warn!(project_id, error = %e, "Could not advance a project deletion")
            }
        }
    }

    // Spans written for a *session* that had already been deleted, and the traces they belong to.
    //
    // The trace sweep cannot cover this: a session's deletion resolved its traces at one instant, and a
    // trace that arrived after has no trace tombstone. Same discipline - leased, backed off, batched -
    // because the records are long-lived and what has to be bounded is the rate.
    match repo
        .claim_deleted_sessions_for_check(DELETED_TRACE_CHECK_LEASE_SECS, DELETED_TRACE_CHECK_BATCH)
        .await
    {
        Ok(claimed) => {
            for (project_id, session_id, claim_token) in claimed {
                // Every trace of the session, resolved *now* rather than from the deletion's snapshot -
                // that snapshot is exactly what a late trace escapes.
                let trace_ids = analytics
                    .repository()
                    .get_trace_ids_for_sessions(&project_id, std::slice::from_ref(&session_id))
                    .await;
                let was_quiet = match trace_ids {
                    Ok(ids) if ids.is_empty() => true,
                    Ok(ids) => {
                        tracing::warn!(
                            project_id,
                            session_id,
                            traces = ids.len(),
                            "Collected traces written for a session that had already been deleted"
                        );
                        // Tombstone them *before* deleting, and the order is load-bearing, not tidy.
                        //
                        // The delete is gated on the tombstone succeeding. Deleting the analytics rows while
                        // the tombstone write failed removes the data but leaves nothing that remembers the
                        // trace should stay deleted - so a writer still holding these spans (the exact reason
                        // this sweep exists) re-commits them, and this time no trace tombstone and no session
                        // tombstone covers them: the resurrection is permanent. Skipping the delete instead
                        // leaves the rows for the next sweep, which retries the whole step; the session
                        // tombstone that brought us here is untouched, so nothing is lost.
                        //
                        // And the delete removes **exactly the tombstoned `ids`**, not the session's current
                        // membership. `delete_sessions` re-resolves the session to its traces, so a late
                        // trace B that committed between the resolution above and the delete would be deleted
                        // without a tombstone - and a later child-only delivery for B (carrying no session
                        // id) then passes every fence and resurrects a headless trace permanently. Deleting
                        // the snapshot we tombstoned keeps delete and tombstone over the identical set; B is
                        // simply collected by the next sweep, which re-resolves and tombstones it too.
                        match repo.record_deleted_traces(&project_id, &ids).await {
                            Ok(()) => {
                                if let Err(ref e) = analytics
                                    .repository()
                                    .delete_traces(&project_id, &ids)
                                    .await
                                {
                                    tracing::warn!(project_id, session_id, error = %e, "Could not sweep a deleted session");
                                }
                            }
                            Err(ref e) => {
                                tracing::warn!(
                                    project_id,
                                    session_id,
                                    error = %e,
                                    "Could not tombstone a late session's traces; leaving the rows for the \
                                     next sweep rather than deleting data nothing would remember to keep gone"
                                );
                            }
                        }
                        // Not quiet either way: on success something was found, on failure the work is still
                        // pending - both mean look again at the base interval rather than backing off. The
                        // trace records now carry the file reconciliation, which is why this does not do it
                        // here.
                        false
                    }
                    Err(e) => {
                        tracing::warn!(project_id, session_id, error = %e, "Could not check a deleted session");
                        false
                    }
                };
                if let Err(e) = repo
                    .record_deleted_session_check(
                        &project_id,
                        &session_id,
                        claim_token,
                        was_quiet,
                        DELETED_TRACE_CHECK_BASE_SECS,
                        DELETED_TRACE_CHECK_MAX_SECS,
                    )
                    .await
                {
                    tracing::warn!(project_id, session_id, error = %e, "Could not record a session check");
                }
                advanced += 1;
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not claim deleted sessions for checking"),
    }

    // Spans written for a trace that had already been deleted.
    //
    // The write path checks the tombstone immediately before writing and re-checks immediately after, so
    // this collects only what a *crash* between those two left behind - and that window is exactly why a
    // pre-write check alone is not a guarantee: the tombstone is a row in this store and the spans go to
    // the analytics store, so nothing makes the pair atomic. Leased, backed-off and batched for the same
    // reason the deleted-project records are: the records are long-lived, so the *rate* is what must be
    // bounded rather than the count.
    match repo
        .claim_deleted_traces_for_check(DELETED_TRACE_CHECK_LEASE_SECS, DELETED_TRACE_CHECK_BATCH)
        .await
    {
        Ok(claimed) => {
            for (project_id, trace_id, claim_token) in claimed {
                let removed = analytics
                    .repository()
                    .delete_traces(&project_id, std::slice::from_ref(&trace_id))
                    .await;
                // "Quiet" requires no error as well as nothing found: a failed delete is a reason to look
                // again soon, not evidence the trace has gone quiet. DuckDB reports rows removed while
                // ClickHouse can only report how many ids it was asked about, so a *count* cannot decide
                // this - what decides it is whether anything is still readable, asked directly.
                let still_there = analytics
                    .repository()
                    .get_spans_for_trace(&project_id, &trace_id)
                    .await;
                // The files, but **only** once the rows are provably gone.
                //
                // Two mistakes are possible here and they pull in opposite directions. Repairing only the
                // analytics store leaves a permanent leak: a crash after the analytics delete but before
                // the association release leaves `ref_count` above zero, which the orphan sweeper cannot
                // select, while this sweep reads analytics as quiet and backs off to a daily floor. But
                // collecting the files *regardless* is worse - a transient delete failure then leaves
                // readable rows pointing at bytes this sweep has taken, which is the dangling reference the
                // whole write-files-before-rows ordering exists to prevent.
                //
                // So: rows first, files only if the delete succeeded and a direct read confirms nothing
                // remains. A scheduling change (`was_quiet`) cannot undo a destructive cleanup, so the
                // condition has to gate the cleanup rather than merely record its outcome.
                let rows_gone = removed.is_ok()
                    && matches!(still_there.as_ref().map(|spans| spans.is_empty()), Ok(true));
                let files = if rows_gone {
                    let outcome = file_service
                        .cleanup_traces(&project_id, std::slice::from_ref(&trace_id))
                        .await;
                    if let Err(ref e) = outcome {
                        tracing::warn!(project_id, trace_id, error = %e, "Could not collect a deleted trace's files");
                    }
                    outcome.is_ok()
                } else {
                    tracing::warn!(
                        project_id,
                        trace_id,
                        "Rows for a deleted trace are still readable, so its files are left alone: \
                         collecting them would leave a readable row pointing at bytes that are gone"
                    );
                    false
                };

                // "Quiet" requires *everything* to have come back clean: the rows gone and the files
                // collected. Deciding it from the analytics store alone let a file leak sit behind a
                // backed-off schedule.
                let was_quiet = rows_gone && files;
                if let Err(ref e) = removed {
                    tracing::warn!(project_id, trace_id, error = %e, "Could not sweep a deleted trace");
                } else if !was_quiet {
                    tracing::warn!(
                        project_id,
                        trace_id,
                        "Collected data written for a trace that had already been deleted"
                    );
                }
                if let Err(e) = repo
                    .record_deleted_trace_check(
                        &project_id,
                        &trace_id,
                        claim_token,
                        was_quiet,
                        DELETED_TRACE_CHECK_BASE_SECS,
                        DELETED_TRACE_CHECK_MAX_SECS,
                    )
                    .await
                {
                    tracing::warn!(project_id, trace_id, error = %e, "Could not record a trace check");
                }
                advanced += 1;
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not claim deleted traces for checking"),
    }

    let orgs = repo
        .get_stale_claimed_organizations(stale_after_secs)
        .await
        .context("Failed to look for tombstoned organizations")?;
    for org_id in &orgs {
        match finish_organization_deletion(database, analytics, file_service, None, org_id).await {
            Ok(()) => advanced += 1,
            Err(e) => {
                tracing::warn!(org_id, error = %e, "Could not advance an organization deletion")
            }
        }
    }

    // Projects whose rows are already gone, and this is the part that makes the guarantee hold for an
    // *arbitrarily* delayed writer rather than for one that finishes within a few sweeps. The tombstone is
    // removed on finite evidence; a writer that read the fence before it can still commit afterwards, and
    // with the row gone nothing would know the project had existed. `deleted_projects` knows, so those
    // rows are collected however late they appear - and the residual becomes the retention below rather
    // than a handful of minutes.
    // A leased, bounded, backed-off batch. The records are permanent, so what has to be bounded is the
    // *rate*: one claim per id (no replica repeats another's work), a lease so a slow batch is not
    // re-claimed while it runs, a geometric backoff per quiet check, and a cap per sweep.
    match repo
        .claim_deleted_projects_for_check(
            DELETED_PROJECT_CHECK_LEASE_SECS,
            DELETED_PROJECT_CHECK_BATCH,
        )
        .await
    {
        Ok(deleted) => {
            for (project_id, claim_token) in deleted {
                // Files, unconditionally. Gating this on the analytics count was wrong in the one
                // direction that matters: a batch can pass the fence, pause, and then store file bytes and
                // their `trace_files` associations while its spans are dropped by the check next to the
                // write - so the analytics count is zero while bytes remain, unreachable and counted
                // against the quota of a project that no longer exists.
                let files = file_service.delete_project(&project_id).await;
                if let Err(ref e) = files {
                    tracing::warn!(project_id, error = %e, "Could not collect a deleted project's files");
                }
                let rows = analytics.repository().count_project_rows(&project_id).await;
                if let Err(ref e) = rows {
                    tracing::warn!(project_id, error = %e, "Could not check a deleted project");
                }

                if let Ok(count) = rows
                    && count > 0
                {
                    tracing::warn!(
                        project_id,
                        rows = count,
                        "Collected rows that arrived for a project after its row was deleted"
                    );
                    if let Err(e) = analytics
                        .repository()
                        .delete_project_data(&project_id)
                        .await
                    {
                        tracing::warn!(project_id, error = %e, "Could not collect them");
                    } else {
                        advanced += 1;
                    }
                }

                // Quiet means *nothing at all*: no files, no rows, and no error looking. Deciding it from
                // the analytics count alone counted a sweep that had just deleted a late writer's files -
                // or one whose storage delete failed - as evidence the project had gone quiet, and pushed
                // the next look toward a day away. Anything found or any error keeps it at the base
                // interval.
                let was_quiet = matches!(files, Ok(0)) && matches!(rows, Ok(0));
                if let Err(e) = repo
                    .record_deleted_project_check(
                        &project_id,
                        claim_token,
                        was_quiet,
                        DELETED_PROJECT_CHECK_BASE_SECS,
                        DELETED_PROJECT_CHECK_MAX_SECS,
                    )
                    .await
                {
                    tracing::warn!(project_id, error = %e, "Could not record a deleted project check");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not claim deleted projects for checking"),
    }

    // Nothing prunes `deleted_projects`, and that is the design rather than an omission.
    //
    // Any retention is a bound on how late a pre-tombstone writer may commit and still be collected, and
    // there is no honest value for one: a stalled writer is not on a schedule. Keeping the record forever
    // removes the bound - "no data outlives its project" stops being "for seven days". The cost is one
    // narrow row per project ever deleted, and ids are cuid2, so a record is never ambiguous about which
    // project it speaks for. `forget_deleted_projects` exists for an operator who wants them gone; the
    // only thing deleting one loses is the collection of a write in flight since then.

    if advanced > 0 {
        // "Advanced", not "finished": most passes only add to the evidence a tombstone waits for, and a
        // log line claiming a deletion had completed when its row is still there is worse than none.
        tracing::debug!(
            projects = projects.len(),
            organizations = orgs.len(),
            advanced,
            "Advanced pending deletions"
        );
    }
    Ok(advanced)
}

/// The timer that advances every pending deletion, and recovers any that was abandoned.
///
/// Startup alone is not enough, and the gap is not hypothetical: a process that dies one second after
/// claiming and restarts immediately leaves a claim the startup sweep reads as *fresh* - it is younger
/// than the staleness threshold, which exists so a deletion in progress is never mistaken for an
/// abandoned one. Nothing then looks again until the next restart, so the file stays unassociable and
/// the project stays hidden for as long as the process happens to live.
///
/// Both kinds of claim are swept here rather than in two tasks: they are the same failure with two
/// owners, and one timer is one thing to reason about.
pub fn start_claim_recovery_task(
    database: Arc<TransactionalService>,
    analytics: Arc<AnalyticsService>,
    file_service: Arc<FileService>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(CLAIM_RECOVERY_INTERVAL_SECS));
        // The first tick fires immediately, and it is kept: nothing sweeps before this task now, because
        // doing it inline made every new instance wait for work that is not urgent.
        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::debug!("Claim recovery task shutting down");
                        break;
                    }
                }
                _ = ticker.tick() => {
                    match advance_pending_deletions(
                        &database,
                        &analytics,
                        &file_service,
                        PROJECT_DELETION_CLAIM_STALE_SECS,
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "Could not advance pending deletions"),
                    }
                    match crate::data::files::cleanup::cleanup_zero_ref_files(
                        file_service.storage(),
                        &database,
                        FILE_DELETION_CLAIM_STALE_SECS,
                    )
                    .await
                    {
                        Ok(0) => {}
                        Ok(n) => tracing::debug!(deleted = n, "Reclaimed unreferenced files"),
                        Err(e) => tracing::warn!(error = %e, "Could not sweep unreferenced files"),
                    }
                }
            }
        }
    })
}

/// Invalidate API key caches for an organization
///
/// Fetches all API key hashes and invalidates their individual caches,
/// then invalidates the organization's API key list cache.
async fn invalidate_org_api_key_caches(
    repo: &dyn crate::data::traits::TransactionalRepository,
    cache: &CacheService,
    org_id: &str,
) {
    // Get all key hashes for this organization
    match repo.get_api_key_hashes_for_org(org_id).await {
        Ok(hashes) => {
            // Invalidate individual key caches
            for hash in hashes {
                let key = CacheKey::api_key_by_hash(&hash);
                if let Err(e) = cache.delete(&key).await {
                    tracing::debug!(
                        org_id = %org_id,
                        error = %e,
                        "Failed to invalidate API key cache"
                    );
                }
            }
        }
        Err(e) => {
            tracing::debug!(
                org_id = %org_id,
                error = %e,
                "Failed to get API key hashes for cache invalidation"
            );
        }
    }

    // Invalidate organization's API key list cache
    let list_key = CacheKey::api_keys_for_org(org_id);
    if let Err(e) = cache.delete(&list_key).await {
        tracing::debug!(
            org_id = %org_id,
            error = %e,
            "Failed to invalidate API key list cache"
        );
    }
}

#[cfg(test)]
mod tombstone_ordering_tests {
    /// The late-session sweep must not delete analytics rows unless their trace tombstone was recorded.
    ///
    /// The invariant, and why it cannot be a behavioural test here: `advance_pending_deletions` takes the
    /// concrete service enums, not trait objects, so there is no seam to inject a failing
    /// `record_deleted_traces` without standing up a fault-injecting database. What the test *can* pin is
    /// the shape the correctness rests on - `delete_sessions` reachable only through the `Ok` arm of
    /// `record_deleted_traces`. Move it out of that arm, and a tombstone-write failure deletes the data with
    /// nothing left to remember it should stay gone; a writer still holding those spans then resurrects them
    /// permanently, since neither a trace nor a session tombstone now covers them.
    ///
    /// Structural, by reading this file, because that regression compiles and passes every other test.
    #[test]
    fn a_late_session_delete_is_gated_on_its_trace_tombstone() {
        let source = include_str!("cleanup.rs");

        // The window: from the tombstone write to the end of its match. `delete_sessions` must appear inside
        // (as delete_traces of the tombstoned ids) must appear inside it, and only after an `Ok(())` arm - never at the block's top level where a failed tombstone would
        // fall through to it.
        let anchor = source
            .find("match repo.record_deleted_traces(&project_id, &ids).await {")
            .expect("the late-session tombstone match has moved; re-anchor this test");
        let after = &source[anchor..];
        let ok_arm = after
            .find("Ok(()) => {")
            .expect("the tombstone match should have an Ok arm that performs the delete");
        let err_arm = after
            .find("Err(ref e) => {")
            .expect("the tombstone match should have an Err arm that skips the delete");
        // The window ends where the late-session handling does, at the trace-sweep section that follows.
        let window_end = after
            .find("// Spans written for a trace that had already been deleted.")
            .expect("the late-session block should be followed by the trace sweep");
        let window = &after[..window_end];
        let needle = "delete_traces(&project_id, &ids)";

        // Exactly one delete, not merely one inside the Ok arm: an *additional* unguarded delete after the
        // match is the precise regression, and a test that only confirms a guarded one exists would pass
        // beside it. Verified - the weaker form did.
        let deletes: Vec<usize> = window.match_indices(needle).map(|(i, _)| i).collect();
        assert_eq!(
            deletes.len(),
            1,
            "the late-session sweep must delete in exactly one place; {} found. A second, unguarded \
             delete removes data that a failed tombstone write left nothing to remember.",
            deletes.len()
        );
        let delete = deletes[0];
        assert!(
            ok_arm < delete && delete < err_arm,
            "the delete must sit inside the Ok arm of record_deleted_traces - between the Ok arm at \
             {ok_arm} and the Err arm at {err_arm}, but it is at {delete}. Outside it, a failed tombstone \
             write deletes data that nothing then remembers to keep deleted."
        );
    }
}
