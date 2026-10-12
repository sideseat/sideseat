//! Cross-store repair after independently restored backups.
//!
//! There is deliberately no claim here that the stores share a backup watermark. The procedure replays the
//! durable deletion facts it still has, reconstructs ownership from surviving analytics rows, releases
//! ownership with no survivor, and only then permits orphan collection.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;

use crate::files::cleanup::{cleanup_orphan_temp_files, cleanup_zero_ref_files_governed};
use crate::files::{FileRestoreRepairReport, FileService, FileServiceError, MissingFileReference};
use sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS;
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    AnalyticsRepository, DeletionRecord, DeletionScope, SurvivorReferences, TransactionalRepository,
};
use sideseat_ports::types::ProjectId;

const JOURNAL_PAGE_SIZE: usize = 256;
const TRACE_PAGE_SIZE: usize = 256;

type TransactionalStore = dyn TransactionalRepository + Send + Sync;
type AnalyticsStore = dyn AnalyticsRepository + Send + Sync;

#[derive(Debug, Error)]
pub enum RestoreRepairError {
    #[error(transparent)]
    Data(#[from] DataError),
    #[error(transparent)]
    File(#[from] FileServiceError),
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct JournalReplayReport {
    pub highest_sequence_examined: i64,
    pub entries_applied: u64,
    pub traces_applied: u64,
    pub sessions_applied: u64,
    pub spans_applied: u64,
    pub projects_applied: u64,
    pub organizations_applied: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct AssociationRepairReport {
    pub projects_scanned: u64,
    pub analytics_traces_scanned: u64,
    pub ownership_traces_scanned: u64,
    pub projects_without_metadata_deleted: u64,
    pub unreachable_analytics_rows_deleted: u64,
    pub unreachable_staged_payloads_deleted: u64,
    pub unreachable_blobs_deleted: u64,
    pub temp_files_processed: u64,
    pub orphan_files_deleted: u64,
    pub files: FileRestoreRepairReport,
}

/// Replay every deletion still present in the restored transactional store.
///
/// The cursor advances by the highest sequence *examined*, not by entries returned. A newer build may have
/// written a journal vocabulary this build does not understand; adapters omit those rows but still advance
/// the examined cursor, so known entries behind them remain reachable.
pub async fn replay_deletion_journal(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    files: &FileService,
) -> Result<JournalReplayReport, RestoreRepairError> {
    let mut report = JournalReplayReport::default();
    loop {
        let (entries, examined) = database
            .deletions_since(report.highest_sequence_examined, JOURNAL_PAGE_SIZE)
            .await?;
        if examined == report.highest_sequence_examined {
            break;
        }
        for (_, record) in entries {
            apply_deletion(&record, database, analytics, files, &mut report).await?;
            report.entries_applied += 1;
        }
        report.highest_sequence_examined = examined;
    }
    Ok(report)
}

async fn apply_deletion(
    record: &DeletionRecord,
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    files: &FileService,
    report: &mut JournalReplayReport,
) -> Result<(), RestoreRepairError> {
    match record.scope {
        DeletionScope::Trace => {
            let traces = [record.target_id.clone()];
            analytics.delete_traces(&record.project_id, &traces).await?;
            files.cleanup_traces(&record.project_id, &traces).await?;
            report.traces_applied += 1;
        }
        DeletionScope::Session => {
            let sessions = [record.target_id.clone()];
            let traces = analytics
                .delete_sessions(&record.project_id, &sessions)
                .await?;
            files.cleanup_traces(&record.project_id, &traces).await?;
            report.sessions_applied += 1;
        }
        DeletionScope::Span => {
            let Some(span_id) = record.span_id.as_ref() else {
                return Ok(());
            };
            let spans = [(record.target_id.clone(), span_id.clone())];
            analytics.delete_spans(&record.project_id, &spans).await?;
            files
                .reconcile_trace_survivors(
                    &record.project_id,
                    std::slice::from_ref(&record.target_id),
                    analytics.as_ref(),
                )
                .await?;
            report.spans_applied += 1;
        }
        DeletionScope::Project => {
            let project_id = ProjectId::from(record.target_id.as_str());
            analytics.delete_project_data(&project_id).await?;
            files.delete_project(&project_id).await?;
            report.projects_applied += 1;
        }
        DeletionScope::Organization => {
            for project_id in database.list_project_ids(&record.target_id).await? {
                let project_id = ProjectId::from(project_id);
                analytics.delete_project_data(&project_id).await?;
                files.delete_project(&project_id).await?;
            }
            report.organizations_applied += 1;
        }
    }
    Ok(())
}

/// Reconcile both directions of ownership drift, then collect uploaded-file orphans.
///
/// The analytics traversal finds missing ownership; the transactional traversal finds durable ownership whose
/// analytics row disappeared. They intentionally overlap: every operation is idempotent and a second pass
/// reaching zero changes is the restore procedure's fixed-point check.
pub async fn reconcile_restored_associations(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    files: &FileService,
) -> Result<AssociationRepairReport, RestoreRepairError> {
    reconcile_restored_associations_with(database, analytics, analytics.as_ref(), files).await
}

/// That every stored trace of every project was found and its ownership repaired: what the orphan sweep needs
/// before it may run.
///
/// The association table is the only index from a file to the rows that name it, and a restored backup may be
/// missing exactly the associations a live span needs. The repair rebuilds them from the traces the analytics
/// traversal finds - `SurvivorReferences::stored_trace_ids_after`, which reads every replica and fails rather
/// than answer from part of them - so after a complete traversal every file a stored span or record names is
/// owned again, and the sweep's own claim, which asks the associations, keeps it. A traversal that stops early
/// returns its error before this exists, and the sweep takes it by value, so nothing collects orphans on the
/// strength of a partial one.
struct DiscoveryComplete(());

/// The orphan sweep that ends a restore, once every trace was found and repaired.
async fn sweep_restored_orphans(
    _discovered: DiscoveryComplete,
    files: &FileService,
) -> Result<u64, RestoreRepairError> {
    Ok(cleanup_zero_ref_files_governed(
        files.storage(),
        files.database(),
        FILE_DELETION_CLAIM_STALE_SECS,
        files.governance(),
    )
    .await?)
}

/// `reconcile_restored_associations`, with the reads that find and repair each trace's ownership taken from
/// `survivors` - the analytics store's own, but for a test that makes one of them fail.
async fn reconcile_restored_associations_with(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    survivors: &dyn SurvivorReferences,
    files: &FileService,
) -> Result<AssociationRepairReport, RestoreRepairError> {
    let temp =
        cleanup_orphan_temp_files(files.temp_dir(), files.storage(), files.database()).await?;
    let mut report = AssociationRepairReport {
        temp_files_processed: temp.total_processed(),
        ..AssociationRepairReport::default()
    };
    let mut projects = database
        .restore_project_ids(usize::MAX)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    projects.extend(analytics.analytics_project_ids(usize::MAX).await?);

    for project_id in projects {
        report.projects_scanned += 1;
        if database.get_project(project_id.as_str()).await?.is_none() {
            report.projects_without_metadata_deleted += 1;
            report.unreachable_analytics_rows_deleted +=
                analytics.count_project_rows(&project_id).await?;
            let _ = analytics.delete_project_data(&project_id).await?;
            report.unreachable_staged_payloads_deleted +=
                database.delete_project_staged_payloads(&project_id).await?;
            report.unreachable_blobs_deleted += files.delete_project(&project_id).await?;
            continue;
        }

        // Every trace's ownership is rebuilt before any is released: a file a stale association of one trace
        // holds may be named by a live trace a later page reaches, and releasing it first reclaimed its bytes
        // before that trace was repaired.
        repair_project_ownership(&project_id, database, survivors, files, &mut report).await?;
        // Then what no survivor names is released. Only a trace that holds an association has any to release.
        visit_owning_traces(
            OwningStep::Release,
            &project_id,
            database,
            survivors,
            files,
            &mut report.files,
        )
        .await?;
    }

    report.orphan_files_deleted = sweep_restored_orphans(DiscoveryComplete(()), files).await?;
    normalize_missing_content(&mut report.files.missing_content);
    Ok(report)
}

/// Rebuild the file ownership the restored analytics rows name, and release and collect nothing: the first step
/// of a restore repair, before the deletion journal and retention reclaim files.
///
/// Those passes reclaim a file once the associations of the traces they delete are gone. A restored
/// transactional backup can lack the association a live trace holds on the same file, so without this the file
/// was reclaimed by a deleted trace's cleanup while the live trace still named it. Rebuilt first, the live
/// trace's association keeps the file counted through both.
pub async fn rebuild_restored_ownership(
    database: &Arc<TransactionalStore>,
    analytics: &Arc<AnalyticsStore>,
    files: &FileService,
) -> Result<AssociationRepairReport, RestoreRepairError> {
    let mut report = AssociationRepairReport::default();
    let mut projects = database
        .restore_project_ids(usize::MAX)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    projects.extend(analytics.analytics_project_ids(usize::MAX).await?);
    for project_id in projects {
        report.projects_scanned += 1;
        if database.get_project(project_id.as_str()).await?.is_none() {
            continue;
        }
        repair_project_ownership(
            &project_id,
            database,
            analytics.as_ref(),
            files,
            &mut report,
        )
        .await?;
    }
    normalize_missing_content(&mut report.files.missing_content);
    Ok(report)
}

/// Rebuild one project's ownership from both traversals: the traces its stored rows name, read from every
/// copy, and the traces that hold an association.
async fn repair_project_ownership(
    project_id: &ProjectId,
    database: &Arc<TransactionalStore>,
    survivors: &dyn SurvivorReferences,
    files: &FileService,
    report: &mut AssociationRepairReport,
) -> Result<(), RestoreRepairError> {
    let mut after = None::<String>;
    loop {
        let trace_ids = survivors
            .stored_trace_ids_after(project_id, after.as_deref(), TRACE_PAGE_SIZE)
            .await?;
        if trace_ids.is_empty() {
            break;
        }
        report.analytics_traces_scanned += trace_ids.len() as u64;
        repair_trace_batch(project_id, &trace_ids, survivors, files, &mut report.files).await?;
        after = trace_ids.last().cloned();
        if trace_ids.len() < TRACE_PAGE_SIZE {
            break;
        }
    }
    // Traces whose only rows are logs: no span lists them, and an older ownership snapshot may hold no row for
    // them either, so the media their logs name is found here or the orphan sweep collects it.
    let mut after = None::<String>;
    loop {
        let trace_ids = survivors
            .media_log_trace_ids(project_id, after.as_deref(), TRACE_PAGE_SIZE)
            .await?;
        if trace_ids.is_empty() {
            break;
        }
        report.analytics_traces_scanned += trace_ids.len() as u64;
        repair_trace_batch(project_id, &trace_ids, survivors, files, &mut report.files).await?;
        after = trace_ids.last().cloned();
        if trace_ids.len() < TRACE_PAGE_SIZE {
            break;
        }
    }
    report.ownership_traces_scanned += visit_owning_traces(
        OwningStep::Repair,
        project_id,
        database,
        survivors,
        files,
        &mut report.files,
    )
    .await?;
    Ok(())
}

async fn repair_trace_batch(
    project_id: &ProjectId,
    trace_ids: &[String],
    survivors: &dyn SurvivorReferences,
    files: &FileService,
    file_report: &mut FileRestoreRepairReport,
) -> Result<(), RestoreRepairError> {
    let repaired = files
        .repair_trace_associations_after_restore(project_id, trace_ids, survivors)
        .await?;
    merge_file_report(file_report, repaired);
    Ok(())
}

/// What a pass over the traces that hold an association does to each page of them.
#[derive(Clone, Copy)]
enum OwningStep {
    /// Rebuild the ownership their stored rows name.
    Repair,
    /// Release the ownership no stored row names.
    Release,
}

/// `step` over every page of the traces that hold an association in the project, in order; the traces visited.
async fn visit_owning_traces(
    step: OwningStep,
    project_id: &ProjectId,
    database: &Arc<TransactionalStore>,
    survivors: &dyn SurvivorReferences,
    files: &FileService,
    file_report: &mut FileRestoreRepairReport,
) -> Result<u64, RestoreRepairError> {
    let mut visited = 0u64;
    let mut after = None::<String>;
    loop {
        let trace_ids = database
            .restore_association_trace_ids(project_id, after.as_deref(), TRACE_PAGE_SIZE)
            .await?;
        if trace_ids.is_empty() {
            break;
        }
        visited += trace_ids.len() as u64;
        match step {
            OwningStep::Repair => {
                repair_trace_batch(project_id, &trace_ids, survivors, files, file_report).await?
            }
            OwningStep::Release => {
                files
                    .reconcile_trace_survivors(project_id, &trace_ids, survivors)
                    .await?
            }
        }
        after = trace_ids.last().cloned();
        if trace_ids.len() < TRACE_PAGE_SIZE {
            break;
        }
    }
    Ok(visited)
}

fn merge_file_report(target: &mut FileRestoreRepairReport, source: FileRestoreRepairReport) {
    target.traces_scanned += source.traces_scanned;
    target.references_scanned += source.references_scanned;
    target.metadata_rebuilt += source.metadata_rebuilt;
    target.associations_rebuilt += source.associations_rebuilt;
    target.missing_content.extend(source.missing_content);
}

fn normalize_missing_content(missing: &mut Vec<MissingFileReference>) {
    missing.sort_by(|left, right| {
        (&left.project_id, &left.trace_id, &left.hash).cmp(&(
            &right.project_id,
            &right.trace_id,
            &right.hash,
        ))
    });
    missing.dedup();
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
