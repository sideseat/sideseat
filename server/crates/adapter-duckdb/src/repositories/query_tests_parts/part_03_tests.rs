/// Regression: Strands hierarchy with TWO generation cycles.
/// Agent(sum) → cycle1(0) → Gen1(tokens) → Botocore1(tokens)
///            → cycle2(0) → Gen2(tokens) → Botocore2(tokens)
/// Should count Gen1 + Gen2 only.
#[tokio::test]
async fn test_strands_multi_cycle_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-strands-multi-cycle";

    let agent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "agent".to_string(),
        parent_span_id: None,
        span_name: "Agent".to_string(),
        observation_type: Some(ObservationType::Agent),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 3000, // aggregated sum
        gen_ai_usage_output_tokens: 600,
        gen_ai_cost_total: 0.04,
        ..Default::default()
    };

    // Cycle 1 → Gen1 → Botocore1
    let cycle1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "cycle-1".to_string(),
        parent_span_id: Some("agent".to_string()),
        span_name: "execute_event_loop_cycle".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        ..Default::default()
    };

    let gen1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "gen-1".to_string(),
        parent_span_id: Some("cycle-1".to_string()),
        span_name: "chat".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    let botocore1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-1".to_string(),
        parent_span_id: Some("gen-1".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    // Cycle 2 → Gen2 → Botocore2
    let cycle2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "cycle-2".to_string(),
        parent_span_id: Some("agent".to_string()),
        span_name: "execute_event_loop_cycle".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        ..Default::default()
    };

    let gen2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "gen-2".to_string(),
        parent_span_id: Some("cycle-2".to_string()),
        span_name: "chat".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 2000,
        gen_ai_usage_output_tokens: 400,
        gen_ai_cost_total: 0.03,
        ..Default::default()
    };

    let botocore2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-2".to_string(),
        parent_span_id: Some("gen-2".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 2000,
        gen_ai_usage_output_tokens: 400,
        gen_ai_cost_total: 0.03,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(
            &conn,
            &[agent, cycle1, gen1, botocore1, cycle2, gen2, botocore2],
        )
        .expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Gen1($0.01, 1000+200) + Gen2($0.03, 2000+400) = $0.04, 3000+600
    // NOT Agent($0.04, 3000+600) which would be double-counted
    assert_eq!(trace.input_tokens, 3000, "Gen1(1000) + Gen2(2000)");
    assert_eq!(trace.output_tokens, 600, "Gen1(200) + Gen2(400)");
    assert_eq!(trace.total_cost, 0.04, "Gen1(0.01) + Gen2(0.03)");
}

/// Regression: Vercel AI SDK pattern where root generation has 0 tokens
/// and child doGenerate spans carry the actual token data.
/// ai.generateText(0 tokens) → ai.generateText.doGenerate(tokens)
/// Should count the child, not the empty root.
#[tokio::test]
async fn test_vercel_root_generation_zero_tokens_child_counted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-vercel";

    // Root generation orchestrator: 0 tokens (just orchestrates)
    let root = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "root-gen".to_string(),
        parent_span_id: None,
        span_name: "ai.generateText".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        // 0 tokens — orchestrator only
        ..Default::default()
    };

    // First doGenerate: succeeds with tokens
    let do_gen1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "do-gen-1".to_string(),
        parent_span_id: Some("root-gen".to_string()),
        span_name: "ai.generateText.doGenerate".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 754,
        gen_ai_usage_output_tokens: 235,
        gen_ai_usage_total_tokens: 989,
        gen_ai_cost_total: 0.0019,
        ..Default::default()
    };

    // Second doGenerate: errors with 0 tokens
    let do_gen2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "do-gen-2".to_string(),
        parent_span_id: Some("root-gen".to_string()),
        span_name: "ai.generateText.doGenerate".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        // 0 tokens — errored
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[root, do_gen1, do_gen2]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Root has 0 tokens → excluded from Path 1
    // do_gen1 has tokens, parent (root) is generation with 0 tokens → included
    // do_gen2 has 0 tokens → excluded
    assert_eq!(trace.input_tokens, 754, "Child doGenerate tokens counted");
    assert_eq!(trace.output_tokens, 235, "Child doGenerate tokens counted");
    assert_eq!(trace.total_cost, 0.0019, "Child doGenerate cost counted");
}

// ========================================================================
// Feed API tests
// ========================================================================

#[tokio::test]
async fn test_get_feed_spans_basic() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Insert test spans
    let spans = vec![
        make_generation_span(project_id, "trace-1", "span-1", None, 0.01, 100),
        make_generation_span(project_id, "trace-2", "span-2", None, 0.02, 200),
        make_agent_span(project_id, "trace-3", "span-3", None),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = FeedSpansParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        ..Default::default()
    };
    let result = get_feed_spans(&conn, &params).expect("Query should succeed");

    // Should return all 3 spans
    assert_eq!(result.len(), 3);
}

#[tokio::test]
async fn test_get_feed_spans_is_observation_filter() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Insert mix of observation and non-observation spans
    let spans = vec![
        make_generation_span(project_id, "trace-1", "gen-1", None, 0.01, 100),
        make_agent_span(project_id, "trace-2", "agent-1", None), // agent has observation_type
        NormalizedSpan {
            project_id: Some(project_id.to_string()),
            trace_id: "trace-3".to_string(),
            span_id: "plain-1".to_string(),
            span_name: "plain-span".to_string(),
            timestamp_start: Utc::now(),
            // No observation_type, no gen_ai_request_model
            ..Default::default()
        },
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();

    // With is_observation = true
    let params = FeedSpansParams {
        project_id: ProjectId::from(project_id),
        limit: 10,
        is_observation: Some(true),
        ..Default::default()
    };
    let result = get_feed_spans(&conn, &params).expect("Query should succeed");

    // Should return only generation and agent spans (both have observation_type)
    assert_eq!(result.len(), 2, "Should filter to observations only");
}

#[tokio::test]
async fn test_get_feed_spans_limit() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Insert 5 spans
    let spans: Vec<_> = (0..5)
        .map(|i| {
            make_generation_span(
                project_id,
                &format!("trace-{}", i),
                &format!("span-{}", i),
                None,
                0.01,
                100,
            )
        })
        .collect();

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = FeedSpansParams {
        project_id: ProjectId::from(project_id),
        limit: 3, // Limit to 3
        ..Default::default()
    };
    let result = get_feed_spans(&conn, &params).expect("Query should succeed");

    assert_eq!(result.len(), 3, "Should respect limit");
}

#[tokio::test]
async fn test_get_feed_spans_cursor_pagination() {
    use chrono::Duration;

    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Create spans with different ingested_at times
    let base_time = Utc::now();
    let spans: Vec<_> = (0..5)
        .map(|i| {
            let mut span = make_generation_span(
                project_id,
                &format!("trace-{}", i),
                &format!("span-{}", i),
                None,
                0.01,
                100,
            );
            span.timestamp_start = base_time + Duration::seconds(i as i64);
            span.ingested_at = Some(base_time + Duration::seconds(i as i64));
            span
        })
        .collect();

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();

    // First page (no cursor)
    let params = FeedSpansParams {
        project_id: ProjectId::from(project_id),
        limit: 2,
        ..Default::default()
    };
    let page1 = get_feed_spans(&conn, &params).expect("Query should succeed");
    assert_eq!(page1.len(), 2, "First page should have 2 spans");

    // Second page (with cursor from last span of page1)
    let last_span = page1.last().unwrap();
    let cursor_time_us = last_span.ingested_at.timestamp_micros();
    let params = FeedSpansParams {
        project_id: ProjectId::from(project_id),
        limit: 2,
        cursor: Some((
            cursor_time_us,
            last_span.span_id.clone(),
            last_span.trace_id.clone(),
        )),
        ..Default::default()
    };
    let page2 = get_feed_spans(&conn, &params).expect("Query should succeed");
    assert_eq!(page2.len(), 2, "Second page should have 2 spans");

    // Verify no overlap between pages
    let page1_ids: Vec<_> = page1.iter().map(|s| &s.span_id).collect();
    let page2_ids: Vec<_> = page2.iter().map(|s| &s.span_id).collect();
    for id in &page2_ids {
        assert!(
            !page1_ids.contains(id),
            "Page 2 should not contain spans from page 1"
        );
    }
}

#[tokio::test]
async fn test_get_feed_spans_empty_project() {
    let (_temp_dir, analytics) = create_test_service().await;

    let conn = analytics.conn();
    let params = FeedSpansParams {
        project_id: ProjectId::from("nonexistent-project"),
        limit: 10,
        ..Default::default()
    };
    let result = get_feed_spans(&conn, &params).expect("Query should succeed");

    assert!(
        result.is_empty(),
        "Should return empty for nonexistent project"
    );
}

// ========================================================================
// A trace filter means what the trace list displays
// ========================================================================

use sideseat_ports::filters::{Filter, NullOp, NumberOp, OptionsOp, StringOp};

fn trace_filter_params(project_id: &str, filters: Vec<Filter>) -> ListTracesParams {
    ListTracesParams {
        project_id: ProjectId::from(project_id),
        page: 1,
        limit: 50,
        filters,
        ..Default::default()
    }
}

/// A token filter compares against the total the row shows, not against one span of it.
///
/// The list displays a trace's tokens as a sum over its spans, and the filter was applied to
/// each span row separately, so "more than 2500 tokens" hid a trace displaying 3000 because no
/// single span reached 2500. The same mismatch let "fewer than 1500" return that trace, since
/// one span qualifies. Both directions are wrong against what the user can see.
#[tokio::test]
async fn a_token_filter_matches_the_total_the_trace_displays() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-aggregate-tokens";

    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
        make_generation_span(project_id, trace_id, "gen-2", Some("agent-1"), 0.02, 2000),
    ];
    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("insert");
    }

    let conn = analytics.conn();
    let displayed = get_trace(&conn, project_id, trace_id)
        .expect("query")
        .expect("trace exists");
    assert_eq!(displayed.total_tokens, 3000, "premise of the test");

    let (rows, total) = list_traces(
        &conn,
        &trace_filter_params(
            project_id,
            vec![Filter::Number {
                column: "total_tokens".to_string(),
                operator: NumberOp::Gt,
                value: 2500.0,
            }],
        ),
    )
    .expect("query");
    assert_eq!(
        rows.len(),
        1,
        "a trace displaying 3000 tokens must match `> 2500`; no single span reaches it"
    );
    assert_eq!(total, 1, "the count must agree with the page");

    let (rows, _) = list_traces(
        &conn,
        &trace_filter_params(
            project_id,
            vec![Filter::Number {
                column: "total_tokens".to_string(),
                operator: NumberOp::Lt,
                value: 1500.0,
            }],
        ),
    )
    .expect("query");
    assert!(
        rows.is_empty(),
        "a trace displaying 3000 tokens must not match `< 1500` because one span does"
    );
}

/// A span id is unique only within a trace, so token dedup must not look outside one.
///
/// The parent/child checks that stop a nested generation from being counted twice matched on span
/// id alone. Two traces from the same framework routinely reuse an id shape, and a generation in
/// trace B whose parent id equalled a span id in trace A suppressed trace A's tokens - the trace
/// reported zero while its span carried usage.
#[tokio::test]
async fn token_dedup_does_not_reach_into_another_trace() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // trace-1: a lone generation whose tokens must count.
    let lone = make_generation_span(project_id, "trace-1", "shared-id", None, 0.01, 500);
    // trace-2: a generation whose parent is "shared-id" - the same span id, another trace.
    let child = make_generation_span(project_id, "trace-2", "child", Some("shared-id"), 0.02, 700);
    let root = make_agent_span(project_id, "trace-2", "shared-id", None);
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[lone, child, root]).expect("insert");
    }

    let conn = analytics.conn();
    let first = get_trace(&conn, project_id, "trace-1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        first.total_tokens, 500,
        "trace-1's generation was suppressed by a same-id span in another trace"
    );
    let second = get_trace(&conn, project_id, "trace-2")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        second.total_tokens, 700,
        "trace-2 should count its own leaf generation once"
    );
}

/// A span with no observation type still reports what it was billed.
///
/// Transport-level instrumentation records usage and cost on a plain span, leaving
/// observation_type NULL. The billing branch for a non-generation span tested
/// `observation_type != 'generation'`, which is NULL for such a span - so the row never
/// qualified and the trace reported zero tokens and zero cost while its span carried both.
#[tokio::test]
async fn a_span_with_no_observation_type_is_still_billed() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut plain = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
    plain.observation_type = None;
    plain.gen_ai_usage_input_tokens = 11;
    plain.gen_ai_usage_output_tokens = 2;
    plain.gen_ai_usage_total_tokens = 13;
    plain.gen_ai_cost_total = 0.004;
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[plain]).expect("insert");
    }

    let conn = analytics.conn();
    let trace = get_trace(&conn, project_id, "trace-1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        trace.total_tokens, 13,
        "a plain span's token usage was not counted"
    );
    assert!(
        (trace.total_cost - 0.004).abs() < 1e-9,
        "a plain span's cost was not counted: {}",
        trace.total_cost
    );
}

/// A span reporting only a total token count is still billed.
///
/// `gen_ai.usage.total_tokens` and OpenInference's `llm.token_count.total` are extracted on their
/// own, with no input/output breakdown - and the predicate admitting rows to aggregation summed
/// the input and output columns, which are zero for such a span. The trace was visible and
/// reported zero tokens.
#[tokio::test]
async fn a_total_only_token_count_is_still_billed() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut total_only = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
    total_only.gen_ai_usage_input_tokens = 0;
    total_only.gen_ai_usage_output_tokens = 0;
    total_only.gen_ai_usage_total_tokens = 42;
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[total_only]).expect("insert");
    }

    let conn = analytics.conn();
    let trace = get_trace(&conn, project_id, "trace-1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        trace.total_tokens, 42,
        "a span reporting only a total was aggregated as zero"
    );
}

/// Cache and reasoning usage count on their own.
///
/// Extraction accepts each of them independently of the input/output breakdown, so a span can
/// report cache reads or reasoning tokens and nothing else - a prompt-cache hit that ran no new
/// completion looks exactly like that. Admission summed input, output and total, all zero here,
/// so the trace reported nothing.
#[tokio::test]
async fn cache_and_reasoning_usage_count_on_their_own() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut cache_only = make_generation_span(project_id, "trace-1", "root", None, 0.0, 0);
    cache_only.gen_ai_usage_input_tokens = 0;
    cache_only.gen_ai_usage_output_tokens = 0;
    cache_only.gen_ai_usage_total_tokens = 0;
    cache_only.gen_ai_usage_cache_read_tokens = 700;

    let mut reasoning_only = make_generation_span(project_id, "trace-2", "root", None, 0.0, 0);
    reasoning_only.gen_ai_usage_input_tokens = 0;
    reasoning_only.gen_ai_usage_output_tokens = 0;
    reasoning_only.gen_ai_usage_total_tokens = 0;
    reasoning_only.gen_ai_usage_reasoning_tokens = 64;
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[cache_only, reasoning_only]).expect("insert");
    }

    let conn = analytics.conn();
    let cached = get_trace(&conn, project_id, "trace-1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        cached.cache_read_tokens, 700,
        "a cache-only span was aggregated as zero"
    );
    let reasoned = get_trace(&conn, project_id, "trace-2")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        reasoned.reasoning_tokens, 64,
        "a reasoning-only span was aggregated as zero"
    );
}

/// An empty "none of" is not a filter, and must not exclude everything.
///
/// The negated form is rendered by excluding the matches of its positive twin, and an empty
/// option list renders as `1 = 1` - so negating the subquery around it excluded every trace.
/// "None of nothing" has to mean "everything".
#[tokio::test]
async fn an_empty_exclusion_list_filters_nothing() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let spans = vec![make_generation_span(
        project_id, "trace-1", "gen-1", None, 0.01, 100,
    )];
    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("insert");
    }

    let conn = analytics.conn();
    let (rows, total) = list_traces(
        &conn,
        &trace_filter_params(
            project_id,
            vec![Filter::StringOptions {
                column: "gen_ai_request_model".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec![],
            }],
        ),
    )
    .expect("query");
    assert_eq!(
        rows.len(),
        1,
        "an empty exclusion list excluded the trace instead of filtering nothing"
    );
    assert_eq!(total, 1, "the count must agree with the page");
}

/// A filtered session list shows the whole session, not the part that matched.
///
/// Selection and membership are separate questions. Answering both with one predicate returned a
/// session of two traces, filtered to one environment, carrying only that trace's times, counts
/// and cost - while opening the session showed both traces.
#[tokio::test]
async fn a_filtered_session_is_still_the_whole_session() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut first = make_generation_span(project_id, "trace-1", "gen-1", None, 0.01, 1000);
    first.session_id = Some("session-5".to_string());
    first.environment = Some("prod".to_string());
    let mut second = make_generation_span(project_id, "trace-2", "gen-2", None, 0.02, 2000);
    second.session_id = Some("session-5".to_string());
    second.environment = Some("dev".to_string());
    second.timestamp_start = first.timestamp_start + chrono::Duration::seconds(30);
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[first, second]).expect("insert");
    }

    let conn = analytics.conn();
    let params = ListSessionsParams {
        project_id: ProjectId::from(project_id),
        page: 1,
        limit: 50,
        environment: Some(vec!["prod".to_string()]),
        ..Default::default()
    };
    let (rows, total) = list_sessions(&conn, &params).expect("query");
    assert_eq!(rows.len(), 1, "the session matches through its prod trace");
    assert_eq!(total, 1, "the count must agree with the page");
    assert_eq!(
        rows[0].trace_count, 2,
        "the session has two traces; the filter selected it, it does not shrink it"
    );
    assert_eq!(
        rows[0].total_tokens, 3000,
        "both traces called the model: {} reported",
        rows[0].total_tokens
    );
}

/// A filter on a value the row displays matches that value, not any span's copy of it.
///
/// A trace records its session on the root span; its children carry none. Asked of span rows,
/// "session is null" was true of every such trace - the children satisfy it - so the filter
/// returned traces displayed under a session, and no filter excluded them.
#[tokio::test]
async fn a_filter_on_a_displayed_attribute_matches_what_the_row_shows() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut with_session = make_agent_span(project_id, "trace-has-session", "agent-1", None);
    with_session.session_id = Some("session-9".to_string());
    // The child carries no session id, which is what made the row-level check wrong.
    let child = make_generation_span(
        project_id,
        "trace-has-session",
        "gen-1",
        Some("agent-1"),
        0.01,
        100,
    );
    let lonely = make_generation_span(project_id, "trace-no-session", "gen-2", None, 0.01, 100);
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[with_session, child, lonely]).expect("insert");
    }

    let conn = analytics.conn();
    let (rows, total) = list_traces(
        &conn,
        &trace_filter_params(
            project_id,
            vec![Filter::Null {
                column: "session_id".to_string(),
                operator: NullOp::IsNull,
            }],
        ),
    )
    .expect("query");
    let ids: Vec<&String> = rows.iter().map(|t| &t.trace_id).collect();
    assert_eq!(
        ids,
        vec![&"trace-no-session".to_string()],
        "only the trace displaying no session has none: {ids:?}"
    );
    assert_eq!(total, 1, "the count must agree with the page");
}

/// "None of" means no span used it, not that some span used something else.
///
/// A trace that called claude-haiku once and another model next came back from the filter that
/// excluded claude-haiku: the second span satisfied "not claude-haiku" on its own.
#[tokio::test]
async fn excluding_a_value_excludes_every_trace_that_used_it() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut mixed = make_generation_span(project_id, "trace-mixed", "gen-1", None, 0.01, 100);
    mixed.gen_ai_request_model = Some("claude-haiku".to_string());
    let mut mixed_second =
        make_generation_span(project_id, "trace-mixed", "gen-2", Some("gen-1"), 0.01, 100);
    mixed_second.gen_ai_request_model = Some("claude-sonnet".to_string());
    let mut other = make_generation_span(project_id, "trace-other", "gen-3", None, 0.01, 100);
    other.gen_ai_request_model = Some("claude-sonnet".to_string());
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[mixed, mixed_second, other]).expect("insert");
    }

    let conn = analytics.conn();
    let (rows, total) = list_traces(
        &conn,
        &trace_filter_params(
            project_id,
            vec![Filter::StringOptions {
                column: "gen_ai_request_model".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["claude-haiku".to_string()],
            }],
        ),
    )
    .expect("query");
    let ids: Vec<&String> = rows.iter().map(|t| &t.trace_id).collect();
    assert_eq!(
        ids,
        vec![&"trace-other".to_string()],
        "the trace that used claude-haiku in one of its two calls must be excluded: {ids:?}"
    );
    assert_eq!(total, 1, "the count must agree with the page");
}

/// Two filters describe the trace, not one span of it.
///
/// The session id lives on the root span and the tokens on its generation child - the ordinary
/// shape for every framework that opens a root agent span. ANDing both conditions on a single
/// span row asks for a span that carries the session *and* the tokens, which no span does, so
/// the trace vanished from a list that showed it under either filter alone.
#[tokio::test]
async fn two_trace_filters_describe_the_trace_not_one_span() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-split-attributes";

    let mut root = make_agent_span(project_id, trace_id, "agent-1", None);
    root.session_id = Some("session-7".to_string());
    let child = make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.02, 2000);
    {
        let conn = analytics.conn();
        insert_batch(&conn, &[root, child]).expect("insert");
    }

    let conn = analytics.conn();
    let session_only = trace_filter_params(
        project_id,
        vec![Filter::String {
            column: "session_id".to_string(),
            operator: StringOp::Eq,
            value: "session-7".to_string(),
        }],
    );
    let (rows, _) = list_traces(&conn, &session_only).expect("query");
    assert_eq!(rows.len(), 1, "premise: the session filter alone matches");

    let mut both = session_only.clone();
    both.filters.push(Filter::Number {
        column: "total_tokens".to_string(),
        operator: NumberOp::Gt,
        value: 100.0,
    });
    let (rows, total) = list_traces(&conn, &both).expect("query");
    assert_eq!(
        rows.len(),
        1,
        "the session is on the root span and the tokens on its child; both describe the trace"
    );
    assert_eq!(total, 1, "the count must agree with the page");
}
