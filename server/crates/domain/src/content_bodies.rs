//! The retired content-body store, drained.
//!
//! Earlier versions dual-wrote every span's `messages`, `tool_definitions`, `tool_names` and `raw_span` as
//! content-addressed objects beside the analytics columns that already held them, with a transactional
//! ownership registry (`content_bodies`, `span_bodies`). The analytics columns are the sole authority now:
//! nothing writes a body and nothing reads one. Measured on the fixture corpus the copies were 81 MB of the
//! 215 MB stored for traces, and staging them cost about a quarter of a request's write path.
//!
//! What remains is the drain, on the existing deletion protocol so it is crash-safe at every step:
//!
//! 1. associations are deleted page by page - no answer depends on them;
//! 2. each object that is now unreferenced is **claimed** (`deleting_at`), its bytes are deleted, and then
//!    its row; a crash after the bytes went leaves a claimed row that the stale-claim pass finishes, and a
//!    crash before leaves an ordinary orphan.
//!
//! A mixed-version cluster stays correct while it drains: an older instance that still stages a body and
//! loses it here falls back to the inline column it wrote in the same request, which is what it already did
//! for any unavailable body.

use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;

use crate::files::FileService;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::error::DataError;
use sideseat_ports::traits::TransactionalRepository;
use sideseat_ports::types::ProjectId;

/// Rows or objects handled per statement or listing.
const DRAIN_PAGE: usize = 256;
/// Claims older than this belong to a worker that crashed.
const DELETION_CLAIM_STALE_SECS: i64 = 300;
/// Live passes leave a recently released object alone for this long, so a rolling upgrade's older instance
/// re-referencing it does not race a deletion it would only have to recover from.
const LIVE_ORPHAN_GRACE_SECS: i64 = 300;

#[derive(Debug, Error)]
pub enum ContentBodyError {
    #[error(transparent)]
    Database(#[from] DataError),
    #[error(transparent)]
    Storage(#[from] FileStorageError),
    #[error(
        "content-body deletion claim could not be finalized for project {project_id}, hash {body_hash}"
    )]
    DeletionFinalization {
        project_id: ProjectId,
        body_hash: String,
    },
}

/// What one drain pass removed.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ContentBodyRestoreCleanupReport {
    pub associations_retired: u64,
    pub stale_claims_finalized: u64,
    pub orphans_deleted: u64,
}

impl ContentBodyRestoreCleanupReport {
    pub fn is_empty(&self) -> bool {
        self.associations_retired == 0
            && self.stale_claims_finalized == 0
            && self.orphans_deleted == 0
    }
}

/// Empties the retired body registry and its objects.
#[derive(Clone)]
pub struct ContentBodyService {
    storage: Arc<dyn FileStorage>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
}

impl ContentBodyService {
    pub fn from_file_service(files: &FileService) -> Self {
        Self {
            storage: Arc::clone(files.storage()),
            database: Arc::clone(files.database()),
        }
    }

    /// One bounded pass for the background task: a page of associations, stale claims, and orphans past the
    /// grace period. Returns the projects whose stored bytes changed, for quota reconciliation.
    pub async fn drain_pass(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(ContentBodyRestoreCleanupReport, HashSet<ProjectId>), ContentBodyError> {
        let mut report = ContentBodyRestoreCleanupReport {
            associations_retired: self
                .database
                .retire_span_body_associations(DRAIN_PAGE)
                .await?,
            ..ContentBodyRestoreCleanupReport::default()
        };
        let mut touched = HashSet::new();
        let stale_before = now - chrono::Duration::seconds(DELETION_CLAIM_STALE_SECS);
        for (project_id, hash) in self
            .database
            .get_stale_claimed_content_bodies(stale_before, DRAIN_PAGE)
            .await?
        {
            self.delete_claimed(&project_id, &hash).await?;
            report.stale_claims_finalized += 1;
            touched.insert(project_id);
        }
        let orphan_before = now - chrono::Duration::seconds(LIVE_ORPHAN_GRACE_SECS);
        for (project_id, hash) in self
            .database
            .get_orphan_content_bodies(orphan_before, DRAIN_PAGE)
            .await?
        {
            if self.claim_and_delete(&project_id, &hash).await? {
                report.orphans_deleted += 1;
                touched.insert(project_id);
            }
        }
        Ok((report, touched))
    }

    /// Drain everything, with no grace period and no page bound, while the restore marker excludes writers.
    ///
    /// Independently restored metadata can be arbitrarily older or newer than analytics, so repair reaches a
    /// fixed point before reads resume.
    pub async fn cleanup_orphans_after_restore(
        &self,
    ) -> Result<ContentBodyRestoreCleanupReport, ContentBodyError> {
        let mut report = ContentBodyRestoreCleanupReport::default();
        loop {
            let retired = self
                .database
                .retire_span_body_associations(DRAIN_PAGE)
                .await?;
            if retired == 0 {
                break;
            }
            report.associations_retired += retired;
        }
        let every_timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(i64::MAX);
        loop {
            let claims = self
                .database
                .get_stale_claimed_content_bodies(every_timestamp, DRAIN_PAGE)
                .await?;
            if claims.is_empty() {
                break;
            }
            for (project_id, hash) in claims {
                self.delete_claimed(&project_id, &hash).await?;
                report.stale_claims_finalized += 1;
            }
        }
        loop {
            let orphans = self
                .database
                .get_orphan_content_bodies(every_timestamp, DRAIN_PAGE)
                .await?;
            if orphans.is_empty() {
                break;
            }
            for (project_id, hash) in orphans {
                if self.claim_and_delete(&project_id, &hash).await? {
                    report.orphans_deleted += 1;
                }
            }
        }
        Ok(report)
    }

    async fn claim_and_delete(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<bool, ContentBodyError> {
        if !self
            .database
            .claim_content_body_for_deletion(project_id, hash)
            .await?
        {
            return Ok(false);
        }
        self.delete_claimed(project_id, hash).await?;
        Ok(true)
    }

    async fn delete_claimed(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<(), ContentBodyError> {
        if let Err(error) = self.storage.delete(project_id, hash).await {
            self.database
                .release_content_body_deletion_claim(project_id, hash)
                .await?;
            return Err(ContentBodyError::Storage(error));
        }
        if self
            .database
            .delete_claimed_content_body(project_id, hash)
            .await?
        {
            return Ok(());
        }
        self.database
            .release_content_body_deletion_claim(project_id, hash)
            .await?;
        Err(ContentBodyError::DeletionFinalization {
            project_id: project_id.clone(),
            body_hash: hash.to_owned(),
        })
    }
}
