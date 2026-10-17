/// A transactional side for a retention pass: SQLite, with the parity project registered for storage accounting
/// so the pass visits it.
async fn retention_transactional(
    temp: &tempfile::TempDir,
) -> Arc<sideseat_adapter_sqlite::SqliteRepository> {
    let storage = AppStorage::init_for_test(temp.path().to_path_buf());
    let service = Arc::new(
        sideseat_adapter_sqlite::SqliteService::init(
            &storage,
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("sqlite"),
    );
    sqlx::query(
        "INSERT INTO project_storage_usage (project_id, logical_bytes, updated_at) VALUES (?, 0, 0)",
    )
    .bind(PROJECT)
    .execute(service.pool())
    .await
    .expect("register the project");
    Arc::new(sideseat_adapter_sqlite::SqliteRepository(service))
}

/// The span ids of the leftover's trace.
async fn leftover_rows(
    store: &(impl AnalyticsRepository + ?Sized),
    project: &ProjectId,
) -> Vec<String> {
    let mut found: Vec<String> = store
        .get_spans_for_trace(project, "trace-left", 10)
        .await
        .expect("spans")
        .into_iter()
        .map(|span| span.span_id)
        .collect();
    found.sort();
    found
}

/// The leftover a collection leaves - the row of a write never acknowledged, naming a record the collection took -
/// as one ordered list of answers: before retention, and after it.
async fn collected_leftover_answers<F>(
    store: &(impl AnalyticsRepository + ?Sized),
    retention: F,
) -> Vec<String>
where
    F: std::future::Future<Output = ()>,
{
    let project = ProjectId::from(PROJECT);
    let old = Utc::now() - chrono::Duration::days(10);
    store
        .insert_raw_records(&[RawRecordRow {
            signal_until: old,
            ..raw_row("leftover", 1, &["trace-left"], b"leftover")
        }])
        .await
        .expect("record");
    store
        .insert_spans(vec![NormalizedSpan {
            timestamp_start: old,
            ingested_at: Some(Utc::now()),
            ..span_of("trace-left", "span-left", "leftover")
        }])
        .await
        .expect("the unacknowledged write's row");
    // The collection: the record's versions go; its row does not.
    store
        .purge_raw_versions(&project, "leftover", 0)
        .await
        .expect("the collection's purge");
    let mut answers = vec![
        format!(
            "records after collection: {}",
            store
                .get_raw_records(&project, &["leftover".to_string()])
                .await
                .expect("records")
                .len()
        ),
        format!(
            "rows after collection: {:?}",
            leftover_rows(store, &project).await
        ),
    ];
    retention.await;
    answers.push(format!(
        "rows after retention: {:?}",
        leftover_rows(store, &project).await
    ));
    answers
}

/// A collection never removes rows, on either backend: the rows naming a collected record are those of writes the
/// client was never told succeeded, kept as at-least-once delivery admits, and both backends answer alike about
/// them - until their trace's retention removes them, alike.
#[tokio::test]
async fn a_collected_record_s_leftover_rows_read_and_expire_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let config = sideseat_core::config::RetentionConfig {
        max_age_minutes: Some(24 * 60),
        max_spans: None,
        ..Default::default()
    };

    let (_duck_temp, duckdb) = duckdb_backend().await;
    let duck_side = tempfile::TempDir::new().expect("temp dir");
    let duck_tx = retention_transactional(&duck_side).await;
    let duck_service = Arc::clone(&duckdb.0);
    let embedded = collected_leftover_answers(&duckdb, async {
        duck_service
            .run_retention_to_completion(&config, 0, None, duck_tx.clone(), duck_tx.clone())
            .await
            .expect("DuckDB retention");
    })
    .await;

    let clickhouse = clickhouse_backend(&url, "sideseat_parity_collected_leftover").await;
    let ch_side = tempfile::TempDir::new().expect("temp dir");
    let ch_tx = retention_transactional(&ch_side).await;
    let ch_service = Arc::clone(&clickhouse.0);
    let distributed = collected_leftover_answers(&clickhouse, async {
        ch_service
            .run_retention_to_completion(&config, 0, None, ch_tx.clone(), ch_tx.clone())
            .await
            .expect("ClickHouse retention");
    })
    .await;

    assert_eq!(
        embedded,
        vec![
            "records after collection: 0".to_string(),
            "rows after collection: [\"span-left\"]".to_string(),
            "rows after retention: []".to_string(),
        ]
    );
    assert_eq!(embedded, distributed, "DuckDB (reference) and ClickHouse");
}

/// One shard on two replicas, the second lagging: a span the first acknowledged, naming an attachment, is not yet
/// on the second. A survivor reconciliation made through the second must still see the span - its reads go to
/// every replica - and keep the file; one that read the second replica's own tables released the association
/// and reclaimed the bytes of a span that exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_replica_reconciliation_keeps_a_file_whose_span_a_lagging_replica_lacks() {
    let (Ok(url), Ok(second_url)) = (
        std::env::var(TWO_REPLICA_URL_ENV),
        std::env::var(TWO_REPLICA_SECOND_URL_ENV),
    ) else {
        eprintln!(
            "clickhouse two-replica: skipped - set {TWO_REPLICA_URL_ENV} and {TWO_REPLICA_SECOND_URL_ENV} (or run \
             `make test-clickhouse-two-replica`)"
        );
        return;
    };
    let database = "sideseat_two_replica_survivors";
    let first: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(replicated_backend_at(&url, database).await);
    let second = sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
        ClickhouseService::init(
            &layout_config(&second_url, database, true),
            Arc::new(sideseat_server::runtime::clock::SystemClock),
        )
        .await
        .expect("the second replica's service"),
    ));
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let lagging = raw_client_at(&second_url, database, &user, &password).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    let fetches = |verb: &'static str| {
        let lagging = lagging.clone();
        async move {
            for table in ["otel_spans_local", "otel_raw_local"] {
                lagging
                    .query(&format!("SYSTEM {verb} FETCHES {table}"))
                    .execute()
                    .await
                    .expect("toggle the second replica's fetches");
            }
        }
    };
    fetches("STOP").await;

    let FilePipeline {
        _temp,
        pipeline,
        transactional,
        files,
        blobs: _,
    } = file_pipeline(&first).await;
    let outcomes = pipeline
        .run_batch_outcomes_for_test(&[export_with_attachment(41)])
        .await;
    let project = ProjectId::from("default");
    let trace = "29".repeat(16);
    let referenced = sideseat_domain::files::FileService::hashes_referenced_by_trace(
        &project,
        &trace,
        first.as_ref(),
    )
    .await;
    let lagging_rows: Result<Vec<String>, _> = lagging
        .query("SELECT span_id FROM otel_spans_local WHERE trace_id = ?")
        .bind(&trace)
        .fetch_all()
        .await;
    let reconciled = files
        .reconcile_trace_survivors(&project, std::slice::from_ref(&trace), &second)
        .await;
    fetches("START").await;

    assert_eq!(
        outcomes,
        vec![sideseat_ingestion::traces::IngestOutcome::Stored]
    );
    let referenced = referenced.expect("references");
    assert!(!referenced.is_empty(), "the span names its attachment");
    assert!(
        lagging_rows.expect("the lagging replica's rows").is_empty(),
        "the fixture must lag: the second replica already holds the span"
    );
    reconciled.expect("the reconciliation");
    for hash in &referenced {
        let file = transactional
            .get_file(&project, hash)
            .await
            .expect("file row")
            .expect("the live span's file is still registered");
        assert!(
            file.ref_count > 0,
            "the live span's file is still owned: {file:?}"
        );
        files
            .get_file(&project, hash)
            .await
            .expect("the live span's attachment is still readable");
    }
}

/// A restore on one shard of two replicas, the second lagging, from a transactional backup that lacks a live
/// trace's file associations: the trace is found only on the first replica, and the restore runs through the
/// second. Its discovery reads every replica, so it finds the trace, rebuilds the association and keeps the file;
/// one that listed the second replica's traces found nothing that names the file, and its orphan sweep deleted the
/// bytes of a span that exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_replica_restore_keeps_a_file_whose_trace_a_lagging_replica_lacks() {
    let (Ok(url), Ok(second_url)) = (
        std::env::var(TWO_REPLICA_URL_ENV),
        std::env::var(TWO_REPLICA_SECOND_URL_ENV),
    ) else {
        eprintln!(
            "clickhouse two-replica: skipped - set {TWO_REPLICA_URL_ENV} and {TWO_REPLICA_SECOND_URL_ENV} (or run \
             `make test-clickhouse-two-replica`)"
        );
        return;
    };
    let database = "sideseat_two_replica_restore";
    let first: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(replicated_backend_at(&url, database).await);
    let second: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(sideseat_adapter_clickhouse::ClickhouseRepository(Arc::new(
            ClickhouseService::init(
                &layout_config(&second_url, database, true),
                Arc::new(sideseat_server::runtime::clock::SystemClock),
            )
            .await
            .expect("the second replica's service"),
        )));
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let lagging = raw_client_at(&second_url, database, &user, &password).with_option(
        sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
        "1",
    );
    let fetches = |verb: &'static str| {
        let lagging = lagging.clone();
        async move {
            for table in ["otel_spans_local", "otel_raw_local"] {
                lagging
                    .query(&format!("SYSTEM {verb} FETCHES {table}"))
                    .execute()
                    .await
                    .expect("toggle the second replica's fetches");
            }
        }
    };
    fetches("STOP").await;

    let FilePipeline {
        _temp,
        pipeline,
        transactional,
        files,
        blobs: _,
    } = file_pipeline(&first).await;
    let outcomes = pipeline
        .run_batch_outcomes_for_test(&[export_with_attachment(43)])
        .await;
    let project = ProjectId::from("default");
    let trace = "2b".repeat(16);
    let referenced = sideseat_domain::files::FileService::hashes_referenced_by_trace(
        &project,
        &trace,
        first.as_ref(),
    )
    .await;
    // The transactional backup predates the trace's associations.
    let unowned = transactional
        .delete_trace_files(&project, std::slice::from_ref(&trace))
        .await;
    if let Ok(hashes) = &unowned {
        for hash in hashes {
            transactional
                .sync_ref_count(&project, hash)
                .await
                .expect("the backup's count");
        }
    }
    let lagging_rows: Result<Vec<String>, _> = lagging
        .query("SELECT span_id FROM otel_spans_local WHERE trace_id = ?")
        .bind(&trace)
        .fetch_all()
        .await;
    let restored =
        sideseat_domain::restore::reconcile_restored_associations(&transactional, &second, &files)
            .await;
    fetches("START").await;

    assert_eq!(
        outcomes,
        vec![sideseat_ingestion::traces::IngestOutcome::Stored]
    );
    let referenced = referenced.expect("references");
    assert!(!referenced.is_empty(), "the span names its attachment");
    assert!(
        !unowned.expect("drop the associations").is_empty(),
        "the premise: the backup holds no association for the trace"
    );
    assert!(
        lagging_rows.expect("the lagging replica's rows").is_empty(),
        "the fixture must lag: the second replica already holds the span"
    );
    restored.expect("the restore");
    for hash in &referenced {
        let file = transactional
            .get_file(&project, hash)
            .await
            .expect("file row")
            .expect("the live span's file is still registered");
        assert!(
            file.ref_count > 0,
            "the restore owns the live span's file again: {file:?}"
        );
        files
            .get_file(&project, hash)
            .await
            .expect("the live span's attachment is still readable");
    }
}
