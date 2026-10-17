//! Orphan file cleanup
//!
//! Handles cleanup of orphaned files that may result from crashes or incomplete operations:
//! - Temp files left behind after crashes
//! - Files with ref_count=0 that weren't properly deleted
//!
//! ## Cleanup Scenarios
//!
//! ### Scenario A: Temp file exists with database metadata
//! The server crashed after database insert but before permanent storage.
//! Action: Move file to permanent storage (complete the interrupted write).
//!
//! ### Scenario B: Temp file exists without database metadata
//! The server crashed before database insert.
//! Action: Delete the orphaned temp file.
//!
//! ### Scenario C: Database has ref_count=0 files
//! Decrement operation completed but file deletion failed.
//! Action: Delete the file from storage and database.

use std::path::Path;
use std::sync::Arc;

use super::error::FileServiceError;
use sideseat_core::utils::collection_fence::CollectionFence;
use sideseat_ports::blobs::{ExpectedContent, FileStorage, FileStorageError, ObjectName};
use sideseat_ports::traits::TransactionalRepository;
use sideseat_ports::types::ProjectId;

enum GcFence {
    Unfenced,
    Acquired(String),
    Skip,
}

async fn acquire_gc_fence(
    governance: Option<&Arc<crate::storage_governance::StorageGovernanceService>>,
    project_id: &ProjectId,
) -> GcFence {
    let Some(governance) = governance else {
        return GcFence::Unfenced;
    };
    let owner = match governance.acquire_maintenance(project_id, "file-gc").await {
        Ok(owner) => owner,
        Err(error) => {
            tracing::warn!(project_id = %project_id, %error, "Could not acquire file-GC fence");
            return GcFence::Skip;
        }
    };
    match governance.current_hold(project_id).await {
        Ok(None) => GcFence::Acquired(owner),
        Ok(Some(hold)) => {
            tracing::debug!(
                project_id = %project_id,
                hold_until = %hold.hold_until,
                "Keeping orphan file under legal hold"
            );
            governance.release_maintenance(project_id, &owner).await;
            GcFence::Skip
        }
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                %error,
                "Could not check legal hold before file GC; keeping the file"
            );
            governance.release_maintenance(project_id, &owner).await;
            GcFence::Skip
        }
    }
}

async fn release_gc_fence(
    governance: Option<&Arc<crate::storage_governance::StorageGovernanceService>>,
    project_id: &ProjectId,
    fence: &GcFence,
) {
    if let (Some(governance), GcFence::Acquired(owner)) = (governance, fence) {
        governance.release_maintenance(project_id, owner).await;
    }
}

async fn reconcile_gc_usage(
    governance: Option<&Arc<crate::storage_governance::StorageGovernanceService>>,
    project_id: &ProjectId,
) {
    if let Some(governance) = governance
        && let Err(error) = governance.reconcile_project(project_id).await
    {
        tracing::warn!(
            project_id = %project_id,
            %error,
            "File GC completed but its quota counter could not be reconciled"
        );
    }
}

/// Run startup cleanup for orphaned temp files
///
/// Scans the temp directory and handles orphaned files:
/// - Files with database metadata: move to permanent storage
/// - Files without database metadata: delete
pub async fn cleanup_orphan_temp_files(
    temp_dir: &Path,
    storage: &Arc<dyn FileStorage>,
    database: &Arc<dyn TransactionalRepository + Send + Sync>,
) -> Result<CleanupStats, FileServiceError> {
    let mut stats = CleanupStats::default();

    if !temp_dir.exists() {
        return Ok(stats);
    }

    let mut entries = match tokio::fs::read_dir(temp_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %temp_dir.display(),
                "Failed to read temp directory for cleanup"
            );
            return Ok(stats);
        }
    };

    while let Some(entry) = entries.next_entry().await.transpose() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read temp directory entry");
                continue;
            }
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Parse filename: {project_id}_{hash}
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        let (project_id, hash, expected) = match parse_temp_file_name(filename) {
            Some(parsed) => parsed,
            None => {
                tracing::warn!(filename, "Invalid temp filename format, deleting");
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!(error = %e, path = %path.display(), "Failed to delete invalid temp file");
                }
                stats.invalid_deleted += 1;
                continue;
            }
        };
        let project_id = ProjectId::from(project_id);

        // Check if database has metadata for this file: its row names the incarnation the bytes belong under.
        // A lookup that fails says nothing about the row: the file stays, for a pass that can read it. Only a row
        // shown absent makes it an orphan.
        let repo = database.as_ref();
        let incarnation = match repo.get_file(&project_id, &hash).await {
            Ok(row) => row.map(|row| row.id),
            Err(error) => {
                tracing::warn!(%error, project_id = %project_id, hash, "Could not read a temp file's row; keeping it");
                stats.finalize_failed += 1;
                continue;
            }
        };

        if let Some(incarnation) = incarnation {
            let object = ObjectName::new(&hash, incarnation);
            // Scenario A: complete the interrupted write - only with the content the file must hold, checked as
            // finalisation checks it: a crash can interrupt the write itself, and a file a racing write damaged must
            // not reach the store. One named without it (an earlier release's) cannot be checked, and goes: its
            // media was finalised before any row named it, so the export it came from was never answered as stored
            // and is written again.
            let Some(expected) = expected else {
                stats.invalid_deleted += 1;
                tracing::warn!(
                    filename,
                    "Temp file without its expected content; deleting it unverified"
                );
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!(error = %e, path = %path.display(), "Failed to delete temp file");
                }
                continue;
            };
            // Finalised from a copy, so the file itself - possibly the last copy of these bytes - survives a
            // finalisation that fails after it took its source (a rename, then a failed sync), for the next pass.
            // A copy of its own: two processes recovering one directory must not share - and truncate - a copy.
            let copy = path.with_extension(format!(
                "{}.recovering-{}",
                path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default(),
                uuid::Uuid::new_v4().simple()
            ));
            if let Err(error) = tokio::fs::copy(&path, &copy).await {
                tokio::fs::remove_file(&copy).await.ok();
                tracing::warn!(%error, path = %path.display(), "Could not copy a temp file to recover it; keeping it");
                stats.finalize_failed += 1;
                continue;
            }
            let finalized = storage
                .finalize_temp(&project_id, object, &copy, expected)
                .await;
            tokio::fs::remove_file(&copy).await.ok();
            match finalized {
                Ok(()) => {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::warn!(error = %e, path = %path.display(), "Failed to delete temp file");
                    }
                    stats.finalized += 1;
                    tracing::debug!(
                        project_id = %project_id,
                        hash,
                        "Finalized orphan temp file to permanent storage"
                    );
                }
                Err(FileStorageError::Corrupt { detail, .. }) => {
                    // The copy was not the content. Whether the file itself is decides its fate: one that is not can
                    // never be stored and goes; one that is - its copy damaged meanwhile - stays for the next pass.
                    if holds(&path, expected).await == Some(false) {
                        tokio::fs::remove_file(&path).await.ok();
                        stats.invalid_deleted += 1;
                        tracing::warn!(project_id = %project_id, hash, detail, "Deleted a temp file that is not its content");
                    } else {
                        stats.finalize_failed += 1;
                        tracing::warn!(project_id = %project_id, hash, detail, "A recovery copy was damaged; keeping the temp file");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        hash,
                        "Failed to finalize temp file to permanent storage"
                    );
                    stats.finalize_failed += 1;
                }
            }
            continue;
        } else {
            // Scenario B: Orphaned temp file (no database record)
            stats.orphaned_deleted += 1;
            tracing::debug!(
                project_id = %project_id,
                hash,
                "Deleting orphan temp file (no database record)"
            );
        }

        // Delete temp file after handling
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(error = %e, path = %path.display(), "Failed to delete temp file");
        }
    }

    if stats.total_processed() > 0 {
        tracing::debug!(
            finalized = stats.finalized,
            orphaned_deleted = stats.orphaned_deleted,
            invalid_deleted = stats.invalid_deleted,
            finalize_failed = stats.finalize_failed,
            "Temp file cleanup complete"
        );
    }

    Ok(stats)
}

/// Delete files nothing references, and finish deletions a crash abandoned.
///
/// Two shapes, one sweep. A zero-reference row whose bytes are still on disk is a decrement that
/// completed while the storage delete failed. A row still claimed for deletion after
/// `stale_claim_secs` is a process that died holding the fence - see the resumption block below for why
/// that cannot be left alone.
///
/// `stale_claim_secs` is a parameter rather than read from the constant here so the caller owns the
/// policy and a test can state the age it means directly.
/// Delete a collected incarnation's bytes once more, after its row: an ingest whose association the collection
/// outran can have stored them between the first delete and the row's, and with the row gone nothing would find
/// them again. The name is that incarnation's alone, so no other row's bytes can be under it.
pub(super) async fn delete_bytes_again(
    storage: &Arc<dyn FileStorage>,
    project_id: &ProjectId,
    object: ObjectName<'_>,
) {
    if let Err(error) = storage.delete(project_id, object).await {
        tracing::warn!(
            project_id = %project_id,
            hash = object.hash,
            %error,
            "Could not delete a collected file's bytes a second time; any stored since are left"
        );
    }
}

pub async fn cleanup_zero_ref_files(
    storage: &Arc<dyn FileStorage>,
    database: &Arc<dyn TransactionalRepository + Send + Sync>,
    stale_claim_secs: i64,
) -> Result<u64, FileServiceError> {
    cleanup_zero_ref_files_governed(
        storage,
        database,
        stale_claim_secs,
        None,
        &CollectionFence::open(),
    )
    .await
}

pub async fn cleanup_zero_ref_files_governed(
    storage: &Arc<dyn FileStorage>,
    database: &Arc<dyn TransactionalRepository + Send + Sync>,
    stale_claim_secs: i64,
    governance: Option<&Arc<crate::storage_governance::StorageGovernanceService>>,
    collection: &CollectionFence,
) -> Result<u64, FileServiceError> {
    // Every claim this sweep takes and acts on is inside one pass: a lease lost while it runs is not opened
    // again before it is done, so no claim it read before the loss is acted on after.
    let Some(_pass) = collection.enter("zero-reference sweep") else {
        return Ok(0);
    };
    let repo = database.as_ref();

    // Get files with ref_count = 0
    let orphan_files = repo.get_orphan_files().await?;

    let mut deleted = 0u64;

    for (project_id, hash) in orphan_files {
        // Before each claim: a sweep in flight when the lease is lost stops at the next file.
        if !collection.is_open() {
            tracing::debug!("Collection is fenced; the zero-reference sweep stops");
            break;
        }
        let project_id = ProjectId::from(project_id);
        let fence = acquire_gc_fence(governance, &project_id).await;
        if matches!(fence, GcFence::Skip) {
            continue;
        }
        let removed = async {
        // The metadata row first, and only if nothing references the file *now*.
        //
        // `get_orphan_files` selects on the stored `ref_count`, which is a cached count - so a stale
        // zero, or one read before an association landed, would have deleted live content. The
        // conditional delete asks the associations directly, and deleting the row before the bytes
        // means the surviving failure is bytes without metadata rather than a row pointing at nothing.
        // Claimed first, exactly as `delete_files_for_traces` does: `get_orphan_files` selects on the
        // stored `ref_count`, which is a cached count, so a stale zero would otherwise delete live
        // content - and without the claim, ingestion could recreate and finalise between the row delete
        // and the byte delete.
        let claim = match repo.claim_file_for_deletion(&project_id, &hash).await {
            Ok(Some(claim)) => claim,
            Ok(None) => {
                tracing::debug!(
                    project_id = %project_id,
                    hash,
                    "Orphan file is referenced or already being deleted; keeping it"
                );
                return false;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    hash,
                    "Failed to claim orphan file for deletion"
                );
                return false;
            }
        };

        // Only the claimed incarnation's bytes: a later one is a row created since, which this claim is not.
        if let Err(e) = storage
            .delete(&project_id, ObjectName::new(&hash, claim.incarnation))
            .await
        {
            // The delete may still land: released, the claim would let an ingest find these bytes and count on
            // them. Kept, the claim is taken over and the deletion finished once it is abandoned.
            if matches!(e, FileStorageError::Abandoned) {
                tracing::warn!(project_id = %project_id, hash, "Abandoned deleting orphan bytes; the claim stays");
                return false;
            }
            if let Err(release_error) = repo
                .release_deletion_claim(&project_id, &hash, &claim)
                .await
            {
                tracing::error!(
                    error = %e,
                    release_error = %release_error,
                    project_id = %project_id,
                    hash,
                    "Could not delete orphan bytes or release the claim; the file cannot be \
                     referenced again until it is cleared"
                );
                return false;
            }
            tracing::warn!(
                error = %e,
                project_id = %project_id,
                hash,
                "Could not delete orphan bytes; released the claim and left the file in place"
            );
            return false;
        }

        match repo
            .delete_file_if_unreferenced(&project_id, &hash, &claim)
            .await
        {
            Ok(true) => {
                delete_bytes_again(storage, &project_id, ObjectName::new(&hash, claim.incarnation)).await;
            }
            // Referenced again while its bytes were being deleted. Counting this as a success was
            // wrong twice over: nothing was cleaned, and the row now points at content that is gone.
            Ok(false) => {
                tracing::error!(
                    project_id = %project_id,
                    hash,
                    "Deleted a claimed file's bytes but its row is referenced again; it will read as \
                     missing content until re-ingested"
                );
                return false;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    hash,
                    "Deleted orphan bytes but could not delete the row; it will read as missing content"
                );
                return false;
            }
        }
        true
        }
        .await;
        if removed {
            reconcile_gc_usage(governance, &project_id).await;
        }
        release_gc_fence(governance, &project_id, &fence).await;
        if !removed {
            continue;
        }
        deleted += 1;
        tracing::debug!(project_id = %project_id, hash, "Deleted orphan file (ref_count=0)");
    }

    // Claims abandoned by a crash, resumed.
    //
    // A claim is durable on purpose - that is what makes it a fence - so a process that dies between
    // claiming and finishing leaves one set. Without resuming it the file is stuck forever: this sweep
    // sees a zero-reference row, tries to claim it, is refused by the claim already there, and skips it,
    // while every ingestion naming that file keeps failing its batch on the same fence.
    //
    // Resumed only after **re-taking the claim against the value that was read**, which is what makes the
    // byte deletion safe. The scan is a snapshot: between it and the delete the row can be released (by a
    // worker whose own object delete failed) or deleted and recreated by an ingestion that then associates
    // the same content hash - and deleting the bytes on the strength of the stale reading removes content a
    // committed span references. `delete_file_if_unreferenced` re-checks afterwards, which is too late: the
    // old code logged exactly that case at error level, and logging it is not preventing it.
    //
    // A successful reclaim also refreshes the claim, so a second worker holding the same stale reading is
    // refused rather than duplicating the work.
    match repo.get_stale_claimed_files(stale_claim_secs).await {
        Ok(stale) => {
            for (project_id, hash, observed) in stale {
                if !collection.is_open() {
                    tracing::debug!("Collection is fenced; abandoned claims wait");
                    return Ok(deleted);
                }
                let project_id = ProjectId::from(project_id);
                let fence = acquire_gc_fence(governance, &project_id).await;
                if matches!(fence, GcFence::Skip) {
                    continue;
                }
                let removed = async {
                let claim = match repo.reclaim_stale_file(&project_id, &hash, observed).await {
                    Ok(Some(claim)) => claim,
                    Ok(None) => {
                        tracing::debug!(
                            project_id = %project_id,
                            hash,
                            "An abandoned claim changed under us; leaving it to whoever holds it now"
                        );
                        return false;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            project_id = %project_id,
                            hash,
                            "Could not re-take an abandoned claim; will retry"
                        );
                        return false;
                    }
                };
                if let Err(e) = storage
                    .delete(&project_id, ObjectName::new(&hash, claim.incarnation))
                    .await
                {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        hash,
                        "Could not finish an abandoned deletion; will retry"
                    );
                    return false;
                }
                match repo
                    .delete_file_if_unreferenced(&project_id, &hash, &claim)
                    .await
                {
                    Ok(true) => {
                        delete_bytes_again(storage, &project_id, ObjectName::new(&hash, claim.incarnation))
                            .await;
                        tracing::debug!(project_id = %project_id, hash, "Finished an abandoned file deletion");
                        true
                    }
                    Ok(false) => {
                        tracing::error!(
                            project_id = %project_id,
                            hash,
                            "An abandoned deletion's file was referenced again after its bytes were \
                             removed; it will read as missing content until re-ingested"
                        );
                        false
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            project_id = %project_id,
                            hash,
                            "Could not delete the row of an abandoned deletion; will retry"
                        );
                        false
                    }
                }
                }
                .await;
                if removed {
                    reconcile_gc_usage(governance, &project_id).await;
                }
                release_gc_fence(governance, &project_id, &fence).await;
                if removed {
                    deleted += 1;
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "Could not look for abandoned file deletions"),
    }

    if deleted > 0 {
        tracing::debug!(deleted, "Orphan file cleanup complete (ref_count=0 files)");
    }

    Ok(deleted)
}

/// Whether the file at `path` holds `expected`: its length and BLAKE3 digest. None if it cannot be read.
async fn holds(path: &Path, expected: ExpectedContent) -> Option<bool> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = blake3::Hasher::new();
        let len = std::io::copy(&mut file, &mut hasher)?;
        Ok(len == expected.len && *hasher.finalize().as_bytes() == expected.digest)
    })
    .await
    .ok()?
    .ok()
}

/// The name of a write's temporary file: `{project_id}_{hash}.{length}.{digest}.{nonce}`, the digest the BLAKE3 hex
/// of the bytes the file must hold, the nonce unique to the write. Unique, so two writes of one content never share
/// a file; carrying the expected content, so a recovery after a crash checks the file before storing it.
pub fn temp_file_name(
    project_id: &str,
    hash: &str,
    expected: ExpectedContent,
    nonce: &str,
) -> String {
    format!(
        "{project_id}_{hash}.{}.{}.{nonce}",
        expected.len,
        expected.digest_hex()
    )
}

/// What a temporary file name says: its project, its hash, and - for a name [`temp_file_name`] made - the content
/// it must hold. A bare `{project_id}_{hash}`, as an earlier release named them, has no expected content.
fn parse_temp_file_name(filename: &str) -> Option<(String, String, Option<ExpectedContent>)> {
    let mut parts = filename.rsplitn(4, '.');
    let (nonce, digest, len, base) = (parts.next()?, parts.next(), parts.next(), parts.next());
    let (Some(digest), Some(len), Some(base)) = (digest, len, base) else {
        let (project_id, hash) = parse_temp_filename(filename)?;
        return Some((project_id, hash, None));
    };
    if nonce.is_empty() || !nonce.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let len = len.parse::<u64>().ok()?;
    let digest: [u8; 32] = hex::decode(digest).ok()?.try_into().ok()?;
    let (project_id, hash) = parse_temp_filename(base)?;
    Some((project_id, hash, Some(ExpectedContent { len, digest })))
}

/// Parse temp filename in format: {project_id}_{hash}
fn parse_temp_filename(filename: &str) -> Option<(String, String)> {
    // Hash is 64 hex chars, so look for underscore at len - 65
    if filename.len() < 66 {
        return None; // Too short
    }

    // Find the last underscore before the 64-char hash
    let hash_start = filename.len() - 64;
    if hash_start == 0 {
        return None; // No project_id
    }

    let separator_pos = hash_start - 1;
    if filename.as_bytes()[separator_pos] != b'_' {
        return None;
    }

    let project_id = &filename[..separator_pos];
    let hash = &filename[hash_start..];

    // Validate hash is 64 hex chars
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    // Validate project_id is not empty
    if project_id.is_empty() {
        return None;
    }

    Some((project_id.to_string(), hash.to_string()))
}

/// Statistics from cleanup operations
#[derive(Debug, Default)]
pub struct CleanupStats {
    /// Files finalized to permanent storage, or found stored already as this content
    pub finalized: u64,
    /// Orphan temp files deleted (no SQLite record)
    pub orphaned_deleted: u64,
    /// Temp files deleted unstored: a name of the wrong format, no expected content to check, or not its content
    pub invalid_deleted: u64,
    /// Files that failed to finalize
    pub finalize_failed: u64,
}

impl CleanupStats {
    /// Total files processed
    pub fn total_processed(&self) -> u64 {
        self.finalized + self.orphaned_deleted + self.invalid_deleted
    }
}

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod tests;
