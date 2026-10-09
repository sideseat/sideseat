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

/// A correction of a datapoint written after a later one - a worker writing the copy it loaded - is stored at its
/// receipt, so it loses at once on both backends, and both answer the later correction as the winner settlement
/// reads.
#[tokio::test]
async fn a_correction_written_after_a_later_one_stays_superseded_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_correction_order").await;

    let base = NormalizedMetric {
        project_id: Some(PROJECT.to_string()),
        metric_name: "queue.depth".to_string(),
        metric_type: MetricType::Gauge,
        datapoint_id: "datapoint-receipt-order".to_string(),
        timestamp: ts(0),
        ..Default::default()
    };
    let earlier = NormalizedMetric {
        value_double: Some(1.0),
        content_digest: "earlier".to_string(),
        ingested_at: Some(ts(10)),
        ..base.clone()
    };
    let later = NormalizedMetric {
        value_double: Some(2.0),
        content_digest: "later".to_string(),
        ingested_at: Some(ts(20)),
        ..base
    };

    let project = ProjectId::from(PROJECT);
    let identity = ("datapoint-receipt-order".to_string(), ts(0));
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        // The later correction first, then the earlier one's late copy.
        repo.insert_metrics(std::slice::from_ref(&later))
            .await
            .expect("later correction");
        repo.insert_metrics(std::slice::from_ref(&earlier))
            .await
            .expect("late copy");
        let stored = repo
            .get_metric(&project, "datapoint-receipt-order")
            .await
            .expect("metric")
            .expect("metric exists");
        let winners = repo
            .metric_winners(&project, std::slice::from_ref(&identity))
            .await
            .expect("winners");
        let winner = winners.get(&identity.0).expect("a winner");
        answers.push((
            stored.value_double,
            winner.content_digest.clone(),
            winner.ingested_at,
        ));
    }
    assert_eq!(
        answers,
        vec![(Some(2.0), "later".to_string(), ts(20)); 2],
        "the correction received later must stay the winner on both backends"
    );
}

/// A record found through its traces' spans on a two-shard cluster, each project on its own shard. Both tables are
/// `Distributed` there, and a plain `IN` over one inside a read of the other is refused
/// (`distributed_product_mode = deny`), so the read failed on every cluster of more than one shard.
#[tokio::test]
async fn survivor_records_are_read_across_a_two_shard_cluster() {
    use sideseat_ports::traits::RawStore;

    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_survivors";
    let store = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);
    let mut per_shard: [Option<String>; 2] = [None, None];
    for n in 0..64 {
        let candidate = format!("survivor-shard-{n}");
        let shard: Vec<u64> = client
            .query("SELECT toUInt64((sipHash64(?) % 2) + 1)")
            .bind(&candidate)
            .fetch_all()
            .await
            .expect("compute the shard");
        let index = (shard[0] - 1) as usize;
        if per_shard[index].is_none() {
            per_shard[index] = Some(candidate);
        }
    }
    let [Some(near), Some(far)] = per_shard else {
        panic!("could not find a project id for each shard");
    };
    for project in [near, far] {
        let record = RawRecordRow {
            project_id: ProjectId::from(project.as_str()),
            signal_until: Utc::now(),
            ..raw_row("raw-survivor", 1, &["trace-survivor"], project.as_bytes())
        };
        store
            .insert_raw_records(std::slice::from_ref(&record))
            .await
            .expect("record");
        store
            .insert_spans(vec![NormalizedSpan {
                project_id: Some(project.clone()),
                timestamp_start: Utc::now(),
                ingested_at: Some(Utc::now()),
                ..span_of("trace-survivor", "span-survivor", "raw-survivor")
            }])
            .await
            .expect("a span naming the record");
        let survivors = store
            .survivor_raw_records(
                &ProjectId::from(project.as_str()),
                &["trace-survivor".to_string()],
            )
            .await
            .expect("the survivors read on a two-shard cluster");
        assert_eq!(
            survivors,
            vec![project.as_bytes().to_vec()],
            "{project}: its own record"
        );
    }
}
