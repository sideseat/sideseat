//! Cross-database cleanup logic for organization and project deletion

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::core::constants::{
    CLAIM_RECOVERY_INTERVAL_SECS, FILE_DELETION_CLAIM_STALE_SECS, PROJECT_DELETION_CLAIM_STALE_SECS,
};
use crate::data::AnalyticsService;
use crate::data::TransactionalService;
use crate::data::cache::{CacheKey, CacheService};
use crate::data::files::FileService;

/// Cleanup for organization deletion
///
/// Performs cleanup in the correct order:
/// 1. Get project_ids before transactional cascade deletes them
/// 2. Delete traces from analytics backend for each project
/// 3. Delete files from filesystem for each project
/// 4. Invalidate API key caches for the organization
/// 5. Delete org from transactional backend (cascades to projects, members, api_keys)
///
/// All cleanup steps are attempted even if some fail. Errors are collected
/// and returned after all steps complete to ensure maximum cleanup.
pub async fn cleanup_organization(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    cache: Option<&CacheService>,
    org_id: &str,
) -> Result<bool> {
    let mut errors: Vec<String> = Vec::new();

    let repo = database.repository();
    let analytics_repo = analytics.repository();

    // 1. Get project_ids before transactional cascade deletes them
    let project_ids = repo.list_project_ids(org_id).await?;

    // 2. Fence every project first, for the reason `cleanup_project` documents: until a project is
    //    claimed it is live, so a batch can arrive behind the cleanup and outlive it. Losing a claim
    //    means someone else is already deleting that project - their cleanup is the same work, and the
    //    org cascade removes the row either way.
    for project_id in &project_ids {
        if let Err(e) = repo.claim_project_for_deletion(project_id).await {
            errors.push(format!("Claim failed for project {}: {}", project_id, e));
        }
    }

    // 3. Delete traces from analytics backend for each project
    for project_id in &project_ids {
        if let Err(e) = analytics_repo.delete_project_data(project_id).await {
            errors.push(format!(
                "Analytics delete failed for project {}: {}",
                project_id, e
            ));
        }
    }

    // 4. Delete files from filesystem for each project
    for project_id in &project_ids {
        if let Err(e) = file_service.delete_project(project_id).await {
            errors.push(format!(
                "File delete failed for project {}: {}",
                project_id, e
            ));
        }
    }

    // 5. Invalidate API key caches for the organization
    if let Some(cache) = cache {
        invalidate_org_api_key_caches(repo.as_ref(), cache, org_id).await;
    }

    // 6. Delete org from transactional backend (cascades to projects, members, api_keys). Not to
    //    `files`: `files.project_id` has no foreign key to `projects`, so step 4 is the only thing
    //    that removes those rows.
    let deleted = repo
        .delete_organization(None, org_id)
        .await
        .context("Failed to delete organization")?;

    // Return error if any cleanup step failed
    if !errors.is_empty() {
        return Err(anyhow!(
            "Organization {} cleanup completed with {} errors: {}",
            org_id,
            errors.len(),
            errors.join("; ")
        ));
    }

    Ok(deleted)
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
///     Claimed --> [*]: data verified gone, row deleted
///     note right of Claimed
///         reads report absent
///         ingestion refuses
///         rename refuses
///     end note
/// ```
///
/// The claim is the fence and the tombstone at once: while it is set the project is not live, so no new
/// data can appear behind the cleanup's back, and it survives a restart, so a crash leaves the project
/// fenced rather than half-deleted and live. The row is removed only after the data is verified gone,
/// making the row's disappearance the *last* fact rather than the first - which is what lets the sweep
/// resume: a claimed row is a cleanup that has not finished.
///
/// Returns `Ok(false)` when there was no live project to delete.
pub async fn cleanup_project(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    _cache: Option<&CacheService>,
    project_id: &str,
) -> Result<bool> {
    let repo = database.repository();

    // The compare-and-set decides who owns this deletion. Losing it means the project was already
    // claimed or already gone - either way there is nothing for this caller to do.
    if !repo
        .claim_project_for_deletion(project_id)
        .await
        .context("Failed to claim project for deletion")?
    {
        return Ok(false);
    }

    finish_project_deletion(database, analytics, file_service, project_id).await?;
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

    // Verified, not assumed. A delete that reported success can still leave rows behind - ClickHouse
    // applies `ALTER TABLE ... DELETE` as an asynchronous mutation - and a batch that passed admission
    // before the claim may have persisted after it. Either way the row must outlive the data.
    let ids = [project_id.to_string()];
    let remaining = analytics_repo
        .count_spans_by_project(&ids)
        .await
        .map(|counts| counts.get(project_id).copied().unwrap_or(0))
        .unwrap_or_else(|e| {
            tracing::warn!(project_id, error = %e, "Could not verify a project's data is gone");
            u64::MAX
        });
    if remaining > 0 {
        return Err(anyhow!(
            "Project {} still has {} spans after deletion; leaving it claimed so the sweep retries",
            project_id,
            remaining
        ));
    }

    repo.delete_project(None, project_id)
        .await
        .context("Failed to delete project row")?;
    Ok(())
}

/// Finish project deletions that a crash or a failed step left claimed.
///
/// The claim is durable so the project stays fenced across a restart - which is what makes a stuck
/// deletion possible in the first place, and worse than one that never started: the project is hidden
/// from every read path while its data is still on disk and still counted against storage. Nothing
/// releases a claim, so this is the only thing that can finish one.
///
/// Every step after the claim is idempotent, so resuming is a retry rather than a special case. Called
/// at startup, which is when a crashed process's claims are found.
pub async fn resume_abandoned_project_deletions(
    database: &Arc<TransactionalService>,
    analytics: &Arc<AnalyticsService>,
    file_service: &Arc<FileService>,
    stale_after_secs: i64,
) -> Result<usize> {
    let stale = database
        .repository()
        .get_stale_claimed_projects(stale_after_secs)
        .await
        .context("Failed to look for abandoned project deletions")?;
    let mut finished = 0;
    for project_id in stale {
        match finish_project_deletion(database, analytics, file_service, &project_id).await {
            Ok(()) => {
                finished += 1;
                tracing::info!(project_id, "Finished an abandoned project deletion");
            }
            // Still claimed, so still fenced and still findable next time.
            Err(e) => {
                tracing::warn!(project_id, error = %e, "Could not finish an abandoned project deletion")
            }
        }
    }
    Ok(finished)
}

/// A periodic sweep that finishes whatever a crash or a failed step left claimed.
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
        // The first tick fires immediately; startup has already swept, so skip it.
        ticker.tick().await;
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
                    match resume_abandoned_project_deletions(
                        &database,
                        &analytics,
                        &file_service,
                        PROJECT_DELETION_CLAIM_STALE_SECS,
                    )
                    .await
                    {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(finished = n, "Finished abandoned project deletions"),
                        Err(e) => tracing::warn!(error = %e, "Could not sweep abandoned project deletions"),
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
