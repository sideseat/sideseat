//! Cross-store cleanup logic for organization and project deletion.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::files::FileService;
use sideseat_core::constants::{
    CLAIM_RECOVERY_INTERVAL_SECS, DELETED_PROJECT_CHECK_BASE_SECS, DELETED_PROJECT_CHECK_BATCH,
    DELETED_PROJECT_CHECK_LEASE_SECS, DELETED_PROJECT_CHECK_MAX_SECS,
    DELETED_TRACE_CHECK_BASE_SECS, DELETED_TRACE_CHECK_BATCH, DELETED_TRACE_CHECK_LEASE_SECS,
    DELETED_TRACE_CHECK_MAX_SECS, FILE_DELETION_CLAIM_STALE_SECS,
    PROJECT_DELETION_CLAIM_STALE_SECS, PROJECT_TOMBSTONE_CLEAN_SWEEPS,
};
use sideseat_ports::cache::{CacheKey, CacheStore};
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::ProjectId;

type TransactionalStore = dyn TransactionalRepository + Send + Sync + 'static;
type AnalyticsStore = dyn AnalyticsRepository + Send + Sync + 'static;

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
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    file_service: &Arc<FileService>,
    cache: Option<&dyn CacheStore>,
    org_id: &str,
) -> Result<bool> {
    let repo = database.as_ref();

    // The claim and its journal entry in **one transaction**, and the entry written only if this caller won.
    //
    // Neither an ordering nor a separate call can give that. Journalling first wrote an entry for every losing
    // caller - and an organization cleanup re-runs while its projects' tombstones remain, so one deletion
    // accumulated permanent, quota-counted records without bound. Journalling afterwards left a fenced
    // organization that a later sweep deletes anyway with no record.
    if !repo
        .claim_organization_for_deletion_journalled(org_id)
        .await
        .context("Failed to claim and journal the organization deletion")?
    {
        return Ok(false);
    }

    // API keys are org-scoped, so their caches go now: the organization is no longer usable.
    if let Some(cache) = cache {
        invalidate_org_api_key_caches(repo, cache, org_id).await;
    }

    finish_organization_deletion(database, analytics, file_service, org_id).await?;
    Ok(true)
}

/// The resumable part of an organization deletion: everything after the claim.
///
/// Idempotent throughout, so the sweep can run it repeatedly - which it must, because the organization
/// row waits for its projects and a project row waits for repeated evidence that its data is gone.
pub async fn finish_organization_deletion(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    file_service: &Arc<FileService>,
    org_id: &str,
) -> Result<()> {
    let repo = database.as_ref();
    let mut errors: Vec<String> = Vec::new();

    // Every project fenced first. A project that is not fenced can still be written to while its data
    // is being deleted, and nothing later in this function would notice.
    for project_id in repo.list_project_ids(org_id).await? {
        let project_id = ProjectId::from(project_id);
        // Fenced and journalled in one transaction, and the entry only if this call won the claim - see
        // `cleanup_organization` for why. The returned bool is discarded here deliberately: losing the claim means
        // another caller owns this project's deletion, which is fine, and the entry was written by whoever won.
        if let Err(e) = repo
            .claim_project_for_deletion_journalled(&project_id)
            .await
        {
            return Err(anyhow!(
                "Failed to fence and journal project {} of organization {}: {}",
                project_id,
                org_id,
                e
            ));
        }
        if let Err(e) =
            finish_project_deletion(database, analytics, file_service, &project_id).await
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

    repo.delete_organization(org_id)
        .await
        .context("Failed to delete organization row")?;
    Ok(())
}

/// Fence a project, delete its owned data, and leave final row removal to the recovery sweep.
///
/// Deletion crosses the transactional, analytics, and object stores, so no transaction can remove everything
/// atomically. The project row therefore becomes a durable tombstone before cleanup starts. Reads and new writes
/// reject a tombstoned project, while repeated sweeps remove writes that passed the fence before it was set.
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
/// The protocol provides three layers:
///
/// 1. No **new** writer passes the fence, because every write path asks whether the project accepts
///    writes and a tombstoned - or absent - project does not.
/// 2. Cleanup keeps running for as long as the row exists, so a late writer's spans are deleted by the
///    next sweep.
/// 3. The row is removed only after [`PROJECT_TOMBSTONE_CLEAN_SWEEPS`] consecutive sweeps have found
///    nothing, and that transaction creates a permanent `deleted_projects` residual. The residual continues
///    collecting a writer that commits after the project row is gone.
///
/// The consequence is that deletion is asynchronous. This returns once the data is deleted and the
/// project is invisible to every read and every write; [`start_claim_recovery_task`] removes the row once
/// it has watched it stay empty.
///
/// The durable tombstone and residual make the protocol independent of the instance that started it.
///
/// Returns `Ok(false)` when there was no live project to delete.
pub async fn cleanup_project(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    file_service: &Arc<FileService>,
    project_id: &ProjectId,
) -> Result<bool> {
    let repo = database.as_ref();

    // The compare-and-set decides who owns this deletion. Losing it means the project was already
    // claimed or already gone - either way there is nothing for this caller to do.
    //
    // Claim and journal atomically. The journal row exists only when this caller wins the fence, so retries do
    // not create duplicate permanent records and a successful claim can never lack restore evidence.
    if !repo
        .claim_project_for_deletion_journalled(project_id)
        .await
        .context("Failed to claim and journal the project deletion")?
    {
        return Ok(false);
    }

    // The resumable phase never appends another project journal row.
    finish_project_deletion(database, analytics, file_service, project_id).await?;
    Ok(true)
}

/// The part of a project deletion that is safe to run again: everything after the claim.
///
/// Every step is idempotent. Called by [`cleanup_project`] after the claim and by the sweep when a claim lease
/// expires.
pub async fn finish_project_deletion(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    file_service: &Arc<FileService>,
    project_id: &ProjectId,
) -> Result<()> {
    let repo = database.as_ref();
    let analytics_repo = analytics.as_ref();
    let mut errors: Vec<String> = Vec::new();

    if let Err(e) = analytics_repo.delete_project_data(project_id).await {
        errors.push(format!("Analytics delete failed: {}", e));
    }

    // Files: bytes then rows, and the `files` rows are not reached by any cascade - `files.project_id`
    // has no foreign key to `projects`, so nothing else would ever remove them.
    if let Err(e) = file_service.delete_project(project_id).await {
        errors.push(format!("File delete failed: {}", e));
    }

    if !errors.is_empty() {
        // Keep the project fenced so the sweep can retry whatever survived.
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
            tracing::warn!(project_id = %project_id, error = %e, "Could not verify a project's data is gone");
            u64::MAX
        });
    if remaining > 0 {
        // Not an error: this is the case the barrier exists for. A writer that read the fence before the
        // tombstone has committed, its rows were just deleted again, and the sweep count below starts
        // over - which is what "collected by continued cleanup" means in practice.
        tracing::info!(
            project_id = %project_id,
            remaining,
            "A late writer's spans were collected; the project's tombstone stays"
        );
    }

    // Repeated clean observations remove the tombstone and create its permanent residual atomically. A sweep
    // that finds data resets the evidence; the residual covers commits that arrive after row removal.
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
        tracing::debug!(project_id = %project_id, "Project tombstone removed");
    } else {
        tracing::debug!(
            project_id = %project_id,
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
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    file_service: &Arc<FileService>,
    stale_after_secs: i64,
) -> Result<usize> {
    let repo = database.as_ref();
    let mut advanced = 0;

    let projects = repo
        .get_stale_claimed_projects(stale_after_secs)
        .await
        .context("Failed to look for tombstoned projects")?;
    for (project_id, observed) in &projects {
        let project_id = ProjectId::from(project_id.as_str());
        // Leased before the work, so N replicas share the backlog instead of each doing all of it. The lease
        // is the tombstone's own timestamp pushed forward, so the fence never lifts; a resumer that dies
        // leaves the project looking abandoned again once the window passes.
        match repo.reclaim_stale_project(&project_id, *observed).await {
            Ok(true) => {}
            Ok(false) => continue, // another replica has it
            Err(e) => {
                tracing::warn!(project_id = %project_id, error = %e, "Could not lease a project deletion");
                continue;
            }
        }
        match finish_project_deletion(database, analytics, file_service, &project_id).await {
            Ok(()) => advanced += 1,
            // Still tombstoned, so still fenced and still found next time.
            Err(e) => {
                tracing::warn!(project_id = %project_id, error = %e, "Could not advance a project deletion")
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
                let project_id = ProjectId::from(project_id);
                // Every trace of the session, resolved *now* rather than from the deletion's snapshot -
                // that snapshot is exactly what a late trace escapes.
                let trace_ids = analytics
                    .as_ref()
                    .get_trace_ids_for_sessions(
                        &project_id,
                        std::slice::from_ref(&session_id),
                        None,
                    )
                    .await;
                let was_quiet = match trace_ids {
                    Ok(ids) if ids.is_empty() => true,
                    Ok(ids) => {
                        tracing::warn!(
                            project_id = %project_id,
                            session_id,
                            traces = ids.len(),
                            "Collected traces written for a session that had already been deleted"
                        );
                        // Journal and tombstone this exact snapshot before deleting it. Re-resolving the session
                        // during deletion could include a newer trace that has no trace tombstone; deleting after
                        // a failed transactional write would leave no durable evidence for any of these traces.
                        // On failure the rows remain for the next session sweep.
                        match repo
                            .record_deleted_traces_journalled(&project_id, &ids)
                            .await
                        {
                            Ok(()) => {
                                if let Err(ref e) =
                                    analytics.as_ref().delete_traces(&project_id, &ids).await
                                {
                                    tracing::warn!(project_id = %project_id, session_id, error = %e, "Could not sweep a deleted session");
                                }
                            }
                            Err(ref e) => {
                                tracing::warn!(
                                project_id = %project_id,
                                    session_id,
                                    error = %e,
                                    "Could not tombstone and journal a late session's traces; leaving the rows \
                                     for the next sweep rather than deleting data nothing would remember to \
                                     keep gone"
                                );
                            }
                        }
                        // Finding rows or failing to record their deletion both keep the check at its base rate.
                        // Trace residuals own subsequent file reconciliation.
                        false
                    }
                    Err(e) => {
                        tracing::warn!(project_id = %project_id, session_id, error = %e, "Could not check a deleted session");
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
                    tracing::warn!(project_id = %project_id, session_id, error = %e, "Could not record a session check");
                }
                advanced += 1;
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not claim deleted sessions for checking"),
    }

    // Spans written for a trace that had already been deleted.
    //
    // Reconcile permanent trace tombstones. Claims are leased, backed off, and batched because the records are
    // long-lived while the work rate must remain bounded.
    match repo
        .claim_deleted_traces_for_check(DELETED_TRACE_CHECK_LEASE_SECS, DELETED_TRACE_CHECK_BATCH)
        .await
    {
        Ok(claimed) => {
            for (project_id, trace_id, claim_token) in claimed {
                let project_id = ProjectId::from(project_id);
                let delete_result = analytics
                    .as_ref()
                    .delete_traces(&project_id, std::slice::from_ref(&trace_id))
                    .await;
                // Backends expose different delete counts, so a direct read establishes whether rows remain.
                let remaining_read = analytics
                    .as_ref()
                    .get_spans_for_trace(&project_id, &trace_id, 1)
                    .await;
                // File associations are released only after a successful delete and an empty verification read;
                // otherwise cleanup could leave readable rows pointing at missing bytes.
                let rows_gone = delete_result.is_ok()
                    && matches!(
                        remaining_read.as_ref().map(|spans| spans.is_empty()),
                        Ok(true)
                    );
                let files_reconciled = if rows_gone {
                    let outcome = file_service
                        .cleanup_traces(&project_id, std::slice::from_ref(&trace_id))
                        .await;
                    if let Err(ref e) = outcome {
                        tracing::warn!(project_id = %project_id, trace_id, error = %e, "Could not collect a deleted trace's files");
                    }
                    outcome.is_ok()
                } else {
                    tracing::warn!(
                        project_id = %project_id,
                        trace_id,
                        "Rows for a deleted trace are still readable, so its files are left alone: \
                         collecting them would leave a readable row pointing at bytes that are gone"
                    );
                    false
                };

                // Back off only after both analytical rows and file associations are reconciled.
                let was_quiet = rows_gone && files_reconciled;
                if let Err(ref e) = delete_result {
                    tracing::warn!(project_id = %project_id, trace_id, error = %e, "Could not sweep a deleted trace");
                } else if !was_quiet {
                    tracing::warn!(
                        project_id = %project_id,
                        trace_id,
                        "Deleted trace is not quiet after cleanup; keeping it on the base schedule"
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
                    tracing::warn!(project_id = %project_id, trace_id, error = %e, "Could not record a trace check");
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
    for (org_id, observed) in &orgs {
        // Leased, as the project loop above is and for the same reasons.
        match repo.reclaim_stale_organization(org_id, *observed).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::warn!(org_id, error = %e, "Could not lease an organization deletion");
                continue;
            }
        }
        match finish_organization_deletion(database, analytics, file_service, org_id).await {
            Ok(()) => advanced += 1,
            Err(e) => {
                tracing::warn!(org_id, error = %e, "Could not advance an organization deletion")
            }
        }
    }

    // Permanent project residuals collect writes that commit after the project row is removed. Claims are
    // exclusive, leased, backed off after quiet checks, and bounded per sweep.
    match repo
        .claim_deleted_projects_for_check(
            DELETED_PROJECT_CHECK_LEASE_SECS,
            DELETED_PROJECT_CHECK_BATCH,
        )
        .await
    {
        Ok(deleted) => {
            for (project_id, claim_token) in deleted {
                let project_id = ProjectId::from(project_id);
                // File ownership is checked independently from analytics because a partially committed batch can
                // leave bytes and associations without an analytical row.
                let file_result = file_service.delete_project(&project_id).await;
                if let Err(ref e) = file_result {
                    tracing::warn!(project_id = %project_id, error = %e, "Could not collect a deleted project's files");
                }
                let row_count_result = analytics.as_ref().count_project_rows(&project_id).await;
                if let Err(ref e) = row_count_result {
                    tracing::warn!(project_id = %project_id, error = %e, "Could not check a deleted project");
                }

                if let Ok(count) = row_count_result.as_ref()
                    && *count > 0
                {
                    tracing::warn!(
                        project_id = %project_id,
                        rows = count,
                        "Found rows that arrived after the project row was deleted"
                    );
                    if let Err(e) = analytics.as_ref().delete_project_data(&project_id).await {
                        tracing::warn!(project_id = %project_id, error = %e, "Could not collect them");
                    } else {
                        tracing::debug!(
                            project_id = %project_id,
                            rows = count,
                            "Collected rows for a deleted project"
                        );
                        advanced += 1;
                    }
                }

                // Back off only when both stores report no work and no error.
                let was_quiet = matches!(file_result, Ok(0)) && matches!(row_count_result, Ok(0));
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
                    tracing::warn!(project_id = %project_id, error = %e, "Could not record a deleted project check");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not claim deleted projects for checking"),
    }

    // `deleted_projects` is permanent because any retention period would bound how late an in-flight writer can
    // commit and still be collected. Operators may explicitly remove residuals through
    // `forget_deleted_projects`.

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
    database: Arc<TransactionalStore>,
    analytics: Arc<AnalyticsStore>,
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
                    match crate::files::cleanup::cleanup_zero_ref_files_governed(
                        file_service.storage(),
                        file_service.database(),
                        FILE_DELETION_CLAIM_STALE_SECS,
                        file_service.governance(),
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
    repo: &dyn sideseat_ports::traits::TransactionalRepository,
    cache: &dyn CacheStore,
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
    /// This source-shape guard pins `delete_traces` inside the successful
    /// `record_deleted_traces_journalled` arm. The composite repository trait has no narrow fault-injection
    /// implementation for this orchestration test, while moving the delete outside that arm still compiles.
    #[test]
    fn a_late_session_delete_is_gated_on_its_trace_tombstone() {
        let source = include_str!("cleanup.rs");

        // Start at the late-session branch so an unconditional delete before the journal call is visible.
        let block_start = source
            .find("\"Collected traces written for a session that had already been deleted\"")
            .expect("the late-session block's log line has moved; re-anchor this test");
        let after = &source[block_start..];
        // The window ends where the late-session handling does, at the trace-sweep section that follows.
        let window_end = after
            .find("// Spans written for a trace that had already been deleted.")
            .expect("the late-session block should be followed by the trace sweep");
        let window = &after[..window_end];

        let record = window
            .find(".record_deleted_traces_journalled(&project_id, &ids)")
            .expect("the late-session record-and-journal call has moved; re-anchor this test");
        let ok_arm = window[record..]
            .find("Ok(()) => {")
            .map(|i| i + record)
            .expect("the record match should have an Ok arm that performs the delete");
        let err_arm = window[record..]
            .find("Err(ref e) => {")
            .map(|i| i + record)
            .expect("the record match should have an Err arm that skips the delete");
        let needle = "delete_traces(&project_id, &ids)";

        // Exactly one delete prevents an additional unguarded call before or after the match.
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
