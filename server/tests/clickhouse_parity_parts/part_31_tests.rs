// The retention sweep's deferred trace, on both analytics backends.

/// What one backend's retention sweep does with a trace whose file a writer is still pending on, as a transcript:
/// the span is evicted, the sweep's reconciliation skips the association and defers the trace, so the cleanup
/// stays owed; the writer aborts, and the owed retry releases the association.
async fn deferred_sweep_answers(
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    retention: impl AsyncFnOnce(
        Arc<dyn sideseat_ports::blobs::RetentionFileReconciler>,
        Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>,
        Arc<dyn sideseat_ports::traits::StorageGovernance + Send + Sync>,
    ),
) -> Vec<String> {
    let (
        FilePipeline {
            _temp,
            pipeline,
            transactional,
            files,
            blobs: _,
        },
        governance,
    ) = file_pipeline_governed(&analytics).await;
    let project = ProjectId::from("default");
    let trace = "47".repeat(16);
    let mut answers = Vec::new();

    let outcomes = pipeline
        .run_batch_outcomes_for_test(&[export_with_attachment(0x47)])
        .await;
    answers.push(format!("ingested: {outcomes:?}"));
    let hashes = sideseat_domain::files::FileService::hashes_referenced_by_trace(
        &project,
        &trace,
        analytics.as_ref(),
    )
    .await
    .expect("references");
    assert_eq!(hashes.len(), 1, "the span names one attachment");
    let hash = hashes[0].clone();
    // No hold is active, and the held-bytes read answers so - a read that compares the same column with an
    // instant the pressure batch's does.
    let held = sideseat_ports::traits::AnalyticsMaintenance::project_held_logical_bytes(
        analytics.as_ref(),
        &project,
        chrono::Utc::now(),
    )
    .await
    .expect("held bytes");
    answers.push(format!("held bytes: {held}"));

    // A batch in flight references the same file in the same trace: one pending writer on the durable row.
    transactional
        .associate_file(&trace, &project, &hash, Some("image/png"), 4096, "blake3")
        .await
        .expect("a writer associates");

    let reconciler: Arc<dyn sideseat_ports::blobs::RetentionFileReconciler> = files.clone();
    retention(reconciler.clone(), transactional.clone(), governance).await;
    let left = analytics
        .get_spans_for_trace(&project, &trace, 10)
        .await
        .expect("spans");
    answers.push(format!("spans after retention: {}", left.len()));
    let associated =
        |transactional: Arc<dyn sideseat_ports::traits::TransactionalRepository + Send + Sync>| {
            let project = project.clone();
            let trace = trace.clone();
            async move {
                transactional
                    .get_file_hashes_for_traces(&project, std::slice::from_ref(&trace))
                    .await
                    .expect("associations")
                    .len()
            }
        };
    answers.push(format!(
        "associations after retention: {}",
        associated(transactional.clone()).await
    ));
    let owed = transactional
        .claim_retention_cleanup(16, 0)
        .await
        .expect("claim");
    answers.push(format!(
        "owed after retention: {:?}",
        owed.iter()
            .map(|(project, trace, _)| format!("{project}/{trace}"))
            .collect::<Vec<_>>()
    ));

    // The writer aborts: its count resolves, and the durable row is left referenced by nothing.
    transactional
        .release_trace_file_association(&project, &trace, &hash)
        .await
        .expect("the writer aborts");
    let deferred = reconciler
        .reconcile_trace_survivors(&project, std::slice::from_ref(&trace), analytics.as_ref())
        .await
        .expect("the owed retry");
    answers.push(format!("deferred on the retry: {deferred:?}"));
    answers.push(format!(
        "associations after the retry: {}",
        associated(transactional.clone()).await
    ));
    answers
}

/// A trace whose file a writer is pending on when retention evicts its span is deferred, not completed, by both
/// backends' sweeps: its cleanup stays owed, and the retry after the writer resolves releases the association.
/// Completing it lost the cleanup, and the abort then left the file associated, referenced by nothing, for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sweep_defers_a_trace_a_writer_is_pending_on_alike() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    // Every span over a limit of none: the pressure path, which both backends finish in the same pass.
    let config = sideseat_core::config::RetentionConfig {
        max_age_minutes: None,
        max_spans: Some(0),
    };

    let (_duck_temp, duckdb) = duckdb_backend().await;
    let duck_service = Arc::clone(&duckdb.0);
    let embedded = deferred_sweep_answers(Arc::new(duckdb), async |files, database, governance| {
        duck_service
            .run_retention_to_completion(&config, u64::MAX, Some(files), database, governance)
            .await
            .expect("DuckDB retention");
    })
    .await;

    let clickhouse = clickhouse_backend(&url, "sideseat_parity_deferred_sweep").await;
    let ch_service = Arc::clone(&clickhouse.0);
    let distributed =
        deferred_sweep_answers(Arc::new(clickhouse), async |files, database, governance| {
            ch_service
                .run_retention_to_completion(&config, u64::MAX, Some(files), database, governance)
                .await
                .expect("ClickHouse retention");
        })
        .await;

    let trace = "47".repeat(16);
    assert_eq!(
        embedded,
        vec![
            "ingested: [Stored]".to_string(),
            "held bytes: 0".to_string(),
            "spans after retention: 0".to_string(),
            "associations after retention: 1".to_string(),
            format!("owed after retention: [\"default/{trace}\"]"),
            "deferred on the retry: []".to_string(),
            "associations after the retry: 0".to_string(),
        ]
    );
    assert_eq!(embedded, distributed, "DuckDB (reference) and ClickHouse");
}
