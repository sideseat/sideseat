/// The two reconciliation reads, on ClickHouse: which fields carry file references, and which traces are empty.
///
/// Both were written for the survivor reconciliation and only DuckDB's had behavioural coverage - the file tests
/// hardwire DuckDB plus SQLite, so ClickHouse's `FINAL` versions of both queries were never executed by any
/// test. Two hand-written statements per question is exactly what the parity suite exists to compare, and these
/// two decide whether a live span keeps its file.
///
/// `FINAL` is the part with teeth: an expired revision's text must not keep an association alive, and an
/// obsolete revision must not make a deleted trace look alive. So the fixture corrects both a span's *content*
/// and a trace's existence, which is what tells a `FINAL` read from a raw one.
#[tokio::test]
async fn the_reconciliation_reads_agree_with_duckdb() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_reconcile").await;

    let old_hash = "a".repeat(64);
    let new_hash = "b".repeat(64);
    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "reconcile-trace".to_string(),
        span_id: "reconcile-span".to_string(),
        span_name: "generation".to_string(),
        timestamp_start: ts(1),
        timestamp_end: Some(ts(1)),
        ..Default::default()
    };

    // A span whose correction *replaces* which file it references. The obsolete revision still names the old
    // hash, so a raw read would keep an association the current span does not justify.
    for (hash, ingested) in [(&old_hash, ts(10)), (&new_hash, ts(20))] {
        let spans = vec![NormalizedSpan {
            messages: Some(format!(r#"[{{"content":"see #!B64!#image/png::{hash}"}}]"#)),
            ingested_at: Some(ingested),
            ..base.clone()
        }];
        duck.insert_spans(spans.clone())
            .await
            .expect("duckdb insert");
        ch.insert_spans(spans).await.expect("clickhouse insert");
    }

    let traces = vec!["reconcile-trace".to_string(), "never-existed".to_string()];

    let mut d_fields = duck
        .file_reference_fields_for_traces(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("duckdb fields");
    let mut c_fields = ch
        .file_reference_fields_for_traces(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("clickhouse fields");
    d_fields.sort();
    c_fields.sort();
    assert_eq!(
        d_fields, c_fields,
        "the two backends disagree about which field text a surviving span carries, so a file kept on one is \
         released on the other"
    );
    assert!(
        c_fields.iter().any(|f| f.contains(&new_hash)),
        "the correction's reference is missing, so its file would be released: {c_fields:?}"
    );
    assert!(
        !c_fields.iter().any(|f| f.contains(&old_hash)),
        "the *obsolete* revision's reference is still returned, so a file the current span does not reference \
         is kept forever - the read is not going through FINAL: {c_fields:?}"
    );

    let mut d_empty = duck
        .traces_without_spans(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("duckdb empty");
    let mut c_empty = ch
        .traces_without_spans(&ProjectId::from(PROJECT), &traces)
        .await
        .expect("clickhouse empty");
    d_empty.sort();
    c_empty.sort();
    assert_eq!(
        d_empty, c_empty,
        "the two backends disagree about which traces retention emptied, so a favourite survives on one and is \
         removed on the other"
    );
    assert_eq!(
        c_empty,
        vec!["never-existed".to_string()],
        "only the trace with no spans is empty; the corrected one is still there"
    );
}

/// Restore discovery must not assume that every project has spans.
///
/// A newer analytics backup can contain a metric-only or log-only project whose transactional metadata is
/// outside the selected recovery point. If the maintenance query only enumerates spans, restore repair never
/// sees that project and its rows remain unreachable forever.
#[tokio::test]
async fn restore_project_discovery_covers_every_signal_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_restore_projects").await;
    let span_project = "a-span-only";
    let metric_project = "b-metric-only";
    let log_project = "c-log-only";

    let span = NormalizedSpan {
        project_id: Some(span_project.to_string()),
        trace_id: "restore-trace".to_string(),
        span_id: "restore-span".to_string(),
        timestamp_start: ts(1),
        timestamp_end: Some(ts(1)),
        ..Default::default()
    };
    duck.insert_spans(vec![span.clone()])
        .await
        .expect("duckdb span");
    ch.insert_spans(vec![span]).await.expect("clickhouse span");

    let metric = NormalizedMetric {
        project_id: Some(metric_project.to_string()),
        metric_name: "restore.metric".to_string(),
        metric_type: MetricType::Sum,
        aggregation_temporality: AggregationTemporality::Cumulative,
        timestamp: ts(2),
        value_int: Some(1),
        ..Default::default()
    };
    duck.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("duckdb metric");
    ch.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("clickhouse metric");

    let log = NormalizedLog {
        project_id: Some(log_project.to_string()),
        log_digest: "restore-log".to_string(),
        ordinal: 0,
        timestamp: ts(3),
        body: serde_json::json!("restore"),
        body_text: Some("restore".to_string()),
        ingested_at: Some(ts(3)),
        ..Default::default()
    };
    duck.insert_logs(std::slice::from_ref(&log))
        .await
        .expect("duckdb log");
    ch.insert_logs(std::slice::from_ref(&log))
        .await
        .expect("clickhouse log");

    let expected = [span_project, metric_project, log_project]
        .into_iter()
        .map(ProjectId::from)
        .collect::<Vec<_>>();
    assert_eq!(
        duck.analytics_project_ids(10)
            .await
            .expect("duckdb project discovery"),
        expected
    );
    assert_eq!(
        ch.analytics_project_ids(10)
            .await
            .expect("clickhouse project discovery"),
        expected
    );
    assert_eq!(
        ch.analytics_project_ids(2)
            .await
            .expect("bounded clickhouse project discovery"),
        expected[..2]
    );

    let metric_project = ProjectId::from(metric_project);
    duck.delete_project_data(&metric_project)
        .await
        .expect("duckdb project delete");
    ch.delete_project_data(&metric_project)
        .await
        .expect("clickhouse project delete");
    let after_delete = vec![expected[0].clone(), expected[2].clone()];
    assert_eq!(
        duck.analytics_project_ids(10)
            .await
            .expect("duckdb discovery after delete"),
        after_delete
    );
    assert_eq!(
        ch.analytics_project_ids(10)
            .await
            .expect("clickhouse discovery after delete"),
        after_delete
    );
}

/// A read that must span **both shards**: the consistency check, and the pre-identity metric count.
///
/// Both of these read the `Distributed` front end, and both read `_local` in a previous version - which on a
/// one-shard fixture is the same thing, so the fix was unfalsifiable. Here the rows are placed so that each
/// question has an answer on the shard the client is *not* connected to: a `_local` read reports clean or zero,
/// a distributed read finds them.
///
/// The sharding key is `sipHash64(project_id)`, so two project ids that hash to different shards are what makes
/// this work. They are found rather than assumed, by asking the cluster which shard each would land on - a
/// hardcoded pair would silently stop testing anything if the key ever changed.
#[tokio::test]
async fn a_two_shard_cluster_reports_anomalies_from_every_shard() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };

    let database = "sideseat_two_shard";
    let service = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);

    // Which shard each candidate project lands on, asked rather than assumed.
    let mut per_shard: [Option<String>; 2] = [None, None];
    for n in 0..64 {
        let candidate = format!("shard-probe-{n}");
        let shard: Vec<u64> = client
            // `toUInt64` explicitly: `(x % 2) + 1` narrows to **UInt16**, which a `Vec<u64>` fetch rejects
            // with a schema mismatch - and the failure reads as a broken cluster rather than a wrong type.
            .query("SELECT toUInt64((sipHash64(?) % 2) + 1)")
            .bind(&candidate)
            .fetch_all()
            .await
            .expect("compute the shard");
        let index = (shard[0] - 1) as usize;
        if per_shard[index].is_none() {
            per_shard[index] = Some(candidate);
        }
        if per_shard.iter().all(Option::is_some) {
            break;
        }
    }
    let [Some(near), Some(far)] = per_shard else {
        panic!("could not find a project id for each shard");
    };

    // A cross-month correction on **each** shard's project, so whichever shard the client reaches, the other
    // one also holds an anomaly. A `_local` read finds at most one.
    let now = Utc::now();
    let this_month = Utc
        .with_ymd_and_hms(now.year(), now.month(), 5, 12, 0, 0)
        .unwrap();
    let last_month = this_month - chrono::Duration::days(20);

    for project in [&near, &far] {
        for (instant, tokens, ingested) in [(last_month, 100, ts(10)), (this_month, 900, ts(20))] {
            service
                .insert_spans(vec![NormalizedSpan {
                    project_id: Some(project.clone()),
                    trace_id: format!("{project}-trace"),
                    span_id: format!("{project}-span"),
                    span_name: "generation".to_string(),
                    observation_type: Some(ObservationType::Generation),
                    timestamp_start: instant,
                    timestamp_end: Some(instant),
                    ingested_at: Some(ingested),
                    gen_ai_usage_input_tokens: tokens,
                    duration_ms: 1000,
                    ..Default::default()
                }])
                .await
                .expect("insert a cross-month correction");
        }
    }

    let policy_count: u64 = client
        .query(&format!(
            "SELECT count() FROM clusterAllReplicas('{REPLICATED_CLUSTER}', system.row_policies) \
             WHERE database = ? AND short_name = 'sideseat_tenant_filter'"
        ))
        .bind(database)
        .fetch_one()
        .await
        .expect("inspect every shard's local row policies");
    assert_eq!(
        policy_count, 14,
        "seven physical-table policies must exist on each of the two shards"
    );

    let tenant_client = client.clone().with_option(
        sideseat_adapter_clickhouse::schema::TENANT_PROJECT_SETTING,
        &near,
    );
    let tenant_rows: Vec<String> = tenant_client
        .query(
            // No explicit project predicate: each remote `_local` policy has to receive the setting
            // forwarded with this Distributed query.
            "SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id",
        )
        .fetch_all()
        .await
        .expect("read one tenant through the Distributed table");
    assert_eq!(tenant_rows, vec![near.clone()]);

    let unscoped_rows: Vec<String> = client
        .query("SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id")
        .fetch_all()
        .await
        .expect("run an unscoped Distributed read");
    assert!(
        unscoped_rows.is_empty(),
        "an unset setting must fail closed on every shard"
    );

    let maintenance_rows: Vec<String> = client
        .clone()
        .with_option(
            sideseat_adapter_clickhouse::schema::TENANT_MAINTENANCE_SETTING,
            "1",
        )
        .query("SELECT DISTINCT project_id FROM otel_spans ORDER BY project_id")
        .fetch_all()
        .await
        .expect("read every tenant through the explicit maintenance path");
    let mut expected_projects = vec![near.clone(), far.clone()];
    expected_projects.sort();
    assert_eq!(maintenance_rows, expected_projects);

    let outcome = service
        .check_partition_consistency()
        .await
        .expect("the consistency check must run on a multi-shard cluster");
    assert_eq!(
        outcome.anomalies, 2,
        "the check found {} of 2 anomalies. One sits on each shard, so anything less means the query reads only \
         the shard the connection reached - and on a real cluster most anomalies are then invisible. A failure \
         *running* it means the distributed subquery was refused (`distributed_product_mode = deny`), which is \
         the other half of the same finding",
        outcome.anomalies
    );

    let recorded = service
        .partition_anomalies()
        .await
        .expect("the anomaly records are readable");
    assert_eq!(
        recorded.len(),
        2,
        "the anomaly *records* are per-shard, so a report described as deployment-wide depends on which shard \
         answered: {recorded:?}"
    );
}
