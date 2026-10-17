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
use sideseat_ports::blobs::ObjectName;
use sideseat_ports::clock::Clock;
use sideseat_ports::types::{NormalizedSpan, StagedPayload, StagedSignal};
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

    let report = replay_deletion_journal(
        &stores.database,
        &stores.analytics,
        &stores.files,
        std::time::Duration::ZERO,
    )
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

/// The incarnation a restored blob store holds the bytes of a file under when no restored row names one.
const BACKED_UP: i64 = 7;

/// Store `bytes` for `hash` as a restored blob store holds them: under its restored row's incarnation, or
/// under another when the transactional backup has no row for it.
async fn restored_blob(stores: &TestStores, project: &ProjectId, hash: &str, bytes: &[u8]) {
    let incarnation = stores
        .database
        .get_file(project, hash)
        .await
        .expect("row lookup")
        .map_or(BACKED_UP, |row| row.id);
    stores
        .files
        .storage()
        .store(project, ObjectName::new(hash, incarnation), bytes)
        .await
        .expect("restored blob");
}

/// Whether any incarnation of `hash`'s bytes is stored.
async fn stored(stores: &TestStores, project: &ProjectId, hash: &str) -> bool {
    !stores
        .files
        .storage()
        .incarnations(project, hash)
        .await
        .expect("blob lookup")
        .is_empty()
}

#[tokio::test]
async fn association_repair_reaches_a_fixed_point_before_gc() {
    let stores = stores().await;
    let present = "c".repeat(64);
    let missing = "d".repeat(64);
    let stale = "e".repeat(64);
    stores
        .database
        .restore_orphan_metadata(&ProjectId::from("default"), &stale, None, 5, "sha256")
        .await
        .expect("stale metadata");
    for (hash, bytes) in [
        (&present, b"live".as_slice()),
        (&stale, b"stale".as_slice()),
    ] {
        restored_blob(&stores, &ProjectId::from("default"), hash, bytes).await;
    }
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

    let first = reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
        .await
        .expect("first repair");
    assert_eq!(first.files.metadata_rebuilt, 1);
    assert_eq!(first.files.associations_rebuilt, 1);
    assert_eq!(
        first.files.missing_content,
        [MissingFileReference {
            project_id: ProjectId::from("default"),
            trace_id: "survivor".to_owned(),
            hash: missing.clone(),
        }]
    );
    assert!(stored(&stores, &ProjectId::from("default"), &present).await);
    assert!(!stored(&stores, &ProjectId::from("default"), &stale).await);
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

    stores
        .database
        .restore_orphan_metadata(&metadata_only, &metadata_blob, None, 11, "sha256")
        .await
        .expect("metadata without project");
    for (project_id, hash) in [
        (&analytics_only, &analytics_blob),
        (&metadata_only, &metadata_blob),
        (&staged_only, &staged_blob),
    ] {
        restored_blob(&stores, project_id, hash, b"unreachable").await;
    }
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
            raw_id: None,
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
        assert!(!stored(&stores, project_id, hash).await);
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

/// Which of a restore's reads a test makes fail.
#[derive(Clone, Copy)]
enum Failure {
    /// The trace traversal answers one full page, then fails: a partial discovery.
    SecondPage,
    /// A trace's repair cannot read its spans' reference fields.
    Repair,
}

/// The analytics store's survivor reads, but for the one `failure` names.
struct PartialDiscovery {
    inner: Arc<AnalyticsStore>,
    failure: Failure,
    pages: std::sync::atomic::AtomicUsize,
}

fn unavailable() -> DataError {
    DataError::Conflict("a replica cannot be read".to_owned())
}

#[async_trait::async_trait]
impl SurvivorReferences for PartialDiscovery {
    async fn file_reference_fields_for_traces(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<String>, DataError> {
        if matches!(self.failure, Failure::Repair) {
            return Err(unavailable());
        }
        self.inner
            .file_reference_fields_for_traces(project_id, trace_ids)
            .await
    }

    async fn survivor_raw_records(
        &self,
        project_id: &ProjectId,
        trace_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, DataError> {
        self.inner.survivor_raw_records(project_id, trace_ids).await
    }

    async fn stored_trace_ids_after(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        let page = self.pages.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match (self.failure, page) {
            (Failure::SecondPage, 0) => Ok((0..limit).map(|n| format!("trace-{n:04}")).collect()),
            (Failure::SecondPage, _) => Err(unavailable()),
            (Failure::Repair, _) => {
                self.inner
                    .stored_trace_ids_after(project_id, after, limit)
                    .await
            }
        }
    }

    async fn media_log_trace_ids(
        &self,
        project_id: &ProjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DataError> {
        self.inner
            .media_log_trace_ids(project_id, after, limit)
            .await
    }
}

/// A restore whose discovery stops early - a page it cannot read, a trace it cannot repair - fails before
/// its orphan sweep, which would otherwise collect a file whose only owner was a trace it never reached. The
/// orphan here has no association and no reference count, so a sweep that ran would delete it.
#[tokio::test]
async fn a_restore_whose_discovery_is_partial_collects_no_orphan() {
    for failure in [Failure::SecondPage, Failure::Repair] {
        let stores = stores().await;
        let project = ProjectId::from("default");
        let orphan = "9".repeat(64);
        stores
            .database
            .restore_orphan_metadata(&project, &orphan, None, 5, "sha256")
            .await
            .expect("metadata");
        restored_blob(
            &stores,
            &project,
            &orphan,
            b"named by a trace the restore never reached",
        )
        .await;
        stores
            .database
            .sync_ref_count(&project, &orphan)
            .await
            .expect("count");
        stores
            .analytics
            .insert_spans(vec![span("reached", "span", None)])
            .await
            .expect("a trace to repair");
        let survivors = PartialDiscovery {
            inner: Arc::clone(&stores.analytics),
            failure,
            pages: std::sync::atomic::AtomicUsize::new(0),
        };

        let outcome = reconcile_restored_associations_with(
            &stores.database,
            &stores.analytics,
            &survivors,
            &stores.files,
        )
        .await;

        assert!(outcome.is_err(), "a partial discovery fails the restore");
        assert!(
            stored(&stores, &project, &orphan).await,
            "the sweep did not run, so the orphan's bytes remain"
        );
        assert!(
            stores
                .database
                .get_file(&project, &orphan)
                .await
                .expect("metadata lookup")
                .is_some()
        );
    }
}

/// A file a stale association of one trace holds, and a live trace on a later page names without an
/// association, survives the restore: every trace's ownership is rebuilt before any is released. Releasing
/// page by page reclaimed the bytes when the stale trace's page came first, before the live one was reached.
#[tokio::test]
async fn a_file_a_later_page_names_survives_an_earlier_pages_release() {
    let stores = stores().await;
    let project = ProjectId::from("default");
    let shared = "8".repeat(64);
    stores
        .database
        .restore_orphan_metadata(&project, &shared, None, 5, "sha256")
        .await
        .expect("metadata");
    restored_blob(&stores, &project, &shared, b"named by the last trace").await;
    stores
        .database
        .restore_durable_trace_file(&project, "a-stale", &shared)
        .await
        .expect("the stale association");
    stores
        .database
        .sync_ref_count(&project, &shared)
        .await
        .expect("count");
    let last = TRACE_PAGE_SIZE + 40;
    let mut spans = vec![span("a-stale", "span", None)];
    spans.extend((0..last).map(|n| {
        span(
            &format!("t-{n:04}"),
            "span",
            (n + 1 == last).then(|| format!(r##"[{{"file":"#!B64!#image/png::{shared}"}}]"##)),
        )
    }));
    stores
        .analytics
        .insert_spans(spans)
        .await
        .expect("analytics restore");

    let report =
        reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
            .await
            .expect("repair");

    assert!(report.files.missing_content.is_empty(), "{report:?}");
    assert!(
        stored(&stores, &project, &shared).await,
        "the live trace's file kept its bytes"
    );
    let owners = stores
        .database
        .get_file_hashes_for_traces(&project, &[format!("t-{:04}", last - 1)])
        .await
        .expect("associations");
    assert_eq!(owners, vec![shared.clone()]);
    let file = stores
        .database
        .get_file(&project, &shared)
        .await
        .expect("file row")
        .expect("registered");
    assert_eq!(file.ref_count, 1, "owned by the live trace alone");
}

/// A file a journalled deletion's trace owns, and a live trace names without the association the backup
/// lost, survives the journal replay once ownership is rebuilt first; replayed before the rebuild, the
/// deleted trace's cleanup reclaimed it while the live trace still named it.
#[tokio::test]
async fn ownership_rebuilt_before_the_journal_keeps_a_file_a_live_trace_names() {
    for rebuild_first in [false, true] {
        let stores = stores().await;
        let project = ProjectId::from("default");
        let shared = "7".repeat(64);
        stores
            .database
            .restore_orphan_metadata(&project, &shared, None, 5, "sha256")
            .await
            .expect("metadata");
        restored_blob(&stores, &project, &shared, b"named by the live trace").await;
        stores
            .database
            .restore_durable_trace_file(&project, "deleted", &shared)
            .await
            .expect("the deleted trace's association");
        stores
            .database
            .sync_ref_count(&project, &shared)
            .await
            .expect("count");
        stores
            .analytics
            .insert_spans(vec![
                span("deleted", "span", None),
                span(
                    "live",
                    "span",
                    Some(format!(r##"[{{"file":"#!B64!#image/png::{shared}"}}]"##)),
                ),
            ])
            .await
            .expect("analytics restore");
        stores
            .database
            .append_deletions(&[DeletionRecord {
                project_id: project.clone(),
                cause: sideseat_ports::traits::DeletionCause::Requested,
                scope: DeletionScope::Trace,
                target_id: "deleted".to_owned(),
                span_id: None,
                recorded_at: TestClock.now(),
            }])
            .await
            .expect("journal");

        if rebuild_first {
            rebuild_restored_ownership(&stores.database, &stores.analytics, &stores.files)
                .await
                .expect("rebuild");
        }
        replay_deletion_journal(
            &stores.database,
            &stores.analytics,
            &stores.files,
            std::time::Duration::ZERO,
        )
        .await
        .expect("replay");

        let kept = stored(&stores, &project, &shared).await;
        assert_eq!(
            kept, rebuild_first,
            "rebuilt first: {rebuild_first}; the live trace's file kept its bytes: {kept}"
        );
    }
}

/// A trace whose only rows are logs is still reached: restored beside an older ownership snapshot that holds no
/// association for it, its media survives the orphan sweep because the repair finds the trace through its logs.
#[tokio::test]
async fn association_repair_reaches_a_trace_whose_only_rows_are_logs() {
    let stores = stores().await;
    let project = ProjectId::from("default");
    let hash = "f".repeat(64);
    let uri = format!("#!B64!#image/png::{hash}");
    stores
        .database
        .restore_orphan_metadata(&project, &hash, Some("image/png"), 9, "sha256")
        .await
        .expect("metadata with no owner");
    restored_blob(&stores, &project, &hash, b"log-media").await;
    stores
        .analytics
        .insert_logs(&[sideseat_ports::types::NormalizedLog {
            project_id: Some("default".to_owned()),
            log_digest: "a".repeat(64),
            trace_id: Some("log-only".to_owned()),
            timestamp: TestClock.now(),
            raw_log: serde_json::json!({"body": {"bytesValue": uri.clone()}}),
            media: vec![uri],
            ..Default::default()
        }])
        .await
        .expect("analytics restore");

    reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
        .await
        .expect("repair");

    assert!(
        stores
            .files
            .file_exists(&project, &hash)
            .await
            .expect("lookup"),
        "the orphan sweep collected media a surviving log names"
    );
}

/// A transactional backup taken while a collector held a file's claim, and a blob store holding the file under
/// its row's incarnation and another: once the repair finds surviving rows naming it, the file takes new
/// references again - the claim the backup caught is void - and the other incarnation is gone, not left as a copy
/// nothing accounts for or collects.
#[tokio::test]
async fn a_surviving_file_restored_mid_collection_is_unclaimed_and_kept_once() {
    let stores = stores().await;
    let project = ProjectId::from("default");
    let hash = "e".repeat(64);
    let row = stores
        .database
        .restore_orphan_metadata(&project, &hash, Some("image/png"), 5, "sha256")
        .await
        .expect("restored row");
    stores
        .database
        .claim_file_for_deletion(&project, &hash)
        .await
        .expect("claim")
        .expect("the backup caught the collector's claim");
    for incarnation in [row, row + 5] {
        stores
            .files
            .storage()
            .store(&project, ObjectName::new(&hash, incarnation), b"bytes")
            .await
            .expect("restored blob");
    }
    stores
        .analytics
        .insert_spans(vec![span(
            "survivor",
            "span",
            Some(format!(r##"[{{"image":"#!B64!#image/png::{hash}"}}]"##)),
        )])
        .await
        .expect("surviving row");

    stores
        .files
        .repair_trace_associations_after_restore(
            &project,
            &["survivor".to_owned()],
            stores.analytics.as_ref(),
        )
        .await
        .expect("repair");

    assert!(
        stores
            .database
            .associate_existing_file("later-trace", &project, &hash)
            .await
            .expect("association"),
        "the restored claim still refuses new references"
    );
    assert_eq!(
        stores
            .files
            .storage()
            .incarnations(&project, &hash)
            .await
            .expect("incarnations"),
        [row],
        "only the row's incarnation is kept"
    );
}

/// Bytes whose row and every reference the restore lost are collected, as is a stale incarnation beside a row's
/// own; the bytes a surviving span names are kept under their row, and the staging area's objects are not files.
#[tokio::test]
async fn objects_no_row_names_are_collected_and_named_ones_kept() {
    let stores = stores().await;
    let project = ProjectId::from("default");
    let named = "c".repeat(64);
    let unnamed = "f".repeat(64);
    let staged = "9".repeat(64);
    restored_blob(&stores, &project, &named, b"live").await;
    let storage = stores.files.storage();
    storage
        .store(&project, ObjectName::new(&unnamed, 7), b"lost")
        .await
        .unwrap();
    storage
        .store(&project, ObjectName::new(&staged, 0), b"staged export")
        .await
        .unwrap();
    stores
        .analytics
        .insert_spans(vec![span(
            "survivor",
            "span",
            Some(format!(r##"[{{"image":"#!B64!#image/png::{named}"}}]"##)),
        )])
        .await
        .unwrap();

    let first = reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
        .await
        .expect("repair");
    let row = stores
        .database
        .get_file(&project, &named)
        .await
        .unwrap()
        .expect("the named file's row is rebuilt");
    // A stale incarnation beside the row's own, as a lost timeline's later ingest leaves one.
    storage
        .store(&project, ObjectName::new(&named, row.id + 50), b"live")
        .await
        .unwrap();
    reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
        .await
        .expect("repair again");
    let fixed = reconcile_restored_associations(&stores.database, &stores.analytics, &stores.files)
        .await
        .expect("fixed point");

    assert_eq!(first.unnamed_objects_deleted, 1);
    assert_eq!(fixed.unnamed_objects_deleted, 0);
    assert!(!stored(&stores, &project, &unnamed).await);
    assert_eq!(
        storage.incarnations(&project, &named).await.unwrap(),
        [row.id]
    );
    assert_eq!(
        storage
            .get(&project, ObjectName::new(&named, row.id))
            .await
            .unwrap(),
        b"live"
    );
    assert!(
        storage
            .exists(&project, ObjectName::new(&staged, 0))
            .await
            .unwrap()
    );
}
