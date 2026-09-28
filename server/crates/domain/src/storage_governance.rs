//! Per-project storage admission and project-wide legal holds.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{
    AnalyticsRepository, DeletionCause, DeletionRecord, DeletionScope, StorageGovernance,
    TransactionalRepository, retention_cleanup_logical_bytes,
};
use sideseat_ports::types::{
    NormalizedLog, NormalizedMetric, NormalizedSpan, ProjectHold, ProjectId, ProjectStorageUsage,
};

const MAINTENANCE_LEASE_SECS: i64 = 2 * 60 * 60;
const MAINTENANCE_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAINTENANCE_WAIT_ATTEMPTS: usize = 400;
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
// One full pressure batch can need both a permanent journal row and a cleanup candidate before any data is
// removed. Their payload is ids and instants, so reserve against the bounded batch rather than against an
// arbitrary percentage of current usage.
const MAX_PREDELETE_RECORDS: u64 = 100_000;
const JOURNAL_LOGICAL_BYTES_PER_RECORD: u64 = 192;
const CLEANUP_CANDIDATE_BYTES_PER_RECORD: u64 = 96;
const REQUIRED_MAINTENANCE_RESERVE: u64 =
    MAX_PREDELETE_RECORDS * (JOURNAL_LOGICAL_BYTES_PER_RECORD + CLEANUP_CANDIDATE_BYTES_PER_RECORD);

#[derive(Debug, Error)]
pub enum GovernanceError {
    #[error(
        "project storage quota exceeded: requested {requested_bytes} bytes, measured {measured_bytes} bytes, ordinary-write limit {ordinary_limit_bytes} bytes"
    )]
    QuotaExceeded {
        requested_bytes: u64,
        measured_bytes: u64,
        ordinary_limit_bytes: u64,
    },
    #[error("storage governance unavailable: {0}")]
    Unavailable(String),
    #[error("project maintenance fence remained busy")]
    Busy,
    #[error(
        "configured quota {quota_bytes} bytes is below held storage {held_bytes} bytes for project {project_id}"
    )]
    HeldBytesExceedQuota {
        project_id: ProjectId,
        held_bytes: u64,
        quota_bytes: u64,
    },
    #[error(
        "maintenance reserve exhausted: requested {requested_bytes} bytes, measured {measured_bytes} bytes, quota {quota_bytes} bytes"
    )]
    MaintenanceReserveExhausted {
        requested_bytes: u64,
        measured_bytes: u64,
        quota_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreQuotaRepairReport {
    pub project_id: ProjectId,
    pub usage_before_bytes: u64,
    pub usage_after_bytes: u64,
    pub ordinary_limit_bytes: u64,
    pub reclaimed_span_bytes: u64,
    pub remaining_overage_bytes: u64,
    pub blocked_by_project_hold: bool,
}

/// One inward-facing service for quota admission, hold fencing and convergence.
pub struct StorageGovernanceService {
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    governance: Arc<dyn StorageGovernance + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    clock: Arc<dyn Clock>,
    quota_bytes: u64,
    maintenance_reserve_bytes: u64,
}

impl StorageGovernanceService {
    pub fn new(
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        governance: Arc<dyn StorageGovernance + Send + Sync>,
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        clock: Arc<dyn Clock>,
        quota_bytes: u64,
    ) -> Self {
        let maintenance_reserve_bytes = REQUIRED_MAINTENANCE_RESERVE.min(quota_bytes);
        Self {
            database,
            governance,
            analytics,
            clock,
            quota_bytes,
            maintenance_reserve_bytes,
        }
    }

    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    pub fn maintenance_reserve_bytes(&self) -> u64 {
        self.maintenance_reserve_bytes
    }

    pub fn ordinary_limit_bytes(&self) -> u64 {
        self.quota_bytes
            .saturating_sub(self.maintenance_reserve_bytes)
    }

    /// Refuse before queueing. A stale high counter gets one synchronous reconciliation and retry.
    pub async fn admit(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
    ) -> Result<ProjectStorageUsage, GovernanceError> {
        let now = self.clock.now();
        let limit = self.ordinary_limit_bytes();
        if let Some(usage) = self
            .governance
            .reserve_project_storage(project_id, additional_bytes, limit, now)
            .await
            .map_err(unavailable)?
        {
            return Ok(usage);
        }

        let mut measured = self.reconcile_project(project_id).await?;
        let needed = measured
            .logical_bytes
            .saturating_add(additional_bytes)
            .saturating_sub(limit);
        if needed > 0 {
            let owner = format!("quota-pressure:{}", Uuid::new_v4());
            if self.try_acquire(project_id, &owner).await? {
                let reclaim_result = async {
                    // A project-wide hold protects all of its current data. Row-level deadlines are still
                    // checked by the adapter so an expired registry row cannot accidentally pin data forever.
                    if self.current_hold(project_id).await?.is_none() {
                        let reclaimed = self
                            .reclaim_pressure_spans_under_fence(project_id, needed)
                            .await?;
                        tracing::debug!(
                            project_id = %project_id,
                            requested_bytes = needed,
                            reclaimed_bytes = reclaimed,
                            "Storage-pressure reclamation completed"
                        );
                    }
                    Ok::<(), GovernanceError>(())
                }
                .await;
                self.release(project_id, &owner).await;
                reclaim_result?;
                measured = self.reconcile_project(project_id).await?;
            }
        }
        match self
            .governance
            .reserve_project_storage(project_id, additional_bytes, limit, self.clock.now())
            .await
            .map_err(unavailable)?
        {
            Some(usage) => Ok(usage),
            None => Err(GovernanceError::QuotaExceeded {
                requested_bytes: additional_bytes,
                measured_bytes: measured.logical_bytes,
                ordinary_limit_bytes: limit,
            }),
        }
    }

    pub async fn current_hold(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<ProjectHold>, GovernanceError> {
        self.governance
            .active_project_hold(project_id, self.clock.now())
            .await
            .map_err(unavailable)
    }

    async fn reclaim_pressure_spans_under_fence(
        &self,
        project_id: &ProjectId,
        target_bytes: u64,
    ) -> Result<u64, GovernanceError> {
        let candidates = self
            .analytics
            .oldest_reclaimable_spans(
                project_id,
                target_bytes,
                self.clock.now(),
                MAX_PREDELETE_RECORDS as usize,
            )
            .await
            .map_err(unavailable)?;
        if candidates.is_empty() {
            return Ok(0);
        }

        let journal_bytes = candidates.iter().fold(0u64, |total, candidate| {
            total.saturating_add(DeletionRecord::logical_bytes_for(
                project_id.as_str(),
                DeletionCause::Pressure,
                DeletionScope::Span,
                &candidate.trace_id,
                Some(&candidate.span_id),
            ))
        });
        let mut trace_ids = candidates
            .iter()
            .map(|candidate| candidate.trace_id.as_str())
            .collect::<Vec<_>>();
        trace_ids.sort_unstable();
        trace_ids.dedup();
        let cleanup_bytes = trace_ids.iter().fold(0u64, |total, trace_id| {
            total.saturating_add(retention_cleanup_logical_bytes(
                project_id.as_str(),
                trace_id,
            ))
        });
        self.reserve_maintenance_under_fence(
            project_id,
            journal_bytes.saturating_add(cleanup_bytes),
        )
        .await?;

        let span_keys = candidates
            .iter()
            .map(|candidate| (candidate.trace_id.clone(), candidate.span_id.clone()))
            .collect::<Vec<_>>();
        self.database
            .record_pressure_eviction(project_id, &span_keys)
            .await
            .map_err(unavailable)?;
        self.analytics
            .delete_spans(project_id, &span_keys)
            .await
            .map_err(unavailable)?;
        Ok(candidates.iter().fold(0u64, |total, candidate| {
            total.saturating_add(candidate.logical_bytes)
        }))
    }

    /// Reserve quota-counted housekeeping bytes while the caller holds the project maintenance fence.
    ///
    /// Unlike ordinary admission this may consume the maintenance reserve, but never exceed the declared quota.
    /// The caller must do this before committing a permanent journal row or cleanup candidate.
    pub async fn reserve_maintenance_under_fence(
        &self,
        project_id: &ProjectId,
        additional_bytes: u64,
    ) -> Result<ProjectStorageUsage, GovernanceError> {
        if let Some(usage) = self
            .governance
            .reserve_project_storage(
                project_id,
                additional_bytes,
                self.quota_bytes,
                self.clock.now(),
            )
            .await
            .map_err(unavailable)?
        {
            return Ok(usage);
        }

        let measured = self.reconcile_project(project_id).await?;
        match self
            .governance
            .reserve_project_storage(
                project_id,
                additional_bytes,
                self.quota_bytes,
                self.clock.now(),
            )
            .await
            .map_err(unavailable)?
        {
            Some(usage) => Ok(usage),
            None => Err(GovernanceError::MaintenanceReserveExhausted {
                requested_bytes: additional_bytes,
                measured_bytes: measured.logical_bytes,
                quota_bytes: self.quota_bytes,
            }),
        }
    }

    pub async fn stamp_spans(&self, spans: &mut [NormalizedSpan]) -> Result<(), GovernanceError> {
        let holds = self
            .holds_for_projects(
                spans
                    .iter()
                    .map(|row| row.project_id.as_deref().unwrap_or("default")),
            )
            .await?;
        for span in spans {
            span.hold_until = holds
                .get(span.project_id.as_deref().unwrap_or("default"))
                .copied()
                .flatten();
        }
        Ok(())
    }

    pub async fn stamp_metrics(
        &self,
        metrics: &mut [NormalizedMetric],
    ) -> Result<(), GovernanceError> {
        let holds = self
            .holds_for_projects(
                metrics
                    .iter()
                    .map(|row| row.project_id.as_deref().unwrap_or("default")),
            )
            .await?;
        for metric in metrics {
            metric.hold_until = holds
                .get(metric.project_id.as_deref().unwrap_or("default"))
                .copied()
                .flatten();
        }
        Ok(())
    }

    pub async fn stamp_logs(&self, logs: &mut [NormalizedLog]) -> Result<(), GovernanceError> {
        let holds = self
            .holds_for_projects(
                logs.iter()
                    .map(|row| row.project_id.as_deref().unwrap_or("default")),
            )
            .await?;
        for log in logs {
            log.hold_until = holds
                .get(log.project_id.as_deref().unwrap_or("default"))
                .copied()
                .flatten();
        }
        Ok(())
    }

    /// Close the writer-admitted-before-hold window after an analytics commit.
    pub async fn patch_after_write(&self, projects: &[ProjectId]) -> Result<(), GovernanceError> {
        let mut projects = projects.to_vec();
        projects.sort();
        projects.dedup();
        for project_id in projects {
            if let Some(hold) = self.current_hold(&project_id).await? {
                self.analytics
                    .patch_project_hold(&project_id, hold.hold_until)
                    .await
                    .map_err(unavailable)?;
            }
        }
        Ok(())
    }

    /// Record first, patch while sharing the retention fence, then re-check once before release.
    pub async fn set_hold(
        &self,
        project_id: &ProjectId,
        hold_until: DateTime<Utc>,
    ) -> Result<ProjectHold, GovernanceError> {
        if !self
            .database
            .project_accepts_writes(project_id.as_str())
            .await
            .map_err(unavailable)?
        {
            return Err(GovernanceError::Unavailable(
                "project is unknown or is being deleted".to_string(),
            ));
        }
        if hold_until <= self.clock.now() {
            return Err(GovernanceError::Unavailable(
                "hold_until must be in the future".to_string(),
            ));
        }
        let owner = format!("hold:{}", Uuid::new_v4());
        self.acquire(project_id, &owner).await?;
        let result = async {
            let hold = self
                .governance
                .set_project_hold(project_id, hold_until, self.clock.now())
                .await
                .map_err(unavailable)?;
            self.analytics
                .patch_project_hold(project_id, hold_until)
                .await
                .map_err(unavailable)?;
            // A writer admitted before the record may have committed while the first mutation ran.
            self.analytics
                .patch_project_hold(project_id, hold_until)
                .await
                .map_err(unavailable)?;
            Ok(hold)
        }
        .await;
        self.release(project_id, &owner).await;
        result
    }

    pub async fn clear_hold(&self, project_id: &ProjectId) -> Result<bool, GovernanceError> {
        // Existing rows retain their deadline. That is the safe failure direction for a writer admitted before
        // release; the hold naturally expires at the deadline even if that writer lands after this call.
        let owner = format!("clear-hold:{}", Uuid::new_v4());
        self.acquire(project_id, &owner).await?;
        let result = self
            .governance
            .clear_project_hold(project_id)
            .await
            .map_err(unavailable);
        self.release(project_id, &owner).await;
        result
    }

    pub async fn acquire_maintenance(
        &self,
        project_id: &ProjectId,
        purpose: &str,
    ) -> Result<String, GovernanceError> {
        let owner = format!("{purpose}:{}", Uuid::new_v4());
        self.acquire(project_id, &owner).await?;
        Ok(owner)
    }

    pub async fn release_maintenance(&self, project_id: &ProjectId, owner: &str) {
        self.release(project_id, owner).await;
    }

    pub async fn reconcile_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectStorageUsage, GovernanceError> {
        let analytics = self
            .analytics
            .project_logical_bytes(project_id)
            .await
            .map_err(unavailable)?;
        let transactional = self
            .governance
            .held_transactional_bytes(project_id)
            .await
            .map_err(unavailable)?;
        let files = self
            .database
            .get_project_storage_bytes(project_id)
            .await
            .map_err(unavailable)?;
        let logical_bytes = analytics
            .saturating_add(transactional)
            .saturating_add(u64::try_from(files).unwrap_or(0));
        self.governance
            .replace_project_storage_usage(project_id, logical_bytes, self.clock.now())
            .await
            .map_err(unavailable)
    }

    /// Recompute and reclaim one restored project's ordinary quota to a fixed point.
    ///
    /// Stale counters are replaced before any decision. Pressure deletion remains journalled, and legal holds
    /// win: an overage composed only of held or permanent transactional bytes is reported rather than deleted.
    pub async fn repair_quota_after_restore(
        &self,
        project_id: &ProjectId,
    ) -> Result<RestoreQuotaRepairReport, GovernanceError> {
        let owner = self
            .acquire_maintenance(project_id, "restore-quota")
            .await?;
        let result = async {
            let before = self.reconcile_project(project_id).await?;
            let limit = self.ordinary_limit_bytes();
            let blocked_by_project_hold = self.current_hold(project_id).await?.is_some();
            let mut after = before;
            let mut reclaimed_span_bytes = 0u64;

            if !blocked_by_project_hold {
                loop {
                    let needed = after.logical_bytes.saturating_sub(limit);
                    if needed == 0 {
                        break;
                    }
                    let reclaimed = self
                        .reclaim_pressure_spans_under_fence(project_id, needed)
                        .await?;
                    if reclaimed == 0 {
                        break;
                    }
                    reclaimed_span_bytes = reclaimed_span_bytes.saturating_add(reclaimed);
                    let next = self.reconcile_project(project_id).await?;
                    if next.logical_bytes >= after.logical_bytes {
                        after = next;
                        break;
                    }
                    after = next;
                }
            }

            Ok(RestoreQuotaRepairReport {
                project_id: project_id.clone(),
                usage_before_bytes: before.logical_bytes,
                usage_after_bytes: after.logical_bytes,
                ordinary_limit_bytes: limit,
                reclaimed_span_bytes,
                remaining_overage_bytes: after.logical_bytes.saturating_sub(limit),
                blocked_by_project_hold,
            })
        }
        .await;
        self.release_maintenance(project_id, &owner).await;
        result
    }

    pub async fn repair_all_quotas_after_restore(
        &self,
    ) -> Result<Vec<RestoreQuotaRepairReport>, GovernanceError> {
        let projects = self
            .governance
            .storage_project_ids(usize::MAX)
            .await
            .map_err(unavailable)?;
        let mut reports = Vec::with_capacity(projects.len());
        for project_id in projects {
            reports.push(self.repair_quota_after_restore(&project_id).await?);
        }
        Ok(reports)
    }

    pub async fn held_bytes(&self, project_id: &ProjectId) -> Result<u64, GovernanceError> {
        let transactional = self
            .governance
            .held_transactional_bytes(project_id)
            .await
            .map_err(unavailable)?;
        let analytics = self
            .analytics
            .project_held_logical_bytes(project_id, self.clock.now())
            .await
            .map_err(unavailable)?;
        let files = if self.current_hold(project_id).await?.is_some() {
            u64::try_from(
                self.database
                    .get_project_storage_bytes(project_id)
                    .await
                    .map_err(unavailable)?,
            )
            .unwrap_or(0)
        } else {
            0
        };
        Ok(transactional
            .saturating_add(analytics)
            .saturating_add(files))
    }

    pub async fn validate_startup(&self) -> Result<(), GovernanceError> {
        for project_id in self
            .governance
            .storage_project_ids(usize::MAX)
            .await
            .map_err(unavailable)?
        {
            let held_bytes = self.held_bytes(&project_id).await?;
            if held_bytes > self.quota_bytes {
                return Err(GovernanceError::HeldBytesExceedQuota {
                    project_id,
                    held_bytes,
                    quota_bytes: self.quota_bytes,
                });
            }
        }
        Ok(())
    }

    pub fn start(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SWEEP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        self.sweep_once().await;
                    }
                }
            }
        })
    }

    async fn sweep_once(&self) {
        let now = self.clock.now();
        match self
            .governance
            .list_active_project_holds(now, usize::MAX)
            .await
        {
            Ok(holds) => {
                for hold in holds {
                    let owner = format!("hold-sweep:{}", Uuid::new_v4());
                    match self.try_acquire(&hold.project_id, &owner).await {
                        Ok(true) => {
                            if let Err(error) = self
                                .analytics
                                .patch_project_hold(&hold.project_id, hold.hold_until)
                                .await
                            {
                                tracing::warn!(
                                    project_id = %hold.project_id,
                                    %error,
                                    "Legal-hold convergence patch failed"
                                );
                            }
                            self.release(&hold.project_id, &owner).await;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(%error, "Could not acquire legal-hold sweep lease")
                        }
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "Could not list legal holds for convergence"),
        }

        match self.governance.storage_project_ids(usize::MAX).await {
            Ok(projects) => {
                for project_id in projects {
                    if let Err(error) = self.reconcile_project(&project_id).await {
                        tracing::warn!(
                            project_id = %project_id,
                            %error,
                            "Storage usage reconciliation failed"
                        );
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Could not list projects for storage reconciliation")
            }
        }
    }

    async fn holds_for_projects<'a>(
        &self,
        projects: impl Iterator<Item = &'a str>,
    ) -> Result<HashMap<String, Option<DateTime<Utc>>>, GovernanceError> {
        let mut projects = projects.map(str::to_string).collect::<Vec<_>>();
        projects.sort();
        projects.dedup();
        let mut holds = HashMap::with_capacity(projects.len());
        for project in projects {
            let project_id = ProjectId::from(project.as_str());
            let hold = self
                .current_hold(&project_id)
                .await?
                .map(|hold| hold.hold_until);
            holds.insert(project, hold);
        }
        Ok(holds)
    }

    async fn acquire(&self, project_id: &ProjectId, owner: &str) -> Result<(), GovernanceError> {
        for _ in 0..MAINTENANCE_WAIT_ATTEMPTS {
            if self.try_acquire(project_id, owner).await? {
                return Ok(());
            }
            tokio::time::sleep(MAINTENANCE_RETRY_DELAY).await;
        }
        Err(GovernanceError::Busy)
    }

    async fn try_acquire(
        &self,
        project_id: &ProjectId,
        owner: &str,
    ) -> Result<bool, GovernanceError> {
        let now = self.clock.now();
        self.governance
            .acquire_project_maintenance(
                project_id,
                owner,
                now,
                now + TimeDelta::seconds(MAINTENANCE_LEASE_SECS),
            )
            .await
            .map_err(unavailable)
    }

    async fn release(&self, project_id: &ProjectId, owner: &str) {
        if let Err(error) = self
            .governance
            .release_project_maintenance(project_id, owner)
            .await
        {
            tracing::warn!(project_id = %project_id, %error, "Could not release maintenance lease");
        }
    }
}

fn unavailable(error: impl std::fmt::Display) -> GovernanceError {
    GovernanceError::Unavailable(error.to_string())
}
#[cfg(test)]
#[path = "storage_governance_tests.rs"]
mod tests;
