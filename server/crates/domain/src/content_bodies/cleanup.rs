use std::collections::HashSet;

use sideseat_ports::types::{ContentBodyObject, SpanBodyAssociation};

use crate::storage_governance::StorageGovernanceService;

use super::{
    ContentBodyError, ContentBodyRestoreCleanupReport, ContentBodyService,
    DELETION_CLAIM_STALE_SECS, ORPHAN_SWEEP_LIMIT,
};

impl ContentBodyService {
    /// Drain every body orphan and abandoned deletion claim while the restore marker excludes writers.
    ///
    /// Unlike the live sweeper this has no grace period and no one-page bound: independently restored
    /// metadata can be arbitrarily older or newer than analytics, so repair must reach a fixed point before
    /// reads resume.
    pub async fn cleanup_orphans_after_restore(
        &self,
    ) -> Result<ContentBodyRestoreCleanupReport, ContentBodyError> {
        let mut report = ContentBodyRestoreCleanupReport::default();
        let all_timestamps = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(i64::MAX);

        loop {
            let claims = self
                .database
                .get_stale_claimed_content_bodies(all_timestamps, ORPHAN_SWEEP_LIMIT)
                .await?;
            if claims.is_empty() {
                break;
            }
            for (project_id, hash) in claims {
                self.try_delete_claimed_orphan(&project_id, &hash).await?;
                report.stale_claims_finalized += 1;
            }
        }

        loop {
            let orphans = self
                .database
                .get_orphan_content_bodies(all_timestamps, ORPHAN_SWEEP_LIMIT)
                .await?;
            if orphans.is_empty() {
                break;
            }
            for (project_id, hash) in orphans {
                if self.try_delete_one_orphan(&project_id, &hash).await? {
                    report.orphans_deleted += 1;
                }
            }
        }

        Ok(report)
    }

    pub(super) async fn sweep_orphans(&self, now: chrono::DateTime<chrono::Utc>) {
        let stale_before = now - chrono::Duration::seconds(DELETION_CLAIM_STALE_SECS);
        match self
            .database
            .get_stale_claimed_content_bodies(stale_before, ORPHAN_SWEEP_LIMIT)
            .await
        {
            Ok(claims) => {
                for (project_id, hash) in claims {
                    self.delete_claimed_orphan(&project_id, &hash).await;
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "Could not list stale content-body deletion claims"
            ),
        }

        match self
            .database
            .get_orphan_content_bodies(now - chrono::Duration::minutes(5), ORPHAN_SWEEP_LIMIT)
            .await
        {
            Ok(orphans) => {
                for (project_id, hash) in orphans {
                    self.delete_one_orphan(&project_id, &hash).await;
                }
            }
            Err(error) => tracing::warn!(%error, "Could not list orphaned content bodies"),
        }
    }

    pub(super) async fn reset_backfill_for_associations(
        &self,
        associations: &[SpanBodyAssociation],
    ) {
        let projects = associations
            .iter()
            .map(|association| association.project_id.clone())
            .collect::<HashSet<_>>();
        for project_id in projects {
            if let Err(error) = self.database.reset_content_body_backfill(&project_id).await {
                tracing::warn!(
                    %error,
                    %project_id,
                    "Could not reset body-backfill progress after confirmation failure"
                );
            }
        }
    }

    pub(super) async fn reconcile_projects(
        &self,
        governance: &StorageGovernanceService,
        objects: &[ContentBodyObject],
    ) {
        let projects = objects
            .iter()
            .map(|object| object.project_id.clone())
            .collect::<HashSet<_>>();
        for project_id in projects {
            if let Err(error) = governance.reconcile_project(&project_id).await {
                tracing::warn!(
                    %error,
                    %project_id,
                    "Could not reconcile quota after rolling back body registration"
                );
            }
        }
    }
}
