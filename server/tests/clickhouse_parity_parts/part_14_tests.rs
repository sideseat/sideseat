/// A write ClickHouse refused outright leaves neither its raw record nor its file behind, and the retry stores
/// both again.
///
/// Records are written before their rows, so a refused write left a record no row names - which span retention
/// never finds - and its attachment was settled against rows that did not exist. The refusal here is a
/// constraint, checked before a block is written, so nothing of the write lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_write_leaves_neither_record_nor_file() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_refused_write";
    let clickhouse: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(clickhouse_backend(&url, database).await);
    let raw = raw_client(&url, database);
    raw.query("ALTER TABLE otel_spans ADD CONSTRAINT refused_write CHECK project_id != 'default'")
        .execute()
        .await
        .expect("fault constraint");
    let FilePipeline {
        _temp,
        pipeline,
        transactional,
        ..
    } = file_pipeline(&clickhouse).await;
    let export = export_with_attachment(32);
    let project = ProjectId::from("default");
    let trace = "20".repeat(16);

    assert_eq!(
        pipeline
            .run_batch_outcomes_for_test(std::slice::from_ref(&export))
            .await,
        vec![sideseat_ingestion::traces::IngestOutcome::Failed]
    );
    assert!(
        transactional
            .get_file_hashes_for_traces(&project, std::slice::from_ref(&trace))
            .await
            .expect("associations")
            .is_empty(),
        "no stored row names the attachment, so the trace holds no file"
    );
    assert!(
        !transactional
            .get_orphan_files()
            .await
            .expect("orphans")
            .is_empty(),
        "the attachment is the sweeper's"
    );
    for _ in 0..4 {
        pipeline.reconcile_raw_records(64).await.expect("reconcile");
    }
    assert!(
        clickhouse
            .raw_records_page(&project, None, 16)
            .await
            .expect("records")
            .is_empty(),
        "the record no row names is collected"
    );

    // The retry, once the store accepts the rows, stores the record and holds the file again.
    raw.query("ALTER TABLE otel_spans DROP CONSTRAINT refused_write")
        .execute()
        .await
        .expect("clear the fault");
    assert_eq!(
        pipeline
            .run_batch_outcomes_for_test(std::slice::from_ref(&export))
            .await,
        vec![sideseat_ingestion::traces::IngestOutcome::Stored]
    );
    assert_eq!(
        clickhouse
            .raw_records_page(&project, None, 16)
            .await
            .expect("records")
            .len(),
        1
    );
    assert!(
        transactional
            .get_orphan_files()
            .await
            .expect("orphans")
            .is_empty(),
        "the retry's rows hold the attachment"
    );
}
