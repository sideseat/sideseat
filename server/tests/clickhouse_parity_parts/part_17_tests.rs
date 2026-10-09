/// A revision written after a later one - a worker writing the copy it loaded - is stored at its receipt, so it
/// is superseded at once on both backends, and both answer the later revision as the winner settlement reads.
#[tokio::test]
async fn a_revision_written_after_a_later_one_stays_superseded_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_receipt_order").await;

    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-receipt-order".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        ..Default::default()
    };
    let earlier = NormalizedSpan {
        gen_ai_usage_total_tokens: 100,
        content_digest: "earlier".to_string(),
        ingested_at: Some(ts(10)),
        ..base.clone()
    };
    let later = NormalizedSpan {
        gen_ai_usage_total_tokens: 900,
        content_digest: "later".to_string(),
        ingested_at: Some(ts(20)),
        ..base
    };

    let identity = ("trace-receipt-order".to_string(), "span-1".to_string());
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        // The later revision first, then the earlier one's late copy.
        repo.insert_spans(vec![later.clone()])
            .await
            .expect("later revision");
        repo.insert_spans(vec![earlier.clone()])
            .await
            .expect("late copy");
        let trace = repo
            .get_trace(&ProjectId::from(PROJECT), "trace-receipt-order")
            .await
            .expect("trace")
            .expect("trace exists");
        let winners = repo
            .span_winners(&ProjectId::from(PROJECT), std::slice::from_ref(&identity))
            .await
            .expect("winners");
        let winner = winners.get(&identity).expect("a winner");
        answers.push((
            trace.total_tokens,
            winner.content_digest.clone(),
            winner.ingested_at,
        ));
    }
    assert_eq!(
        answers,
        vec![(900, "later".to_string(), ts(20)); 2],
        "the revision received later must stay the winner on both backends"
    );
}

/// A datapoint reads back alike from both backends, whole: by its id, in a page of the project's datapoints, in their
/// aggregates and in the filter options.
/// Its JSON-text columns are what a ClickHouse row decodes by name, so one projected under another name failed
/// every metric read there.
#[tokio::test]
async fn a_datapoint_reads_back_alike_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_metric_reads").await;

    let metric = NormalizedMetric {
        project_id: Some(PROJECT.to_string()),
        metric_name: "queue.depth".to_string(),
        metric_type: MetricType::Gauge,
        datapoint_id: "datapoint-read".to_string(),
        content_digest: "read".to_string(),
        timestamp: ts(0),
        start_timestamp: Some(ts(-1)),
        value_double: Some(4.5),
        service_name: Some("queue-service".to_string()),
        scope_name: Some("queue.meter".to_string()),
        attributes: serde_json::json!({"queue": "orders"}),
        resource_attributes: serde_json::json!({"service.name": "queue-service"}),
        scope_attributes: serde_json::json!({"meter.kind": "gauge"}),
        exemplars: serde_json::json!([{"value": 4.5}]),
        raw_metric: serde_json::json!({"name": "queue.depth"}),
        ingested_at: Some(ts(10)),
        ..Default::default()
    };
    let project = ProjectId::from(PROJECT);
    let params = sideseat_ports::types::ListMetricsParams {
        project_id: project.clone(),
        page: 1,
        limit: 10,
        ..Default::default()
    };
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_metrics(std::slice::from_ref(&metric))
            .await
            .expect("metric");
        let stored = repo
            .get_metric(&project, "datapoint-read")
            .await
            .expect("get metric")
            .expect("metric exists");
        let (page, total) = repo.list_metrics(&params).await.expect("list metrics");
        let aggregates = repo
            .aggregate_metrics(&params)
            .await
            .expect("aggregate metrics");
        let options: std::collections::BTreeMap<_, _> = repo
            .get_metric_filter_options(
                &project,
                &["metric_name".to_string(), "service_name".to_string()],
                None,
                None,
            )
            .await
            .expect("filter options")
            .into_iter()
            .map(|(column, rows)| (column, format!("{rows:?}")))
            .collect();
        // Agreement alone would pass two backends that both read nothing.
        assert_eq!(stored.attributes.as_deref(), Some(r#"{"queue":"orders"}"#));
        assert_eq!((page.len(), total), (1, 1), "the page holds the datapoint");
        assert_eq!(
            aggregates
                .iter()
                .map(|row| (row.metric_name.as_str(), row.data_points, row.value_sum))
                .collect::<Vec<_>>(),
            vec![("queue.depth", 1, Some(4.5))],
            "the aggregate counts the datapoint"
        );
        assert_eq!(
            options.get("metric_name").map(String::as_str),
            Some(r#"[FilterOptionRow { value: "queue.depth", count: 1 }]"#),
            "the options offer the datapoint's name"
        );
        answers.push(format!(
            "{stored:?} {page:?} {total} {aggregates:?} {options:?}"
        ));
    }
    assert_eq!(
        answers[0], answers[1],
        "a datapoint must read back alike on both backends"
    );
}
