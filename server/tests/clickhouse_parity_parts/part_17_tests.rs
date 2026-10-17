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
        exemplars: vec![sideseat_ports::types::MetricExemplar {
            value_double: Some(4.5),
            ..Default::default()
        }],
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

/// Versions of one record received in two months read back as one latest version on both backends, by id, by its
/// traces and in replay. ClickHouse resolves `FINAL` a partition at a time, and the records were partitioned by
/// receipt month, so each month's version was a latest one: a record a deletion rewrote read back with its removed
/// content beside the rewrite. Two first deliveries of one body straddling a month still write two versions.
#[tokio::test]
async fn a_record_whose_versions_span_two_months_reads_back_once() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_raw_months").await;

    let january = Utc.with_ymd_and_hms(2026, 1, 31, 23, 0, 0).unwrap();
    let february = Utc.with_ymd_and_hms(2026, 2, 1, 1, 0, 0).unwrap();
    let first = RawRecordRow {
        received_at: january,
        // Within the table's time to live, whatever the receipt.
        signal_until: Utc::now(),
        ..raw_row("raw-months", 1, &["trace-months"], b"received in january")
    };
    let rewrite = RawRecordRow {
        received_at: february,
        origin: RawOrigin::Deleted,
        version: 3,
        trace_ids: Vec::new(),
        record: b"rewritten in february".to_vec(),
        ..first.clone()
    };
    let project = ProjectId::from(PROJECT);
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_raw_records(std::slice::from_ref(&first))
            .await
            .expect("first version");
        repo.append_raw_records(std::slice::from_ref(&rewrite))
            .await
            .expect("rewrite");
        repo.insert_spans(vec![NormalizedSpan {
            timestamp_start: Utc::now(),
            ingested_at: Some(Utc::now()),
            ..span_of("trace-months", "span-months", "raw-months")
        }])
        .await
        .expect("a span naming the record");
        let latest: Vec<(i64, String)> = repo
            .get_raw_records(&project, &["raw-months".to_string()])
            .await
            .expect("latest")
            .into_iter()
            .map(|row| {
                (
                    row.version,
                    String::from_utf8_lossy(&row.record).into_owned(),
                )
            })
            .collect();
        let survivors: Vec<String> = repo
            .survivor_raw_records(&project, &["trace-months".to_string()])
            .await
            .expect("survivors")
            .into_iter()
            .map(|record| String::from_utf8_lossy(&record).into_owned())
            .collect();
        // One record at a time, as a replay pages through them.
        let mut replay = Vec::new();
        let mut after = None;
        loop {
            let page = repo
                .raw_records_page(&project, after.clone(), 1)
                .await
                .expect("replay page");
            let Some(row) = page.into_iter().next() else {
                break;
            };
            after = Some((row.received_at, row.raw_id.clone()));
            replay.push(String::from_utf8_lossy(&row.record).into_owned());
        }
        answers.push((latest, survivors, replay));
    }
    let expected = (
        vec![(3, "rewritten in february".to_string())],
        vec!["rewritten in february".to_string()],
        vec!["rewritten in february".to_string()],
    );
    assert_eq!(answers, vec![expected.clone(), expected]);
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

/// The configuration a test service of `database` runs with: single-node, or distributed on the test cluster.
fn layout_config(url: &str, database: &str, distributed: bool) -> ClickhouseConfig {
    ClickhouseConfig {
        url: url.to_string(),
        database: database.to_string(),
        user: std::env::var(USER_ENV).ok(),
        password: std::env::var(PASSWORD_ENV).ok(),
        timeout_secs: 30,
        compression: false,
        async_insert: false,
        wait_for_async_insert: true,
        cluster: distributed.then(|| REPLICATED_CLUSTER.to_string()),
        distributed,
        insert_quorum: 0,
    }
}

/// Whether a service of `config` refuses to start for a partitioned raw-record table, naming the layout.
async fn refuses_partitioned_raw_records(config: &ClickhouseConfig) -> bool {
    match ClickhouseService::init(
        config,
        std::sync::Arc::new(sideseat_server::runtime::clock::SystemClock),
    )
    .await
    {
        Ok(_) => false,
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("partitioned by `toYYYYMM(received_at)`")
                    && message.contains("drop the configured database"),
                "the refusal names the layout and the reset: {message}"
            );
            true
        }
    }
}

/// A store created while the raw records were partitioned by receipt month is at the supported version, so the
/// version check accepts it, and it reads a record's versions in two months as two latest ones. It is refused at
/// startup instead, with the reset named, as a store at another version is.
#[tokio::test]
async fn a_store_with_month_partitioned_raw_records_is_refused_at_startup() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };
    let database = "sideseat_parity_raw_layout";
    let _created = clickhouse_backend(&url, database).await;
    let client = raw_client(&url, database);
    for statement in [
        "CREATE TABLE otel_raw_months AS otel_raw ENGINE = ReplacingMergeTree(version_key) \
         PARTITION BY toYYYYMM(received_at) ORDER BY (project_id, raw_id)",
        "DROP TABLE otel_raw SYNC",
        "RENAME TABLE otel_raw_months TO otel_raw",
    ] {
        client.query(statement).execute().await.expect(statement);
    }
    assert!(
        refuses_partitioned_raw_records(&layout_config(&url, database, false)).await,
        "a store with month-partitioned raw records started"
    );
}

/// The same refusal on a cluster, where the partitioned table is each shard's local one - through the shard that
/// created the store, and through the other, which holds no version row of its own and would otherwise create the
/// schema around the table it already has.
#[tokio::test]
async fn a_two_shard_store_with_month_partitioned_raw_records_is_refused_at_startup() {
    let Ok(url) = std::env::var(TWO_SHARD_URL_ENV) else {
        eprintln!(
            "clickhouse two-shard: skipped - set {TWO_SHARD_URL_ENV} (or run \
             `make test-clickhouse-two-shard`)"
        );
        return;
    };
    let database = "sideseat_two_shard_raw_layout";
    let _created = replicated_backend_at(&url, database).await;
    let user = std::env::var(USER_ENV).ok();
    let password = std::env::var(PASSWORD_ENV).ok();
    let client = raw_client_at(&url, database, &user, &password);
    for statement in [
        format!(
            "CREATE TABLE otel_raw_local_months ON CLUSTER {REPLICATED_CLUSTER} AS otel_raw_local \
             ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{{shard}}/{database}/otel_raw_months', \
             '{{replica}}', version_key) PARTITION BY toYYYYMM(received_at) ORDER BY (project_id, raw_id)"
        ),
        format!("DROP TABLE otel_raw_local ON CLUSTER {REPLICATED_CLUSTER} SYNC"),
        format!(
            "RENAME TABLE otel_raw_local_months TO otel_raw_local ON CLUSTER {REPLICATED_CLUSTER}"
        ),
    ] {
        client.query(&statement).execute().await.expect(&statement);
    }
    assert!(
        refuses_partitioned_raw_records(&layout_config(&url, database, true)).await,
        "a cluster store with month-partitioned raw records started"
    );
    match std::env::var("SIDESEAT_TEST_CLICKHOUSE_TWO_SHARD_SECOND_URL") {
        Ok(second) => assert!(
            refuses_partitioned_raw_records(&layout_config(&second, database, true)).await,
            "the store started through the shard that did not create it"
        ),
        Err(_) => eprintln!(
            "clickhouse two-shard: the second shard's URL is unset, so only the first is checked"
        ),
    }
}

/// A session is the traces whose earliest span carries its id, on both backends: an id seen only on a trace's later
/// spans names no session, and reading it finds none - where an aggregate over no spans was a row of nulls that
/// failed the read.
#[tokio::test]
async fn a_session_no_trace_belongs_to_is_not_found_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_session_membership").await;
    let span = |span_id: &str, session: &str, second: i64| NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "membership-trace".to_string(),
        span_id: span_id.to_string(),
        span_name: span_id.to_string(),
        session_id: Some(session.to_string()),
        timestamp_start: ts(second),
        ingested_at: Some(ts(100)),
        ..Default::default()
    };
    let project = ProjectId::from(PROJECT);
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![span("root", "first", 0), span("child", "later", 1)])
            .await
            .expect("spans");
        let first = repo.get_session(&project, "first").await.expect("first");
        assert_eq!(first.map(|session| session.trace_count), Some(1));
        assert!(
            repo.get_session(&project, "later")
                .await
                .expect("later")
                .is_none(),
            "a session no trace belongs to was found"
        );
    }
}

/// Frameworks that tie on their trace count come back by name on both backends: in the order an engine's threads
/// finished, one window's breakdown was a different list from one server to the next.
#[tokio::test]
async fn tied_frameworks_come_back_by_name_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_stats_ties").await;
    let names: Vec<String> = (0..20)
        .map(|n| format!("framework-{:02}", (n * 7) % 20))
        .collect();
    let spans: Vec<NormalizedSpan> = names
        .iter()
        .map(|name| NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: format!("trace-{name}"),
            span_id: "span".to_string(),
            span_name: "generation".to_string(),
            framework: Some(name.clone()),
            observation_type: Some(sideseat_ports::types::ObservationType::Generation),
            timestamp_start: ts(0),
            ingested_at: Some(ts(100)),
            ..Default::default()
        })
        .collect();
    let params = sideseat_ports::types::StatsParams {
        project_id: ProjectId::from(PROJECT),
        from_timestamp: ts(-3600),
        to_timestamp: ts(3600),
        timezone: "UTC".parse().unwrap(),
    };
    let mut sorted = names.clone();
    sorted.sort();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(spans.clone()).await.expect("spans");
        let stats = repo.get_project_stats(&params).await.expect("stats");
        let listed: Vec<String> = stats
            .by_framework
            .iter()
            .filter_map(|breakdown| breakdown.framework.clone())
            .collect();
        assert!(!listed.is_empty());
        assert_eq!(listed, sorted[..listed.len()].to_vec());
    }
}

/// A trace is its root's session, else its earliest span's, on both backends and through every read that resolves
/// membership: a child that started before its parent and names another session does not move the trace, and a
/// rootless trace takes its earliest span's.
#[tokio::test]
async fn a_trace_is_its_roots_session_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_root_session").await;
    let span = |trace: &str, span_id: &str, parent: Option<&str>, session: &str, second: i64| {
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent.map(str::to_string),
            span_name: span_id.to_string(),
            session_id: Some(session.to_string()),
            timestamp_start: ts(second),
            ingested_at: Some(ts(100)),
            ..Default::default()
        }
    };
    let project = ProjectId::from(PROJECT);
    let traces = ["rooted".to_string(), "rootless".to_string()];
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![
            span("rooted", "root", None, "root-session", 5),
            span("rooted", "early-child", Some("root"), "child-session", 0),
            span("rootless", "late", Some("gone"), "late-session", 9),
            span("rootless", "early", Some("gone"), "early-session", 1),
        ])
        .await
        .expect("spans");
        assert_eq!(
            repo.get_trace_session_pairs(&project, &traces, None)
                .await
                .expect("pairs"),
            vec![
                ("rooted".to_string(), "root-session".to_string()),
                ("rootless".to_string(), "early-session".to_string()),
            ]
        );
        assert_eq!(
            repo.get_trace_ids_for_sessions(&project, &["child-session".to_string()], None)
                .await
                .expect("child"),
            Vec::<String>::new(),
            "a child's session does not take a trace whose root names another"
        );
        let mut sessions = repo
            .get_session_ids_for_traces(&project, &traces, None)
            .await
            .expect("sessions");
        sessions.sort();
        assert_eq!(sessions, vec!["early-session", "root-session"]);
        assert_eq!(
            repo.get_session(&project, "root-session")
                .await
                .expect("root session")
                .map(|session| session.trace_count),
            Some(1)
        );
        assert!(
            repo.get_session(&project, "child-session")
                .await
                .expect("child session")
                .is_none(),
            "no trace belongs to the session only an early child names"
        );
        // What the trace response reports, and what the dashboard counts, follow the same rule.
        assert_eq!(
            repo.get_trace(&project, "rooted")
                .await
                .expect("trace")
                .and_then(|trace| trace.session_id)
                .as_deref(),
            Some("root-session")
        );
        let stats = repo
            .get_project_stats(&sideseat_ports::types::StatsParams {
                project_id: project.clone(),
                from_timestamp: ts(-3600),
                to_timestamp: ts(3600),
                timezone: "UTC".parse().unwrap(),
            })
            .await
            .expect("stats");
        assert_eq!(stats.counts.sessions, 2, "root-session and early-session");
    }
}

/// A trace's stored membership - canonical session, root stored, first receipt - reads alike on both backends.
#[tokio::test]
async fn trace_memberships_read_alike_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_trace_memberships").await;
    let span =
        |trace: &str, span_id: &str, parent: Option<&str>, session: Option<&str>, received: i64| {
            NormalizedSpan {
                project_id: Some(PROJECT.to_string()),
                trace_id: trace.to_string(),
                span_id: span_id.to_string(),
                parent_span_id: parent.map(str::to_string),
                span_name: span_id.to_string(),
                session_id: session.map(str::to_string),
                timestamp_start: ts(0),
                ingested_at: Some(ts(received)),
                ..Default::default()
            }
        };
    let project = ProjectId::from(PROJECT);
    let traces = [
        "rooted".to_string(),
        "rootless".to_string(),
        "sessionless".to_string(),
        "absent".to_string(),
    ];
    let mut answers = Vec::new();
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![
            span("rooted", "root", None, Some("s"), 200),
            span("rooted", "child", Some("root"), Some("s"), 100),
            span("rootless", "child", Some("gone"), Some("d"), 300),
            span("sessionless", "root", None, None, 400),
        ])
        .await
        .expect("spans");
        let memberships = repo
            .get_trace_memberships(&project, &traces)
            .await
            .expect("memberships");
        assert_eq!(
            memberships
                .iter()
                .map(|m| (m.trace_id.as_str(), m.session_id.as_deref(), m.rooted))
                .collect::<Vec<_>>(),
            vec![
                ("rooted", Some("s"), true),
                ("rootless", Some("d"), false),
                ("sessionless", None, false),
            ]
        );
        answers.push(memberships);
    }
    assert_eq!(
        answers[0], answers[1],
        "first receipts differ between backends"
    );
}

/// Two revisions of a root received at the same instant, naming different sessions, in two inserts: before a merge
/// settles which stands, ClickHouse may read either, so its membership read - which deletions decide on - reports no session and no
/// deciding root, and nothing deletes the trace. DuckDB keeps one winner by insertion order, as its storage does.
#[tokio::test]
async fn tied_root_revisions_decide_nothing_on_clickhouse() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let database = "sideseat_parity_tied_root_revisions";
    let ch = clickhouse_backend(&url, database).await;
    // Two inserts - one insert collapses tied rows as it writes - with merges stopped, so the pair stays unsettled.
    raw_client(&url, database)
        .query("SYSTEM STOP MERGES otel_spans")
        .execute()
        .await
        .expect("stop merges");
    let root = |session: &str| NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "tied".to_string(),
        span_id: "root".to_string(),
        span_name: "root".to_string(),
        session_id: Some(session.to_string()),
        timestamp_start: ts(0),
        ingested_at: Some(ts(100)),
        ..Default::default()
    };
    ch.insert_spans(vec![root("session-d")])
        .await
        .expect("first");
    ch.insert_spans(vec![root("session-l")])
        .await
        .expect("second");
    let memberships = ch
        .get_trace_memberships(&ProjectId::from(PROJECT), &["tied".to_string()])
        .await
        .expect("memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(
        (memberships[0].session_id.as_deref(), memberships[0].rooted),
        (None, false)
    );
}

/// A root corrected - received later - to name no session, both revisions unmerged: its latest revision stands, so
/// the root decides nothing and the trace is held for the grace. `argMax` skips NULLs, which brought the cleared
/// session back.
#[tokio::test]
async fn a_root_corrected_to_no_session_decides_nothing_on_clickhouse() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let database = "sideseat_parity_root_cleared";
    let ch = clickhouse_backend(&url, database).await;
    // Separate inserts - one insert collapses a span's revisions as it writes - with merges stopped.
    raw_client(&url, database)
        .query("SYSTEM STOP MERGES otel_spans")
        .execute()
        .await
        .expect("stop merges");
    let span = |span_id: &str, parent: Option<&str>, session: Option<&str>, received: i64| {
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "cleared".to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent.map(str::to_string),
            span_name: span_id.to_string(),
            session_id: session.map(str::to_string),
            timestamp_start: ts(0),
            ingested_at: Some(ts(received)),
            ..Default::default()
        }
    };
    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![
            span("root", None, Some("session-d"), 100),
            span("child", Some("root"), Some("session-d"), 150),
        ])
        .await
        .expect("first");
        repo.insert_spans(vec![span("root", None, None, 200)])
            .await
            .expect("the correction");
        let memberships = repo
            .get_trace_memberships(&ProjectId::from(PROJECT), &["cleared".to_string()])
            .await
            .expect("memberships");
        assert_eq!(
            memberships
                .iter()
                .map(|m| (m.session_id.as_deref(), m.rooted))
                .collect::<Vec<_>>(),
            vec![(Some("session-d"), false)],
            "the child names the session; the corrected root decides nothing"
        );
    }
}

/// Revisions of one span tied on receipt and differing only in start time can still move the trace - the earliest
/// span decides a rootless trace - so they too leave it undecided on ClickHouse until a merge settles them.
#[tokio::test]
async fn start_time_ties_leave_a_rootless_trace_undecided_on_clickhouse() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let database = "sideseat_parity_start_ties";
    let ch = clickhouse_backend(&url, database).await;
    raw_client(&url, database)
        .query("SYSTEM STOP MERGES otel_spans")
        .execute()
        .await
        .expect("stop merges");
    let span = |span_id: &str, session: &str, start: i64| NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "ties".to_string(),
        span_id: span_id.to_string(),
        parent_span_id: Some("gone".to_string()),
        span_name: span_id.to_string(),
        session_id: Some(session.to_string()),
        timestamp_start: ts(start),
        ingested_at: Some(ts(100)),
        ..Default::default()
    };
    ch.insert_spans(vec![span("a", "session-d", 0), span("b", "session-l", 5)])
        .await
        .expect("first");
    ch.insert_spans(vec![span("a", "session-d", 10)])
        .await
        .expect("the tied revision");
    let memberships = ch
        .get_trace_memberships(&ProjectId::from(PROJECT), &["ties".to_string()])
        .await
        .expect("memberships");
    assert_eq!(
        memberships
            .iter()
            .map(|m| (m.session_id.as_deref(), m.rooted))
            .collect::<Vec<_>>(),
        vec![(None, false)]
    );
}
