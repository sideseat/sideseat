//! A log record's media through the real ingest path: stored by content hash under the record's trace, durable once
//! the rows are, and named where a release looks.

use std::sync::Arc;

use base64::prelude::*;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, KeyValueList, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use sideseat_adapter_blob_storage::FilesystemStorage;
use sideseat_adapter_cache::CacheService;
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::storage::AppStorage;
use sideseat_domain::files::FileService;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
use sideseat_ports::types::ProjectId;

#[derive(Debug)]
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("instant")
    }
}

fn png(fill: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.resize(4096, fill);
    bytes
}

fn record(bytes: Vec<u8>, trace: Option<[u8; 16]>) -> LogRecord {
    LogRecord {
        time_unix_nano: 1_700_000_000_000_000_000,
        trace_id: trace.map(|t| t.to_vec()).unwrap_or_default(),
        body: Some(AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: vec![KeyValue {
                    key: "inline_data".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::BytesValue(bytes)),
                    }),
                }],
            })),
        }),
        ..Default::default()
    }
}

fn hash_of(bytes: &[u8]) -> String {
    blake3::hash(BASE64_STANDARD.encode(bytes).as_bytes())
        .to_hex()
        .to_string()
}

/// The stores a log ingest writes through, over one temp directory.
struct Stores {
    _temp: tempfile::TempDir,
    clock: Arc<dyn Clock>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    files: FileService,
}

async fn stores() -> Stores {
    let temp = tempfile::TempDir::new().unwrap();
    let app_storage = AppStorage::init_for_test(temp.path().to_path_buf());
    for dir in ["duckdb", "files", "files_temp"] {
        tokio::fs::create_dir_all(temp.path().join(dir))
            .await
            .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let database: Arc<dyn TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&app_storage, Arc::clone(&clock))
                .await
                .unwrap(),
        )));
    // As the server assembles it: the domain answers each row's body views over what the adapter stores.
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(sideseat_domain::dedup::DedupAnalyticsRepository::new(
            Box::new(DuckdbRepository(Arc::new(
                DuckdbService::init(&app_storage, Arc::clone(&clock))
                    .await
                    .unwrap(),
            ))),
        ));
    let cache = Arc::new(
        CacheService::new(&sideseat_core::config::CacheConfig {
            backend: sideseat_core::config::CacheBackendType::Memory,
            max_entries: 1000,
            eviction_policy: sideseat_core::config::EvictionPolicy::TinyLfu,
            redis_url: None,
        })
        .await
        .unwrap(),
    );
    let files = FileService::new(
        sideseat_core::config::FilesConfig {
            enabled: true,
            storage: sideseat_core::config::StorageBackend::Filesystem,
            quota_bytes: 64 * 1024 * 1024,
            filesystem_path: Some(temp.path().join("files").to_string_lossy().to_string()),
            s3: None,
        },
        temp.path().join("files_temp"),
        Arc::new(FilesystemStorage::new(temp.path().join("files"))),
        Arc::clone(&database),
        cache,
    )
    .await
    .unwrap();
    Stores {
        _temp: temp,
        clock,
        database,
        analytics,
        files,
    }
}

/// One default-project export of these records, as the collector delivers it.
fn export(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(opentelemetry_proto::tonic::resource::v1::Resource {
                attributes: vec![KeyValue {
                    key: crate::otlp::PROJECT_ID_ATTR.to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(
                            sideseat_core::constants::DEFAULT_PROJECT_ID.to_string(),
                        )),
                    }),
                }],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

#[tokio::test]
async fn a_log_s_media_is_stored_owned_by_its_trace_and_a_traceless_log_keeps_it_inline() {
    let stores = stores().await;
    let (clock, database, analytics, files) = (
        &stores.clock,
        &stores.database,
        &stores.analytics,
        &stores.files,
    );
    let trace = [7u8; 16];
    let request = export(vec![record(png(1), Some(trace)), record(png(2), None)]);
    let stored = super::ingest(
        &request,
        analytics.as_ref(),
        database.as_ref(),
        Some(files),
        clock.now(),
    )
    .await
    .expect("ingest");
    assert_eq!(stored.stored, 2);

    let project = ProjectId::from(sideseat_core::constants::DEFAULT_PROJECT_ID);
    let traced = hash_of(&png(1));
    let traceless = hash_of(&png(2));
    assert!(
        files.file_exists(&project, &traced).await.unwrap(),
        "the traced record's media was not stored"
    );
    assert!(
        !files.file_exists(&project, &traceless).await.unwrap(),
        "a record with no trace to own its media stored it anyway"
    );
    let trace_hex = hex::encode(trace);
    assert_eq!(
        FileService::hashes_referenced_by_trace(&project, &trace_hex, analytics.as_ref())
            .await
            .unwrap(),
        vec![traced.clone()],
        "a release would not see what the trace's log names"
    );
    assert!(
        !database
            .release_trace_file_association(&project, &trace_hex, &traced)
            .await
            .unwrap(),
        "the association is still provisional after the rows committed"
    );
}

/// A producer's reference to bytes another project holds is not this project's: verified against this project's own
/// store, it is replaced with a note, and the log never names the other project's media.
#[tokio::test]
async fn a_reference_to_another_project_s_media_becomes_the_note() {
    let stores = stores().await;
    let bytes = png(3);
    let hash = hash_of(&bytes);
    stores
        .files
        .storage()
        .store(&ProjectId::from("another-project"), &hash, &bytes)
        .await
        .unwrap();
    let uri = format!("#!B64!#image/png::{hash}");
    let mut named = record(Vec::new(), Some([9u8; 16]));
    named.body = Some(AnyValue {
        value: Some(any_value::Value::StringValue(format!("see {uri}"))),
    });
    super::ingest(
        &export(vec![named]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    let (rows, _) = stores
        .analytics
        .list_logs(&sideseat_ports::types::ListLogsParams {
            project_id: ProjectId::from(sideseat_core::constants::DEFAULT_PROJECT_ID),
            page: 1,
            limit: 10,
            trace_id: None,
            span_id: None,
            severity_text: None,
            severity_number_min: None,
            service_name: None,
            environment: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
        .unwrap();
    let body = rows[0].body.clone().unwrap_or_default();
    assert!(body.contains("[content not stored (image/png)]"), "{body}");
    assert!(!body.contains(&uri), "{body}");
}

fn default_project() -> ProjectId {
    ProjectId::from(sideseat_core::constants::DEFAULT_PROJECT_ID)
}

fn string_record(text: String, trace: Option<[u8; 16]>) -> LogRecord {
    LogRecord {
        body: Some(AnyValue {
            value: Some(any_value::Value::StringValue(text)),
        }),
        ..record(Vec::new(), trace)
    }
}

/// A producer's reference this project holds the bytes of is named where the release looks, as the record's own
/// media is: its association survives reconciliation while the log does.
#[tokio::test]
async fn a_verified_producer_reference_is_seen_by_the_release() {
    let stores = stores().await;
    let bytes = png(4);
    let hash = hash_of(&bytes);
    let project = default_project();
    stores
        .files
        .storage()
        .store(&project, &hash, &bytes)
        .await
        .unwrap();
    stores
        .database
        .upsert_file(
            &project,
            &hash,
            Some("image/png"),
            bytes.len() as i64,
            "sha256",
        )
        .await
        .unwrap();
    let trace = [5u8; 16];
    super::ingest(
        &export(vec![string_record(
            format!("see #!B64!#image/png::{hash}"),
            Some(trace),
        )]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    assert_eq!(
        FileService::hashes_referenced_by_trace(
            &project,
            &hex::encode(trace),
            stores.analytics.as_ref()
        )
        .await
        .unwrap(),
        vec![hash],
        "the release would not see the producer's verified reference"
    );
}

/// A trace deleted before its logs commit keeps neither: the rows written for it go, and so do this export's
/// associations, so nothing names bytes the deletion may have reclaimed.
#[tokio::test]
async fn logs_written_for_a_deleted_trace_take_their_associations_with_them() {
    let stores = stores().await;
    let trace = [6u8; 16];
    let project = default_project();
    stores
        .database
        .record_deleted_traces(&project, &[hex::encode(trace)])
        .await
        .unwrap();
    super::ingest(
        &export(vec![record(png(5), Some(trace))]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    assert!(
        stores
            .database
            .get_file_hashes_for_traces(&project, &[hex::encode(trace)])
            .await
            .unwrap()
            .is_empty(),
        "an association outlived its deleted trace"
    );
    assert!(
        FileService::hashes_referenced_by_trace(
            &project,
            &hex::encode(trace),
            stores.analytics.as_ref()
        )
        .await
        .unwrap()
        .is_empty(),
        "a row still names media for a deleted trace"
    );
}

/// A record with no trace cannot own a reference, so its own is the note - and only its own: another record of the
/// export naming the same media it stored keeps the reference.
#[tokio::test]
async fn a_traceless_reference_is_noted_in_its_own_record_only() {
    let stores = stores().await;
    let bytes = png(6);
    let uri = format!("#!B64!#image/png::{}", hash_of(&bytes));
    super::ingest(
        &export(vec![
            record(bytes, Some([8u8; 16])),
            string_record(format!("see {uri}"), None),
        ]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    let (rows, _) = stores
        .analytics
        .list_logs(&sideseat_ports::types::ListLogsParams {
            project_id: default_project(),
            page: 1,
            limit: 10,
            trace_id: None,
            span_id: None,
            severity_text: None,
            severity_number_min: None,
            service_name: None,
            environment: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
        .unwrap();
    let bodies: Vec<String> = rows
        .iter()
        .map(|row| row.body.clone().unwrap_or_default())
        .collect();
    assert!(
        bodies.iter().any(|body| body.contains(&uri)),
        "the traced record lost its media: {bodies:?}"
    );
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("[content not stored (image/png)]")),
        "the traceless record kept a reference it cannot own: {bodies:?}"
    );
}

/// A record the fences drop holds no association, even where a surviving record of another trace stores the same
/// bytes.
#[tokio::test]
async fn a_dropped_record_s_trace_holds_no_association() {
    let stores = stores().await;
    let mut unstorable = record(png(7), Some([2u8; 16]));
    unstorable.time_unix_nano = u64::MAX;
    unstorable.observed_time_unix_nano = u64::MAX;
    super::ingest(
        &export(vec![record(png(7), Some([1u8; 16])), unstorable]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    let project = default_project();
    assert_eq!(
        stores
            .database
            .get_file_hashes_for_traces(&project, &[hex::encode([1u8; 16])])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        stores
            .database
            .get_file_hashes_for_traces(&project, &[hex::encode([2u8; 16])])
            .await
            .unwrap()
            .is_empty(),
        "the dropped record's trace holds an association"
    );
}

/// A key spelled like a reference is the producer's text, not a reference: it is neither verified nor replaced,
/// and two such keys both survive - rewriting them collided their entries and lost a value.
#[tokio::test]
async fn a_key_spelled_like_a_reference_stays_the_producer_s_text() {
    let stores = stores().await;
    let first = format!("#!B64!#image/png::{}", "9".repeat(64));
    let second = format!("#!B64!#image/png::{}", "8".repeat(64));
    let mut keyed = record(Vec::new(), Some([4u8; 16]));
    let pair = |key: &str, value: &str| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    };
    keyed.body = Some(AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![pair(&first, "one"), pair(&second, "two")],
        })),
    });
    super::ingest(
        &export(vec![keyed]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    let (rows, _) = stores
        .analytics
        .list_logs(&sideseat_ports::types::ListLogsParams {
            project_id: default_project(),
            page: 1,
            limit: 10,
            trace_id: None,
            span_id: None,
            severity_text: None,
            severity_number_min: None,
            service_name: None,
            environment: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(&rows[0].body.clone().unwrap_or_default()).unwrap();
    assert_eq!(body[&first], "one", "{body}");
    assert_eq!(body[&second], "two", "{body}");
}

/// An unbacked reference is noted in the record its columns are derived from, so the body and its text agree: the
/// value becomes the note in both, and a key spelled the same keeps the producer's text in both.
#[tokio::test]
async fn the_note_reaches_the_body_and_its_text_alike_and_never_a_key() {
    let stores = stores().await;
    let uri = format!("#!B64!#image/png::{}", "7".repeat(64));
    let pair = |key: &str, value: &str| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    };
    let mut keyed = record(Vec::new(), Some([3u8; 16]));
    keyed.body = Some(AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![pair(&uri, "key text"), pair("content", &uri)],
        })),
    });
    super::ingest(
        &export(vec![keyed]),
        stores.analytics.as_ref(),
        stores.database.as_ref(),
        Some(&stores.files),
        stores.clock.now(),
    )
    .await
    .expect("ingest");
    let (rows, _) = stores
        .analytics
        .list_logs(&sideseat_ports::types::ListLogsParams {
            project_id: default_project(),
            page: 1,
            limit: 10,
            trace_id: None,
            span_id: None,
            severity_text: None,
            severity_number_min: None,
            service_name: None,
            environment: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
        .unwrap();
    let note = "[content not stored (image/png)]";
    let body: serde_json::Value =
        serde_json::from_str(&rows[0].body.clone().unwrap_or_default()).unwrap();
    assert_eq!(body[&uri], "key text", "{body}");
    assert_eq!(body["content"], note, "{body}");
    let text = rows[0].body_text.clone().unwrap_or_default();
    assert!(text.contains(&uri) && text.contains(note), "{text}");
    assert_eq!(
        text.matches(&uri).count(),
        1,
        "the key alone keeps the spelling: {text}"
    );
}

/// A reference re-derivation cannot reach - in messages its raw form spells otherwise, or in a record whose raw form
/// does not parse back - is still replaced: no unbacked reference is stored.
#[test]
fn a_reference_rederivation_cannot_reach_is_replaced_where_it_lands() {
    let uri = format!("#!B64!#image/png::{}", "6".repeat(64));
    let note = "[content not stored (image/png)]";
    let mut log = sideseat_ports::types::NormalizedLog {
        raw_log: serde_json::json!("not a record"),
        body: serde_json::json!({"content": uri.clone()}),
        body_text: Some(format!("content {uri}")),
        messages: Some(serde_json::json!([{"content": uri.clone()}]).to_string()),
        ..Default::default()
    };
    super::note_unbacked(&mut log, std::slice::from_ref(&uri));
    for field in [
        log.body.to_string(),
        log.messages.clone().unwrap_or_default(),
    ] {
        assert!(!field.contains(&uri), "{field}");
        assert!(field.contains(note), "{field}");
    }
    assert_eq!(
        log.body_text, None,
        "a body text that cannot be re-derived gives way to the noted body"
    );
    let charged = log.logical_bytes;
    assert_eq!(
        charged,
        crate::accounting::log_logical_bytes(&mut log),
        "the record's charge follows its rewritten content"
    );
}

/// A reference whose hash is no content hash names no file, so it is the producer's text: never noted, and so never
/// joined with what decoding puts after it.
#[test]
fn a_reference_without_a_content_hash_is_text() {
    let short = format!("#!B64!#image/png::{}", "a".repeat(63));
    let log = sideseat_ports::types::NormalizedLog {
        body: serde_json::json!({"content": format!("{short}\\u0061")}),
        ..Default::default()
    };
    assert!(super::derived_references(&log).is_empty());
}

/// A note rewrites what is shown, never what joins: the frame key the producer's content stated stays.
#[test]
fn a_note_keeps_the_frame_key() {
    let uri = format!("#!B64!#image/png::{}", "5".repeat(64));
    let record = string_record(format!("see {uri}"), Some([1u8; 16]));
    let mut log = sideseat_ports::types::NormalizedLog {
        raw_log: serde_json::to_value(&record).unwrap(),
        frame_key: Some(42),
        ..Default::default()
    };
    super::note_unbacked(&mut log, std::slice::from_ref(&uri));
    assert_eq!(log.frame_key, Some(42));
    assert!(
        log.body
            .to_string()
            .contains("[content not stored (image/png)]")
    );
}
