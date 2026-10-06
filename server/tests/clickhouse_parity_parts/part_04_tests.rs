async fn assert_detail_and_feed_parity(
    duck: &sideseat_adapter_duckdb::DuckdbRepository,
    ch: &sideseat_adapter_clickhouse::ClickhouseRepository,
    spans: &[NormalizedSpan],
) {
    // --- single span and bulk counts ---------------------------------------
    for (trace_id, span_id) in [("trace-a", "a-root"), ("trace-d", "d-root")] {
        let d = duck
            .get_span(&ProjectId::from(PROJECT), trace_id, span_id)
            .await
            .expect("duckdb get_span");
        let c = ch
            .get_span(&ProjectId::from(PROJECT), trace_id, span_id)
            .await
            .expect("clickhouse get_span");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_span(&d),
                describe_span(&c),
                "get_span({span_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_span({span_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }
    }

    let span_keys: Vec<(String, String)> = spans
        .iter()
        .map(|s| (s.trace_id.clone(), s.span_id.clone()))
        .collect();
    let d = duck
        .get_span_counts_bulk(&ProjectId::from(PROJECT), &span_keys)
        .await
        .expect("duckdb counts");
    let c = ch
        .get_span_counts_bulk(&ProjectId::from(PROJECT), &span_keys)
        .await
        .expect("clickhouse counts");
    let describe_counts = |m: &std::collections::HashMap<(String, String), _>| {
        let mut described: Vec<String> = m
            .iter()
            .map(
                |((trace, span), counts): (
                    &(String, String),
                    &sideseat_ports::types::SpanCounts,
                )| {
                    format!(
                        "{trace}/{span}=events:{},links:{}",
                        counts.event_count, counts.link_count
                    )
                },
            )
            .collect();
        described.sort();
        described
    };
    assert_eq!(
        describe_counts(&d),
        describe_counts(&c),
        "get_span_counts_bulk differs between backends"
    );

    // --- project feed ------------------------------------------------------
    // The feed endpoints page with a (ingested_at, span_id) cursor, whose SQL is written twice.
    let feed_params = sideseat_ports::types::FeedSpansParams {
        ingested_before_us: None,
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        cursor: None,
        start_time: None,
        end_time: None,
        is_observation: None,
    };
    let d = duck
        .get_feed_spans(&feed_params)
        .await
        .expect("duckdb feed spans");
    let c = ch
        .get_feed_spans(&feed_params)
        .await
        .expect("clickhouse feed spans");
    assert!(!d.is_empty(), "the feed returned nothing to compare");
    assert_eq!(
        d.iter().map(describe_span).collect::<Vec<_>>(),
        c.iter().map(describe_span).collect::<Vec<_>>(),
        "get_feed_spans differs between backends"
    );

    let feed_messages = sideseat_ports::types::FeedMessagesParams {
        ingested_before_us: None,
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        cursor: None,
        start_time: None,
        end_time: None,
    };
    let d = duck
        .get_project_messages(&feed_messages)
        .await
        .expect("duckdb project messages");
    let c = ch
        .get_project_messages(&feed_messages)
        .await
        .expect("clickhouse project messages");
    assert!(
        !d.rows.is_empty(),
        "the project message feed returned nothing to compare"
    );
    assert_eq!(
        d.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
        c.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
        "get_project_messages differs between backends"
    );

    // A window whose start falls *inside* a span. The fixture's spans each run for one second, so a
    // window starting half a second after trace-a's first generation began still contains the moment
    // it finished - and a completed response carries its span's end time, so its message belongs in
    // that window. Selecting rows by the span's start dropped it before reconstruction could see it.
    let straddling = sideseat_ports::types::FeedMessagesParams {
        ingested_before_us: None,
        project_id: ProjectId::from(PROJECT),
        limit: 50,
        cursor: None,
        start_time: Some(ts(1) + chrono::Duration::milliseconds(500)),
        end_time: None,
    };
    let d_straddle = duck
        .get_project_messages(&straddling)
        .await
        .expect("duckdb straddling window");
    let c_straddle = ch
        .get_project_messages(&straddling)
        .await
        .expect("clickhouse straddling window");
    assert_eq!(
        d_straddle
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_straddle
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "a window starting inside a span differs between backends"
    );
    assert!(
        d_straddle.rows.iter().any(|r| r.span_id == "a-gen-1"),
        "the span that began before the window and finished inside it is missing: {:?}",
        d_straddle
            .rows
            .iter()
            .map(|r| &r.span_id)
            .collect::<Vec<_>>()
    );

    // Cursor paging, which is a different mechanism from LIMIT/OFFSET and was only ever called
    // with `cursor: None`.
    let feed_page =
        |cursor: Option<(i64, String, String)>| sideseat_ports::types::FeedSpansParams {
            ingested_before_us: None,
            project_id: ProjectId::from(PROJECT),
            limit: 3,
            cursor,
            start_time: None,
            end_time: None,
            is_observation: None,
        };
    let d_first = duck
        .get_feed_spans(&feed_page(None))
        .await
        .expect("duckdb feed page 1");
    let c_first = ch
        .get_feed_spans(&feed_page(None))
        .await
        .expect("clickhouse feed page 1");
    assert_eq!(
        d_first.iter().map(describe_span).collect::<Vec<_>>(),
        c_first.iter().map(describe_span).collect::<Vec<_>>(),
        "the first cursor page of the span feed differs between backends"
    );
    assert_eq!(d_first.len(), 3, "the feed's first page should be full");
    // Each backend's cursor comes from its own page: the cursor carries `ingested_at`, which is
    // the server clock at write time and therefore differs between the two for the same span. A
    // cursor from one applied to the other selects nothing, which says nothing about either.
    let duck_cursor = d_first.last().map(|s| {
        (
            s.ingested_at.timestamp_micros(),
            s.span_id.clone(),
            s.trace_id.clone(),
        )
    });
    let ch_cursor = c_first.last().map(|s| {
        (
            s.ingested_at.timestamp_micros(),
            s.span_id.clone(),
            s.trace_id.clone(),
        )
    });
    let d_second = duck
        .get_feed_spans(&feed_page(duck_cursor))
        .await
        .expect("duckdb feed page 2");
    let c_second = ch
        .get_feed_spans(&feed_page(ch_cursor))
        .await
        .expect("clickhouse feed page 2");
    assert_eq!(
        d_second.iter().map(describe_span).collect::<Vec<_>>(),
        c_second.iter().map(describe_span).collect::<Vec<_>>(),
        "the second cursor page of the span feed differs between backends"
    );
    let first_ids: std::collections::BTreeSet<&String> =
        d_first.iter().map(|s| &s.span_id).collect();
    assert!(
        !d_second.is_empty() && d_second.iter().all(|s| !first_ids.contains(&s.span_id)),
        "the cursor returned rows the first page already had"
    );

    let messages_page =
        |cursor: Option<(i64, String, String)>| sideseat_ports::types::FeedMessagesParams {
            ingested_before_us: None,
            project_id: ProjectId::from(PROJECT),
            limit: 2,
            cursor,
            start_time: None,
            end_time: None,
        };
    let d_first = duck
        .get_project_messages(&messages_page(None))
        .await
        .expect("duckdb message feed page 1");
    let c_first = ch
        .get_project_messages(&messages_page(None))
        .await
        .expect("clickhouse message feed page 1");
    assert_eq!(
        d_first
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_first
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "the first cursor page of the message feed differs between backends"
    );
    // Per-backend cursor again, for the same reason.
    let duck_cursor = d_first.rows.last().map(|r| {
        (
            r.ingested_at.timestamp_micros(),
            r.span_id.clone(),
            r.trace_id.clone(),
        )
    });
    let ch_cursor = c_first.rows.last().map(|r| {
        (
            r.ingested_at.timestamp_micros(),
            r.span_id.clone(),
            r.trace_id.clone(),
        )
    });
    let d_second = duck
        .get_project_messages(&messages_page(duck_cursor))
        .await
        .expect("duckdb message feed page 2");
    let c_second = ch
        .get_project_messages(&messages_page(ch_cursor))
        .await
        .expect("clickhouse message feed page 2");
    assert_eq!(
        d_second
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_second
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "the second cursor page of the message feed differs between backends"
    );
    // Equality alone is satisfied by two empty pages, or by two backends both ignoring the cursor
    // and repeating page one.
    let first_ids: std::collections::BTreeSet<&String> =
        d_first.rows.iter().map(|r| &r.span_id).collect();
    assert!(
        !d_second.rows.is_empty(),
        "the message feed's second page was empty, so the comparison proves nothing"
    );
    assert!(
        d_second
            .rows
            .iter()
            .all(|r| !first_ids.contains(&r.span_id)),
        "the message cursor returned rows the first page already had"
    );
}

async fn assert_summary_read_parity(
    duck: &sideseat_adapter_duckdb::DuckdbRepository,
    ch: &sideseat_adapter_clickhouse::ClickhouseRepository,
    spans: &[NormalizedSpan],
) {
    // --- filter options, all three scopes ----------------------------------
    // Every option's count is shown in the UI next to it, so an approximate count on one backend
    // and an exact one on the other means the same project reports different numbers.
    let describe_option_map =
        |m: std::collections::HashMap<String, Vec<sideseat_ports::traits::FilterOptionRow>>| {
            let mut described: Vec<String> = m
                .into_iter()
                .map(|(column, rows)| {
                    let mut values: Vec<String> = rows
                        .iter()
                        .map(|r| format!("{}={}", r.value, r.count))
                        .collect();
                    values.sort();
                    format!("{column}: {}", values.join(","))
                })
                .collect();
            described.sort();
            described
        };

    // The trace list exposes view column names, which the repositories map to span columns.
    let trace_columns: Vec<String> = sideseat_ports::types::TRACE_FILTER_OPTION_COLUMNS
        .iter()
        .map(|(view_column, _)| view_column.to_string())
        .collect();
    let d = duck
        .get_trace_filter_options(&ProjectId::from(PROJECT), &trace_columns, None, None)
        .await
        .expect("duckdb trace options");
    let c = ch
        .get_trace_filter_options(&ProjectId::from(PROJECT), &trace_columns, None, None)
        .await
        .expect("clickhouse trace options");
    // Compared against the fixture, not only against each other: two backends returning empty maps,
    // or omitting every column asked for, satisfied equality.
    assert_eq!(
        describe_option_map(d),
        describe_option_map(c),
        "get_trace_filter_options differs between backends"
    );
    let d = duck
        .get_trace_filter_options(&ProjectId::from(PROJECT), &trace_columns, None, None)
        .await
        .expect("duckdb trace options");
    let described = describe_option_map(d);
    for expected in [
        // Every span carries this environment, and there are ten traces.
        "environment: test=10",
        // Two sessions, one covering two traces and one covering one.
        "session_id: session-1=2,session-2=1,session-3=1",
        "user_id: user-1=2",
        // The same names the trace list displays, including trace-c's: it has no root span, so
        // its name comes from the earliest named span, exactly as the list's fallback does. Listing
        // root spans only omitted it, and filtering by the name the UI showed returned nothing.
        "trace_name: agent=2,cost-only=1,earliest-named=1,generation=1,http-post=1,plain-span=2,\
         tool=1,usage-only=1",
    ] {
        assert!(
            described.iter().any(|line| line == expected),
            "trace filter options are missing {expected:?}: {described:?}"
        );
    }

    let span_columns: Vec<String> = sideseat_ports::types::SPAN_FILTER_OPTION_COLUMNS
        .iter()
        .map(|c| c.to_string())
        .collect();
    for observations_only in [false, true] {
        let d = duck
            .get_span_filter_options(
                &ProjectId::from(PROJECT),
                &span_columns,
                None,
                None,
                observations_only,
            )
            .await
            .expect("duckdb span options");
        let c = ch
            .get_span_filter_options(
                &ProjectId::from(PROJECT),
                &span_columns,
                None,
                None,
                observations_only,
            )
            .await
            .expect("clickhouse span options");
        let described = describe_option_map(d);
        assert_eq!(
            described,
            describe_option_map(c),
            "get_span_filter_options(observations_only={observations_only}) differs"
        );
        // Against the fixture, not only against each other: two empty maps satisfied equality.
        //
        // "GenAI only" means the same predicate the trace and session lists use, so trace-g's span -
        // GenAI attributes, no observation type, which is what transport-level instrumentation
        // produces - is kept either way. All six spans carrying a model are offered under both
        // settings; a backend that restricted this to observations would report five and hide a span
        // whose trace the trace list shows.
        for (column, value) in [
            ("gen_ai_request_model", "claude-haiku"),
            ("gen_ai_system", "bedrock"),
        ] {
            let expected = format!("{column}: {value}=6");
            assert!(
                described.contains(&expected),
                "span options are missing {expected:?} \
                 (observations_only={observations_only}): {described:?}"
            );
        }

        // And the flag still excludes something: trace-e and trace-f are plain spans with no GenAI
        // data at all. Without this the assertions above would pass for a backend ignoring the flag.
        let names = described
            .iter()
            .find(|d| d.starts_with("span_name: "))
            .expect("span_name options");
        assert_eq!(
            names.contains("plain-span"),
            !observations_only,
            "span_name options with observations_only={observations_only}: {names}"
        );
    }

    let session_columns: Vec<String> = sideseat_ports::types::SESSION_FILTER_OPTION_COLUMNS
        .iter()
        .map(|c| c.to_string())
        .collect();
    let d = duck
        .get_session_filter_options(&ProjectId::from(PROJECT), &session_columns, None, None)
        .await
        .expect("duckdb session options");
    let c = ch
        .get_session_filter_options(&ProjectId::from(PROJECT), &session_columns, None, None)
        .await
        .expect("clickhouse session options");
    let described = describe_option_map(d);
    assert_eq!(
        described,
        describe_option_map(c),
        "get_session_filter_options differs between backends"
    );
    // Counted in sessions here, not traces: all three sessions carry the environment, one carries
    // the user. Two empty maps would otherwise pass.
    assert_eq!(
        described,
        vec!["environment: test=3", "user_id: user-1=1"],
        "session options do not match the fixture"
    );

    // --- project span counts -----------------------------------------------
    let d = duck
        .count_spans_by_project(&[ProjectId::from(PROJECT)])
        .await
        .expect("duckdb project counts");
    let c = ch
        .count_spans_by_project(&[ProjectId::from(PROJECT)])
        .await
        .expect("clickhouse project counts");
    assert_eq!(
        d.get(PROJECT),
        c.get(PROJECT),
        "count_spans_by_project differs between backends"
    );
    // What deletion verification reads: every row a project owns, not only its spans.
    //
    // A *metric* is inserted first, because that is the whole point of the read and a span-only fixture
    // would pass while a backend that forgot the metrics table entirely still counted correctly. Deletion
    // decides whether a tombstone may go on the strength of this number, so a metric it cannot see is a
    // project row removed while its metrics remain - unreachable, because every read finds data through
    // that row.
    let metric = NormalizedMetric {
        project_id: Some(PROJECT.to_string()),
        metric_name: "parity.counter".to_string(),
        metric_type: MetricType::Sum,
        aggregation_temporality: AggregationTemporality::Cumulative,
        is_monotonic: Some(true),
        timestamp: ts(0),
        value_int: Some(7),
        ..Default::default()
    };
    duck.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("duckdb metric insert");
    ch.insert_metrics(std::slice::from_ref(&metric))
        .await
        .expect("clickhouse metric insert");

    let d_all = duck
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect("duckdb project rows");
    let c_all = ch
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect("clickhouse project rows");
    assert_eq!(
        d_all, c_all,
        "count_project_rows differs between backends, so deletion would verify differently"
    );
    assert_eq!(
        d_all,
        spans.len() as u64 + 1,
        "the project row count must include the metric, or a deleted project's metrics outlive it"
    );
    assert_eq!(
        d.get(PROJECT),
        Some(&(spans.len() as u64)),
        "the project span count does not match what was inserted"
    );

    // --- stats -------------------------------------------------------------
    let stats_params = sideseat_ports::types::StatsParams {
        project_id: ProjectId::from(PROJECT),
        from_timestamp: ts(-3600),
        to_timestamp: ts(3600),
        timezone: "UTC".parse().unwrap(),
    };
    let d = duck
        .get_project_stats(&stats_params)
        .await
        .expect("duckdb stats");
    let c = ch
        .get_project_stats(&stats_params)
        .await
        .expect("clickhouse stats");
    assert_eq!(
        format!("{d:?}"),
        format!("{c:?}"),
        "get_project_stats differs between backends"
    );
}

#[tokio::test]
async fn clickhouse_matches_duckdb_on_every_read() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "clickhouse parity: skipped - set {URL_ENV} to a ClickHouse HTTP endpoint \
             (or run `make test-clickhouse`)"
        );
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity").await;

    let spans = fixture_spans();
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans.clone())
        .await
        .expect("clickhouse insert");

    assert_basic_read_parity(&duck, &ch).await;
    assert_list_and_filter_parity(&duck, &ch, &spans).await;
    assert_detail_and_feed_parity(&duck, &ch, &spans).await;
    assert_summary_read_parity(&duck, &ch, &spans).await;
}

/// Deleting must remove the same rows on both backends.
///
/// A dialect difference here is the worst kind: the user asks for data to be gone, one backend
/// obliges and the other keeps it, and the API answers 204 either way. ClickHouse deletes through
/// an asynchronous mutation, so the test waits for the rows to actually disappear instead of
/// reading a count - which is also why the count itself is not compared (see
/// `AnalyticsRepository::delete_traces`).
/// A span delivered twice with *different* data must read the same on both backends.
///
/// The two backends resolve a duplicate span in opposite directions. ClickHouse stores spans in
/// `ReplacingMergeTree(ingested_at)`, so `FINAL` keeps the **latest** delivery. DuckDB's dedup
/// subquery joined on `MIN(ingested_at)`, keeping the **first**. Retries usually carry identical
/// payloads, which is why every other test passes - but an exporter that re-sends a span with
/// corrected usage made the same project report different tokens depending on which backend served
/// it, and no test could see it because the fixture's duplicate spans are identical.
///
/// Latest wins, because that is what the ClickHouse engine enforces and a later delivery is the more
/// complete record.
#[tokio::test]
async fn a_span_redelivered_with_new_data_reads_the_same_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_redelivery").await;

    let base = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-redelivered".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        span_category: Some(SpanCategory::LLM),
        gen_ai_system: Some("bedrock".to_string()),
        gen_ai_request_model: Some("claude-haiku".to_string()),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        status_code: Some("OK".to_string()),
        environment: Some("test".to_string()),
        ..Default::default()
    };

    // The same message shape ingestion writes; the two deliveries differ so the test can tell which
    // one the pipeline was handed.
    let payload = |text: &str| {
        Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": text}
            }])
            .to_string(),
        )
    };

    // First delivery: 100 tokens. Second: the same span, corrected to 900, ingested later.
    let first = NormalizedSpan {
        gen_ai_usage_input_tokens: 60,
        gen_ai_usage_output_tokens: 40,
        gen_ai_usage_total_tokens: 100,
        messages: payload("the first attempt"),
        ingested_at: Some(ts(10)),
        ..base.clone()
    };
    let second = NormalizedSpan {
        gen_ai_usage_input_tokens: 500,
        gen_ai_usage_output_tokens: 400,
        gen_ai_usage_total_tokens: 900,
        messages: payload("the corrected answer"),
        ingested_at: Some(ts(20)),
        ..base
    };

    for repo in [&duck as &dyn AnalyticsRepository, &ch] {
        repo.insert_spans(vec![first.clone()])
            .await
            .expect("first delivery");
        repo.insert_spans(vec![second.clone()])
            .await
            .expect("second delivery");
    }

    let d = duck
        .get_trace(&ProjectId::from(PROJECT), "trace-redelivered")
        .await
        .expect("duckdb trace")
        .expect("trace exists");
    let c = ch
        .get_trace(&ProjectId::from(PROJECT), "trace-redelivered")
        .await
        .expect("clickhouse trace")
        .expect("trace exists");

    assert_eq!(
        (d.total_tokens, c.total_tokens),
        (900, 900),
        "the corrected delivery must win on both backends: duckdb={} clickhouse={}",
        d.total_tokens,
        c.total_tokens
    );

    // And the message query hands the pipeline the same rows. It read otel_spans directly on DuckDB
    // while ClickHouse read with FINAL, so reconstruction saw two copies of a re-delivered span on one
    // backend and one on the other - the message dedup usually hid it, and the totals had to be
    // protected against it by hand.
    let params = MessageQueryParams {
        project_id: ProjectId::from(PROJECT),
        trace_id: Some("trace-redelivered".to_string()),
        ..Default::default()
    };
    let d_rows = duck.get_messages(&params).await.expect("duckdb messages");
    let c_rows = ch.get_messages(&params).await.expect("clickhouse messages");
    assert_eq!(
        d_rows.rows.len(),
        1,
        "the re-delivered span must reach the pipeline once, not twice"
    );
    assert!(
        d_rows.rows[0]
            .messages_json
            .contains("the corrected answer"),
        "the pipeline was handed the superseded delivery: {}",
        d_rows.rows[0].messages_json
    );
    assert_eq!(
        d_rows
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        c_rows
            .rows
            .iter()
            .map(describe_message_row)
            .collect::<Vec<_>>(),
        "the message query returns different rows per backend for a re-delivered span"
    );
    assert_eq!(
        d.span_count, c.span_count,
        "one span delivered twice is one span on both backends"
    );
}
