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
