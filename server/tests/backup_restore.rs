//! Gated proof that the embedded backup procedure can be restored and repaired.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde_json::Value;
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::config::{
    CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, StorageBackend,
};
use sideseat_core::constants::{DUCKDB_DB_FILENAME, RESTORE_PENDING_MARKER, SQLITE_DB_FILENAME};
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_domain::files::{FileService, FileServiceError};
use sideseat_ports::blobs::FileStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{
    AnalyticsRepository, DeletionCause, DeletionRecord, DeletionScope, TransactionalRepository,
};
use sideseat_ports::types::{NormalizedSpan, ProjectId};
use tempfile::TempDir;

#[derive(Debug)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

#[tokio::test]
async fn backup_destroy_restore_repair_preserves_only_the_right_data() {
    if std::env::var_os("SIDESEAT_RUN_BACKUP_RESTORE_TEST").is_none() {
        eprintln!("skipped: set SIDESEAT_RUN_BACKUP_RESTORE_TEST=1");
        return;
    }

    let root = TempDir::new().expect("temp root");
    let live = root.path().join("live");
    let backup = root.path().join("backup");
    fs::create_dir_all(&backup).expect("backup directory");
    let present = "a".repeat(64);
    let missing = "b".repeat(64);

    {
        let storage = AppStorage::init_for_test(live.clone());
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let sqlite = Arc::new(
            SqliteService::init(&storage, Arc::clone(&clock))
                .await
                .expect("sqlite"),
        );
        let database: Arc<dyn TransactionalRepository + Send + Sync> =
            Arc::new(SqliteRepository(Arc::clone(&sqlite)));
        let duckdb = Arc::new(
            DuckdbService::init(&storage, Arc::clone(&clock))
                .await
                .expect("duckdb"),
        );
        let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
            Arc::new(DuckdbRepository(Arc::clone(&duckdb)));
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
        let blob_storage: Arc<dyn FileStorage> =
            Arc::new(FilesystemStorage::new(storage.subdir(DataSubdir::Files)));
        let files = FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 1024 * 1024 * 1024,
                filesystem_path: None,
                s3: None,
            },
            storage.subdir(DataSubdir::FilesTemp),
            Arc::clone(&blob_storage),
            Arc::clone(&database),
            cache,
        )
        .await
        .expect("files");

        blob_storage
            .store(&ProjectId::from("default"), &present, b"surviving bytes")
            .await
            .expect("present blob");
        database
            .restore_orphan_metadata(
                &ProjectId::from("default"),
                &missing,
                Some("image/png"),
                123,
                "sha256",
            )
            .await
            .expect("missing metadata");
        database
            .restore_durable_trace_file(&ProjectId::from("default"), "survivor", &missing)
            .await
            .expect("missing ownership");
        database
            .sync_ref_count(&ProjectId::from("default"), &missing)
            .await
            .expect("missing ref count");

        let now = Utc::now();
        analytics
            .insert_spans(vec![
                span(
                    "survivor",
                    "live",
                    now,
                    Some(format!(
                        r##"[{{"present":"#!B64!#image/png::{present}","missing":"#!B64!#image/png::{missing}"}}]"##
                    )),
                ),
                span("aged", "old", now - Duration::hours(2), None),
                span("requested-delete", "gone", now, None),
            ])
            .await
            .expect("analytics rows");

        // Analytics backup predates the requested deletion.
        duckdb.checkpoint().await.expect("duckdb checkpoint");
        fs::copy(
            storage.subdir_path(DataSubdir::Duckdb, DUCKDB_DB_FILENAME),
            backup.join(DUCKDB_DB_FILENAME),
        )
        .expect("duckdb backup");

        // Transactional backup is newer and contains the deletion journal.
        database
            .append_deletions(&[DeletionRecord {
                project_id: ProjectId::from("default"),
                cause: DeletionCause::Requested,
                scope: DeletionScope::Trace,
                target_id: "requested-delete".to_owned(),
                span_id: None,
                recorded_at: now,
            }])
            .await
            .expect("journal");
        sqlite.checkpoint().await.expect("sqlite checkpoint");
        fs::copy(
            storage.subdir_path(DataSubdir::Sqlite, SQLITE_DB_FILENAME),
            backup.join(SQLITE_DB_FILENAME),
        )
        .expect("sqlite backup");
        copy_tree(&storage.subdir(DataSubdir::Files), &backup.join("files")).expect("blob backup");

        drop(files);
        drop(analytics);
        drop(database);
        duckdb.close().await.expect("duckdb close");
        sqlite.close().await;
    }

    // Destroy the live stores, then restore each one from its independently timed backup.
    fs::remove_dir_all(&live).expect("destroy live data");
    let restored = AppStorage::init_for_test(live.clone());
    fs::copy(
        backup.join(SQLITE_DB_FILENAME),
        restored.subdir_path(DataSubdir::Sqlite, SQLITE_DB_FILENAME),
    )
    .expect("restore sqlite");
    fs::copy(
        backup.join(DUCKDB_DB_FILENAME),
        restored.subdir_path(DataSubdir::Duckdb, DUCKDB_DB_FILENAME),
    )
    .expect("restore duckdb");
    copy_tree(&backup.join("files"), &restored.subdir(DataSubdir::Files)).expect("restore blobs");
    fs::write(
        restored.data_path(RESTORE_PENDING_MARKER),
        "restore pending\n",
    )
    .expect("restore marker");

    let first_report = root.path().join("repair-first.json");
    run_repair(&live, &first_report);
    assert!(!restored.data_path(RESTORE_PENDING_MARKER).exists());
    let first: Value =
        serde_json::from_slice(&fs::read(&first_report).expect("first report")).expect("json");
    assert_eq!(
        first["association_repair_before_quota"]["files"]["associations_rebuilt"],
        1
    );
    assert_eq!(
        first["association_repair_before_quota"]["files"]["missing_content"][0]["hash"],
        missing
    );

    let second_report = root.path().join("repair-second.json");
    run_repair(&live, &second_report);
    let second: Value =
        serde_json::from_slice(&fs::read(&second_report).expect("second report")).expect("json");
    assert_eq!(
        second["association_repair_before_quota"]["files"]["associations_rebuilt"], 0,
        "a second repair must be a fixed point"
    );

    let storage = AppStorage::init_for_test(live);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let sqlite = Arc::new(
        SqliteService::init(&storage, Arc::clone(&clock))
            .await
            .expect("restored sqlite"),
    );
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(sqlite));
    let duckdb = Arc::new(
        DuckdbService::init(&storage, clock)
            .await
            .expect("restored duckdb"),
    );
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(DuckdbRepository(duckdb));
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
    let files = FileService::new(
        FilesConfig {
            enabled: true,
            storage: StorageBackend::Filesystem,
            quota_bytes: 1024 * 1024 * 1024,
            filesystem_path: None,
            s3: None,
        },
        storage.subdir(DataSubdir::FilesTemp),
        Arc::new(FilesystemStorage::new(storage.subdir(DataSubdir::Files))),
        Arc::clone(&database),
        cache,
    )
    .await
    .expect("restored files");

    assert!(
        analytics
            .get_trace(&ProjectId::from("default"), "survivor")
            .await
            .expect("survivor")
            .is_some()
    );
    for trace_id in ["aged", "requested-delete"] {
        assert!(
            analytics
                .get_trace(&ProjectId::from("default"), trace_id)
                .await
                .expect("deleted trace lookup")
                .is_none(),
            "{trace_id} was resurrected"
        );
    }
    assert_eq!(
        files
            .get_file(&ProjectId::from("default"), &present)
            .await
            .expect("surviving file")
            .data,
        b"surviving bytes"
    );
    assert!(matches!(
        files.get_file(&ProjectId::from("default"), &missing).await,
        Err(FileServiceError::ContentUnavailable { .. })
    ));
    let mut associations = database
        .get_file_hashes_for_traces(&ProjectId::from("default"), &["survivor".to_owned()])
        .await
        .expect("restored associations");
    associations.sort();
    assert_eq!(associations, vec![present, missing]);
}

fn span(
    trace_id: &str,
    span_id: &str,
    timestamp_start: chrono::DateTime<Utc>,
    messages: Option<String>,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("default".to_owned()),
        trace_id: trace_id.to_owned(),
        span_id: span_id.to_owned(),
        timestamp_start,
        messages,
        ..NormalizedSpan::default()
    }
}

fn run_repair(data_dir: &Path, report: &Path) {
    let binary = std::env::var("CARGO_BIN_EXE_sideseat")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_BIN_EXE_sideseat")));
    let output = Command::new(binary)
        .env("SIDESEAT_DATA_DIR", data_dir)
        .env("SIDESEAT_SECRETS_BACKEND", "file")
        .args([
            "--no-auth",
            "--otel-retention-max-age",
            "60",
            "system",
            "restore-repair",
            "--report",
        ])
        .arg(report)
        .output()
        .expect("run restore repair");
    assert!(
        output.status.success(),
        "restore repair failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
