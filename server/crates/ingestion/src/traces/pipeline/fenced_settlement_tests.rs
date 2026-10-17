//! A wave that loses this process's store lease part-way settles nothing, and its message is not acknowledged.
//!
//! The lease is what keeps a restore and a write apart: a wave that ran without it may have written media a
//! restore took back meanwhile, and no read of the stored rows can see that. So such a wave's payloads stay
//! staged and their messages pending, for a pass that holds the lease to write again (`waves.rs`). Settlement once
//! reported a payload whose release the closed fence refused as settled all the same, and the queue consumer
//! acknowledged it: the message was gone, and the staging sweep, which settles by reading alone, would retire the
//! payload without writing it again. The recovery paths - the shutdown drain and claimed messages - wrote and then
//! settled in a pass of their own, so a lease lost during the write and taken again before the settlement retired
//! the payload in the new term.
//!
//! Here the file store drops the lease while the wave finalises the export's media - after the wave entered its
//! pass, before it settles - which is the interleaving the fence exists for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use base64::Engine;
use chrono::{TimeZone, Utc};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sideseat_core::utils::collection_fence::{CollectionFence, HeldUntil, Passes};
use sideseat_ports::blobs::{FileStorage, FileStorageError, ObjectName, ObjectVisitor};
use sideseat_ports::types::ProjectId;
use tokio::sync::watch;

use super::*;
use crate::staging::StagedWrite;

/// A file store that drops the lease the moment a media file is moved into place, once armed - and, as the lease's
/// holder would, takes it again in a later term as soon as no pass is running, when asked to.
struct LeaseDroppingStorage {
    inner: Arc<dyn FileStorage>,
    held: watch::Sender<Option<HeldUntil>>,
    passes: Arc<Passes>,
    armed: AtomicBool,
    retake: bool,
}

fn held(term: u64) -> Option<HeldUntil> {
    Some(HeldUntil::new(
        tokio::time::Instant::now() + std::time::Duration::from_secs(600),
        term,
    ))
}

#[async_trait]
impl FileStorage for LeaseDroppingStorage {
    async fn store(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        data: &[u8],
    ) -> Result<(), FileStorageError> {
        self.inner.store(project_id, object, data).await
    }

    async fn get(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<Vec<u8>, FileStorageError> {
        self.inner.get(project_id, object).await
    }

    async fn exists(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<bool, FileStorageError> {
        self.inner.exists(project_id, object).await
    }

    async fn delete(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
    ) -> Result<(), FileStorageError> {
        self.inner.delete(project_id, object).await
    }

    async fn incarnations(
        &self,
        project_id: &ProjectId,
        hash: &str,
    ) -> Result<Vec<i64>, FileStorageError> {
        self.inner.incarnations(project_id, hash).await
    }

    async fn scan_objects(&self, visitor: &mut dyn ObjectVisitor) -> Result<(), FileStorageError> {
        self.inner.scan_objects(visitor).await
    }

    async fn delete_project(&self, project_id: &ProjectId) -> Result<u64, FileStorageError> {
        self.inner.delete_project(project_id).await
    }

    async fn finalize_temp(
        &self,
        project_id: &ProjectId,
        object: ObjectName<'_>,
        temp_path: &std::path::Path,
        expected: sideseat_ports::blobs::ExpectedContent,
    ) -> Result<(), FileStorageError> {
        let result = self
            .inner
            .finalize_temp(project_id, object, temp_path, expected)
            .await;
        if self.armed.swap(false, Ordering::SeqCst) {
            self.held.send_replace(None);
            if self.retake {
                let held_until = self.held.clone();
                let passes = Arc::clone(&self.passes);
                tokio::spawn(async move {
                    while !passes.is_empty() {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                    held_until.send_replace(held(1));
                });
            }
        }
        result
    }
}

#[derive(Debug)]
struct TestClock;

impl sideseat_ports::clock::Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }
}

struct Fenced {
    _temp: tempfile::TempDir,
    pipeline: TracePipeline,
    storage: Arc<LeaseDroppingStorage>,
    database: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
}

/// A pipeline over temporary stores with files on, its staging and its waves fenced by a lease the file store
/// drops.
async fn fenced_pipeline(retake: bool) -> Fenced {
    use sideseat_adapter_blob_storage::FilesystemStorage;
    use sideseat_adapter_cache::CacheService;
    use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
    use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
    use sideseat_core::config::{
        CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, StorageBackend,
    };
    use sideseat_core::storage::{AppStorage, DataSubdir};
    use sideseat_domain::pricing::PricingService;
    use sideseat_messaging::TopicService;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let app = AppStorage::init_for_test(temp.path().to_path_buf());
    tokio::fs::create_dir_all(temp.path().join("duckdb"))
        .await
        .expect("duckdb dir");
    let analytics: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(DuckdbRepository(Arc::new(
            DuckdbService::init(&app, Arc::new(TestClock))
                .await
                .expect("duckdb"),
        )));
    let database: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync> =
        Arc::new(SqliteRepository(Arc::new(
            SqliteService::init(&app, Arc::new(TestClock))
                .await
                .expect("sqlite"),
        )));
    let cache = Arc::new(
        CacheService::new(&CacheConfig {
            backend: CacheBackendType::Memory,
            max_entries: 16,
            eviction_policy: EvictionPolicy::TinyLfu,
            redis_url: None,
        })
        .await
        .expect("memory cache"),
    );
    let (held_until, receiver) = watch::channel(held(0));
    let passes = Arc::new(Passes::default());
    let storage = Arc::new(LeaseDroppingStorage {
        inner: Arc::new(FilesystemStorage::new(temp.path().join("files"))),
        held: held_until,
        passes: Arc::clone(&passes),
        armed: AtomicBool::new(false),
        retake,
    });
    let temp_files = app.subdir(DataSubdir::FilesTemp);
    tokio::fs::create_dir_all(&temp_files)
        .await
        .expect("files temp dir");
    let files = Arc::new(
        FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 0,
                filesystem_path: Some(temp.path().join("files").display().to_string()),
                s3: None,
            },
            temp_files,
            Arc::clone(&storage) as Arc<dyn FileStorage>,
            Arc::clone(&database),
            cache,
        )
        .await
        .expect("file service"),
    );
    let staging = StagingService::new(
        Arc::clone(&storage) as Arc<dyn FileStorage>,
        Arc::clone(&database),
        Arc::clone(&analytics),
        Arc::new(TestClock),
        sideseat_core::config::RetentionConfig::default(),
        5,
    )
    .with_collection_fence(CollectionFence::following_passes(receiver, passes));
    let pipeline = TracePipeline::new(
        analytics,
        Arc::new(PricingService::init_for_test().expect("offline pricing")),
        Arc::new(TopicService::new(sideseat_adapter_topics::memory_backend())),
        files,
        Arc::new(staging),
    );
    Fenced {
        _temp: temp,
        pipeline,
        storage,
        database,
    }
}

/// One span carrying an image, which the wave finalises as a media file.
fn export() -> ExportTraceServiceRequest {
    let picture = base64::engine::general_purpose::STANDARD
        .encode((0..4096u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![61; 16],
                    span_id: vec![1; 8],
                    name: "generation".into(),
                    start_time_unix_nano: 1_700_000_000_000_000_000,
                    end_time_unix_nano: 1_700_000_000_000_000_100,
                    attributes: vec![KeyValue {
                        key: "input.value".into(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "data:image/png;base64,{picture}"
                            ))),
                        }),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn body(request: &ExportTraceServiceRequest) -> ReceivedPayload {
    ReceivedPayload::new(
        request.encode_to_vec(),
        sideseat_domain::raw_payload::RawContent::Protobuf,
        Utc::now(),
    )
}

async fn staged(
    pipeline: &TracePipeline,
    request: &ExportTraceServiceRequest,
) -> (StagedPayload, StagedPayloadRef) {
    let received = body(request);
    let reference = pipeline
        .staging
        .stage(
            "default",
            StagedSignal::Traces,
            &received.staged(),
            received.received_at,
            crate::traces::confirmation_records(request),
            StagedWrite {
                partition_key: "p".to_string(),
                raw_id: None,
            },
        )
        .await
        .expect("stage");
    let payload = pipeline
        .staging
        .registration(&reference.id)
        .await
        .expect("registration")
        .expect("staged");
    (payload, reference)
}

/// A queue delivery whose wave loses the lease is neither settled nor acknowledged, and its payload stays staged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wave_that_loses_the_lease_settles_nothing_and_is_not_acknowledged() {
    let fenced = fenced_pipeline(false).await;
    let request = export();
    let (payload, _) = staged(&fenced.pipeline, &request).await;
    fenced.storage.armed.store(true, Ordering::SeqCst);

    let answer = fenced
        .pipeline
        .run_waves(
            std::slice::from_ref(&request),
            &[body(&request)],
            &[Some(payload.clone())],
            false,
        )
        .await
        .remove(0);

    assert!(answer.fenced, "the lease was lost during the wave");
    assert!(
        !answer.settled,
        "a payload whose release the fence refused is not settled"
    );
    assert!(
        !TracePipeline::acknowledges(&answer),
        "the message stays pending for a consumer that holds the lease"
    );
    assert!(
        fenced
            .database
            .get_staged_payload(&payload.id)
            .await
            .expect("registry")
            .is_some(),
        "the payload stays staged"
    );
}

/// The recovery path - a claimed message, the shutdown drain - holds the lease from its write to its settlement:
/// a lease lost during the write and taken again before the settlement leaves the delivery pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovered_delivery_whose_write_lost_the_lease_stays_pending() {
    let fenced = fenced_pipeline(true).await;
    let request = export();
    let (payload, reference) = staged(&fenced.pipeline, &request).await;
    fenced.storage.armed.store(true, Ordering::SeqCst);

    assert!(
        !fenced.pipeline.process_staged_reference(&reference).await,
        "a delivery written across a lost lease is not acknowledged"
    );
    assert!(
        fenced
            .database
            .get_staged_payload(&payload.id)
            .await
            .expect("registry")
            .is_some(),
        "the payload stays staged, to be written again under the lease"
    );
    assert_eq!(
        fenced
            .database
            .get_staged_payload(&payload.id)
            .await
            .expect("registry")
            .expect("staged")
            .redrive_attempts,
        0,
        "nor is a fenced attempt counted against it"
    );

    // Under the lease, taken again in a later term, the same delivery is written and settled.
    let fence = fenced.pipeline.staging.collection_fence();
    for _ in 0..1000 {
        if fence.held_through(1) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(fence.held_through(1), "the lease was taken again");
    assert!(fenced.pipeline.process_staged_reference(&reference).await);
    assert!(
        fenced
            .database
            .get_staged_payload(&payload.id)
            .await
            .expect("registry")
            .is_none()
    );
}
