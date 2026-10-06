#[tokio::test]
async fn deleting_removes_the_same_rows_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_delete").await;

    let spans = fixture_spans();
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans.clone())
        .await
        .expect("clickhouse insert");

    /// Span ids still present, sorted, from whichever backend.
    async fn remaining(repo: &impl AnalyticsRepository) -> Vec<String> {
        let params = ListSpansParams {
            project_id: ProjectId::from(PROJECT),
            page: 1,
            limit: 200,
            ..Default::default()
        };
        let (rows, _) = repo.list_spans(&params).await.expect("list spans");
        let mut ids: Vec<String> = rows.into_iter().map(|r| r.span_id).collect();
        ids.sort();
        ids
    }

    /// ClickHouse mutations are asynchronous, so poll until the expectation holds rather than
    /// sleeping a guessed interval or trusting the returned count.
    async fn settle(repo: &impl AnalyticsRepository, expected: &[String]) -> Vec<String> {
        for _ in 0..100 {
            let actual = remaining(repo).await;
            if actual == expected {
                return actual;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        remaining(repo).await
    }

    assert_eq!(
        remaining(&duck).await,
        remaining(&ch).await,
        "the two backends disagree before anything was deleted"
    );

    // Each step asserts which ids are gone and which remain, on the reference backend, before
    // comparing. Equality alone is satisfied by two backends that both deleted nothing.

    // One span out of a trace, leaving its sibling.
    let pair = [("trace-c".to_string(), "c-child-1".to_string())];
    duck.delete_spans(&ProjectId::from(PROJECT), &pair)
        .await
        .expect("duckdb delete span");
    ch.delete_spans(&ProjectId::from(PROJECT), &pair)
        .await
        .expect("clickhouse delete span");
    let after_duck = remaining(&duck).await;
    assert!(
        !after_duck.contains(&"c-child-1".to_string())
            && after_duck.contains(&"c-child-2".to_string()),
        "deleting one span took the wrong rows: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_spans removed different rows on the two backends"
    );

    // A session spanning two traces: session-1 covers trace-a and trace-b, so this must remove
    // spans from both. Deleting session-2 would have touched a single trace, and an assertion that
    // only compared the backends would have passed even if neither deleted anything.
    duck.delete_sessions(&ProjectId::from(PROJECT), &["session-1".to_string()])
        .await
        .expect("duckdb delete session");
    ch.delete_sessions(&ProjectId::from(PROJECT), &["session-1".to_string()])
        .await
        .expect("clickhouse delete session");
    let after_duck = remaining(&duck).await;
    for gone in ["a-root", "a-gen-1", "a-gen-2", "b-root"] {
        assert!(
            !after_duck.contains(&gone.to_string()),
            "deleting session-1 left {gone} behind: {after_duck:?}"
        );
    }
    assert!(
        after_duck.contains(&"c-child-2".to_string()),
        "deleting session-1 took a span from another session: {after_duck:?}"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_sessions removed different rows on the two backends"
    );

    // A session recorded on the root span only, which is how several frameworks record it. Deleting
    // the rows that *name* the session removes the root and keeps its children - and reports
    // success, leaving spans that no longer belong to any session and so can never be deleted by
    // session again. session-1 above cannot catch it, because the fixture repeats its id on every
    // span; trace-i carries session-3 on its root and nothing on its generation child.
    duck.delete_sessions(&ProjectId::from(PROJECT), &["session-3".to_string()])
        .await
        .expect("duckdb delete root-only session");
    ch.delete_sessions(&ProjectId::from(PROJECT), &["session-3".to_string()])
        .await
        .expect("clickhouse delete root-only session");
    let after_duck = remaining(&duck).await;
    for gone in ["i-root", "i-gen"] {
        assert!(
            !after_duck.contains(&gone.to_string()),
            "deleting the root-only session left {gone} behind: {after_duck:?}"
        );
    }
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "deleting a root-only session removed different rows on the two backends"
    );

    // What is left of a trace, by trace id.
    duck.delete_traces(&ProjectId::from(PROJECT), &["trace-c".to_string()])
        .await
        .expect("duckdb delete trace");
    ch.delete_traces(&ProjectId::from(PROJECT), &["trace-c".to_string()])
        .await
        .expect("clickhouse delete trace");
    let after_duck = remaining(&duck).await;
    assert_eq!(
        after_duck,
        vec![
            "d-root".to_string(),
            "e-root".to_string(),
            "f-root".to_string(),
            "g-root".to_string(),
            "h-root".to_string(),
            "j-root".to_string()
        ],
        "deleting trace-c should leave exactly the unrelated traces"
    );
    assert_eq!(
        settle(&ch, &after_duck).await,
        after_duck,
        "delete_traces removed different rows on the two backends"
    );

    // Everything that is left.
    duck.delete_project_data(&ProjectId::from(PROJECT))
        .await
        .expect("duckdb delete project");
    ch.delete_project_data(&ProjectId::from(PROJECT))
        .await
        .expect("clickhouse delete project");
    assert!(
        remaining(&duck).await.is_empty(),
        "delete_project_data left rows behind on duckdb"
    );
    assert!(
        settle(&ch, &[]).await.is_empty(),
        "delete_project_data left rows behind on clickhouse"
    );
}

/// The fixture has to actually exercise the cases the parity assertions exist for; a fixture that
/// silently lost its no-root trace or its tag spread would let both backends agree on nothing.
#[test]
fn the_fixture_covers_the_cases_parity_depends_on() {
    let spans = fixture_spans();

    let trace_c: Vec<_> = spans.iter().filter(|s| s.trace_id == "trace-c").collect();
    assert!(
        !trace_c.is_empty() && trace_c.iter().all(|s| s.parent_span_id.is_some()),
        "trace-c must have no root span, so trace_name exercises the earliest-named fallback"
    );

    let trace_a_tags: Vec<&String> = spans
        .iter()
        .filter(|s| s.trace_id == "trace-a")
        .flat_map(|s| s.tags.iter())
        .collect();
    let distinct = trace_a_tags
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(
        trace_a_tags.len() > distinct,
        "trace-a must spread overlapping tags across spans, so the tags union is tested"
    );
    assert!(
        spans
            .iter()
            .filter(|s| s.trace_id == "trace-a")
            .filter(|s| s.observation_type == Some(ObservationType::Generation))
            .count()
            > 1,
        "trace-a must hold several generation spans, so token dedup is tested"
    );

    assert!(
        spans.iter().any(|s| s.session_id.is_none()),
        "one trace must have no session, so the no-session path is tested"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.status_code.as_deref() == Some("ERROR")),
        "one span must carry an error, so has_error is tested"
    );
    assert!(
        spans
            .iter()
            .any(|s| s.observation_type != Some(ObservationType::Generation)
                && s.gen_ai_usage_total_tokens == 0),
        "one span must have no tokens, so the zero-not-null path is tested"
    );
    assert!(
        spans.iter().any(|s| s.observation_type.is_none()),
        "one span must have no observation type, or the include_nongenai filter excludes nothing"
    );

    let starts: Vec<_> = spans.iter().map(|s| s.timestamp_start).collect();
    let distinct_starts: std::collections::BTreeSet<_> = starts.iter().collect();
    assert!(
        starts.len() > distinct_starts.len(),
        "two spans must start at the same instant, or nothing tests the pagination tiebreak"
    );

    let sessions: std::collections::BTreeSet<_> =
        spans.iter().filter_map(|s| s.session_id.as_ref()).collect();
    assert!(
        sessions.len() >= 2,
        "at least two sessions, so session aggregation is not trivially one group"
    );
    let session_1_traces: std::collections::BTreeSet<_> = spans
        .iter()
        .filter(|s| s.session_id.as_deref() == Some("session-1"))
        .map(|s| &s.trace_id)
        .collect();
    assert!(
        session_1_traces.len() >= 2,
        "session-1 must span several traces, so session totals cross trace boundaries"
    );
}

/// Two labelled series recorded at the same instant survive on both backends, and a re-delivery of
/// either does not become a second datapoint.
///
/// The ClickHouse metrics table is a `ReplacingMergeTree`, and its sorting key held only
/// `(project_id, metric_name, toDate(timestamp), timestamp)` - no attributes. A replacing engine treats
/// rows with an equal sorting key as versions of one row, so `requests{status=200}` and
/// `requests{status=500}` from a single export collapsed into one row at the next merge, with the 200
/// already returned to the exporter. DuckDB, append-only and with no identity at all, had the opposite
/// failure: the same export delivered twice was stored twice.
///
/// `FINAL` is what a read applies, so the test reads through `count_project_rows` on both sides rather
/// than inspecting parts, and `OPTIMIZE ... FINAL` forces the merge instead of waiting for one - without
/// it the collapse is invisible for as long as the rows sit in separate parts, which is exactly why the
/// defect survived until now.
#[tokio::test]
async fn distinct_metric_series_survive_and_a_redelivery_does_not_duplicate() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "clickhouse parity: skipped - set {URL_ENV} to a ClickHouse HTTP endpoint \
             (or run `make test-clickhouse`)"
        );
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_metrics").await;

    let series = |status: i64| {
        let mut metric = NormalizedMetric {
            project_id: Some(PROJECT.to_string()),
            metric_name: "http.server.requests".to_string(),
            metric_type: MetricType::Sum,
            aggregation_temporality: AggregationTemporality::Cumulative,
            is_monotonic: Some(true),
            // One instant, which is the ordinary case: an export stamps every datapoint of a metric
            // with the same collection time.
            timestamp: ts(0),
            value_int: Some(status),
            attributes: serde_json::json!({"http.response.status_code": status}),
            ..Default::default()
        };
        // Stamped as the extractor does, from the OTLP material rather than the JSON rendering.
        metric.datapoint_id = sideseat_ingestion::metrics::datapoint_id(
            &metric,
            &sideseat_ingestion::metrics::IdentityInputs {
                attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue {
                    key: "http.response.status_code".to_string(),
                    value: Some(opentelemetry_proto::tonic::common::v1::AnyValue {
                        value: Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(
                                status,
                            ),
                        ),
                    }),
                }],
                resource_attributes: &[],
                scope_attributes: &[],
                time_unix_nano: ts(0).timestamp_nanos_opt().unwrap_or(0) as u64,
                start_time_unix_nano: 0,
            },
        );
        metric
    };
    let export = vec![series(200), series(500)];
    assert_ne!(
        export[0].datapoint_id, export[1].datapoint_id,
        "two label sets must not share an identity, or the storage cannot tell them apart"
    );

    // Delivered twice, so the second pass is a re-delivery - the case DuckDB used to duplicate.
    for _ in 0..2 {
        duck.insert_metrics(&export).await.expect("duckdb metrics");
        ch.insert_metrics(&export)
            .await
            .expect("clickhouse metrics");
    }

    // Force the merge that decides whether a replacing engine keeps both rows.
    raw_client(&url, "sideseat_parity_metrics")
        .query("OPTIMIZE TABLE otel_metrics FINAL")
        .execute()
        .await
        .expect("clickhouse optimize");

    let duck_rows = duck
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect("duckdb project rows");
    let ch_rows = ch
        .count_project_rows(&ProjectId::from(PROJECT))
        .await
        .expect("clickhouse project rows");
    assert_eq!(
        duck_rows, ch_rows,
        "the backends disagree about how many datapoints exist: duckdb={duck_rows} clickhouse={ch_rows}"
    );
    assert_eq!(
        ch_rows, 2,
        "both labelled series must survive, and neither re-delivery may add a third"
    );

    // And on DuckDB the duplicates are absent from the *table*, not merely from a count. A
    // `COUNT(DISTINCT ...)` would pass while two rows held two possibly different measurements of one
    // instant, with nothing to say which is current.
    let physical = duck
        .0
        .count_metric_rows_for_test(PROJECT)
        .await
        .expect("count physical metric rows");
    assert_eq!(
        physical, 2,
        "a re-delivered datapoint must replace its row, not append another"
    );
}

/// A deletion has happened by the time it returns.
///
/// ClickHouse mutations are asynchronous by default, so `ALTER ... DELETE` used to schedule the work and
/// return - and the trace-deletion route then deleted the files those spans referenced and answered 204.
/// For as long as the mutation took, a read returned spans whose content was already gone, and a failed
/// mutation left them that way for good. `mutations_sync = 2` is what makes the 204 mean what it says.
///
/// Read immediately, with no sleep and no retry: a poll would pass either way, which is precisely the
/// property under test.
#[tokio::test]
async fn a_deleted_trace_is_gone_before_the_delete_returns() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "clickhouse parity: skipped - set {URL_ENV} to a ClickHouse HTTP endpoint \
             (or run `make test-clickhouse`)"
        );
        return;
    };

    let ch = clickhouse_backend(&url, "sideseat_parity_delete").await;
    let spans = fixture_spans();
    ch.insert_spans(spans.clone()).await.expect("insert");

    let doomed: Vec<String> = spans
        .iter()
        .map(|s| s.trace_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(1)
        .collect();
    let survivors: Vec<&str> = spans
        .iter()
        .map(|s| s.trace_id.as_str())
        .filter(|t| !doomed.iter().any(|d| d == t))
        .collect();
    assert!(
        !survivors.is_empty(),
        "the fixture must keep a trace, or the test cannot tell deletion from truncation"
    );

    ch.delete_traces(&ProjectId::from(PROJECT), &doomed)
        .await
        .expect("delete the trace");

    let (traces, _) = ch
        .list_traces(&trace_params())
        .await
        .expect("list traces right after the delete");
    let remaining: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
    assert!(
        !remaining.contains(&doomed[0].as_str()),
        "the deleted trace is still readable, so the 204 that follows means 'scheduled' rather than \
         'deleted' - and its files have already been removed: {remaining:?}"
    );
    assert!(
        remaining.contains(&survivors[0]),
        "only the named trace may go: {remaining:?}"
    );
}

/// Every `ALTER ... DELETE` in the ClickHouse backend waits for its mutation.
///
/// A structural check, because the failure is invisible: a new delete site written without the setting
/// compiles, passes, and silently returns before it has deleted anything. Reading the source is the only
/// way to catch the *absence* of a setting.
#[test]
fn every_clickhouse_delete_waits_for_its_mutation() {
    let sources = [
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/adapter-clickhouse/src/repositories/query.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/adapter-clickhouse/src/lib.rs"
        )),
    ];
    for source in sources {
        for (line_number, line) in source.lines().enumerate() {
            if !line.contains("ALTER TABLE") || !line.contains("DELETE WHERE") {
                continue;
            }
            // The statement is built by `format!`, so the setting is either spelled here or appended
            // through the shared constant on the following lines of the same call.
            let tail: String = source
                .lines()
                .skip(line_number)
                .take(6)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                tail.contains("mutations_sync") || tail.contains("AWAIT_MUTATION"),
                "line {} builds a DELETE that does not wait for its mutation: {}",
                line_number + 1,
                line.trim()
            );
        }
    }
}

/// The traversal watermark hides rows ingested after the traversal began, on both backends.
///
/// The defect: a feed page is chosen by ingestion time, but the reconstruction context loaded around it
/// was unbounded in that dimension. A span ingested *during* the traversal could enter an earlier page's
/// context, win deduplication against a span still to be paged, and then be scoped off the page it was
/// not selected for - so the older copy was suppressed and the newer one never returned. Bounding both by
/// one watermark makes a traversal a view of a single instant.
///
/// Checked on both the span feed and the message feed, because both take the bound and the two dialects
/// spell it differently (`EPOCH_US(ingested_at)` against `toInt64(toUnixTimestamp64Micro(ingested_at))`).
#[tokio::test]
async fn the_feed_watermark_hides_later_rows_on_both_backends() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!(
            "clickhouse parity: skipped - set {URL_ENV} to a ClickHouse HTTP endpoint \
             (or run `make test-clickhouse`)"
        );
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_watermark").await;

    // Two spans with distinct ingestion times: one "before" the traversal and one "after".
    let early_ingest = ts(0);
    let late_ingest = ts(600);
    let span = |span_id: &str, ingested: chrono::DateTime<Utc>| {
        NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: format!("trace-{span_id}"),
        span_id: span_id.to_string(),
        span_name: "gen".to_string(),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        observation_type: Some(ObservationType::Generation),
        span_category: Some(SpanCategory::LLM),
        ingested_at: Some(ingested),
        messages: Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": "hello"}
            }])
            .to_string(),
        ),
        ..Default::default()
    }
    };
    let spans = vec![
        span("early001", early_ingest),
        span("late00001", late_ingest),
    ];
    duck.insert_spans(spans.clone()).await.expect("duckdb");
    ch.insert_spans(spans.clone()).await.expect("clickhouse");

    // A watermark between the two: only the early span is visible for the traversal.
    let watermark_us = ts(300).timestamp_micros();

    async fn check(
        label: &str,
        backend: &(impl sideseat_ports::traits::AnalyticsRepository + ?Sized),
        watermark_us: i64,
    ) {
        let bounded = backend
            .get_feed_spans(&sideseat_ports::types::FeedSpansParams {
                project_id: ProjectId::from(PROJECT),
                limit: 50,
                ingested_before_us: Some(watermark_us),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("{label}: bounded feed spans: {e}"));
        let ids: Vec<&str> = bounded.iter().map(|s| s.span_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["early001"],
            "{label}: a span ingested after the traversal began must not appear in it"
        );

        // Without the bound both are there, which is what makes the assertion above about the bound
        // rather than about the fixture.
        let unbounded = backend
            .get_feed_spans(&sideseat_ports::types::FeedSpansParams {
                project_id: ProjectId::from(PROJECT),
                limit: 50,
                ingested_before_us: None,
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("{label}: unbounded feed spans: {e}"));
        assert_eq!(
            unbounded.len(),
            2,
            "{label}: both spans exist; the bound is what hides one"
        );

        // The message feed takes the same bound, and its context load is the half that mattered.
        let messages = backend
            .get_project_messages(&sideseat_ports::types::FeedMessagesParams {
                project_id: ProjectId::from(PROJECT),
                limit: 50,
                ingested_before_us: Some(watermark_us),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("{label}: bounded feed messages: {e}"));
        let ids: Vec<&str> = messages.rows.iter().map(|r| r.span_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["early001"],
            "{label}: the message feed must apply the watermark too"
        );
    }

    check("duckdb", &duck, watermark_us).await;
    check("clickhouse", &ch, watermark_us).await;

    // And **membership** takes the same bound. It is the half that was missing: with the rows bounded and the
    // session resolved from the current table, a traversal read one instant's rows and another instant's
    // grouping - so the context loaded around a page came from a session the traversal's own rows do not
    // place the trace in, and replayed history could be returned twice.
    //
    // One trace, delivered into session-early and re-delivered into session-late. What both backends must
    // guarantee is that the *bound is applied*: below the watermark the answer is never the later session.
    // Only DuckDB can guarantee it is the earlier one - `ReplacingMergeTree` may already have merged the
    // pre-watermark version away, and then no query on that engine can see it (the residual documented on
    // `ch_dedup_spans_as_of_watermark`). Observed: ClickHouse answers `[]` here, which is that residual and
    // not this bug - before the bound was honoured at all it answered `session-late`, which is what this
    // forbids.
    let membership =
        |span_id: &str, session: &str, ingested: chrono::DateTime<Utc>| NormalizedSpan {
            trace_id: "trace-moved".to_string(),
            session_id: Some(session.to_string()),
            ..span(span_id, ingested)
        };
    let moved = vec![
        membership("moved001", "session-early", early_ingest),
        membership("moved001", "session-late", late_ingest),
    ];
    duck.insert_spans(moved.clone())
        .await
        .expect("duckdb moved");
    ch.insert_spans(moved).await.expect("clickhouse moved");

    for (label, backend, exact) in [
        (
            "duckdb",
            &duck as &dyn sideseat_ports::traits::AnalyticsRepository,
            true,
        ),
        (
            "clickhouse",
            &ch as &dyn sideseat_ports::traits::AnalyticsRepository,
            false,
        ),
    ] {
        let traces = vec!["trace-moved".to_string()];
        let early = ("trace-moved".to_string(), "session-early".to_string());
        let late = ("trace-moved".to_string(), "session-late".to_string());

        let bounded = backend
            .get_trace_session_pairs(&ProjectId::from(PROJECT), &traces, Some(watermark_us))
            .await
            .unwrap_or_else(|e| panic!("{label}: bounded pairs: {e}"));
        assert!(
            !bounded.contains(&late),
            "{label}: a session the trace moved to *after* the traversal began must not be its \
             membership within it: {bounded:?}"
        );
        if exact {
            assert_eq!(
                bounded,
                vec![early.clone()],
                "{label}: retains every version, so the answer is exactly the earlier session"
            );
        }

        let now = backend
            .get_trace_session_pairs(&ProjectId::from(PROJECT), &traces, None)
            .await
            .unwrap_or_else(|e| panic!("{label}: current pairs: {e}"));
        assert_eq!(
            now,
            vec![late],
            "{label}: unbounded it is the current session, so the bound is what decides"
        );

        let sessions = backend
            .get_session_ids_for_traces(&ProjectId::from(PROJECT), &traces, Some(watermark_us))
            .await
            .unwrap_or_else(|e| panic!("{label}: bounded sessions: {e}"));
        assert!(
            !sessions.contains(&"session-late".to_string()),
            "{label}: nor when the same question is asked as \"which sessions\": {sessions:?}"
        );

        // The other direction: which traces a session holds. Unbounded, the old session holds nothing,
        // because the trace has moved on - so a bounded answer naming it is the bound working.
        let expanded_now = backend
            .get_trace_ids_for_sessions(
                &ProjectId::from(PROJECT),
                &["session-early".to_string()],
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("{label}: current expansion: {e}"));
        assert!(
            expanded_now.is_empty(),
            "{label}: the trace has moved on, so unbounded the old session holds nothing: \
             {expanded_now:?}"
        );
        // The session the trace moved *to* holds nothing as of the watermark, on both backends - it did not
        // hold the trace yet. Asserted for both because it also exercises the bound's bind order: with the
        // watermark and the project id swapped this query fails outright rather than answering wrongly.
        let too_early = backend
            .get_trace_ids_for_sessions(
                &ProjectId::from(PROJECT),
                &["session-late".to_string()],
                Some(watermark_us),
            )
            .await
            .unwrap_or_else(|e| panic!("{label}: bounded expansion of the later session: {e}"));
        assert!(
            too_early.is_empty(),
            "{label}: as of the watermark the trace had not moved to session-late: {too_early:?}"
        );

        if exact {
            let expanded = backend
                .get_trace_ids_for_sessions(
                    &ProjectId::from(PROJECT),
                    &["session-early".to_string()],
                    Some(watermark_us),
                )
                .await
                .unwrap_or_else(|e| panic!("{label}: bounded expansion: {e}"));
            assert_eq!(
                expanded,
                vec!["trace-moved".to_string()],
                "{label}: as of the watermark the trace is still in the session it started in"
            );
        }
    }
}

/// A span redelivered *during* a traversal still appears in it, on DuckDB.
///
/// The subtle half of the watermark. Deduplication picks the newest row of each span, so a bound applied
/// *outside* it is worse than no bound for a re-delivered span: the newest row is rejected by the bound and
/// the older row was never selected, so the span is missing from every page of the traversal. Bounding the
/// *choice* - the newest row that existed when the traversal began - is what a watermark means.
///
/// DuckDB only, and deliberately so. ClickHouse deduplicates with `FINAL`, which cannot be asked for "the
/// version as of an instant", and a merge may have physically removed the earlier version - so there is
/// nothing to select. The limit is per backend, like the latency ceilings, and stating it is the honest
/// alternative to implying a guarantee the storage cannot give.
#[tokio::test]
async fn a_span_redelivered_during_a_traversal_still_appears_in_it() {
    let (_temp, duck) = duckdb_backend().await;

    let original = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "trace-redeliver".to_string(),
        span_id: "span0001".to_string(),
        span_name: "gen".to_string(),
        timestamp_start: ts(0),
        timestamp_end: Some(ts(1)),
        duration_ms: 1000,
        observation_type: Some(ObservationType::Generation),
        span_category: Some(SpanCategory::LLM),
        // Ingested before the traversal begins.
        ingested_at: Some(ts(0)),
        messages: Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": "first"}
            }])
            .to_string(),
        ),
        ..Default::default()
    };
    duck.insert_spans(vec![original.clone()])
        .await
        .expect("original");

    let watermark_us = ts(300).timestamp_micros();

    // Re-delivered *after* the watermark, with corrected content - the shape a retrying exporter produces.
    let redelivered = NormalizedSpan {
        ingested_at: Some(ts(600)),
        messages: Some(
            serde_json::json!([{
                "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": "corrected"}
            }])
            .to_string(),
        ),
        ..original.clone()
    };
    duck.insert_spans(vec![redelivered])
        .await
        .expect("redelivery");

    let page = duck
        .get_project_messages(&sideseat_ports::types::FeedMessagesParams {
            project_id: ProjectId::from(PROJECT),
            limit: 50,
            ingested_before_us: Some(watermark_us),
            ..Default::default()
        })
        .await
        .expect("bounded feed messages");
    let ids: Vec<&str> = page.rows.iter().map(|r| r.span_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["span0001"],
        "a span that existed when the traversal began must appear in it, even after a re-delivery: \
         bounding outside the dedup rejected the new row and never selected the old one, so it vanished"
    );
    assert!(
        page.rows[0].messages_json.contains("first"),
        "and the version served is the one that existed at the watermark, not the later correction: {}",
        page.rows[0].messages_json
    );

    // And the *reconstruction context* the endpoint loads next must agree with the page. Bounded outside
    // its own dedup, this query returned nothing for a span the page had selected - so the page held a span
    // whose context contained no version of it, and reconstruction saw a fragment of a trace it was told to
    // treat as whole.
    let context = duck
        .get_messages(&MessageQueryParams {
            project_id: ProjectId::from(PROJECT),
            trace_ids: Some(vec!["trace-redeliver".to_string()]),
            ingested_before_us: Some(watermark_us),
            ..Default::default()
        })
        .await
        .expect("bounded context load");
    let context_ids: Vec<&str> = context.rows.iter().map(|r| r.span_id.as_str()).collect();
    assert_eq!(
        context_ids, ids,
        "the context load and the page query must see the same spans at one watermark"
    );
    assert!(
        context.rows[0].messages_json.contains("first"),
        "and the same version of each: {}",
        context.rows[0].messages_json
    );
}
