//! Cross-store repair after independently restored backups.
//!
//! There is deliberately no claim here that the stores share a backup watermark. The procedure replays the
//! durable deletion facts it still has, reconstructs ownership from surviving analytics rows, releases
//! ownership with no survivor, and only then permits orphan collection.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;

use crate::content_bodies::{
    ContentBodyError, ContentBodyRestoreCleanupReport, ContentBodyService,
};
use crate::files::cleanup::{cleanup_orphan_temp_files, cleanup_zero_ref_files_governed};
use crate::files::{FileRestoreRepairReport, FileService, FileServiceError, MissingFileReference};
use sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS;
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{
    AnalyticsRepository, DeletionRecord, DeletionScope, TransactionalRepository,
};
use sideseat_ports::types::{ListTracesParams, ProjectId};

const JOURNAL_PAGE_SIZE: usize = 256;
const TRACE_PAGE_SIZE: u32 = 256;

type TransactionalStore = dyn TransactionalRepository + Send + Sync;
type AnalyticsStore = dyn AnalyticsRepository + Send + Sync;

#[derive(Debug, Error)]
pub enum RestoreRepairError {
    #[error(transparent)]
    Data(#[from] DataError),
    #[error(transparent)]
    File(#[from] FileServiceError),
    #[error(transparent)]
    ContentBody(#[from] ContentBodyError),
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
    pub content_bodies: ContentBodyRestoreCleanupReport,
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
            ContentBodyService::from_file_service(files)
                .cleanup_spans(&record.project_id, &spans)
                .await?;
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
    let temp =
        cleanup_orphan_temp_files(files.temp_dir(), files.storage(), files.database()).await?;
    let mut report = AssociationRepairReport {
        temp_files_processed: temp.total_processed(),
        ..AssociationRepairReport::default()
    };
    let bodies = ContentBodyService::from_file_service(files);

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

        let mut trace_page = 1;
        loop {
            let (traces, _) = analytics
                .list_traces(&ListTracesParams {
                    project_id: project_id.clone(),
                    page: trace_page,
                    limit: TRACE_PAGE_SIZE,
                    include_nongenai: true,
                    ..ListTracesParams::default()
                })
                .await?;
            if traces.is_empty() {
                break;
            }
            let trace_ids = traces
                .iter()
                .map(|trace| trace.trace_id.clone())
                .collect::<Vec<_>>();
            report.analytics_traces_scanned += trace_ids.len() as u64;
            reconcile_trace_batch(
                &project_id,
                &trace_ids,
                analytics,
                files,
                &bodies,
                &mut report.files,
            )
            .await?;
            if traces.len() < TRACE_PAGE_SIZE as usize {
                break;
            }
            trace_page += 1;
        }

        let mut after = None::<String>;
        loop {
            let trace_ids = database
                .restore_association_trace_ids(
                    &project_id,
                    after.as_deref(),
                    TRACE_PAGE_SIZE as usize,
                )
                .await?;
            if trace_ids.is_empty() {
                break;
            }
            report.ownership_traces_scanned += trace_ids.len() as u64;
            reconcile_trace_batch(
                &project_id,
                &trace_ids,
                analytics,
                files,
                &bodies,
                &mut report.files,
            )
            .await?;
            after = trace_ids.last().cloned();
            if trace_ids.len() < TRACE_PAGE_SIZE as usize {
                break;
            }
        }
    }

    report.content_bodies = bodies.cleanup_orphans_after_restore().await?;
    report.orphan_files_deleted = cleanup_zero_ref_files_governed(
        files.storage(),
        files.database(),
        FILE_DELETION_CLAIM_STALE_SECS,
        files.governance(),
    )
    .await?;
    normalize_missing_content(&mut report.files.missing_content);
    Ok(report)
}

async fn reconcile_trace_batch(
    project_id: &ProjectId,
    trace_ids: &[String],
    analytics: &Arc<AnalyticsStore>,
    files: &FileService,
    bodies: &ContentBodyService,
    file_report: &mut FileRestoreRepairReport,
) -> Result<(), RestoreRepairError> {
    let repaired = files
        .repair_trace_associations_after_restore(project_id, trace_ids, analytics.as_ref())
        .await?;
    merge_file_report(file_report, repaired);
    bodies
        .reconcile_trace_survivors(project_id, trace_ids, analytics.as_ref())
        .await?;
    files
        .reconcile_trace_survivors(project_id, trace_ids, analytics.as_ref())
        .await?;
    Ok(())
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
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_cache::CacheService;
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::config::{
        CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, StorageBackend,
    };
    use sideseat_core::storage::{AppStorage, DataSubdir};
    use sideseat_ports::blobs::FileStorage;
    use sideseat_ports::clock::Clock;
    use sideseat_ports::types::{ContentBodyObject, NormalizedSpan, StagedPayload, StagedSignal};
    use tempfile::TempDir;

    #[derive(Debug)]
    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
        }
    }

    struct TestStores {
        _root: TempDir,
        database: Arc<TransactionalStore>,
        analytics: Arc<AnalyticsStore>,
        files: FileService,
    }

    async fn stores() -> TestStores {
        let root = TempDir::new().expect("temp dir");
        let app_storage = AppStorage::init_for_test(root.path().to_path_buf());
        let clock: Arc<dyn Clock> = Arc::new(TestClock);
        let sqlite = Arc::new(
            SqliteService::init(&app_storage, Arc::clone(&clock))
                .await
                .expect("sqlite"),
        );
        let database: Arc<TransactionalStore> = Arc::new(SqliteRepository(Arc::clone(&sqlite)));
        let duckdb = Arc::new(
            DuckdbService::init(&app_storage, Arc::clone(&clock))
                .await
                .expect("duckdb"),
        );
        let analytics: Arc<AnalyticsStore> = Arc::new(DuckdbRepository(duckdb));
        let cache = Arc::new(
            CacheService::new(&CacheConfig {
                backend: CacheBackendType::Memory,
                max_entries: 100,
                eviction_policy: EvictionPolicy::TinyLfu,
                redis_url: None,
            })
            .await
            .expect("cache"),
        );
        let storage: Arc<dyn FileStorage> = Arc::new(FilesystemStorage::new(
            app_storage.subdir(DataSubdir::Files),
        ));
        let files = FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 1024 * 1024,
                filesystem_path: None,
                s3: None,
            },
            app_storage.subdir(DataSubdir::FilesTemp),
            storage,
            Arc::clone(&database),
            cache,
        )
        .await
        .expect("files");
        TestStores {
            _root: root,
            database,
            analytics,
            files,
        }
    }

    fn span(trace_id: &str, span_id: &str, messages: Option<String>) -> NormalizedSpan {
        NormalizedSpan {
            project_id: Some("default".to_owned()),
            trace_id: trace_id.to_owned(),
            span_id: span_id.to_owned(),
            timestamp_start: TestClock.now(),
            messages,
            ..NormalizedSpan::default()
        }
    }

    #[tokio::test]
    async fn journal_replay_removes_a_trace_resurrected_by_the_analytics_restore() {
        let stores = stores().await;
        stores
            .analytics
            .insert_spans(vec![span("resurrected", "span", None)])
            .await
            .expect("analytics restore");
        stores
            .database
            .append_deletions(&[DeletionRecord {
                project_id: ProjectId::from("default"),
                cause: sideseat_ports::traits::DeletionCause::Requested,
                scope: DeletionScope::Trace,
                target_id: "resurrected".to_owned(),
                span_id: None,
                recorded_at: TestClock.now(),
            }])
            .await
            .expect("newer transactional backup");

        let report = replay_deletion_journal(&stores.database, &stores.analytics, &stores.files)
            .await
            .expect("replay");

        assert_eq!(report.entries_applied, 1);
        assert_eq!(report.traces_applied, 1);
        assert!(
            stores
                .analytics
                .get_trace(&ProjectId::from("default"), "resurrected")
                .await
                .expect("trace lookup")
                .is_none()
        );
    }

    #[tokio::test]
    async fn association_repair_reaches_a_fixed_point_before_gc() {
        let stores = stores().await;
        let present = "c".repeat(64);
        let missing = "d".repeat(64);
        let stale = "e".repeat(64);
        let orphan_body = ContentBodyService::hash(b"orphan body");
        for (hash, bytes) in [
            (&present, b"live".as_slice()),
            (&stale, b"stale".as_slice()),
            (&orphan_body, b"orphan body".as_slice()),
        ] {
            stores
                .files
                .storage()
                .store(&ProjectId::from("default"), hash, bytes)
                .await
                .expect("restored blob");
        }
        stores
            .database
            .restore_orphan_metadata(&ProjectId::from("default"), &stale, None, 5, "sha256")
            .await
            .expect("stale metadata");
        stores
            .database
            .restore_durable_trace_file(&ProjectId::from("default"), "gone", &stale)
            .await
            .expect("stale ownership");
        stores
            .database
            .sync_ref_count(&ProjectId::from("default"), &stale)
            .await
            .expect("stale count");
        stores
            .database
            .register_content_bodies(&[ContentBodyObject {
                project_id: ProjectId::from("default"),
                body_hash: orphan_body.clone(),
                logical_bytes: 11,
            }])
            .await
            .expect("orphan body metadata");
        stores
            .analytics
            .insert_spans(vec![span(
                "survivor",
                "span",
                Some(format!(
                    r##"[{{"present":"#!B64!#image/png::{present}","missing":"#!B64!#::{missing}"}}]"##
                )),
            )])
            .await
            .expect("analytics restore");

        let first =
            reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
                .await
                .expect("first repair");
        assert_eq!(first.files.metadata_rebuilt, 1);
        assert_eq!(first.files.associations_rebuilt, 1);
        assert_eq!(first.content_bodies.orphans_deleted, 1);
        assert_eq!(
            first.files.missing_content,
            [MissingFileReference {
                project_id: ProjectId::from("default"),
                trace_id: "survivor".to_owned(),
                hash: missing.clone(),
            }]
        );
        assert!(
            stores
                .files
                .storage()
                .exists(&ProjectId::from("default"), &present)
                .await
                .expect("live blob lookup")
        );
        assert!(
            !stores
                .files
                .storage()
                .exists(&ProjectId::from("default"), &stale)
                .await
                .expect("stale blob lookup")
        );
        assert!(
            !stores
                .files
                .storage()
                .exists(&ProjectId::from("default"), &orphan_body)
                .await
                .expect("orphan body lookup")
        );
        assert!(
            stores
                .database
                .get_file(&ProjectId::from("default"), &stale)
                .await
                .expect("stale metadata lookup")
                .is_none()
        );

        let second =
            reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
                .await
                .expect("fixed point");
        assert_eq!(second.files.metadata_rebuilt, 0);
        assert_eq!(second.files.associations_rebuilt, 0);
        assert_eq!(second.content_bodies.orphans_deleted, 0);
        assert_eq!(second.orphan_files_deleted, 0);
        assert_eq!(second.files.missing_content.len(), 1);
    }

    #[tokio::test]
    async fn association_repair_removes_projects_missing_from_transactional_restore() {
        let stores = stores().await;
        let analytics_only = ProjectId::from("analytics-only");
        let metadata_only = ProjectId::from("metadata-only");
        let staged_only = ProjectId::from("staged-only");
        let analytics_blob = "a".repeat(64);
        let metadata_blob = "b".repeat(64);
        let staged_blob = "f".repeat(64);

        for (project_id, hash) in [
            (&analytics_only, &analytics_blob),
            (&metadata_only, &metadata_blob),
            (&staged_only, &staged_blob),
        ] {
            stores
                .files
                .storage()
                .store(project_id, hash, b"unreachable")
                .await
                .expect("unreachable blob");
        }
        stores
            .database
            .restore_orphan_metadata(&metadata_only, &metadata_blob, None, 11, "sha256")
            .await
            .expect("metadata without project");
        let mut unreachable = span(
            "unreachable-trace",
            "span",
            Some(format!(r##"[{{"file":"#!B64!#::{analytics_blob}"}}]"##)),
        );
        unreachable.project_id = Some(analytics_only.to_string());
        stores
            .analytics
            .insert_spans(vec![unreachable])
            .await
            .expect("analytics project without metadata");
        stores
            .database
            .create_staged_payload(&StagedPayload {
                id: "restored-staged-payload".to_owned(),
                project_id: staged_only.clone(),
                signal: StagedSignal::Traces,
                blob_hash: staged_blob.clone(),
                byte_len: 11,
                created_at: TestClock.now(),
                redrive_attempts: 0,
                unconfirmed: false,
                records: Vec::new(),
            })
            .await
            .expect("staged payload without project");

        let report =
            reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
                .await
                .expect("repair");

        assert_eq!(report.projects_without_metadata_deleted, 3);
        assert_eq!(report.unreachable_analytics_rows_deleted, 1);
        assert_eq!(report.unreachable_staged_payloads_deleted, 1);
        assert_eq!(report.unreachable_blobs_deleted, 3);
        assert!(
            stores
                .analytics
                .get_trace(&analytics_only, "unreachable-trace")
                .await
                .expect("unreachable trace lookup")
                .is_none()
        );
        assert!(
            stores
                .database
                .get_file(&metadata_only, &metadata_blob)
                .await
                .expect("unreachable metadata lookup")
                .is_none()
        );
        for (project_id, hash) in [
            (&analytics_only, &analytics_blob),
            (&metadata_only, &metadata_blob),
            (&staged_only, &staged_blob),
        ] {
            assert!(
                !stores
                    .files
                    .storage()
                    .exists(project_id, hash)
                    .await
                    .expect("unreachable blob lookup")
            );
        }
        assert!(
            stores
                .database
                .get_staged_payload("restored-staged-payload")
                .await
                .expect("staged payload lookup")
                .is_none()
        );

        let second =
            reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
                .await
                .expect("fixed point");
        assert_eq!(second.projects_without_metadata_deleted, 0);
        assert_eq!(second.unreachable_analytics_rows_deleted, 0);
        assert_eq!(second.unreachable_staged_payloads_deleted, 0);
        assert_eq!(second.unreachable_blobs_deleted, 0);
    }
}
