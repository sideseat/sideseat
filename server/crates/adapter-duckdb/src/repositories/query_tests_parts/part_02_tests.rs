/// Test get_traces_for_session with nested generations
#[tokio::test]
async fn test_get_traces_for_session_nested_generations_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let session_id = "session-traces";

    // Create two traces in the same session with different patterns
    // Trace 1: nested generations (should count $0.01)
    let mut spans1 = vec![
        make_agent_span(project_id, "trace-1", "agent-1", None),
        make_generation_span(
            project_id,
            "trace-1",
            "parent-gen",
            Some("agent-1"),
            0.01,
            1000,
        ),
        make_generation_span(
            project_id,
            "trace-1",
            "child-gen",
            Some("parent-gen"),
            0.01,
            1000,
        ),
    ];
    // Trace 2: sibling generations (should count $0.03)
    let mut spans2 = vec![
        make_agent_span(project_id, "trace-2", "agent-2", None),
        make_generation_span(project_id, "trace-2", "gen-a", Some("agent-2"), 0.01, 1000),
        make_generation_span(project_id, "trace-2", "gen-b", Some("agent-2"), 0.02, 2000),
    ];

    // All in same session
    for span in spans1.iter_mut().chain(spans2.iter_mut()) {
        span.session_id = Some(session_id.to_string());
    }

    // Set timestamps for ordering
    let base_ts = Utc::now();
    for span in &mut spans1 {
        span.timestamp_start = base_ts;
    }
    for span in &mut spans2 {
        span.timestamp_start = base_ts + chrono::Duration::seconds(1);
    }

    {
        let conn = analytics.conn();
        let all_spans: Vec<_> = spans1.into_iter().chain(spans2).collect();
        insert_batch(&conn, &all_spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let traces =
        get_traces_for_session(&conn, project_id, session_id).expect("Query should succeed");

    assert_eq!(traces.len(), 2, "Should have 2 traces");

    let trace1 = traces
        .iter()
        .find(|t| t.trace_id == "trace-1")
        .expect("Trace 1 should exist");
    let trace2 = traces
        .iter()
        .find(|t| t.trace_id == "trace-2")
        .expect("Trace 2 should exist");

    // Trace 1: nested - should count only leaf ($0.01)
    assert_eq!(trace1.total_cost, 0.01, "Trace 1 should not double-count");
    assert_eq!(
        trace1.total_tokens, 1000,
        "Trace 1 tokens should not double-count"
    );

    // Trace 2: siblings - should count both ($0.03)
    assert_eq!(
        trace2.total_cost, 0.03,
        "Trace 2 should count both siblings"
    );
    assert_eq!(
        trace2.total_tokens, 3000,
        "Trace 2 tokens should count both"
    );
}

/// Test cache tokens in list_sessions
#[tokio::test]
async fn test_list_sessions_cache_tokens_from_generation_leaf() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let session_id = "session-cache";

    // Strands pattern: parent generation has cache tokens, child is Span type
    let parent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-cache".to_string(),
        span_id: "parent-chat".to_string(),
        parent_span_id: None,
        span_name: "chat".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 100,
        gen_ai_usage_output_tokens: 50,
        gen_ai_usage_total_tokens: 150,
        gen_ai_usage_cache_read_tokens: 100,
        gen_ai_usage_cache_write_tokens: 200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    // Real Strands: child is Span type
    let child = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-cache".to_string(),
        span_id: "child-bedrock".to_string(),
        parent_span_id: Some("parent-chat".to_string()),
        span_name: "chat us.amazon.nova".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 100,
        gen_ai_usage_output_tokens: 50,
        gen_ai_usage_total_tokens: 0,
        gen_ai_usage_cache_read_tokens: 0,
        gen_ai_usage_cache_write_tokens: 0,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[parent, child]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = ListSessionsParams {
        project_id: ProjectId::from(project_id),
        page: 0,
        limit: 100,
        ..Default::default()
    };
    let (sessions, _total) = list_sessions(&conn, &params).expect("Query should succeed");

    assert_eq!(sessions.len(), 1, "Should have 1 session");
    let session = &sessions[0];

    // Parent generation is leaf (no generation child) - all its data counted
    assert_eq!(
        session.input_tokens, 100,
        "Input tokens from generation leaf"
    );
    assert_eq!(
        session.output_tokens, 50,
        "Output tokens from generation leaf"
    );
    assert_eq!(
        session.cache_read_tokens, 100,
        "Cache read from generation leaf"
    );
    assert_eq!(
        session.cache_write_tokens, 200,
        "Cache write from generation leaf"
    );
    assert_eq!(session.total_cost, 0.01, "Cost from generation leaf");
}

/// Test get_traces_for_session cache tokens
#[tokio::test]
async fn test_get_traces_for_session_cache_tokens_from_generation_leaf() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let session_id = "session-cache-traces";

    // Strands pattern in two traces: parent generation + child Span
    let parent1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: "parent-1".to_string(),
        parent_span_id: None,
        span_name: "chat".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 100,
        gen_ai_usage_cache_read_tokens: 50,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    let child1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: "child-1".to_string(),
        parent_span_id: Some("parent-1".to_string()),
        span_name: "bedrock".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 100,
        gen_ai_usage_cache_read_tokens: 0,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    let parent2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-2".to_string(),
        span_id: "parent-2".to_string(),
        parent_span_id: None,
        span_name: "chat".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now() + chrono::Duration::seconds(1),
        gen_ai_usage_input_tokens: 200,
        gen_ai_usage_cache_read_tokens: 75,
        gen_ai_cost_total: 0.02,
        ..Default::default()
    };

    let child2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: "trace-2".to_string(),
        span_id: "child-2".to_string(),
        parent_span_id: Some("parent-2".to_string()),
        span_name: "bedrock".to_string(),
        session_id: Some(session_id.to_string()),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now() + chrono::Duration::seconds(1),
        gen_ai_usage_input_tokens: 200,
        gen_ai_usage_cache_read_tokens: 0,
        gen_ai_cost_total: 0.02,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[parent1, child1, parent2, child2]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let traces =
        get_traces_for_session(&conn, project_id, session_id).expect("Query should succeed");

    assert_eq!(traces.len(), 2, "Should have 2 traces");

    let trace1 = traces
        .iter()
        .find(|t| t.trace_id == "trace-1")
        .expect("Trace 1 should exist");
    let trace2 = traces
        .iter()
        .find(|t| t.trace_id == "trace-2")
        .expect("Trace 2 should exist");

    // Each trace counts its generation leaf (parent has no generation child)
    assert_eq!(
        trace1.input_tokens, 100,
        "Trace 1 input from generation leaf"
    );
    assert_eq!(
        trace1.cache_read_tokens, 50,
        "Trace 1 cache from generation leaf"
    );
    assert_eq!(trace1.total_cost, 0.01, "Trace 1 cost from generation leaf");

    assert_eq!(
        trace2.input_tokens, 200,
        "Trace 2 input from generation leaf"
    );
    assert_eq!(
        trace2.cache_read_tokens, 75,
        "Trace 2 cache from generation leaf"
    );
    assert_eq!(trace2.total_cost, 0.02, "Trace 2 cost from generation leaf");
}

/// Test session with multiple traces having different nesting patterns
#[tokio::test]
async fn test_session_multiple_traces_aggregation() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let session_id = "session-multi";

    // Trace 1: nested - only root counted ($0.01, 1000 tokens)
    let mut spans1 = vec![
        make_generation_span(project_id, "trace-1", "parent", None, 0.01, 1000),
        make_generation_span(project_id, "trace-1", "child", Some("parent"), 0.01, 1000),
    ];

    // Trace 2: siblings - both are roots ($0.01 + $0.02 = $0.03, 3000 tokens)
    let mut spans2 = vec![
        make_generation_span(project_id, "trace-2", "gen-a", None, 0.01, 1000),
        make_generation_span(project_id, "trace-2", "gen-b", None, 0.02, 2000),
    ];

    // Trace 3: single root ($0.05, 500 tokens)
    let mut spans3 = vec![make_generation_span(
        project_id, "trace-3", "single", None, 0.05, 500,
    )];

    for span in spans1
        .iter_mut()
        .chain(spans2.iter_mut())
        .chain(spans3.iter_mut())
    {
        span.session_id = Some(session_id.to_string());
    }

    // Set distinct timestamps
    let base_ts = Utc::now();
    spans1[0].timestamp_start = base_ts;
    spans1[1].timestamp_start = base_ts;
    spans2[0].timestamp_start = base_ts + chrono::Duration::seconds(1);
    spans2[1].timestamp_start = base_ts + chrono::Duration::seconds(1);
    spans3[0].timestamp_start = base_ts + chrono::Duration::seconds(2);

    {
        let conn = analytics.conn();
        let all_spans: Vec<_> = spans1.into_iter().chain(spans2).chain(spans3).collect();
        insert_batch(&conn, &all_spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_session(&conn, project_id, session_id).expect("Query should succeed");
    let session = result.expect("Session should exist");

    // Session total: trace1($0.01) + trace2($0.03) + trace3($0.05) = $0.09
    assert_eq!(
        session.total_cost, 0.09,
        "Session should sum all root generation costs"
    );
    // Tokens: trace1(1000) + trace2(3000) + trace3(500) = 4500
    assert_eq!(
        session.total_tokens, 4500,
        "Session should sum all root generation tokens"
    );
    assert_eq!(session.trace_count, 3, "Session should have 3 traces");
}

/// Test deeply nested (4 levels) generations - only root counted
#[tokio::test]
async fn test_deeply_nested_4_levels() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-4-levels";

    // 4-level nesting: gen1 -> gen2 -> gen3 -> gen4
    // Only gen1 is a root (has no generation parent)
    let spans = vec![
        make_generation_span(project_id, trace_id, "gen-1", None, 0.10, 1000),
        make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.10, 1000),
        make_generation_span(project_id, trace_id, "gen-3", Some("gen-2"), 0.10, 1000),
        make_generation_span(project_id, trace_id, "gen-4", Some("gen-3"), 0.10, 1000),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Only gen-1 (root) counted = $0.10, 1000 tokens
    assert_eq!(trace.total_cost, 0.10, "Should count only root generation");
    assert_eq!(
        trace.total_tokens, 1000,
        "Should count only root generation tokens"
    );
}

/// Test trace with no generation spans (only agent/tool spans)
#[tokio::test]
async fn test_trace_no_generations() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-no-gen";

    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_agent_span(project_id, trace_id, "agent-2", Some("agent-1")),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // No generations = zero cost/tokens
    assert_eq!(trace.total_cost, 0.0, "No generations = zero cost");
    assert_eq!(trace.total_tokens, 0, "No generations = zero tokens");
}

/// Test multiple independent generation roots in same trace
#[tokio::test]
async fn test_multiple_independent_generation_roots() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-multi-roots";

    // gen-1 is root (parent agent-1 is not a generation)
    // gen-2 is NOT root (parent gen-1 is a generation)
    // gen-3 is root (parent agent-2 is not a generation)
    // gen-4 is NOT root (parent gen-3 is a generation)
    // Total = gen-1 ($0.01) + gen-3 ($0.02) = $0.03
    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 100),
        make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.01, 100),
        make_agent_span(project_id, trace_id, "agent-2", None),
        make_generation_span(project_id, trace_id, "gen-3", Some("agent-2"), 0.02, 200),
        make_generation_span(project_id, trace_id, "gen-4", Some("gen-3"), 0.02, 200),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Two roots: gen-1 + gen-3 = $0.01 + $0.02 = $0.03
    assert_eq!(
        trace.total_cost, 0.03,
        "Should sum costs from generation roots"
    );
    assert_eq!(
        trace.total_tokens, 300,
        "Should sum tokens from generation roots"
    );
}

/// Test nested generations where only root is counted
#[tokio::test]
async fn test_nested_generations_only_leaf_counted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-nested-gen";

    // Parent generation has a generation child → excluded
    // Child generation has no generation child → included (leaf)
    let spans = vec![
        make_generation_span(project_id, trace_id, "parent", None, 0.10, 1000),
        make_generation_span(project_id, trace_id, "child", Some("parent"), 0.01, 100),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Only leaf generation is counted
    assert_eq!(trace.total_cost, 0.01, "Only leaf generation cost");
    assert_eq!(trace.total_tokens, 100, "Only leaf generation tokens");
}

/// Test realistic Strands data where parent "chat" has complete data including cache.
/// Child "bedrock" is observation_type=Span (not Generation) in real Strands data,
/// so the parent generation (which has no generation child) is the leaf and gets counted.
#[tokio::test]
async fn test_chain_mixed_values_across_levels() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-mixed-values";

    // Realistic Strands: parent "chat" is generation with ALL data
    let parent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "parent".to_string(),
        parent_span_id: None,
        span_name: "chat".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 500,
        gen_ai_usage_cache_read_tokens: 500,
        gen_ai_usage_cache_write_tokens: 100,
        gen_ai_cost_input: 0.01,
        gen_ai_cost_output: 0.02,
        gen_ai_cost_cache_read: 0.001,
        gen_ai_cost_cache_write: 0.002,
        gen_ai_cost_total: 0.033,
        ..Default::default()
    };

    // Child is Span type (real Strands botocore pattern), excluded by Path 2
    // since trace has generation spans
    let child = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "child".to_string(),
        parent_span_id: Some("parent".to_string()),
        span_name: "bedrock".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 500,
        gen_ai_cost_input: 0.01,
        gen_ai_cost_output: 0.02,
        gen_ai_cost_total: 0.03,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[parent, child]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Parent generation is leaf (no generation child) - counted with complete data
    assert_eq!(trace.input_tokens, 1000, "Input from generation leaf");
    assert_eq!(trace.output_tokens, 500, "Output from generation leaf");
    assert_eq!(
        trace.cache_read_tokens, 500,
        "Cache read from generation leaf"
    );
    assert_eq!(
        trace.cache_write_tokens, 100,
        "Cache write from generation leaf"
    );
    assert_eq!(trace.input_cost, 0.01, "Input cost from generation leaf");
    assert_eq!(trace.output_cost, 0.02, "Output cost from generation leaf");
    assert_eq!(
        trace.cache_read_cost, 0.001,
        "Cache read cost from generation leaf"
    );
    assert_eq!(
        trace.cache_write_cost, 0.002,
        "Cache write cost from generation leaf"
    );
}

// ========================================================================
// Regression: Strands/botocore non-generation spans with tokens
// ========================================================================

/// Regression: botocore RPC span (observation_type=Span) carries tokens but
/// parent agent span has 0 tokens. Old query filtered observation_type='generation'
/// which excluded botocore spans entirely, yielding 0 tokens at trace level.
#[tokio::test]
async fn test_strands_botocore_tokens_from_non_generation_span() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-strands-botocore";

    // Agent span: parent, 0 tokens (StrandsAgents pattern)
    let agent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "agent-span".to_string(),
        parent_span_id: None,
        span_name: "Agent".to_string(),
        observation_type: Some(ObservationType::Agent),
        timestamp_start: Utc::now(),
        ..Default::default()
    };

    // Botocore RPC span: child, has tokens but observation_type=Span
    let botocore = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-span".to_string(),
        parent_span_id: Some("agent-span".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 940,
        gen_ai_usage_output_tokens: 160,
        gen_ai_usage_total_tokens: 1100,
        gen_ai_cost_input: 0.003,
        gen_ai_cost_output: 0.002,
        gen_ai_cost_total: 0.005,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[agent, botocore]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    assert_eq!(trace.input_tokens, 940, "Botocore input tokens counted");
    assert_eq!(trace.output_tokens, 160, "Botocore output tokens counted");
    assert_eq!(trace.total_tokens, 1100, "Botocore total tokens counted");
    assert_eq!(trace.total_cost, 0.005, "Botocore cost counted");
}

/// Regression: multiple botocore calls under one agent should all be summed.
#[tokio::test]
async fn test_strands_multiple_botocore_spans_summed() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-strands-multi";

    let agent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "agent".to_string(),
        parent_span_id: None,
        span_name: "Agent".to_string(),
        observation_type: Some(ObservationType::Agent),
        timestamp_start: Utc::now(),
        ..Default::default()
    };

    let botocore1 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-1".to_string(),
        parent_span_id: Some("agent".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 500,
        gen_ai_usage_output_tokens: 100,
        gen_ai_cost_total: 0.003,
        ..Default::default()
    };

    let botocore2 = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-2".to_string(),
        parent_span_id: Some("agent".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 800,
        gen_ai_usage_output_tokens: 200,
        gen_ai_cost_total: 0.005,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[agent, botocore1, botocore2]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    assert_eq!(trace.input_tokens, 1300, "Sum of both botocore input");
    assert_eq!(trace.output_tokens, 300, "Sum of both botocore output");
    assert_eq!(trace.total_cost, 0.008, "Sum of both botocore cost");
}

/// Regression: LangGraph pattern where botocore(tokens) -> parent generation(tokens)
/// should NOT double-count. Parent has tokens so child is excluded.
#[tokio::test]
async fn test_langgraph_no_double_count_with_token_parent() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-langgraph-dedup";

    let generation = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "generation".to_string(),
        parent_span_id: None,
        span_name: "ChatOpenAI".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1100,
        gen_ai_usage_output_tokens: 200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    // Child has same tokens (duplicated) — should be excluded by NOT EXISTS
    let botocore = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore".to_string(),
        parent_span_id: Some("generation".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1100,
        gen_ai_usage_output_tokens: 200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[generation, botocore]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    assert_eq!(trace.input_tokens, 1100, "Only parent counted");
    assert_eq!(trace.output_tokens, 200, "Only parent counted");
    assert_eq!(trace.total_cost, 0.01, "Only parent cost");
}

/// Regression: full Strands hierarchy where Agent has aggregated tokens.
/// Agent(tokens=sum) → execute_event_loop_cycle(0) → Generation(tokens) → Botocore(tokens)
/// Only Generation should be counted (Path 1). Agent is excluded (Path 2 fails:
/// generations exist in trace). Botocore excluded (Path 2 fails: generations exist).
/// Cycle excluded (0 tokens).
#[tokio::test]
async fn test_strands_full_hierarchy_agent_with_aggregated_tokens() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-strands-full";

    // Agent span: root, has aggregated tokens (sum of all generations)
    let agent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "agent".to_string(),
        parent_span_id: None,
        span_name: "Agent".to_string(),
        observation_type: Some(ObservationType::Agent),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 2000, // sum of gen1 + gen2
        gen_ai_usage_output_tokens: 400,
        gen_ai_usage_total_tokens: 2400,
        gen_ai_cost_total: 0.02,
        ..Default::default()
    };

    // Intermediate cycle span: 0 tokens
    let cycle = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "cycle-1".to_string(),
        parent_span_id: Some("agent".to_string()),
        span_name: "execute_event_loop_cycle".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        ..Default::default()
    };

    // Generation span (chat): has tokens
    let generation = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "gen-1".to_string(),
        parent_span_id: Some("cycle-1".to_string()),
        span_name: "chat".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 200,
        gen_ai_usage_total_tokens: 1200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    // Botocore child of generation: has tokens but is not a generation type
    let botocore = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "botocore-1".to_string(),
        parent_span_id: Some("gen-1".to_string()),
        span_name: "Bedrock Runtime.Converse".to_string(),
        observation_type: Some(ObservationType::Span),
        timestamp_start: Utc::now(),
        gen_ai_usage_input_tokens: 1000,
        gen_ai_usage_output_tokens: 200,
        gen_ai_usage_total_tokens: 1200,
        gen_ai_cost_total: 0.01,
        ..Default::default()
    };

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[agent, cycle, generation, botocore]).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Only the Generation span should be counted (Path 1):
    // - Agent: Path 2 fails (generations exist in trace)
    // - Cycle: 0 tokens, excluded
    // - Generation: Path 1 succeeds (parent cycle is not a generation)
    // - Botocore: Path 2 fails (generations exist in trace)
    assert_eq!(
        trace.input_tokens, 1000,
        "Only generation's tokens, not agent's aggregated"
    );
    assert_eq!(trace.output_tokens, 200, "Only generation's output");
    assert_eq!(trace.total_cost, 0.01, "Only generation's cost");
}
