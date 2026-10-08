/// A ClickHouse insert that fails after its rows landed does not cost those rows their media.
///
/// A failing materialized view fails the insert after ClickHouse has written the source block, so the span is
/// readable although the write reported failure. If the batch released the export's file associations on the
/// strength of that failure, and the exporter never retried, the readable span would name a file the sweeper
/// then reclaims. The batch settles them by what is stored instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_insert_that_landed_keeps_its_media() {
    use base64::Engine;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    use sideseat_core::config::{
        CacheBackendType, CacheConfig, EvictionPolicy, FilesConfig, RetentionConfig, StorageBackend,
    };

    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_landed_failure";
    let clickhouse: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(clickhouse_backend(&url, database).await);
    // The source block is written, then the view throws: rows land and the insert fails.
    let raw = raw_client(&url, database);
    raw.query("CREATE TABLE landed_failure_sink (x UInt8) ENGINE = Null")
        .execute()
        .await
        .expect("fault sink");
    raw.query(
        "CREATE MATERIALIZED VIEW landed_failure_view TO landed_failure_sink AS \
         SELECT throwIf(1, 'injected failure after the rows landed') AS x FROM otel_spans",
    )
    .execute()
    .await
    .expect("fault view");

    let temp = tempfile::TempDir::new().expect("temp dir");
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let clock: Arc<dyn sideseat_ports::clock::Clock> =
        Arc::new(sideseat_server::runtime::clock::SystemClock);
    let sqlite = Arc::new(
        sideseat_adapter_sqlite::SqliteService::init(&storage, Arc::clone(&clock))
            .await
            .expect("sqlite"),
    );
    let transactional: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync> =
        Arc::new(sideseat_adapter_sqlite::SqliteRepository(sqlite));
    let blobs: Arc<dyn sideseat_ports::blobs::FileStorage> = Arc::new(
        sideseat_adapter_blob_storage::FilesystemStorage::new(temp.path().join("files")),
    );
    let temp_files = storage.subdir(sideseat_core::storage::DataSubdir::FilesTemp);
    tokio::fs::create_dir_all(&temp_files)
        .await
        .expect("files temp");
    let files = Arc::new(
        sideseat_domain::files::FileService::new(
            FilesConfig {
                enabled: true,
                storage: StorageBackend::Filesystem,
                quota_bytes: 0,
                filesystem_path: Some(temp.path().join("files").display().to_string()),
                s3: None,
            },
            temp_files,
            Arc::clone(&blobs),
            Arc::clone(&transactional),
            Arc::new(
                sideseat_adapter_cache::CacheService::new(&CacheConfig {
                    backend: CacheBackendType::Memory,
                    max_entries: 16,
                    eviction_policy: EvictionPolicy::TinyLfu,
                    redis_url: None,
                })
                .await
                .expect("cache"),
            ),
        )
        .await
        .expect("file service"),
    );
    let pipeline = sideseat_ingestion::traces::TracePipeline::new(
        Arc::clone(&clickhouse),
        Arc::new(sideseat_domain::pricing::PricingService::init_for_test().expect("pricing")),
        Arc::new(sideseat_messaging::TopicService::new(
            sideseat_adapter_topics::memory_backend(),
        )),
        Arc::clone(&files),
        Arc::new(sideseat_ingestion::staging::StagingService::new(
            Arc::clone(&blobs),
            Arc::clone(&transactional),
            Arc::clone(&clickhouse),
            clock,
            RetentionConfig::default(),
            5,
        )),
    );

    // An export whose attachment is extracted into the file store. Dated now: a span older than the table's
    // retention is dropped as it is inserted, and would never land at all.
    let start = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    )
    .expect("nanoseconds");
    let picture = base64::engine::general_purpose::STANDARD
        .encode((0..4096u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    let export = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![31; 16],
                    span_id: vec![1; 8],
                    name: "generation".into(),
                    start_time_unix_nano: start,
                    end_time_unix_nano: start + 100,
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
    };

    // The client never retries: this one answer is all there will be.
    let outcomes = pipeline
        .run_batch_outcomes_for_test(std::slice::from_ref(&export))
        .await;
    assert_eq!(
        outcomes,
        vec![sideseat_ingestion::traces::IngestOutcome::Failed],
        "the write failed, and the export is answered so"
    );

    let project = ProjectId::from("default");
    let trace = "1f".repeat(16);
    let landed = clickhouse
        .file_reference_fields_for_traces(&project, std::slice::from_ref(&trace))
        .await
        .expect("stored fields");
    assert!(
        !landed.is_empty(),
        "the premise: the span row landed although the insert failed"
    );
    let referenced = sideseat_domain::files::FileService::hashes_referenced_by_trace(
        &project,
        &trace,
        clickhouse.as_ref(),
    )
    .await
    .expect("references");
    assert!(
        !referenced.is_empty(),
        "the span landed despite the failure, and it names its attachment"
    );
    for hash in &referenced {
        let file = transactional
            .get_file(&project, hash)
            .await
            .expect("file row")
            .expect("the landed span's file is registered");
        assert!(
            file.ref_count > 0,
            "the landed span's file is still owned: {file:?}"
        );
    }
    let orphans = transactional.get_orphan_files().await.expect("orphans");
    assert!(
        orphans.iter().all(|(_, hash)| !referenced.contains(hash)),
        "a file a readable span names must not be reclaimable: {orphans:?}"
    );
    // The sweeper runs, and the bytes are still there to read.
    sideseat_domain::files::cleanup::cleanup_zero_ref_files(&blobs, &transactional, 0)
        .await
        .expect("sweep");
    for hash in &referenced {
        files
            .get_file(&project, hash)
            .await
            .expect("the landed span's attachment is readable after a sweep");
    }

    // The server's own retry of the refused export, once the fault is gone, is an exact redelivery: it is
    // answered as stored, keeps no second raw copy, and leaves the file held.
    raw.query("DROP VIEW landed_failure_view")
        .execute()
        .await
        .expect("clear the fault");
    let outcomes = pipeline
        .run_batch_outcomes_for_test(std::slice::from_ref(&export))
        .await;
    assert_eq!(
        outcomes,
        vec![sideseat_ingestion::traces::IngestOutcome::Stored]
    );
    let records = clickhouse
        .raw_records_page(&project, None, 16)
        .await
        .expect("records");
    assert_eq!(records.len(), 1, "the retry kept no second copy");
    let orphans = transactional.get_orphan_files().await.expect("orphans");
    assert!(
        orphans.iter().all(|(_, hash)| !referenced.contains(hash)),
        "{orphans:?}"
    );
}
