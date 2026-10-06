use super::*;
use chrono::{TimeZone, Utc};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
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

    let cache_config = sideseat_core::config::CacheConfig {
        backend: sideseat_core::config::CacheBackendType::Memory,
        max_entries: 1000,
        eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
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
        .unwrap_or_else(|| app_storage.subdir(sideseat_core::storage::DataSubdir::Files));
    FileService::new(
        config,
        app_storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp),
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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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

#[tokio::test]
async fn restored_metadata_without_bytes_is_reported_as_unavailable() {
    let (temp_dir, database, cache) = setup_test().await;
    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = create_file_service(
        FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
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

    database
        .restore_orphan_metadata(
            &ProjectId::from("default"),
            &test_hash(),
            Some("image/png"),
            42,
            "sha256",
        )
        .await
        .expect("restore metadata");

    assert!(matches!(
        service
            .get_file(&ProjectId::from("default"), &test_hash())
            .await,
        Err(FileServiceError::ContentUnavailable { .. })
    ));
    assert!(matches!(
        service
            .get_file_metadata(&ProjectId::from("default"), &test_hash())
            .await,
        Err(FileServiceError::ContentUnavailable { .. })
    ));
}

#[tokio::test]
async fn restore_repair_rebuilds_associations_before_orphan_gc_and_reports_missing_bytes() {
    let (temp_dir, database, cache) = setup_test().await;
    let app_storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = create_file_service(
        FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
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

    let present = "c".repeat(64);
    let missing = "d".repeat(64);
    service
        .storage
        .store(&ProjectId::from("default"), &present, b"restored bytes")
        .await
        .expect("restore blob bytes");

    let analytics_dir = TempDir::new().expect("analytics temp dir");
    let analytics_storage = AppStorage::init_for_test(analytics_dir.path().to_path_buf());
    let analytics = DuckdbRepository(Arc::new(
        DuckdbService::init(&analytics_storage, Arc::new(TestClock))
            .await
            .expect("duckdb"),
    ));
    analytics
        .insert_spans(vec![sideseat_ports::types::NormalizedSpan {
            project_id: Some("default".to_owned()),
            trace_id: "restored-trace".to_owned(),
            span_id: "restored-span".to_owned(),
            timestamp_start: TestClock.now(),
            messages: Some(format!(
                r##"[{{"present":"#!B64!#image/png::{present}","missing":"#!B64!#::{missing}"}}]"##
            )),
            ..Default::default()
        }])
        .await
        .expect("restore analytics row");

    let report = service
        .repair_trace_associations_after_restore(
            &ProjectId::from("default"),
            &["restored-trace".to_owned()],
            &analytics,
        )
        .await
        .expect("repair associations");
    assert_eq!(report.metadata_rebuilt, 1);
    assert_eq!(report.associations_rebuilt, 1);
    assert_eq!(
        report.missing_content,
        vec![MissingFileReference {
            project_id: ProjectId::from("default"),
            trace_id: "restored-trace".to_owned(),
            hash: missing,
        }]
    );

    let metadata = database
        .get_file(&ProjectId::from("default"), &present)
        .await
        .expect("metadata read")
        .expect("metadata rebuilt");
    assert_eq!(metadata.ref_count, 1);
    assert_eq!(metadata.media_type.as_deref(), Some("image/png"));
    assert_eq!(
            database
                .get_file_hashes_for_traces(
                    &ProjectId::from("default"),
                    &["restored-trace".to_owned()],
                )
                .await
                .expect("association read"),
            vec![present.clone()]
        );

    let deleted = cleanup::cleanup_zero_ref_files(
        service.storage(),
        service.database(),
        sideseat_core::constants::FILE_DELETION_CLAIM_STALE_SECS,
    )
    .await
    .expect("orphan GC");
    assert_eq!(deleted, 0);
    assert!(
        service
            .file_exists(&ProjectId::from("default"), &present)
            .await
            .expect("blob exists"),
        "repair must rebuild the association before GC can claim the blob"
    );
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
            storage: sideseat_core::config::StorageBackend::Filesystem,
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
                .get_file_reference_counts_for_traces(project_id, std::slice::from_ref(&trace_id))
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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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
        async fn survivor_raw_records(
            &self,
            _project_id: &ProjectId,
            _trace_ids: &[String],
        ) -> Result<Vec<Vec<u8>>, sideseat_ports::error::DataError> {
            Ok(Vec::new())
        }

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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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
        storage: sideseat_core::config::StorageBackend::Filesystem,
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
