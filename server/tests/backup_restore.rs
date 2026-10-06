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
use sideseat_ports::types::{
    ListLogsParams, MessageQueryParams, NormalizedLog, NormalizedSpan, ProjectId, RawOrigin,
    RawRecordRow, StagedSignal,
};
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
        // The authority the rows are derived from: one record per trace, so the restore's journal replay has
        // raw content to reach as well as rows.
        analytics
            .insert_raw_records(&[
                raw_record("survivor", now),
                raw_record("requested-delete", now),
            ])
            .await
            .expect("raw records");
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
        // Log-carried messages for the survivor and for the trace the journal deletes: the backup holds both,
        // and the repair must leave the first joined to its span and take the second with its trace.
        analytics
            .insert_logs(&[
                message_log(
                    "survivor",
                    "live",
                    "survivor-log",
                    now,
                    "restored from logs",
                ),
                message_log(
                    "requested-delete",
                    "gone",
                    "deleted-log",
                    now,
                    "deleted with its trace",
                ),
            ])
            .await
            .expect("message logs");

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
    let survivor_messages = analytics
        .get_messages(&MessageQueryParams {
            project_id: ProjectId::from("default"),
            trace_id: Some("survivor".to_owned()),
            ..Default::default()
        })
        .await
        .expect("survivor messages");
    assert_eq!(survivor_messages.rows.len(), 1);
    assert!(
        survivor_messages.rows[0]
            .log_messages_json
            .contains("restored from logs"),
        "a restored log record still joins its span: {}",
        survivor_messages.rows[0].log_messages_json
    );
    let (deleted_logs, _) = analytics
        .list_logs(&ListLogsParams {
            project_id: ProjectId::from("default"),
            page: 1,
            limit: 10,
            trace_id: Some("requested-delete".to_owned()),
            ..Default::default()
        })
        .await
        .expect("deleted trace logs");
    assert!(
        deleted_logs.is_empty(),
        "the repair replays the deletion over the trace's log records too"
    );
    let mut associations = database
        .get_file_hashes_for_traces(&ProjectId::from("default"), &["survivor".to_owned()])
        .await
        .expect("restored associations");
    associations.sort();
    assert_eq!(associations, vec![present, missing]);

    // The restore replayed a trace deletion, so the raw record holding that trace is queued for
    // reconciliation - the only way deleted content leaves the authority. The survivor's record is not
    // touched by the replay, and remains readable.
    let project = ProjectId::from("default");
    let queued: Vec<String> = analytics
        .pending_raw_records(usize::MAX)
        .await
        .expect("reconciliation queue")
        .into_iter()
        .map(|entry| entry.raw_id)
        .collect();
    assert!(
        queued.contains(&"raw-requested-delete".to_owned()),
        "the replayed deletion must enqueue the record holding its trace: {queued:?}"
    );
    let survivor_record = analytics
        .get_raw_records(&project, &["raw-survivor".to_owned()])
        .await
        .expect("survivor raw record");
    assert_eq!(survivor_record.len(), 1);
    assert_eq!(survivor_record[0].origin, RawOrigin::Received);

    // A legal hold reaches the authority as it reaches the rows, so a restored record cannot expire while
    // the hold stands.
    let until = Utc::now() + Duration::days(30);
    analytics
        .patch_project_hold(&project, until)
        .await
        .expect("patch the hold");
    let held = analytics
        .get_raw_records(&project, &["raw-survivor".to_owned()])
        .await
        .expect("held raw record");
    assert!(
        held[0].hold_until.is_some(),
        "a legal hold must reach the raw records"
    );
}

/// One stored record for a trace, as ingestion would write it.
fn raw_record(trace_id: &str, at: chrono::DateTime<Utc>) -> RawRecordRow {
    RawRecordRow {
        project_id: ProjectId::from("default"),
        raw_id: format!("raw-{trace_id}"),
        signal: StagedSignal::Traces,
        received_at: at,
        origin: RawOrigin::Received,
        version: at.timestamp_micros(),
        signal_until: at,
        hold_until: None,
        trace_ids: vec![trace_id.to_owned()],
        record: b"SSR1\0\0body".to_vec(),
    }
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

fn message_log(
    trace_id: &str,
    span_id: &str,
    digest: &str,
    timestamp: chrono::DateTime<Utc>,
    text: &str,
) -> NormalizedLog {
    NormalizedLog {
        project_id: Some("default".to_owned()),
        log_digest: digest.to_owned(),
        timestamp,
        time: Some(timestamp),
        trace_id: Some(trace_id.to_owned()),
        span_id: Some(span_id.to_owned()),
        event_name: Some("gen_ai.user.message".to_owned()),
        messages: Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": timestamp}},
                "content": {"content": text}
            }])
            .to_string(),
        ),
        ..NormalizedLog::default()
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
