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
mod references;

use sideseat_ports::blobs::{FileContent, FileStorage, FileStorageError};
use sideseat_ports::cache::{CacheStore, TypedCache};
use sideseat_ports::traits::TransactionalRepository;
use sideseat_ports::types::ProjectId;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sideseat_core::config::FilesConfig;
use sideseat_core::constants::CACHE_TTL_FILE_QUOTA;
use sideseat_core::utils::file_uri::parse_file_uri;
use sideseat_ports::cache::CacheKey;

pub use error::FileServiceError;
pub use references::collect_file_references_in_str;

/// File metadata without content
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// File size in bytes
    pub size_bytes: i64,
    /// MIME type (e.g., "image/png")
    pub media_type: Option<String>,
}

/// One analytics reference whose content-addressed bytes were not present after a restore.
///
/// There is no safe repair for this class: the row contains only a hash, not the original bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingFileReference {
    pub project_id: ProjectId,
    pub trace_id: String,
    pub hash: String,
}

/// What file-association repair changed, and what it could not repair.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct FileRestoreRepairReport {
    pub traces_scanned: u64,
    pub references_scanned: u64,
    pub metadata_rebuilt: u64,
    pub associations_rebuilt: u64,
    pub missing_content: Vec<MissingFileReference>,
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
        Self::new_inner(config, temp_dir, storage, database, cache, None, true).await
    }

    pub async fn new_governed(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
        governance: Arc<crate::storage_governance::StorageGovernanceService>,
    ) -> Result<Self, FileServiceError> {
        Self::new_inner(
            config,
            temp_dir,
            storage,
            database,
            cache,
            Some(governance),
            true,
        )
        .await
    }

    /// Construct the service without running orphan GC.
    ///
    /// Restore repair needs this ordering: rebuild associations from surviving analytics rows, then
    /// permit GC. Running the ordinary constructor against independently restored stores could delete a
    /// live blob in the gap, making a repairable missing association unrecoverable.
    pub async fn new_governed_deferred_cleanup(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
        governance: Arc<crate::storage_governance::StorageGovernanceService>,
    ) -> Result<Self, FileServiceError> {
        Self::new_inner(
            config,
            temp_dir,
            storage,
            database,
            cache,
            Some(governance),
            false,
        )
        .await
    }

    async fn new_inner(
        config: FilesConfig,
        temp_dir: PathBuf,
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        cache: Arc<dyn CacheStore>,
        governance: Option<Arc<crate::storage_governance::StorageGovernanceService>>,
        run_cleanup: bool,
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

        if run_cleanup {
            service.run_startup_cleanup().await;
        }

        Ok(service)
    }

    /// Run the ordinary startup cleanup after any restore repair has rebuilt ownership.
    pub async fn run_startup_cleanup(&self) {
        if !self.config.enabled {
            return;
        }
        if let Err(error) =
            cleanup::cleanup_orphan_temp_files(&self.temp_dir, &self.storage, &self.database).await
        {
            tracing::warn!(%error, "Failed to cleanup orphan temp files on startup");
        }
        if let Err(error) = cleanup::cleanup_zero_ref_files_governed(
            &self.storage,
            &self.database,
            sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS,
            self.governance.as_ref(),
        )
        .await
        {
            tracing::warn!(%error, "Failed to cleanup zero-ref files on startup");
        }
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
        let metadata = repo.get_file(project_id, hash).await?;
        let media_type = metadata.as_ref().and_then(|file| file.media_type.clone());

        // Get data from storage
        let data = self
            .storage
            .get(project_id, hash)
            .await
            .map_err(|e| match e {
                FileStorageError::NotFound { .. } if metadata.is_some() => {
                    FileServiceError::ContentUnavailable {
                        project_id: project_id.to_string(),
                        hash: hash.to_string(),
                    }
                }
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
            return Err(FileServiceError::ContentUnavailable {
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

    /// Rebuild uploaded-file ownership from the references in surviving analytics rows.
    ///
    /// This is the restore-specific direction of reconciliation: ordinary retention starts from
    /// transactional associations and releases those no survivor needs, while a mismatched restore can
    /// have the survivor and the bytes but have lost the association itself. The association is restored
    /// as durable and the cached count is derived from the resulting rows before any orphan GC may run.
    ///
    /// A surviving row whose bytes are absent is not fabricated. Its hash is returned in
    /// [`FileRestoreRepairReport::missing_content`] so an operator and the API can distinguish corruption
    /// from a reference that never existed.
    pub async fn repair_trace_associations_after_restore(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<FileRestoreRepairReport, FileServiceError> {
        let mut report = FileRestoreRepairReport::default();
        if !self.config.enabled {
            return Ok(report);
        }

        for trace_id in trace_ids {
            report.traces_scanned += 1;
            let fields = analytics
                .file_reference_fields_for_traces(project_id, std::slice::from_ref(trace_id))
                .await?;
            let mut uris = Vec::new();
            for field in &fields {
                collect_file_references_in_str(field, &mut uris);
            }

            let mut references = std::collections::BTreeMap::<String, Option<String>>::new();
            for uri in uris {
                if let Some(parsed) = parse_file_uri(&uri) {
                    references
                        .entry(parsed.hash.to_owned())
                        .or_insert_with(|| parsed.media_type.map(str::to_owned));
                }
            }
            for hash in raw_media_of_survivors(project_id, trace_id, analytics).await? {
                references.entry(hash).or_insert(None);
            }
            report.references_scanned += references.len() as u64;

            let associated = self
                .database
                .get_file_hashes_for_traces(project_id, std::slice::from_ref(trace_id))
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();

            for (hash, media_type) in references {
                if !self.storage.exists(project_id, &hash).await? {
                    report.missing_content.push(MissingFileReference {
                        project_id: project_id.clone(),
                        trace_id: trace_id.clone(),
                        hash,
                    });
                    continue;
                }

                if self.database.get_file(project_id, &hash).await?.is_none() {
                    let bytes = match self.storage.get(project_id, &hash).await {
                        Ok(bytes) => bytes,
                        Err(FileStorageError::NotFound { .. }) => {
                            report.missing_content.push(MissingFileReference {
                                project_id: project_id.clone(),
                                trace_id: trace_id.clone(),
                                hash,
                            });
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    self.database
                        .restore_orphan_metadata(
                            project_id,
                            &hash,
                            media_type.as_deref(),
                            i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                            "sha256",
                        )
                        .await?;
                    report.metadata_rebuilt += 1;
                }

                self.database
                    .restore_durable_trace_file(project_id, trace_id, &hash)
                    .await?;
                if !associated.contains(&hash) {
                    report.associations_rebuilt += 1;
                }
                self.database.sync_ref_count(project_id, &hash).await?;
            }
        }

        report.missing_content.sort_by(|left, right| {
            (&left.project_id, &left.trace_id, &left.hash).cmp(&(
                &right.project_id,
                &right.trace_id,
                &right.hash,
            ))
        });
        report.missing_content.dedup();
        Ok(report)
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
        // **A failed re-check restores every association it released.** Handing them on to be reclaimed, as
        // this once did, let a span that committed between the scan and the release lose its file. Dropping
        // them silently instead left their stored `ref_count` above zero with no association to rediscover
        // them from, which the orphan sweeper never reclaims. Restored as durable, they are counted again and
        // the next pass, whose scans succeed, releases the ones nothing references.
        let now_referenced = match Self::referenced_hashes(project_id, trace_id, analytics).await {
            Ok(hashes) => hashes,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    project_id = %project_id,
                    trace_id,
                    released = removed.len(),
                    "Could not re-check survivors after releasing their associations; restoring them, so a \
                     span that committed during the release keeps its file, and leaving them to the next pass"
                );
                for hash in &removed {
                    repo.restore_durable_trace_file(project_id, trace_id, hash)
                        .await?;
                    repo.sync_ref_count(project_id, hash).await?;
                }
                return Ok(Vec::new());
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

    /// The file hashes a trace's stored rows and raw records reference - the survivor scan's answer, for a caller
    /// that must decide whether rows it cannot vouch for hold a reference.
    pub async fn hashes_referenced_by_trace(
        project_id: &ProjectId,
        trace_id: &str,
        analytics: &dyn sideseat_ports::traits::SurvivorReferences,
    ) -> Result<Vec<String>, FileServiceError> {
        Self::referenced_hashes(project_id, trace_id, analytics).await
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
        hashes.extend(raw_media_of_survivors(project_id, trace_id, analytics).await?);
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
        let deleted = self.storage.delete_project(project_id).await?;
        self.database.delete_project_files(project_id).await?;

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

/// The media hashes of the raw records the surviving spans of one trace name.
///
/// A record the reader cannot read is reported rather than skipped: skipping it would release media a
/// surviving row's record needs.
async fn raw_media_of_survivors(
    project_id: &ProjectId,
    trace_id: &str,
    analytics: &dyn sideseat_ports::traits::SurvivorReferences,
) -> Result<Vec<String>, FileServiceError> {
    let records = analytics
        .survivor_raw_records(project_id, std::slice::from_ref(&trace_id.to_string()))
        .await
        .map_err(FileServiceError::from)?;
    let mut hashes = Vec::new();
    for record in &records {
        let media = crate::raw_payload::media_hashes(record)?;
        hashes.extend(media.iter().map(hex::encode));
    }
    Ok(hashes)
}

#[async_trait::async_trait]
impl sideseat_ports::blobs::RetentionFileReconciler for FileService {
    fn is_enabled(&self) -> bool {
        FileService::is_enabled(self)
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
#[path = "mod_tests.rs"]
mod tests;
