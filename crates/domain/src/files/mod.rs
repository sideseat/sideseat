//! File storage layer
//!
//! Provides binary file storage with deduplication for the application.
//! Files are stored outside DuckDB with hash-based content addressing.
//!
//! ## Architecture
//!
//! - `storage` - Trait definition for file storage backends
//! - `filesystem` - Local filesystem implementation
//! - `error` - Error types for file operations
//!
//! ## Storage Layout
//!
//! Files are organized per-project with sharded directories:
//! ```text
//! {base_path}/
//! └── {project_id}/
//!     └── {hash[0:2]}/
//!         └── {hash[2:4]}/
//!             └── {hash}
//! ```
//!
//! ## Usage
//!
//! ```text
//! let file_service = create_file_service(config, storage, database, cache).await?;
//!
//! // Get file content
//! let content = file_service.get_file(project_id, hash).await?;
//!
//! // Cleanup after trace deletion
//! file_service.cleanup_traces(project_id, &trace_ids).await?;
//! ```

pub mod cleanup;
pub mod error;

use sideseat_ports::blobs::{FileContent, FileStorage, FileStorageError};
use sideseat_ports::cache::{CacheStore, TypedCache};
use sideseat_ports::traits::TransactionalRepository;
use sideseat_ports::types::ProjectId;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::traces::extract::files::collect_file_references_in_str;
use sideseat_core::core::config::FilesConfig;
use sideseat_core::core::constants::CACHE_TTL_FILE_QUOTA;
use sideseat_core::utils::file_uri::parse_file_uri;
use sideseat_ports::cache::CacheKey;

pub use error::FileServiceError;

/// File metadata without content
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// File size in bytes
    pub size_bytes: i64,
    /// MIME type (e.g., "image/png")
    pub media_type: Option<String>,
}

/// Main file service coordinating storage, metadata, and cleanup
pub struct FileService {
    /// Storage backend (filesystem or S3)
    storage: Arc<dyn FileStorage>,
    /// Transactional database for metadata operations
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    /// Configuration
    config: FilesConfig,
    /// Path to temp directory
    temp_dir: PathBuf,
    /// Shared cache service (Redis in SaaS, in-memory in local)
    cache: Arc<dyn CacheStore>,
    /// Shared project fence for legal hold and destructive maintenance.
    governance: Option<Arc<crate::storage_governance::StorageGovernanceService>>,
}

impl FileService {
    /// Create a file service from already selected ports.
    pub async fn new(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
    ) -> Result<Self, FileServiceError> {
        Self::new_inner(config, temp_dir, storage, database, cache, None).await
    }

    pub async fn new_governed(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
        governance: Arc<crate::storage_governance::StorageGovernanceService>,
    ) -> Result<Self, FileServiceError> {
        Self::new_inner(config, temp_dir, storage, database, cache, Some(governance)).await
    }

    async fn new_inner(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
        governance: Option<Arc<crate::storage_governance::StorageGovernanceService>>,
    ) -> Result<Self, FileServiceError> {
        tracing::debug!(
            enabled = config.enabled,
            storage = %config.storage,
            quota_bytes = config.quota_bytes,
            "File service initialized"
        );

        let service = Self {
            storage,
            database,
            config,
            temp_dir,
            cache,
            governance,
        };

        // Run startup cleanup for orphan temp files
        if service.config.enabled
            && let Err(e) = cleanup::cleanup_orphan_temp_files(
                &service.temp_dir,
                &service.storage,
                &service.database,
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to cleanup orphan temp files on startup");
        }

        // Run startup cleanup for files with ref_count=0 (failed storage deletions)
        if service.config.enabled
            && let Err(e) = cleanup::cleanup_zero_ref_files_governed(
                &service.storage,
                &service.database,
                sideseat_core::core::constants::FILE_DELETION_CLAIM_STALE_SECS,
                service.governance.as_ref(),
            )
            .await
        {
            tracing::warn!(error = %e, "Failed to cleanup zero-ref files on startup");
        }

        Ok(service)
    }

    /// Check if file storage is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the temp directory path for writing temp files
    pub fn temp_dir(&self) -> &PathBuf {
        &self.temp_dir
    }

    pub fn governance(&self) -> Option<&Arc<crate::storage_governance::StorageGovernanceService>> {
        self.governance.as_ref()
    }

    /// Get file content
    pub async fn get_file(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<FileContent, FileServiceError> {
        if !self.config.enabled {
            return Err(FileServiceError::Disabled);
        }

        // Get metadata for media_type via repository trait
        let repo = self.database.as_ref();
        let media_type = repo
            .get_file(project_id, hash)
            .await?
            .and_then(|f| f.media_type);

        // Get data from storage
        let data = self
            .storage
            .get(project_id, hash)
            .await
            .map_err(|e| match e {
                FileStorageError::NotFound { .. } => FileServiceError::NotFound {
                    project_id: project_id.to_string(),
                    hash: hash.to_string(),
                },
                e => FileServiceError::Storage(e),
            })?;

        Ok(FileContent { data, media_type })
    }

    /// Check if a file exists
    pub async fn file_exists(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<bool, FileServiceError> {
        if !self.config.enabled {
            return Ok(false);
        }

        Ok(self.storage.exists(project_id, hash).await?)
    }

    /// Get file metadata without loading content
    ///
    /// Returns size and media_type from database metadata.
    pub async fn get_file_metadata(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<FileMetadata, FileServiceError> {
        if !self.config.enabled {
            return Err(FileServiceError::Disabled);
        }

        // Get metadata via repository trait
        let repo = self.database.as_ref();
        let file_row =
            repo.get_file(project_id, hash)
                .await?
                .ok_or_else(|| FileServiceError::NotFound {
                    project_id: project_id.to_string(),
                    hash: hash.to_string(),
                })?;

        // Verify file exists in storage
        if !self.storage.exists(project_id, hash).await? {
            return Err(FileServiceError::NotFound {
                project_id: project_id.to_string(),
                hash: hash.to_string(),
            });
        }

        Ok(FileMetadata {
            size_bytes: file_row.size_bytes,
            media_type: file_row.media_type,
        })
    }

    /// Release the associations of traces whose *expired* spans referenced them, keeping the survivors'.
    ///
    /// The counterpart to [`Self::cleanup_traces`], and the distinction is the defect this exists to fix.
    /// Retention expires individual span identities, not whole traces - but it handed every affected
    /// `trace_id` to the trace-wide cleanup, which removes **all** of a trace's associations. So when one old
    /// span of a busy trace expired, the surviving spans of that trace were left pointing at bytes that had
    /// been reclaimed: the dangling reference the write-files-before-rows ordering exists to prevent,
    /// produced by retention instead.
    ///
    /// Gating the trace-wide delete on "the trace is now empty" was considered and is wrong in **both**
    /// directions, which is why this is a reconciliation rather than a condition:
    ///
    /// - a still-active trace would keep every association forever, so a file referenced only by an expired
    ///   span is never reclaimed - a continuously busy trace retains expired bytes indefinitely, and blobs
    ///   are supposed to be reclaimable;
    /// - and "verify empty, then delete by trace" is a read-then-act race: an ingestion can create an
    ///   association in between, and the span then commits holding a reference to bytes just reclaimed.
    ///
    /// So the survivors are asked for directly. `file_reference_fields_for_traces` reads the four fields that
    /// can carry a reference from the **winning** spans that remain, the domain's single scanner turns them
    /// into URIs, and everything else the trace held is released - `pending_writers = 0` only, which is what
    /// protects a batch whose association exists before its span row does.
    pub async fn reconcile_trace_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<(), FileServiceError> {
        if !self.config.enabled || trace_ids.is_empty() {
            return Ok(());
        }

        let repo = self.database.as_ref();
        let mut reconcile: Vec<String> = Vec::new();
        let mut first_error: Option<FileServiceError> = None;

        // Per trace, because `keep` is per trace: a file a survivor of trace A references says nothing about
        // trace B's associations, and unioning the survivor sets across traces would keep B's alive on A's
        // evidence.
        for trace_id in trace_ids {
            match self
                .reconcile_one_trace(project_id, trace_id, analytics, repo)
                .await
            {
                Ok(removed) => reconcile.extend(removed),
                // **Kept going, and the reclaim still runs.** Returning here left the associations this loop
                // had already deleted with a positive stored `ref_count`, and the orphan sweeper selects on
                // *zero* - so an earlier trace's file became permanently unreclaimable because a later trace
                // failed. The failure is reported once, after the reclaim it must not cancel.
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        trace_id,
                        "Could not reconcile this trace; continuing so the traces already released are still \
                         reclaimed"
                    );
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        reconcile.sort_unstable();
        reconcile.dedup();
        let reclaimed = self.reclaim_unreferenced(project_id, reconcile).await;

        match first_error {
            Some(e) => Err(e),
            None => reclaimed,
        }
    }

    /// One trace's release, with the **re-check** that closes the commit-after-scan window.
    ///
    /// The survivor scan is a snapshot, and the release happens after it. In between, an ingestion can
    /// associate a file (`pending_writers = 1`), commit its span, and confirm (`pending_writers = 0`) - so the
    /// release sees a zero counter and a hash the stale snapshot did not contain, and reclaims bytes a
    /// committed span references. `pending_writers` alone does not close this: it is zero at exactly the wrong
    /// moment.
    ///
    /// So the release is followed by a **second scan**, and any hash that has appeared is re-associated before
    /// any byte is deleted. This is the same four-step shape the trace-deletion protocol uses - act, re-check,
    /// compensate - and for the same reason: no transaction spans the analytics and transactional stores, so
    /// the window cannot be removed, only compensated.
    ///
    /// What remains is bounded rather than open: a batch that associates *after* the release holds a row the
    /// release never saw, so nothing released it; and one that associated *before* it had `pending_writers`
    /// above zero and was skipped. The uncovered case is therefore a batch that associates after the release
    /// and commits after the re-check, whose association is intact throughout.
    async fn reconcile_one_trace(
        &self,
        project_id: &ProjectId,
        trace_id: &str,
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
        repo: &(dyn sideseat_ports::traits::TransactionalRepository + Send + Sync),
    ) -> Result<Vec<String>, FileServiceError> {
        let keep = Self::referenced_hashes(project_id, trace_id, analytics).await?;

        let removed = repo
            .release_trace_files_except(project_id, trace_id, &keep)
            .await?;
        if removed.is_empty() {
            return Ok(removed);
        }

        // The re-check. Only the released hashes matter, so this compares against them rather than
        // recomputing a whole set difference.
        //
        // **A failure here must not discard `removed`.** Propagating with `?` dropped the hashes whose
        // associations had *already* been deleted, so their stored `ref_count` was never recomputed - and the
        // orphan sweeper selects on zero, so those files became permanently unreclaimable, with no association
        // left for a retry to rediscover them from. So the failure is reported and the released set is still
        // returned for reconciliation; the conservative direction, since recomputing a count is idempotent.
        let now_referenced = match Self::referenced_hashes(project_id, trace_id, analytics).await {
            Ok(hashes) => hashes,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    project_id = %project_id,
                    trace_id,
                    released = removed.len(),
                    "Could not re-check survivors after releasing their associations. The released hashes are \
                     still reconciled, so nothing leaks - but a span that committed during the release cannot \
                     be compensated on this pass and its file may be reclaimed"
                );
                return Ok(removed);
            }
        };

        let mut kept_after_all = Vec::new();
        let mut released = Vec::new();
        for hash in removed {
            if now_referenced.contains(&hash) {
                kept_after_all.push(hash);
            } else {
                released.push(hash);
            }
        }

        for hash in &kept_after_all {
            // Restored **durable**, before any byte is deleted, so the span that arrived mid-flight keeps its
            // reference. Not `insert_trace_file`: that leaves the row provisional, and a later batch that
            // references the same file and then fails would decrement `pending_writers` to zero, find a
            // non-durable row and delete it - taking the association of a span that committed long before.
            repo.restore_durable_trace_file(project_id, trace_id, hash)
                .await?;
            repo.sync_ref_count(project_id, hash).await?;
            tracing::warn!(
                project_id = %project_id,
                trace_id,
                hash,
                "A span referencing this file committed between the survivor scan and the release; the \
                 association has been restored before any bytes were reclaimed"
            );
        }

        tracing::debug!(
            project_id = %project_id,
            trace_id,
            released = released.len(),
            restored = kept_after_all.len(),
            "Released associations of expired spans, keeping the survivors'"
        );
        Ok(released)
    }

    /// The file hashes the surviving winning spans of one trace reference.
    async fn referenced_hashes(
        project_id: &ProjectId,
        trace_id: &str,
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<Vec<String>, FileServiceError> {
        let fields = analytics
            .file_reference_fields_for_traces(
                project_id,
                std::slice::from_ref(&trace_id.to_string()),
            )
            .await
            .map_err(FileServiceError::from)?;

        let mut uris = Vec::new();
        for field in &fields {
            collect_file_references_in_str(field, &mut uris);
        }
        // The association is keyed by hash, while a reference carries an optional media type - so the hash is
        // what has to be compared, and taking the whole URI would release a file whose reference spells the
        // same hash with a media type.
        let mut hashes: Vec<String> = uris
            .iter()
            .filter_map(|uri| parse_file_uri(uri).map(|parsed| parsed.hash.to_string()))
            .collect();
        hashes.sort_unstable();
        hashes.dedup();
        Ok(hashes)
    }

    /// Cleanup files for deleted traces
    ///
    /// Decrements ref_count for each file associated with the traces.
    /// Deletes files when ref_count reaches 0.
    ///
    /// If storage deletion fails, the database metadata is preserved (with ref_count=0)
    /// so the startup cleanup job can retry later.
    pub async fn cleanup_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<(), FileServiceError> {
        if trace_ids.is_empty() {
            return Ok(());
        }

        crate::content_bodies::ContentBodyService::from_file_service(self)
            .cleanup_traces(project_id, trace_ids)
            .await?;
        if !self.config.enabled {
            return Ok(());
        }

        let repo = self.database.as_ref();

        // How many references these traces hold per file, not just which files.
        //
        // `get_file_hashes_for_traces` returns each hash once, and the loop decremented once per hash -
        // so deleting three traces that all referenced one file removed three associations and one
        // reference, leaving the file permanently unreachable and uncollectable.
        let references = repo
            .get_file_reference_counts_for_traces(project_id, trace_ids)
            .await?;

        // Delete the associations, and take the set to reconcile from *the delete*.
        //
        // The read above is kept for the counts it reports, but it cannot decide which files to reconcile:
        // an association added between the read and the delete is removed here and absent from the read, so
        // its file's stored count would never be recomputed - and the orphan sweeper selects on that count,
        // so nothing would ever reclaim it. Unioning both sets covers the association that arrived late and
        // the one whose row was already gone.
        let removed = repo.delete_trace_files(project_id, trace_ids).await?;
        let mut hashes: Vec<String> = references.into_iter().map(|(hash, _)| hash).collect();
        hashes.extend(removed);
        hashes.sort_unstable();
        hashes.dedup();
        self.reclaim_unreferenced(project_id, hashes).await
    }

    /// Recompute each file's reference count and delete the ones nothing references.
    ///
    /// Shared by [`Self::cleanup_traces`] and [`Self::reconcile_trace_survivors`], because the two differ
    /// only in *which* associations they remove - what happens to a file afterwards is the same question, and
    /// it is the delicate part: the claim, the bytes, then the row, each ordered so the surviving failure is a
    /// leak rather than a row promising content that is gone.
    async fn reclaim_unreferenced(
        &self,
        project_id: &ProjectId,
        hashes: Vec<String>,
    ) -> Result<(), FileServiceError> {
        let repo = self.database.as_ref();

        // Recompute each count from the associations that remain, and delete when none do.
        //
        // Subtracting a previously-read number is not safe against a concurrent cleanup: both would read
        // three, both subtract three, and a file four traces referenced would reach zero and be deleted
        // under the fourth. Recomputing cannot do that - whatever else happened, the count becomes the
        // truth.
        for hash in hashes {
            // The count is kept accurate for display, but it is not what authorises the deletion.
            repo.sync_ref_count(project_id, &hash).await?;

            // The metadata row goes first, and only if nothing references the file *at that instant*.
            //
            // Deciding from a count read earlier races with ingestion and loses: cleanup recomputes
            // zero, a new trace associates, and the delete fires anyway - taking the bytes from under a
            // span that was just committed. With the condition inside the statement, the association
            // makes the delete match nothing and the file stays.
            //
            // Deleting the row before the bytes also means the surviving failure mode is bytes with no
            // metadata - a leak - rather than a row pointing at bytes that are gone, which is what a
            // reader would see as corruption.
            // Claim first, then the bytes, then the row.
            //
            // The claim is what closes the window a conditional delete alone leaves open: with the row
            // deleted, ingestion could recreate it, associate and finalise the bytes, and this loop
            // would then delete content a committed row references. `associate_file` refuses through a
            // claim, so that ingestion fails its batch and retries - by which time the file is either
            // gone, and the retry writes it again with the bytes in hand, or the claim was released.
            if !repo.claim_file_for_deletion(project_id, &hash).await? {
                tracing::debug!(
                    project_id = %project_id,
                    hash,
                    "File is referenced or already being deleted; leaving it"
                );
                continue;
            }

            if let Err(e) = self.storage.delete(project_id, &hash).await {
                // The row is still there, holding the claim, so nothing has been lost - but the claim
                // has to go or the file becomes permanently unassociable.
                if let Err(release_error) = repo.release_deletion_claim(project_id, &hash).await {
                    tracing::error!(
                        project_id = %project_id,
                        hash,
                        error = %e,
                        release_error = %release_error,
                        "Could not delete file bytes and could not release the deletion claim; the \
                         file cannot be referenced again until it is cleared"
                    );
                    continue;
                }
                tracing::warn!(
                    project_id = %project_id,
                    hash,
                    error = %e,
                    "Could not delete file bytes; released the claim and left the file in place"
                );
                continue;
            }

            // Bytes gone, so the row must go too - it is the only thing that could still be found.
            if !repo.delete_file_if_unreferenced(project_id, &hash).await? {
                tracing::error!(
                    project_id = %project_id,
                    hash,
                    "Deleted a claimed file's bytes but its row is now referenced; it will read as \
                     missing content until re-ingested"
                );
                continue;
            }

            tracing::debug!(project_id = %project_id, hash, "Deleted orphaned file");
        }

        self.invalidate_quota_cache(&[project_id]).await;
        if let Some(governance) = &self.governance
            && let Err(error) = governance.reconcile_project(project_id).await
        {
            tracing::warn!(
                project_id = %project_id,
                %error,
                "File cleanup completed but its project quota could not be reconciled"
            );
        }

        Ok(())
    }

    /// Delete all files for a project
    pub async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileServiceError> {
        // Bodies share the physical project namespace with uploaded files, while ownership metadata is
        // separate. Delete both metadata families after the one physical project delete.
        let deleted = self.storage.delete_project(project_id).await?;

        let repo = self.database.as_ref();
        repo.delete_project_files(project_id).await?;
        repo.delete_project_bodies(project_id).await?;

        self.invalidate_quota_cache(&[project_id]).await;

        tracing::debug!(project_id = %project_id, deleted, "Deleted all project files");

        Ok(deleted)
    }

    /// Get storage usage for a project
    pub async fn get_storage_bytes(&self, project_id: &ProjectId) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }

        let repo = self.database.as_ref();
        Ok(repo.get_project_storage_bytes(project_id).await?)
    }

    /// Check if project has quota for additional bytes. Returns true if within quota.
    /// Uses CacheService (Redis/memory) with TTL to avoid hitting the DB on every batch.
    pub async fn check_quota(
        &self,
        project_id: &ProjectId,
        additional_bytes: i64,
    ) -> Result<bool, FileServiceError> {
        if !self.config.enabled {
            return Ok(true);
        }
        let key = CacheKey::file_quota(project_id);
        let current = match self.cache.get::<i64>(&key).await.unwrap_or(None) {
            Some(cached) => cached,
            None => {
                let bytes = self.get_storage_bytes(project_id).await?;
                let _ = self
                    .cache
                    .set(
                        &key,
                        &bytes,
                        Some(Duration::from_secs(CACHE_TTL_FILE_QUOTA)),
                    )
                    .await;
                bytes
            }
        };
        let quota = i64::try_from(self.config.quota_bytes).unwrap_or(i64::MAX);
        Ok((current + additional_bytes) <= quota)
    }

    /// Invalidate cached quota for projects after storage changes (writes or deletes).
    /// Called by persist layer after file writes and by cleanup after deletions.
    pub async fn invalidate_quota_cache(&self, project_ids: &[&str]) {
        for project_id in project_ids {
            self.cache
                .invalidate_key(&CacheKey::file_quota(project_id))
                .await;
        }
    }

    /// Get total file storage used by all projects in an organization
    pub async fn get_org_storage_bytes(&self, org_id: &str) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }
        let repo = self.database.as_ref();
        Ok(repo.get_org_file_storage_bytes(org_id).await?)
    }

    /// Get total file storage used across all orgs a user belongs to
    pub async fn get_user_storage_bytes(&self, user_id: &str) -> Result<i64, FileServiceError> {
        if !self.config.enabled {
            return Ok(0);
        }
        let repo = self.database.as_ref();
        Ok(repo.get_user_file_storage_bytes(user_id).await?)
    }

    /// Get the storage backend
    pub fn storage(&self) -> &Arc<dyn FileStorage> {
        &self.storage
    }

    /// Get the transactional database
    pub fn database(&self) -> &Arc<dyn TransactionalRepository + Send + Sync> {
        &self.database
    }
}

#[async_trait::async_trait]
impl sideseat_ports::blobs::RetentionFileReconciler for FileService {
    fn is_enabled(&self) -> bool {
        FileService::is_enabled(self)
    }

    async fn reconcile_body_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<(), String> {
        crate::content_bodies::ContentBodyService::from_file_service(self)
            .reconcile_trace_survivors(project_id, trace_ids, analytics)
            .await
            .map_err(|error| error.to_string())
    }

    async fn reconcile_trace_survivors(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<(), String> {
        FileService::reconcile_trace_survivors(self, project_id, trace_ids, analytics)
            .await
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_cache::CacheService;
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::core::storage::AppStorage;
    use sideseat_ports::clock::Clock;
    use sideseat_ports::traits::{EntityQuery, SpanStore};
    use tempfile::TempDir;
    use tokio::fs;

    #[derive(Debug)]
    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
        }
    }

    fn test_hash() -> String {
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string()
    }

    async fn setup_test() -> (
        TempDir,
        Arc<dyn TransactionalRepository + Send + Sync>,
        Arc<CacheService>,
    ) {
        let temp_dir = TempDir::new().unwrap();
        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let sqlite_service = SqliteService::init(&app_storage, Arc::new(TestClock))
            .await
            .unwrap();
        let database: Arc<dyn TransactionalRepository + Send + Sync> =
            Arc::new(SqliteRepository(Arc::new(sqlite_service)));

        let cache_config = sideseat_core::core::config::CacheConfig {
            backend: sideseat_core::core::config::CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: sideseat_core::core::config::EvictionPolicy::TinyLfu,
            redis_url: None,
        };
        let cache = Arc::new(CacheService::new(&cache_config).await.unwrap());

        (temp_dir, database, cache)
    }

    async fn create_file_service(
        config: FilesConfig,
        app_storage: &AppStorage,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<CacheService>,
    ) -> Result<FileService, FileServiceError> {
        let files_path = config
            .filesystem_path
            .as_ref()
            .map(|path| sideseat_core::utils::file::expand_path(path))
            .unwrap_or_else(|| app_storage.subdir(sideseat_core::core::storage::DataSubdir::Files));
        FileService::new(
            config,
            app_storage.subdir(sideseat_core::core::storage::DataSubdir::FilesTemp),
            Arc::new(FilesystemStorage::new(files_path)),
            database,
            cache,
        )
        .await
    }

    #[tokio::test]
    async fn test_file_service_disabled() {
        let (temp_dir, database, cache) = setup_test().await;

        let config = FilesConfig {
            enabled: false,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database, cache)
            .await
            .unwrap();

        assert!(!service.is_enabled());

        let result = service
            .get_file(&ProjectId::from("default"), &test_hash())
            .await;
        assert!(matches!(result, Err(FileServiceError::Disabled)));
    }

    #[tokio::test]
    async fn test_file_service_get_file() {
        let (temp_dir, database, cache) = setup_test().await;

        let config = FilesConfig {
            enabled: true,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap();

        // Store a file directly
        service
            .storage
            .store(&ProjectId::from("default"), &test_hash(), b"test content")
            .await
            .unwrap();

        // Insert metadata via repository trait
        let repo = database.as_ref();
        repo.upsert_file(
            &ProjectId::from("default"),
            &test_hash(),
            Some("text/plain"),
            12,
            "sha256",
        )
        .await
        .unwrap();

        // Get file through service
        let content = service
            .get_file(&ProjectId::from("default"), &test_hash())
            .await
            .unwrap();
        assert_eq!(content.data, b"test content");
        assert_eq!(content.media_type, Some("text/plain".to_string()));
    }

    /// Client-owned identifiers are only unique inside a project.
    ///
    /// This is the pre-policy tenant oracle: later RLS and row-policy work must keep returning the selected
    /// tenant's row rather than merely making every read empty. It deliberately collides every identifier that
    /// crosses a storage boundary: trace, span, session and content hash. Distinct tenant payloads make any leak
    /// observable instead of allowing equal fixture values to conceal it.
    #[tokio::test]
    async fn colliding_client_ids_remain_isolated_across_analytics_metadata_and_blobs() {
        let (temp_dir, database, cache) = setup_test().await;
        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let files = create_file_service(
            FilesConfig {
                enabled: true,
                storage: sideseat_core::core::config::StorageBackend::Filesystem,
                quota_bytes: 1024 * 1024,
                filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
                s3: None,
            },
            &app_storage,
            Arc::clone(&database),
            cache,
        )
        .await
        .expect("file service");

        let analytics_dir = TempDir::new().expect("analytics temp dir");
        tokio::fs::create_dir_all(analytics_dir.path().join("duckdb"))
            .await
            .expect("duckdb directory");
        let analytics_storage = AppStorage::init_for_test(analytics_dir.path().to_path_buf());
        let analytics = DuckdbRepository(Arc::new(
            DuckdbService::init(&analytics_storage, Arc::new(TestClock))
                .await
                .expect("duckdb"),
        ));

        let projects = [
            (ProjectId::from("tenant-a"), "tenant-a"),
            (ProjectId::from("tenant-b"), "tenant-b"),
        ];
        let mut spans = Vec::new();

        // Several independent collisions keep this a property of the project scope rather than one magic id.
        for case in 1_u8..=8 {
            let trace_id = format!("client-trace-{case}");
            let span_id = format!("client-span-{case}");
            let session_id = format!("client-session-{case}");
            let hash = format!("{case:064x}");

            for (project_id, tenant_label) in &projects {
                let payload = format!("{tenant_label}-payload-{case}");
                files
                    .storage
                    .store(project_id, &hash, payload.as_bytes())
                    .await
                    .expect("store tenant blob");
                database
                    .upsert_file(
                        project_id,
                        &hash,
                        Some(&format!("application/x-{tenant_label}")),
                        payload.len() as i64,
                        "sha256",
                    )
                    .await
                    .expect("store tenant metadata");
                database
                    .insert_trace_file(&trace_id, project_id, &hash)
                    .await
                    .expect("associate tenant trace");

                spans.push(sideseat_ports::types::NormalizedSpan {
                    project_id: Some(project_id.to_string()),
                    trace_id: trace_id.clone(),
                    span_id: span_id.clone(),
                    session_id: Some(session_id.clone()),
                    user_id: Some(format!("{tenant_label}-user")),
                    span_name: format!("{tenant_label}-span-{case}"),
                    environment: Some(tenant_label.to_string()),
                    timestamp_start: Utc
                        .timestamp_opt(1_700_000_000 + i64::from(case), 0)
                        .single()
                        .expect("timestamp"),
                    ..Default::default()
                });
            }
        }
        analytics
            .insert_spans(spans)
            .await
            .expect("insert colliding spans");

        for case in 1_u8..=8 {
            let trace_id = format!("client-trace-{case}");
            let span_id = format!("client-span-{case}");
            let session_id = format!("client-session-{case}");
            let hash = format!("{case:064x}");

            for (project_id, tenant_label) in &projects {
                let span = analytics
                    .get_span(project_id, &trace_id, &span_id)
                    .await
                    .expect("read tenant span")
                    .expect("tenant span exists");
                assert_eq!(
                    span.span_name.as_deref(),
                    Some(format!("{tenant_label}-span-{case}").as_str())
                );

                let trace = analytics
                    .get_trace(project_id, &trace_id)
                    .await
                    .expect("read tenant trace")
                    .expect("tenant trace exists");
                assert_eq!(trace.environment.as_deref(), Some(*tenant_label));

                let session = analytics
                    .get_session(project_id, &session_id)
                    .await
                    .expect("read tenant session")
                    .expect("tenant session exists");
                assert_eq!(session.environment.as_deref(), Some(*tenant_label));
                assert_eq!(
                    session.user_id.as_deref(),
                    Some(format!("{tenant_label}-user").as_str())
                );

                let content = files
                    .get_file(project_id, &hash)
                    .await
                    .expect("read tenant blob");
                assert_eq!(
                    content.data,
                    format!("{tenant_label}-payload-{case}").as_bytes()
                );
                assert_eq!(
                    content.media_type.as_deref(),
                    Some(format!("application/x-{tenant_label}").as_str())
                );

                let association_counts = database
                    .get_file_reference_counts_for_traces(
                        project_id,
                        std::slice::from_ref(&trace_id),
                    )
                    .await
                    .expect("read tenant associations");
                assert_eq!(association_counts, vec![(hash.clone(), 1)]);
            }
        }
    }

    /// Retention expiring *some* of a trace's spans must not reclaim the survivors' files.
    ///
    /// The live defect: retention selects individual span identities, then handed every affected `trace_id` to
    /// `cleanup_traces`, which deletes **all** of a trace's associations. So a busy trace with one expired span
    /// lost the file references of every span still in it, and those spans then pointed at bytes that had been
    /// reclaimed - the dangling reference the write-files-before-rows ordering exists to prevent, produced by
    /// retention.
    ///
    /// The fixture is the shape that distinguishes the fix from both wrong answers: one expired span uniquely
    /// referencing file A, one surviving span referencing file B. Trace-wide deletion takes B as well;
    /// trace-wide *preservation* (only cleaning an emptied trace) never reclaims A. Only reconciliation gets
    /// both right.
    #[tokio::test]
    async fn reconciliation_keeps_a_surviving_spans_file_and_releases_the_expired_ones() {
        let (temp_dir, database, cache) = setup_test().await;
        fs::create_dir_all(temp_dir.path().join("files"))
            .await
            .unwrap();
        fs::create_dir_all(temp_dir.path().join("files_temp"))
            .await
            .unwrap();

        let config = FilesConfig {
            enabled: true,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };
        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap();

        // A real analytics store, because the survivor set is a fact about spans - a stub would be asserting
        // against my own idea of what the query returns.
        let analytics_dir = TempDir::new().unwrap();
        tokio::fs::create_dir_all(analytics_dir.path().join("duckdb"))
            .await
            .unwrap();
        let analytics_storage = AppStorage::init_for_test(analytics_dir.path().to_path_buf());
        let duck = Arc::new(
            DuckdbService::init(&analytics_storage, std::sync::Arc::new(TestClock))
                .await
                .expect("duckdb"),
        );

        let expired_hash = "a".repeat(64);
        let surviving_hash = "b".repeat(64);
        let repo = database.as_ref();
        for hash in [&expired_hash, &surviving_hash] {
            service
                .storage
                .store(&ProjectId::from("default"), hash, b"bytes")
                .await
                .unwrap();
            repo.upsert_file(&ProjectId::from("default"), hash, None, 5, "sha256")
                .await
                .unwrap();
            repo.insert_trace_file("trace1", &ProjectId::from("default"), hash)
                .await
                .unwrap();
        }

        // The trace still has one span, and it references B only. A's span is the one retention just expired,
        // so it is simply absent - which is what the survivor scan reads.
        sideseat_ports::traits::SpanStore::insert_spans(
            &DuckdbRepository(Arc::clone(&duck)),
            vec![sideseat_ports::types::NormalizedSpan {
                project_id: Some("default".to_string()),
                trace_id: "trace1".to_string(),
                span_id: "survivor".to_string(),
                span_name: "still-here".to_string(),
                messages: Some(format!(
                    r#"[{{"content":"see #!B64!#image/png::{surviving_hash}"}}]"#
                )),
                timestamp_start: chrono::DateTime::UNIX_EPOCH,
                ..Default::default()
            }],
        )
        .await
        .expect("insert the surviving span");

        service
            .reconcile_trace_survivors(
                &ProjectId::from("default"),
                &["trace1".to_string()],
                &DuckdbRepository(Arc::clone(&duck)),
            )
            .await
            .expect("reconcile");

        assert!(
            !service
                .file_exists(&ProjectId::from("default"), &expired_hash)
                .await
                .unwrap(),
            "the expired span's file was not reclaimed, so a busy trace retains expired bytes forever"
        );
        assert!(
            service
                .file_exists(&ProjectId::from("default"), &surviving_hash)
                .await
                .unwrap(),
            "the surviving span's file was reclaimed, leaving a live span pointing at bytes that are gone - \
             the exact dangling reference this reconciliation exists to prevent"
        );
    }

    /// A span that commits **between the scan and the release** keeps its file.
    ///
    /// The window `pending_writers` cannot close, because at the decisive moment the counter is legitimately
    /// zero: reconciliation scans trace T and does not see hash H; an ingestion then associates H, commits its
    /// span, and confirms - dropping `pending_writers` back to zero. The release now sees a zero counter and a
    /// hash the stale snapshot did not contain, and reclaims bytes a committed span references.
    ///
    /// The compensation is a second scan after the release, restoring any association whose reference has
    /// appeared, before any byte is deleted. This test drives exactly that interleaving by writing the span
    /// *after* the first scan would have run - which is what the previous test could not do, because it left
    /// the writer pending throughout and so never reached the dangerous state.
    #[tokio::test]
    async fn a_span_committing_between_the_scan_and_the_release_keeps_its_file() {
        let (temp_dir, database, cache) = setup_test().await;
        fs::create_dir_all(temp_dir.path().join("files"))
            .await
            .unwrap();
        fs::create_dir_all(temp_dir.path().join("files_temp"))
            .await
            .unwrap();

        let config = FilesConfig {
            enabled: true,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };
        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap();

        // No analytics store here: the stub below *is* the analytics side, which is the point of the narrow
        // port - the interleaving is what is under test, not a query.
        //
        // A file associated and *confirmed* - so `pending_writers` is zero - whose span is not in the store
        // when the first scan runs.
        let racing = "d".repeat(64);
        let repo = database.as_ref();
        service
            .storage
            .store(&ProjectId::from("default"), &racing, b"bytes")
            .await
            .unwrap();
        repo.upsert_file(&ProjectId::from("default"), &racing, None, 5, "sha256")
            .await
            .unwrap();
        repo.insert_trace_file("trace1", &ProjectId::from("default"), &racing)
            .await
            .unwrap();

        // An analytics repository that writes the span on its *second* read, which is precisely the
        // interleaving: the first scan sees nothing, the re-check sees the committed span.
        struct RacingAnalytics {
            calls: std::sync::atomic::AtomicUsize,
            hash: String,
        }

        #[async_trait::async_trait]
        impl sideseat_ports::traits::SurvivorReferences for RacingAnalytics {
            async fn file_reference_fields_for_traces(
                &self,
                _project_id: &ProjectId,
                _trace_ids: &[String],
            ) -> Result<Vec<String>, sideseat_ports::error::DataError> {
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    // The scan, before the span exists.
                    return Ok(Vec::new());
                }
                // The re-check, after it committed.
                Ok(vec![format!("#!B64!#image/png::{}", self.hash)])
            }
        }

        let racing_analytics = RacingAnalytics {
            calls: std::sync::atomic::AtomicUsize::new(0),
            hash: racing.clone(),
        };

        service
            .reconcile_trace_survivors(
                &ProjectId::from("default"),
                &["trace1".to_string()],
                &racing_analytics,
            )
            .await
            .expect("reconcile");

        assert!(
            service
                .file_exists(&ProjectId::from("default"), &racing)
                .await
                .unwrap(),
            "a span that committed between the survivor scan and the release lost its file - the re-check did \
             not restore the association, so a committed span points at bytes that are gone"
        );
    }

    /// A concurrent ingestion's association is not released, because its writer is counted before its span row
    /// exists.
    ///
    /// This is the race that makes "check the trace is empty, then delete by trace" unsound: the survivor scan
    /// reads spans, and a batch that has associated a file but not yet written its span is invisible to it. What
    /// protects that batch is `pending_writers`, which `associate_file` increments *first* - so the release is
    /// conditional on it being zero rather than on the scan having seen something.
    #[tokio::test]
    async fn reconciliation_leaves_an_association_a_batch_still_holds() {
        let (temp_dir, database, cache) = setup_test().await;
        fs::create_dir_all(temp_dir.path().join("files"))
            .await
            .unwrap();
        fs::create_dir_all(temp_dir.path().join("files_temp"))
            .await
            .unwrap();

        let config = FilesConfig {
            enabled: true,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };
        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap();

        // A real analytics store, because the survivor set is a fact about spans - a stub would be asserting
        // against my own idea of what the query returns.
        let analytics_dir = TempDir::new().unwrap();
        tokio::fs::create_dir_all(analytics_dir.path().join("duckdb"))
            .await
            .unwrap();
        let analytics_storage = AppStorage::init_for_test(analytics_dir.path().to_path_buf());
        let duck = Arc::new(
            DuckdbService::init(&analytics_storage, std::sync::Arc::new(TestClock))
                .await
                .expect("duckdb"),
        );

        // An in-flight batch: bytes stored, association created through the **real** path, span row not written
        // yet. `associate_file` is what increments `pending_writers`; the test-only `insert_trace_file` leaves
        // it at zero, so building the fixture with that would have been a fixture unable to show the property -
        // which is what the first version of this test did.
        let in_flight = "c".repeat(64);
        let repo = database.as_ref();
        service
            .storage
            .store(&ProjectId::from("default"), &in_flight, b"bytes")
            .await
            .unwrap();
        repo.associate_file(
            "trace1",
            &ProjectId::from("default"),
            &in_flight,
            None,
            5,
            "sha256",
        )
        .await
        .unwrap();

        // No spans at all for this trace, so the survivor set is empty - the worst case for the in-flight batch.
        service
            .reconcile_trace_survivors(
                &ProjectId::from("default"),
                &["trace1".to_string()],
                &DuckdbRepository(Arc::clone(&duck)),
            )
            .await
            .expect("reconcile");

        let held = repo
            .get_file_hashes_for_traces(&ProjectId::from("default"), &["trace1".to_string()])
            .await
            .expect("read the associations back");
        assert!(
            held.contains(&in_flight),
            "the in-flight batch's association was released, so its span will commit holding a reference to \
             bytes that have been reclaimed"
        );
    }

    #[tokio::test]
    async fn test_file_service_cleanup_traces() {
        let (temp_dir, database, cache) = setup_test().await;

        // Create directories
        fs::create_dir_all(temp_dir.path().join("files"))
            .await
            .unwrap();
        fs::create_dir_all(temp_dir.path().join("files_temp"))
            .await
            .unwrap();

        let config = FilesConfig {
            enabled: true,
            storage: sideseat_core::core::config::StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024,
            filesystem_path: Some(temp_dir.path().join("files").to_string_lossy().to_string()),
            s3: None,
        };

        let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        let service = create_file_service(config, &app_storage, database.clone(), cache)
            .await
            .unwrap();

        // Store a file
        service
            .storage
            .store(&ProjectId::from("default"), &test_hash(), b"test content")
            .await
            .unwrap();

        // Insert metadata with ref_count = 1 via repository trait
        let repo = database.as_ref();
        repo.upsert_file(
            &ProjectId::from("default"),
            &test_hash(),
            None,
            12,
            "sha256",
        )
        .await
        .unwrap();

        // Associate with trace
        repo.insert_trace_file("trace1", &ProjectId::from("default"), &test_hash())
            .await
            .unwrap();

        // Cleanup the trace
        service
            .cleanup_traces(&ProjectId::from("default"), &["trace1".to_string()])
            .await
            .unwrap();

        // File should be deleted (ref_count was 1, now 0)
        assert!(
            !service
                .file_exists(&ProjectId::from("default"), &test_hash())
                .await
                .unwrap()
        );

        // Metadata should be gone
        let file = repo
            .get_file(&ProjectId::from("default"), &test_hash())
            .await
            .unwrap();
        assert!(file.is_none());
    }
}
