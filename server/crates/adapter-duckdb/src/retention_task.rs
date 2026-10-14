//! The retention task: the loop that drains recorded cleanup and applies the retention limits, on a timer.

use std::sync::Arc;
use std::time::Duration;

use sideseat_core::config::RetentionConfig;
use sideseat_core::utils::collection_fence::CollectionFence;
use sideseat_ports::blobs::RetentionFileReconciler;
use sideseat_ports::traits::{StorageGovernance, TransactionalRepository};
use sideseat_ports::types::ProjectId;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::*;

impl DuckdbService {
    #[allow(clippy::too_many_arguments)]
    pub fn start_retention_task(
        self: &Arc<Self>,
        config: RetentionConfig,
        quota_bytes: u64,
        mut shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<dyn RetentionFileReconciler>>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        governance: Arc<dyn StorageGovernance + Send + Sync>,
        fence: CollectionFence,
    ) -> Option<JoinHandle<()>> {
        // **The task starts even with no limits configured**, because it is also the only thing that drains
        // outstanding cleanup. A deployment that crashed mid-cleanup and restarted with retention switched off
        // would otherwise leave those associations, ref-counts and favourites owed forever - and the record is
        // the only thing that knows about them, so nothing else can find them. Retention itself is skipped in
        // that case; the claim loop is not.
        let retention_configured = config.max_spans.is_some() || config.max_age_minutes.is_some();
        if !retention_configured {
            tracing::debug!(
                "Retention has no limits configured; the task still runs to drain any outstanding cleanup"
            );
        }

        let db = Arc::clone(self);
        tracing::debug!(
            max_spans = ?config.max_spans,
            max_age_minutes = ?config.max_age_minutes,
            "Starting retention task"
        );

        Some(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(DUCKDB_RETENTION_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            tracing::debug!("Retention task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        // Retention deletes; none of it runs while this process's store lease is lost.
                        let Some(_pass) = fence.enter("retention") else {
                            tracing::debug!("Collection is fenced; retention waits");
                            continue;
                        };
                        // First, whatever a previous cycle recorded and did not finish. Claimed and leased, so
                        // several instances drain different candidates; the reconciliation is idempotent, so
                        // acting on one whose deletion never committed is a no-op rather than damage - which is
                        // why this needs no state machine to tell the two apart.
                        match database
                            .claim_retention_cleanup(Self::RETENTION_CLEANUP_CLAIM, Self::RETENTION_CLEANUP_LEASE_SECS)
                            .await
                        {
                            Ok(claimed) if !claimed.is_empty() => {
                                // Grouped with their **claim tokens**, because completion is conditional on
                                // them: a stale worker that finished old work must not delete a newer intent.
                                // In order, projects and their traces, so the cleanup writes go out the same way
                                // every time: the claim returns its rows in no order of its own.
                                let mut claimed = claimed;
                                claimed.sort_unstable();
                                let mut by_project: std::collections::BTreeMap<String, Vec<(String, i64)>> =
                                    std::collections::BTreeMap::new();
                                for (project_id, trace_id, token) in claimed {
                                    by_project
                                        .entry(project_id)
                                        .or_default()
                                        .push((trace_id, token));
                                }
                                tracing::debug!(
                                    projects = by_project.len(),
                                    "Resuming retention cleanup a previous cycle did not finish"
                                );
                                for (project_id, claimed) in &by_project {
                                    if !fence.is_open() {
                                        break;
                                    }
                                    let project_id = ProjectId::from(project_id.as_str());
                                    let owner = format!(
                                        "duckdb-cleanup:{}:{}",
                                        std::process::id(),
                                        db.clock.now().timestamp_micros()
                                    );
                                    let lock_now = db.clock.now();
                                    let lease_until = lock_now + chrono::TimeDelta::hours(2);
                                    let acquired = match governance
                                        .acquire_project_maintenance(
                                            &project_id,
                                            &owner,
                                            lock_now,
                                            lease_until,
                                        )
                                        .await
                                    {
                                        Ok(acquired) => acquired,
                                        Err(error) => {
                                            tracing::warn!(
                                                project_id = %project_id,
                                                %error,
                                                "Could not acquire retention-cleanup fence"
                                            );
                                            false
                                        }
                                    };
                                    if !acquired {
                                        continue;
                                    }

                                    let held = match governance
                                        .active_project_hold(&project_id, db.clock.now())
                                        .await
                                    {
                                        Ok(hold) => hold.is_some(),
                                        Err(error) => {
                                            // Uncertainty is resolved in the preservation direction.
                                            tracing::warn!(
                                                project_id = %project_id,
                                                %error,
                                                "Could not check legal hold before retention cleanup"
                                            );
                                            true
                                        }
                                    };
                                    let trace_ids: Vec<String> =
                                        claimed.iter().map(|(t, _)| t.clone()).collect();
                                    if !held {
                                        Self::finish_retention_cleanup(
                                            &DuckdbRepository(Arc::clone(&db)),
                                            &database,
                                            file_service.as_ref(),
                                            &project_id,
                                            &trace_ids,
                                            Some(claimed),
                                            &fence,
                                        )
                                        .await;
                                    }
                                    if let Err(error) = governance
                                        .release_project_maintenance(&project_id, &owner)
                                        .await
                                    {
                                        tracing::warn!(
                                            project_id = %project_id,
                                            %error,
                                            "Could not release retention-cleanup fence"
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!(
                                error = %e,
                                "Could not claim outstanding retention cleanup; it stays recorded and is \
                                 retried next cycle"
                            ),
                        }

                        if !retention_configured {
                            continue;
                        }

                        let owner = format!(
                            "duckdb-retention:{}:{}",
                            std::process::id(),
                            db.clock.now().timestamp_micros()
                        );
                        let projects = match governance.storage_project_ids(usize::MAX).await {
                            Ok(projects) => projects,
                            Err(error) => {
                                tracing::warn!(%error, "Could not list projects for retention fencing");
                                continue;
                            }
                        };
                        for project_id in projects {
                            if !fence.is_open() {
                                break;
                            }
                            let lock_now = db.clock.now();
                            let lease_until = lock_now + chrono::TimeDelta::hours(2);
                            let acquired = match governance
                                .acquire_project_maintenance(
                                    &project_id,
                                    &owner,
                                    lock_now,
                                    lease_until,
                                )
                                .await
                            {
                                Ok(acquired) => acquired,
                                Err(error) => {
                                    tracing::warn!(project_id = %project_id, %error, "Could not acquire retention fence");
                                    false
                                }
                            };
                            if !acquired {
                                continue;
                            }

                            let held = match governance
                                .active_project_hold(&project_id, db.clock.now())
                                .await
                            {
                                Ok(hold) => hold.is_some(),
                                Err(error) => {
                                    tracing::warn!(
                                        project_id = %project_id,
                                        %error,
                                        "Could not check legal hold before retention"
                                    );
                                    true
                                }
                            };
                            let retention_result = if held {
                                None
                            } else {
                                Some(
                                    db.run_project_retention(
                                        &config,
                                        quota_bytes,
                                        &project_id,
                                        &database,
                                        &governance,
                                        &fence,
                                    )
                                        .await,
                                )
                            };
                            match retention_result {
                                None => {}
                                Some(Ok(result)) => {
                                    // Keep file/ref-count cleanup under the same project lease as the
                                    // analytical deletion. Otherwise a hold can begin after the span
                                    // transaction commits but before its already-recorded cleanup removes
                                    // associated bytes.
                                    if let Some(trace_ids) =
                                        result.trace_ids_by_project.get(project_id.as_str())
                                    {
                                        Self::finish_retention_cleanup(
                                            &DuckdbRepository(Arc::clone(&db)),
                                            &database,
                                            file_service.as_ref(),
                                            &project_id,
                                            trace_ids,
                                            result
                                                .cleanup_tokens
                                                .get(project_id.as_str())
                                                .map(|tokens| tokens.as_slice()),
                                            &fence,
                                        )
                                        .await;
                                    }
                                }
                                Some(Err(error)) => {
                                    tracing::warn!(
                                        project_id = %project_id,
                                        %error,
                                        "Retention cleanup failed"
                                    );
                                }
                            }

                            if let Err(error) = governance
                                .release_project_maintenance(&project_id, &owner)
                                .await
                            {
                                tracing::warn!(project_id = %project_id, %error, "Could not release retention fence");
                            }
                        }
                    }
                }
            }
        }))
    }
}
