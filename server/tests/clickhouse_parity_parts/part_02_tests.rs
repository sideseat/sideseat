
#[tokio::test]
async fn clickhouse_search_matches_duckdb_on_ordering_pagination_and_unknowns() {
    let Ok(url) = std::env::var(URL_ENV) else {
        eprintln!("clickhouse parity: skipped - set {URL_ENV} (or run `make test-clickhouse`)");
        return;
    };

    let (_temp, duck) = duckdb_backend().await;
    let ch = clickhouse_backend(&url, "sideseat_parity_search").await;
    let timestamp = ts(500);
    let ingested_at = ts(600);
    let long_prompt = (0..=sideseat_ports::types::SEARCH_TERMS_PER_FIELD)
        .map(|index| {
            if index == sideseat_ports::types::SEARCH_TERMS_PER_FIELD {
                "hiddenneedle".to_string()
            } else {
                format!("token{index}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut spans = vec![
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "search-a".to_string(),
            span_id: "a".to_string(),
            span_name: "rejected-phrase".to_string(),
            timestamp_start: timestamp,
            input_preview: Some("alpha x beta".to_string()),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "search-b".to_string(),
            span_id: "b".to_string(),
            span_name: "exact-phrase".to_string(),
            timestamp_start: timestamp,
            input_preview: Some("alpha beta".to_string()),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "search-c".to_string(),
            span_id: "c".to_string(),
            span_name: "truncated".to_string(),
            timestamp_start: timestamp,
            input_preview: Some(long_prompt),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
        NormalizedSpan {
            project_id: Some(PROJECT.to_string()),
            trace_id: "search-d".to_string(),
            span_id: "d".to_string(),
            span_name: "role-aware-phrase".to_string(),
            timestamp_start: ts(499),
            messages: Some(
                serde_json::json!([
                    {"role": "user", "content": "alpha beta"},
                    {"role": "assistant", "content": "alpha x beta"}
                ])
                .to_string(),
            ),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
    ];
    sideseat_domain::search::index_spans(&mut spans);
    spans.push(NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "search-legacy".to_string(),
        span_id: "legacy".to_string(),
        span_name: "legacy-unindexed".to_string(),
        timestamp_start: ts(498),
        input_preview: Some("historical searchable text".to_string()),
        ingested_at: Some(ingested_at),
        ..Default::default()
    });
    spans.push(NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "search-backfill-race".to_string(),
        span_id: "race".to_string(),
        content_digest: "stale-digest".to_string(),
        span_name: "backfill-race".to_string(),
        timestamp_start: ts(496),
        input_preview: Some("stale backfill race".to_string()),
        ingested_at: Some(ingested_at),
        ..Default::default()
    });
    duck.insert_spans(spans.clone())
        .await
        .expect("duckdb insert");
    ch.insert_spans(spans).await.expect("clickhouse insert");

    async fn run(
        repository: &impl SearchIndex,
        expression: sideseat_ports::types::SearchExpr,
        cursor: Option<sideseat_ports::types::SearchCursor>,
        max_examined: u32,
    ) -> (
        Vec<(String, bool)>,
        Option<sideseat_ports::types::SearchCursor>,
        u32,
        bool,
        bool,
    ) {
        let page = sideseat_domain::search::SearchService::execute(
            repository,
            &SearchQuery {
                project_id: ProjectId::from(PROJECT),
                signal: SearchSignal::Spans,
                expression,
                limit: 1,
                max_examined,
                cursor,
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .expect("search");
        let hits = page
            .hits
            .into_iter()
            .map(|hit| {
                let SearchRecord::Span(span) = hit.record else {
                    panic!("span search returned a log");
                };
                (span.trace_id, hit.indeterminate)
            })
            .collect();
        (
            hits,
            page.next_cursor,
            page.examined,
            page.examination_limit_reached,
            page.search_indexing_complete,
        )
    }

    let phrase =
        sideseat_domain::search::parse(r#"prompt:"alpha beta""#, SearchSignal::Spans).unwrap();
    let duck_first = run(&duck, phrase.clone(), None, 1).await;
    let ch_first = run(&ch, phrase.clone(), None, 1).await;
    assert_eq!(duck_first, ch_first);
    assert!(duck_first.0.is_empty());
    assert_eq!(duck_first.2, 1);
    let cursor = duck_first.1.clone().expect("empty page advances");

    let duck_second = run(&duck, phrase.clone(), Some(cursor.clone()), 1).await;
    let ch_second = run(&ch, phrase, Some(cursor), 1).await;
    assert_eq!(duck_second, ch_second);
    assert_eq!(duck_second.0, vec![("search-b".to_string(), false)]);

    let nested = sideseat_domain::search::parse(
        "span_name:truncated AND NOT (prompt:missing OR prompt:hiddenneedle)",
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_unknown = run(&duck, nested.clone(), None, 10).await;
    let ch_unknown = run(&ch, nested, None, 10).await;
    assert_eq!(duck_unknown, ch_unknown);
    assert_eq!(
        duck_unknown.0,
        vec![("search-c".to_string(), true)],
        "a capped negative clause must be returned as indeterminate"
    );

    let wrong_role = sideseat_domain::search::parse(
        r#"span_name:role-aware-phrase AND completion:"alpha beta""#,
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_wrong_role = run(&duck, wrong_role.clone(), None, 10).await;
    let ch_wrong_role = run(&ch, wrong_role, None, 10).await;
    assert_eq!(duck_wrong_role, ch_wrong_role);
    assert!(
        duck_wrong_role.0.is_empty(),
        "a phrase in a user message must not verify in the completion field"
    );

    let prompt_role = sideseat_domain::search::parse(
        r#"span_name:role-aware-phrase AND prompt:"alpha beta""#,
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_prompt = run(&duck, prompt_role.clone(), None, 10).await;
    let ch_prompt = run(&ch, prompt_role, None, 10).await;
    assert_eq!(duck_prompt, ch_prompt);
    assert_eq!(duck_prompt.0, vec![("search-d".to_string(), false)]);

    let legacy = sideseat_domain::search::parse(
        "span_name:legacy-unindexed AND prompt:historical",
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_legacy = run(&duck, legacy.clone(), None, 10).await;
    let ch_legacy = run(&ch, legacy, None, 10).await;
    assert_eq!(duck_legacy, ch_legacy);
    assert_eq!(duck_legacy.0, vec![("search-legacy".to_string(), false)]);
    assert!(
        !duck_legacy.4,
        "scan fallback must disclose that the historical row is not indexed"
    );
    let project_id = ProjectId::from(PROJECT);
    async fn stale_race_document(
        repository: &impl SearchIndex,
        project_id: &ProjectId,
    ) -> sideseat_ports::types::SearchBackfillDocument {
        repository
            .search_backfill_page(project_id, SearchSignal::Spans, 10)
            .await
            .expect("search backfill source page")
            .into_iter()
            .find_map(|source| match &source.id {
                sideseat_ports::types::SearchRecordId::Span { trace_id, .. }
                    if trace_id == "search-backfill-race" =>
                {
                    Some(sideseat_ports::types::SearchBackfillDocument {
                        id: source.id,
                        expected_content_digest: source.expected_content_digest,
                        document: sideseat_domain::search::document_for_source(&source.source),
                    })
                }
                _ => None,
            })
            .expect("race source")
    }
    let stale_duck = stale_race_document(&duck, &project_id).await;
    let stale_ch = stale_race_document(&ch, &project_id).await;
    let mut correction = NormalizedSpan {
        project_id: Some(PROJECT.to_string()),
        trace_id: "search-backfill-race".to_string(),
        span_id: "race".to_string(),
        content_digest: "fresh-digest".to_string(),
        span_name: "backfill-race".to_string(),
        timestamp_start: ts(496),
        input_preview: Some("fresh backfill race".to_string()),
        ingested_at: Some(ts(601)),
        ..Default::default()
    };
    sideseat_domain::search::index_spans(std::slice::from_mut(&mut correction));
    duck.insert_spans(vec![correction.clone()])
        .await
        .expect("duck correction during backfill");
    ch.insert_spans(vec![correction])
        .await
        .expect("clickhouse correction during backfill");
    duck.write_search_backfill(
        &project_id,
        SearchSignal::Spans,
        std::slice::from_ref(&stale_duck),
    )
    .await
    .expect("stale duck backfill write");
    ch.write_search_backfill(
        &project_id,
        SearchSignal::Spans,
        std::slice::from_ref(&stale_ch),
    )
    .await
    .expect("stale clickhouse backfill write");
    let stale_race = sideseat_domain::search::parse(
        "span_name:backfill-race AND prompt:stale",
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_stale_race = run(&duck, stale_race.clone(), None, 10).await;
    assert!(duck_stale_race.0.is_empty());
    assert!(run(&ch, stale_race, None, 10).await.0.is_empty());
    let fresh_race = sideseat_domain::search::parse(
        "span_name:backfill-race AND prompt:fresh",
        SearchSignal::Spans,
    )
    .unwrap();
    assert_eq!(
        run(&duck, fresh_race.clone(), None, 10).await.0,
        vec![("search-backfill-race".to_string(), false)]
    );
    assert_eq!(
        run(&ch, fresh_race, None, 10).await.0,
        vec![("search-backfill-race".to_string(), false)]
    );
    assert_eq!(
        sideseat_domain::search::SearchService::backfill_project_page(
            &duck,
            &project_id,
            SearchSignal::Spans,
            10,
        )
        .await
        .expect("duck span search backfill"),
        1
    );
    assert_eq!(
        sideseat_domain::search::SearchService::backfill_project_page(
            &ch,
            &project_id,
            SearchSignal::Spans,
            10,
        )
        .await
        .expect("clickhouse span search backfill"),
        1
    );
    assert_eq!(
        sideseat_domain::search::SearchService::backfill_project_page(
            &duck,
            &project_id,
            SearchSignal::Spans,
            10,
        )
        .await
        .expect("completed duck span search backfill"),
        0,
        "complete markers are the durable checkpoint"
    );
    let backfilled_legacy = sideseat_domain::search::parse(
        "span_name:legacy-unindexed AND prompt:historical",
        SearchSignal::Spans,
    )
    .unwrap();
    let duck_backfilled = run(&duck, backfilled_legacy.clone(), None, 10).await;
    let ch_backfilled = run(&ch, backfilled_legacy, None, 10).await;
    assert_eq!(duck_backfilled, ch_backfilled);
    assert!(duck_backfilled.4);

    let mut logs = vec![
        NormalizedLog {
            project_id: Some(PROJECT.to_string()),
            log_digest: "log-a".to_string(),
            ordinal: 0,
            timestamp,
            body: serde_json::json!("alpha x beta"),
            body_text: Some("alpha x beta".to_string()),
            severity_number: 9,
            severity_text: Some("INFO".to_string()),
            attributes: serde_json::json!({"route": "/a"}),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
        NormalizedLog {
            project_id: Some(PROJECT.to_string()),
            log_digest: "log-b".to_string(),
            ordinal: 0,
            timestamp,
            body: serde_json::json!("alpha beta"),
            body_text: Some("alpha beta".to_string()),
            severity_number: 17,
            severity_text: Some("ERROR".to_string()),
            attributes: serde_json::json!({"route": "/b"}),
            ingested_at: Some(ingested_at),
            ..Default::default()
        },
    ];
    sideseat_domain::search::index_logs(&mut logs);
    logs.push(NormalizedLog {
        project_id: Some(PROJECT.to_string()),
        log_digest: "log-legacy".to_string(),
        ordinal: 0,
        timestamp: ts(497),
        body: serde_json::json!("historical log text"),
        body_text: Some("historical log text".to_string()),
        severity_number: 5,
        ingested_at: Some(ingested_at),
        ..Default::default()
    });
    duck.insert_logs(&logs).await.expect("duckdb logs");
    ch.insert_logs(&logs).await.expect("clickhouse logs");

    async fn run_logs(
        repository: &impl SearchIndex,
        expression: sideseat_ports::types::SearchExpr,
        cursor: Option<sideseat_ports::types::SearchCursor>,
        max_examined: u32,
    ) -> (
        Vec<(String, u32, bool)>,
        Option<sideseat_ports::types::SearchCursor>,
        u32,
        bool,
        bool,
    ) {
        let page = sideseat_domain::search::SearchService::execute(
            repository,
            &SearchQuery {
                project_id: ProjectId::from(PROJECT),
                signal: SearchSignal::Logs,
                expression,
                limit: 1,
                max_examined,
                cursor,
                from_timestamp: None,
                to_timestamp: None,
            },
        )
        .await
        .expect("log search");
        let hits = page
            .hits
            .into_iter()
            .map(|hit| {
                let SearchRecord::Log(log) = hit.record else {
                    panic!("log search returned a span");
                };
                (log.log_digest, log.ordinal, hit.indeterminate)
            })
            .collect();
        (
            hits,
            page.next_cursor,
            page.examined,
            page.examination_limit_reached,
            page.search_indexing_complete,
        )
    }

    let log_phrase =
        sideseat_domain::search::parse(r#"body:"alpha beta""#, SearchSignal::Logs).unwrap();
    let duck_log_first = run_logs(&duck, log_phrase.clone(), None, 1).await;
    let ch_log_first = run_logs(&ch, log_phrase.clone(), None, 1).await;
    assert_eq!(duck_log_first, ch_log_first);
    assert!(duck_log_first.0.is_empty());
    let log_cursor = duck_log_first.1.clone().expect("empty log page advances");
    let duck_log_second = run_logs(&duck, log_phrase.clone(), Some(log_cursor.clone()), 1).await;
    let ch_log_second = run_logs(&ch, log_phrase, Some(log_cursor), 1).await;
    assert_eq!(duck_log_second, ch_log_second);
    assert_eq!(duck_log_second.0, vec![("log-b".to_string(), 0, false)]);

    let legacy_log = sideseat_domain::search::parse("body:historical", SearchSignal::Logs).unwrap();
    let duck_legacy_log = run_logs(&duck, legacy_log.clone(), None, 10).await;
    let ch_legacy_log = run_logs(&ch, legacy_log, None, 10).await;
    assert_eq!(duck_legacy_log, ch_legacy_log);
    assert_eq!(
        duck_legacy_log.0,
        vec![("log-legacy".to_string(), 0, false)]
    );
    assert!(!duck_legacy_log.4);
    assert_eq!(
        sideseat_domain::search::SearchService::backfill_project_page(
            &duck,
            &project_id,
            SearchSignal::Logs,
            10,
        )
        .await
        .expect("duck log search backfill"),
        1
    );
    assert_eq!(
        sideseat_domain::search::SearchService::backfill_project_page(
            &ch,
            &project_id,
            SearchSignal::Logs,
            10,
        )
        .await
        .expect("clickhouse log search backfill"),
        1
    );
    let backfilled_log =
        sideseat_domain::search::parse("body:historical", SearchSignal::Logs).unwrap();
    let duck_backfilled_log = run_logs(&duck, backfilled_log.clone(), None, 10).await;
    let ch_backfilled_log = run_logs(&ch, backfilled_log, None, 10).await;
    assert_eq!(duck_backfilled_log, ch_backfilled_log);
    assert!(duck_backfilled_log.4);
}

async fn assert_basic_read_parity(
    duck: &sideseat_adapter_duckdb::DuckdbRepository,
    ch: &sideseat_adapter_clickhouse::ClickhouseRepository,
) {
    // --- traces list -------------------------------------------------------
    let (duck_traces, duck_total) = duck
        .list_traces(&trace_params())
        .await
        .expect("duckdb traces");
    let (ch_traces, ch_total) = ch
        .list_traces(&trace_params())
        .await
        .expect("clickhouse traces");
    assert_eq!(
        duck_total, ch_total,
        "trace total count differs: duckdb={duck_total} clickhouse={ch_total}"
    );
    assert!(!duck_traces.is_empty(), "fixture produced no traces");
    assert_eq!(
        sorted(duck_traces.iter().map(describe_trace).collect()),
        sorted(ch_traces.iter().map(describe_trace).collect()),
        "list_traces differs between backends"
    );
    // Ordering is part of the contract, not just membership.
    assert_eq!(
        duck_traces
            .iter()
            .map(|t| t.trace_id.clone())
            .collect::<Vec<_>>(),
        ch_traces
            .iter()
            .map(|t| t.trace_id.clone())
            .collect::<Vec<_>>(),
        "list_traces returns the same traces in a different order"
    );

    // --- single trace ------------------------------------------------------
    for trace_id in ["trace-a", "trace-b", "trace-c", "trace-d"] {
        let d = duck
            .get_trace(&ProjectId::from(PROJECT), trace_id)
            .await
            .expect("duckdb get_trace");
        let c = ch
            .get_trace(&ProjectId::from(PROJECT), trace_id)
            .await
            .expect("clickhouse get_trace");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_trace(&d),
                describe_trace(&c),
                "get_trace({trace_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_trace({trace_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }
    }

    // A trace list row and a single-trace fetch must agree with each other too: the two
    // projections were copied, so they could drift within one backend.
    let listed = duck_traces
        .iter()
        .find(|t| t.trace_id == "trace-a")
        .expect("trace-a in list");
    let fetched = ch
        .get_trace(&ProjectId::from(PROJECT), "trace-a")
        .await
        .expect("clickhouse get_trace")
        .expect("trace-a exists");
    assert_eq!(
        describe_trace(listed),
        describe_trace(&fetched),
        "the trace list and single-trace projections disagree"
    );

    // --- spans -------------------------------------------------------------
    let span_params = ListSpansParams {
        project_id: ProjectId::from(PROJECT),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    let (duck_spans, duck_span_total) = duck.list_spans(&span_params).await.expect("duckdb spans");
    let (ch_spans, ch_span_total) = ch.list_spans(&span_params).await.expect("clickhouse spans");
    assert_eq!(
        duck_span_total, ch_span_total,
        "span total count differs: duckdb={duck_span_total} clickhouse={ch_span_total}"
    );
    assert_eq!(
        sorted(duck_spans.iter().map(describe_span).collect()),
        sorted(ch_spans.iter().map(describe_span).collect()),
        "list_spans differs between backends"
    );

    for trace_id in ["trace-a", "trace-c"] {
        let d = duck
            .get_spans_for_trace(&ProjectId::from(PROJECT), trace_id, 100)
            .await
            .expect("duckdb spans for trace");
        let c = ch
            .get_spans_for_trace(&ProjectId::from(PROJECT), trace_id, 100)
            .await
            .expect("clickhouse spans for trace");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "get_spans_for_trace({trace_id}) differs between backends"
        );
    }

    // --- sessions ----------------------------------------------------------
    let session_params = ListSessionsParams {
        project_id: ProjectId::from(PROJECT),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    let (duck_sessions, duck_session_total) = duck
        .list_sessions(&session_params)
        .await
        .expect("duckdb sessions");
    let (ch_sessions, ch_session_total) = ch
        .list_sessions(&session_params)
        .await
        .expect("clickhouse sessions");
    assert_eq!(
        duck_session_total, ch_session_total,
        "session total count differs: duckdb={duck_session_total} clickhouse={ch_session_total}"
    );
    assert_eq!(
        sorted(duck_sessions.iter().map(describe_session).collect()),
        sorted(ch_sessions.iter().map(describe_session).collect()),
        "list_sessions differs between backends"
    );

    // The session whose id is on the root span only: its totals must include the child span, which
    // carries the tokens. Restricting the aggregation to rows that name the session counted the
    // root alone and reported a session with no tokens at all.
    let root_only = duck
        .get_session(&ProjectId::from(PROJECT), "session-3")
        .await
        .expect("duckdb get_session")
        .expect("session-3 exists");
    assert_eq!(
        root_only.span_count, 2,
        "the session's span count excluded the child span"
    );
    assert_eq!(
        root_only.total_tokens, 330,
        "the session's tokens excluded the child span, which is the span that has them"
    );
    assert_eq!(
        describe_session(&root_only),
        describe_session(
            &ch.get_session(&ProjectId::from(PROJECT), "session-3")
                .await
                .expect("clickhouse get_session")
                .expect("session-3 exists")
        ),
        "get_session(session-3) differs between backends"
    );

    // And the same session as the *list* reports it. The single-session query resolves the
    // session's traces first, so it sees the child; the list grouped rows by the id they carry,
    // which is a different set - so the row a user sees in the list and the page they open from it
    // disagreed.
    let listed = duck_sessions
        .iter()
        .find(|s| s.session_id == "session-3")
        .expect("session-3 in the list");
    assert_eq!(
        describe_session(listed),
        describe_session(&root_only),
        "the session list and the single-session view disagree"
    );

    for session_id in ["session-1", "session-2"] {
        let d = duck
            .get_session(&ProjectId::from(PROJECT), session_id)
            .await
            .expect("duckdb get_session");
        let c = ch
            .get_session(&ProjectId::from(PROJECT), session_id)
            .await
            .expect("clickhouse get_session");
        match (d, c) {
            (Some(d), Some(c)) => assert_eq!(
                describe_session(&d),
                describe_session(&c),
                "get_session({session_id}) differs between backends"
            ),
            (d, c) => panic!(
                "get_session({session_id}) presence differs: duckdb={} clickhouse={}",
                d.is_some(),
                c.is_some()
            ),
        }

        let d = duck
            .get_traces_for_session(&ProjectId::from(PROJECT), session_id)
            .await
            .expect("duckdb traces for session");
        let c = ch
            .get_traces_for_session(&ProjectId::from(PROJECT), session_id)
            .await
            .expect("clickhouse traces for session");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "get_traces_for_session({session_id}) differs between backends"
        );

        let mut d = duck
            .get_trace_ids_for_sessions(&ProjectId::from(PROJECT), &[session_id.to_string()], None)
            .await
            .expect("duckdb trace ids");
        let mut c = ch
            .get_trace_ids_for_sessions(&ProjectId::from(PROJECT), &[session_id.to_string()], None)
            .await
            .expect("clickhouse trace ids");
        d.sort();
        c.sort();
        assert_eq!(
            d, c,
            "get_trace_ids_for_sessions({session_id}) differs between backends"
        );

        // The mirror direction, which the feed's context expansion depends on: given the session's traces,
        // both backends must name the same sessions. The two dialects build this one separately, and
        // ClickHouse's `session_id` is Nullable, so the empty-and-null exclusion is a real difference to
        // check rather than a formality.
        let trace_ids: Vec<String> = d.clone();
        let mut d = duck
            .get_session_ids_for_traces(&ProjectId::from(PROJECT), &trace_ids, None)
            .await
            .expect("duckdb session ids");
        let mut c = ch
            .get_session_ids_for_traces(&ProjectId::from(PROJECT), &trace_ids, None)
            .await
            .expect("clickhouse session ids");
        d.sort();
        c.sort();
        assert_eq!(
            d, c,
            "get_session_ids_for_traces({session_id}) differs between backends"
        );

        // And which trace is in which session, which is what the feed groups conversations by. Built
        // separately in each dialect - `MIN`/`min` over a Nullable column on one side - so a disagreement
        // here is a feed that collapses a cross-trace replay on one backend and duplicates it on the other.
        let mut d = duck
            .get_trace_session_pairs(&ProjectId::from(PROJECT), &trace_ids, None)
            .await
            .expect("duckdb trace/session pairs");
        let mut c = ch
            .get_trace_session_pairs(&ProjectId::from(PROJECT), &trace_ids, None)
            .await
            .expect("clickhouse trace/session pairs");
        d.sort();
        c.sort();
        assert_eq!(
            d, c,
            "get_trace_session_pairs({session_id}) differs between backends"
        );
        assert!(
            !d.is_empty(),
            "the fixture must name a session, or this comparison proves nothing"
        );
    }

    // --- message rows ------------------------------------------------------
    // The rows every messages endpoint feeds to the SideML pipeline. The goldens prove the
    // pipeline is right; they say nothing about whether this backend hands it the same rows, and
    // the two dialects build these queries separately - including the content filter and the
    // ordering the pipeline's tie-breaks depend on.
    let message_scopes = [
        (
            "span",
            MessageQueryParams {
                project_id: ProjectId::from(PROJECT),
                span_id: Some("a-gen-1".to_string()),
                trace_id: Some("trace-a".to_string()),
                ..Default::default()
            },
        ),
        (
            "trace",
            MessageQueryParams {
                project_id: ProjectId::from(PROJECT),
                trace_id: Some("trace-a".to_string()),
                ..Default::default()
            },
        ),
        (
            "session",
            MessageQueryParams {
                project_id: ProjectId::from(PROJECT),
                session_id: Some("session-1".to_string()),
                ..Default::default()
            },
        ),
        (
            "trace with no messages",
            MessageQueryParams {
                project_id: ProjectId::from(PROJECT),
                trace_id: Some("trace-d".to_string()),
                ..Default::default()
            },
        ),
        (
            // Several traces at once, which is how the project feed loads the traces on a page in
            // full before narrowing the reconstruction back to the page.
            "many traces",
            MessageQueryParams {
                project_id: ProjectId::from(PROJECT),
                trace_ids: Some(vec!["trace-a".to_string(), "trace-b".to_string()]),
                ..Default::default()
            },
        ),
    ];
    for (label, params) in message_scopes {
        let d = duck.get_messages(&params).await.expect("duckdb messages");
        let c = ch.get_messages(&params).await.expect("clickhouse messages");
        // An empty answer on both sides is equal and proves nothing, so the fixture is required
        // to produce rows where it is meant to - and none where the content filter should bite.
        match label {
            // The content filter keeps a row with no messages when it carries an error, so this
            // scope has exactly one row and it is empty of messages. Asserting only "every row is
            // empty" passed for zero rows, which is the case that would mean the filter had
            // dropped the error row entirely.
            "trace with no messages" => {
                assert_eq!(
                    d.rows.len(),
                    1,
                    "the error-only trace must still return its row: {:?}",
                    d.rows.iter().map(|r| &r.span_id).collect::<Vec<_>>()
                );
                assert_eq!(d.rows[0].messages_json, "[]");
                assert_eq!(d.rows[0].status_code.as_deref(), Some("ERROR"));
            }
            // Both traces must come back, not just the first: an IN over several ids is a new
            // clause in both dialects, and one that silently matched only one id would still look
            // non-empty.
            "many traces" => {
                let traces: std::collections::BTreeSet<&str> =
                    d.rows.iter().map(|r| r.trace_id.as_str()).collect();
                assert_eq!(
                    traces.into_iter().collect::<Vec<_>>(),
                    vec!["trace-a", "trace-b"],
                    "the multi-trace query did not return both traces"
                );
            }
            _ => assert!(
                !d.rows.is_empty(),
                "get_messages({label}) returned nothing, so this comparison is vacuous"
            ),
        }
        // Compared in order: the pipeline's dedup and history detection walk rows in the order
        // the query returns them, so two backends agreeing on the set but not the sequence can
        // still produce different feeds.
        assert_eq!(
            d.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
            c.rows.iter().map(describe_message_row).collect::<Vec<_>>(),
            "get_messages({label}) differs between backends"
        );
    }

    // --- filter options ----------------------------------------------------
    // The tag options feed the UI's filter dropdown, and its counts come from the same tags
    // column the trace projection unions.
    let describe_options = |rows: Vec<sideseat_ports::traits::FilterOptionRow>| {
        let mut described: Vec<String> = rows
            .into_iter()
            .map(|r| format!("{}={}", r.value, r.count))
            .collect();
        described.sort();
        described
    };
    let d = duck
        .get_trace_tags_options(&ProjectId::from(PROJECT), None, None)
        .await
        .expect("duckdb tags");
    let c = ch
        .get_trace_tags_options(&ProjectId::from(PROJECT), None, None)
        .await
        .expect("clickhouse tags");
    let described = describe_options(d);
    assert_eq!(
        described,
        describe_options(c),
        "get_trace_tags_options differs between backends"
    );
    // The fixture's tags, with the trace counts they carry: alpha is on trace-a and trace-b, the
    // rest on trace-a alone. Two empty lists would otherwise pass.
    //
    // `say "café"` is the one that matters here: ClickHouse returns tag values as raw JSON, and
    // unquoting them by trimming the outer quotes offered this tag as `say \"caf\u00e9\"` - a value
    // the filter could never match. Both backends must produce the decoded string.
    assert_eq!(
        described,
        vec!["alpha=2", "beta=1", "gamma=1", "say \"café\"=1", "shared=1"],
        "the tag options do not match the fixture's tags"
    );
}
