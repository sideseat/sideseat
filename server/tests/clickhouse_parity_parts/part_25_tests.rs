/// One log identity written many ways - several copies in one batch, an older redelivery, writers racing - reads as
/// one record on both backends, the revision written last: DuckDB keeps it by replacing on write, with no index to
/// refuse a second row, ClickHouse by its ReplacingMergeTree on `ingested_at`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_log_identity_written_many_ways_reads_as_one_record_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let clickhouse: Arc<dyn AnalyticsRepository + Send + Sync> =
        Arc::new(clickhouse_backend(&url, "sideseat_parity_log_identity").await);
    let (_temp, duck) = duckdb_backend().await;
    let duck: Arc<dyn AnalyticsRepository + Send + Sync> = Arc::new(duck);
    let copy = |second: i64| NormalizedLog {
        project_id: Some(PROJECT.to_string()),
        log_digest: "a".repeat(64),
        ordinal: 0,
        timestamp: ts(900),
        time: Some(ts(900)),
        severity_text: Some("WARN".to_string()),
        body: serde_json::json!("one identity"),
        body_text: Some("one identity".to_string()),
        raw_log: serde_json::json!({"body": {"stringValue": "one identity"}}),
        ingested_at: Some(ts(1000 + second)),
        ..Default::default()
    };
    let mut answers = Vec::new();
    for backend in [&duck, &clickhouse] {
        backend
            .insert_logs(&[copy(5), copy(9), copy(7)])
            .await
            .expect("one batch");
        backend
            .insert_logs(&[copy(3)])
            .await
            .expect("an older redelivery");
        let writers: Vec<_> = (10..18)
            .map(|second| {
                let backend = Arc::clone(backend);
                tokio::spawn(async move { backend.insert_logs(&[copy(second)]).await })
            })
            .collect();
        for writer in writers {
            writer.await.expect("a writer").expect("a write");
        }
        let (rows, total) = backend
            .list_logs(&sideseat_ports::types::ListLogsParams {
                project_id: ProjectId::from(PROJECT),
                page: 1,
                limit: 10,
                ..Default::default()
            })
            .await
            .expect("list");
        let one = backend
            .get_log(&ProjectId::from(PROJECT), &"a".repeat(64), 0)
            .await
            .expect("get")
            .expect("the record");
        answers.push((total, rows.len(), one.ingested_at, one.body_text));
    }
    assert_eq!(answers[0], answers[1]);
    assert_eq!(
        answers[0],
        (1, 1, ts(1017), Some("one identity".to_string())),
        "one record, written last"
    );
}
