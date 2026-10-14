/// A payload's worth of identities is answered in one call on both backends: thousands of 64-character ids, which
/// written into one statement passed ClickHouse's `max_query_size` and failed every staged-payload sweep. Every
/// confirmation, winner and record lookup answers for all of them, as DuckDB's already did, and a pressure
/// eviction's worth of spans is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_payload_s_worth_of_identities_is_confirmed_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    // Past 256 KiB as 64-hex ids bound into one statement, with room to spare.
    const IDENTITIES: usize = 6_000;
    let (_duck_dir, duckdb) = duckdb_backend().await;
    let clickhouse = clickhouse_backend(&url, "sideseat_parity_payload_confirmations").await;
    let project = ProjectId::from(PROJECT);
    let at = Utc::now() - chrono::Duration::hours(1);
    let id = |kind: &str, n: usize| format!("{:0>64}", format!("{kind}{n}"));
    let metrics: Vec<NormalizedMetric> = (0..IDENTITIES)
        .map(|n| NormalizedMetric {
            project_id: Some(PROJECT.to_string()),
            metric_name: "queue.depth".to_string(),
            metric_type: MetricType::Gauge,
            datapoint_id: id("d", n),
            content_digest: id("c", n),
            timestamp: at + chrono::Duration::milliseconds(i64::try_from(n % 100).expect("small")),
            value_int: Some(1),
            ingested_at: Some(at),
            ..Default::default()
        })
        .collect();
    let spans: Vec<NormalizedSpan> = (0..IDENTITIES)
        .map(|n| NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: id("t", n),
            session_id: Some(id("e", n)),
            raw_id: Some(id("r", n)),
            span_id: id("s", n),
            span_name: "generation".to_string(),
            timestamp_start: at,
            content_digest: id("g", n),
            ingested_at: Some(at),
            ..Default::default()
        })
        .collect();
    let logs: Vec<sideseat_ports::types::NormalizedLog> = (0..IDENTITIES)
        .map(|n| sideseat_ports::types::NormalizedLog {
            project_id: Some(PROJECT.to_string()),
            log_digest: id("l", n),
            timestamp: at,
            ingested_at: Some(at),
            raw_log: serde_json::json!({"body": {"stringValue": "a payload's log"}}),
            ..Default::default()
        })
        .collect();
    let datapoints: Vec<(String, DateTime<Utc>)> = metrics
        .iter()
        .map(|m| (m.datapoint_id.clone(), m.timestamp))
        .collect();
    let span_records: Vec<(String, String, String)> = spans
        .iter()
        .map(|s| {
            (
                s.trace_id.clone(),
                s.span_id.clone(),
                s.content_digest.clone(),
            )
        })
        .collect();
    let span_identities: Vec<(String, String)> = spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();
    let log_records: Vec<(String, u32, Option<DateTime<Utc>>)> = logs
        .iter()
        .map(|l| (l.log_digest.clone(), 0, Some(l.timestamp)))
        .collect();
    for backend in [
        &duckdb as &(dyn AnalyticsRepository + Send + Sync),
        &clickhouse as &(dyn AnalyticsRepository + Send + Sync),
    ] {
        for chunk in metrics.chunks(1_000) {
            backend.insert_metrics(chunk).await.expect("metrics");
        }
        backend.insert_spans(spans.clone()).await.expect("spans");
        backend.insert_logs(&logs).await.expect("logs");
        let winners = backend
            .metric_winners(&project, &datapoints)
            .await
            .expect("metric winners");
        assert_eq!(winners.len(), IDENTITIES, "every datapoint's winner");
        assert!(
            backend
                .spans_match_content(&project, &span_records)
                .await
                .expect("span confirmation"),
            "every span is stored as staged"
        );
        assert_eq!(
            backend
                .spans_with_matching_content(&project, &span_records)
                .await
                .expect("matching spans")
                .len(),
            IDENTITIES
        );
        assert_eq!(
            backend
                .span_winners(&project, &span_identities)
                .await
                .expect("span winners")
                .len(),
            IDENTITIES
        );
        assert!(
            backend
                .logs_match_content(&project, &log_records)
                .await
                .expect("log confirmation"),
            "every log is stored as staged"
        );
        let traces: Vec<String> = spans.iter().map(|s| s.trace_id.clone()).collect();
        assert_eq!(
            backend
                .get_trace_session_pairs(&project, &[traces.clone(), traces.clone()].concat(), None)
                .await
                .expect("trace sessions")
                .len(),
            IDENTITIES
        );
        assert_eq!(
            backend
                .get_span_counts_bulk(&project, &span_identities)
                .await
                .expect("span counts")
                .len(),
            IDENTITIES
        );
        assert_eq!(
            backend
                .span_raw_ids(&project, &span_identities)
                .await
                .expect("span records")
                .len(),
            IDENTITIES
        );
        assert_eq!(
            backend
                .raw_records_named_in_traces(&project, &traces)
                .await
                .expect("records named")
                .len(),
            IDENTITIES
        );
        // And one record that is not stored makes the whole answer false, wherever it falls.
        let mut missing = span_records.clone();
        missing[IDENTITIES - 1].2 = id("x", 0);
        assert!(
            !backend
                .spans_match_content(&project, &missing)
                .await
                .expect("span confirmation"),
            "a record not stored is not confirmed, in the last chunk too"
        );
        // A pressure eviction's worth of spans deleted in one call, and every trace found emptied.
        backend
            .delete_spans(&project, &span_identities)
            .await
            .expect("a payload's spans deleted");
        assert_eq!(
            backend
                .traces_without_spans(&project, &traces)
                .await
                .expect("emptied traces")
                .len(),
            IDENTITIES
        );
    }
}
