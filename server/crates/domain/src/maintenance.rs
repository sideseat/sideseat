//! Periodic storage maintenance: drain the retired content-body store and catch up the search index.
//!
//! One task, because both walk every project at a gentle rate and neither has a deadline.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::{ProjectId, SearchSignal};

use crate::content_bodies::ContentBodyService;
use crate::storage_governance::StorageGovernanceService;

const INTERVAL_SECS: u64 = 30;
const PROJECT_PAGE_SIZE: u32 = 100;
const SEARCH_BACKFILL_PAGE_SIZE: usize = 16;

/// Start the maintenance loop. Every tick runs one bounded body-drain pass, reconciles the quota of every
/// project whose stored bytes it changed, and advances the search backfill by one page per project.
pub fn start(
    bodies: ContentBodyService,
    governance: Arc<StorageGovernanceService>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    clock: Arc<dyn sideseat_ports::clock::Clock>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(INTERVAL_SECS));
        // Progress is stored, so replaying missed ticks would only repeat scans.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    drain_bodies(&bodies, &governance, clock.now()).await;
                    backfill_search(database.as_ref(), analytics.as_ref()).await;
                }
            }
        }
    })
}

async fn drain_bodies(
    bodies: &ContentBodyService,
    governance: &StorageGovernanceService,
    now: chrono::DateTime<chrono::Utc>,
) {
    match bodies.drain_pass(now).await {
        Ok((report, touched)) => {
            if !report.is_empty() {
                tracing::info!(
                    associations = report.associations_retired,
                    objects = report.orphans_deleted,
                    claims = report.stale_claims_finalized,
                    "Drained retired content bodies"
                );
            }
            for project_id in touched {
                if let Err(error) = governance.reconcile_project(&project_id).await {
                    tracing::warn!(%error, %project_id, "Could not reconcile quota after a body drain");
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, "Content-body drain pass failed; it resumes next tick")
        }
    }
}

async fn backfill_search(
    database: &(dyn TransactionalRepository + Send + Sync),
    analytics: &(dyn AnalyticsRepository + Send + Sync),
) {
    let mut page = 1;
    loop {
        let projects = match database.list_projects(page, PROJECT_PAGE_SIZE).await {
            Ok((projects, _)) => projects
                .into_iter()
                .map(|project| ProjectId::from(project.id.as_str()))
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::warn!(%error, "Could not list projects for search backfill");
                return;
            }
        };
        if projects.is_empty() {
            return;
        }
        for project_id in &projects {
            for signal in [SearchSignal::Spans, SearchSignal::Logs] {
                backfill_one(analytics, project_id, signal).await;
            }
        }
        if projects.len() < PROJECT_PAGE_SIZE as usize {
            return;
        }
        page += 1;
    }
}

async fn backfill_one(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    project_id: &ProjectId,
    signal: SearchSignal,
) {
    match crate::search::SearchService::backfill_project_page(
        analytics,
        project_id,
        signal,
        SEARCH_BACKFILL_PAGE_SIZE,
    )
    .await
    {
        Ok(count) if count == SEARCH_BACKFILL_PAGE_SIZE => {
            tracing::warn!(%project_id, ?signal, count, "Search-index backfill drift remains");
        }
        Ok(count) if count > 0 => {
            tracing::info!(
                %project_id,
                ?signal,
                count,
                "Search-index backfill reached the current project tail"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                %error,
                %project_id,
                ?signal,
                "Search-index backfill page failed; complete markers were not advanced"
            );
        }
    }
}
