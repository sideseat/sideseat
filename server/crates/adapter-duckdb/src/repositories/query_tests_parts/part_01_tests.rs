use super::*;
use chrono::TimeZone;

// ============================================================================
// Integration tests for leaf generation span filtering (cost deduplication)
// ============================================================================

use crate::DuckdbService;
use crate::repositories::span::insert_batch;
use sideseat_core::storage::AppStorage;
use sideseat_ports::types::{NormalizedSpan, ObservationType};
use tempfile::TempDir;

async fn create_test_service() -> (TempDir, DuckdbService) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let duckdb_dir = temp_dir.path().join("duckdb");
    tokio::fs::create_dir_all(&duckdb_dir)
        .await
        .expect("Failed to create duckdb dir");
    let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
    let service = DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
        .await
        .expect("Failed to init analytics service");
    (temp_dir, service)
}

fn make_generation_span(
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    cost: f64,
    tokens: i64,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_name: format!("generation-{}", span_id),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        gen_ai_cost_total: cost,
        gen_ai_usage_total_tokens: tokens,
        gen_ai_usage_input_tokens: tokens / 2,
        gen_ai_usage_output_tokens: tokens / 2,
        ..Default::default()
    }
}

fn make_agent_span(
    project_id: &str,
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_name: format!("agent-{}", span_id),
        observation_type: Some(ObservationType::Agent),
        timestamp_start: Utc::now(),
        ..Default::default()
    }
}

#[tokio::test]
async fn typed_deletes_preserve_tenant_and_identity_boundaries() {
    let (_tmp, service) = create_test_service().await;
    let spans = [
        make_agent_span("project-a", "shared-trace", "span-1", None),
        make_agent_span("project-a", "shared-trace", "span-2", None),
        make_agent_span("project-a", "other-trace", "span-1", None),
        make_agent_span("project-b", "shared-trace", "span-1", None),
    ];

    let conn = service.conn();
    insert_batch(&conn, &spans).expect("insert fixture");

    assert_eq!(
        delete_spans(
            &conn,
            "project-a",
            &[("shared-trace".to_string(), "span-1".to_string())],
        )
        .expect("delete one span"),
        1
    );
    assert_eq!(
        delete_traces(&conn, "project-a", &["other-trace".to_string()]).expect("delete one trace"),
        1
    );

    let remaining = |project: &str, trace: &str, span: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM otel_spans \
                 WHERE project_id = ? AND trace_id = ? AND span_id = ?",
            duckdb::params![project, trace, span],
            |row| row.get(0),
        )
        .expect("count remaining identity")
    };
    assert_eq!(remaining("project-a", "shared-trace", "span-1"), 0);
    assert_eq!(remaining("project-a", "shared-trace", "span-2"), 1);
    assert_eq!(remaining("project-a", "other-trace", "span-1"), 0);
    assert_eq!(
        remaining("project-b", "shared-trace", "span-1"),
        1,
        "colliding identity in another tenant must survive"
    );
}

#[tokio::test]
async fn get_span_returns_the_latest_delivery() {
    let (_temp_dir, analytics) = create_test_service().await;
    let event_time = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp");
    let first_ingest = DateTime::<Utc>::from_timestamp(1_700_000_001, 0).expect("valid timestamp");
    let second_ingest = DateTime::<Utc>::from_timestamp(1_700_000_002, 0).expect("valid timestamp");

    let first = NormalizedSpan {
        project_id: Some("test-project".to_string()),
        trace_id: "trace-point-read".to_string(),
        span_id: "span-point-read".to_string(),
        span_name: "old delivery".to_string(),
        timestamp_start: event_time,
        ingested_at: Some(first_ingest),
        ..Default::default()
    };
    let second = NormalizedSpan {
        span_name: "winning delivery".to_string(),
        ingested_at: Some(second_ingest),
        ..first.clone()
    };
    let another = NormalizedSpan {
        span_id: "another-span".to_string(),
        timestamp_start: event_time + chrono::Duration::seconds(1),
        ..first.clone()
    };

    let conn = analytics.conn();
    insert_batch(&conn, &[first, second, another]).expect("insert deliveries");
    let row = get_span(&conn, "test-project", "trace-point-read", "span-point-read")
        .expect("point read")
        .expect("span exists");

    assert_eq!(row.span_name.as_deref(), Some("winning delivery"));
    assert_eq!(row.ingested_at, second_ingest);

    let trace_rows =
        get_spans_for_trace(&conn, "test-project", "trace-point-read", 10).expect("trace spans");
    assert_eq!(trace_rows.len(), 2, "one winning row per span identity");
    assert_eq!(trace_rows[0].span_name.as_deref(), Some("winning delivery"));
    assert_eq!(
        get_spans_for_trace(&conn, "test-project", "trace-point-read", 1)
            .expect("bounded trace spans")
            .len(),
        1,
        "the port limit must reach the database query"
    );
}

#[tokio::test]
async fn bulk_span_counts_use_the_latest_delivery() {
    let (_temp_dir, analytics) = create_test_service().await;
    let event_time = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp");
    let first_ingest = DateTime::<Utc>::from_timestamp(1_700_000_001, 0).expect("valid timestamp");
    let second_ingest = DateTime::<Utc>::from_timestamp(1_700_000_002, 0).expect("valid timestamp");
    let first = NormalizedSpan {
        project_id: Some("test-project".to_string()),
        trace_id: "trace-counts".to_string(),
        span_id: "span-counts".to_string(),
        span_name: "first".to_string(),
        timestamp_start: event_time,
        ingested_at: Some(first_ingest),
        event_count: 3,
        link_count: 2,
        ..Default::default()
    };
    let second = NormalizedSpan {
        span_name: "second".to_string(),
        ingested_at: Some(second_ingest),
        event_count: 1,
        link_count: 4,
        ..first.clone()
    };

    let conn = analytics.conn();
    insert_batch(&conn, &[first, second]).expect("insert revisions");
    let counts = get_span_counts_bulk(
        &conn,
        "test-project",
        &[("trace-counts".to_string(), "span-counts".to_string())],
    )
    .expect("counts");
    let counts = counts
        .get(&("trace-counts".to_string(), "span-counts".to_string()))
        .expect("requested identity");
    assert_eq!(counts.event_count, 1);
    assert_eq!(counts.link_count, 4);
}

/// Test nested generation spans (Strands pattern):
/// Agent -> Generation(parent) -> Generation(child)
/// Both generations have the same cost/tokens, but only the leaf should be counted.
#[tokio::test]
async fn test_get_trace_nested_generations_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-nested";

    // Strands pattern: agent -> parent_gen -> child_gen
    // Both generations have $0.01 cost, but we should only count $0.01 total
    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(
            project_id,
            trace_id,
            "parent-gen",
            Some("agent-1"),
            0.01,
            1000,
        ),
        make_generation_span(
            project_id,
            trace_id,
            "child-gen",
            Some("parent-gen"),
            0.01,
            1000,
        ),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Should count only the leaf generation (child-gen), not both
    assert_eq!(
        trace.total_cost, 0.01,
        "Should not double-count nested generations"
    );
    assert_eq!(trace.total_tokens, 1000, "Should not double-count tokens");
}

#[tokio::test]
async fn corrected_unbilled_child_reactivates_the_winning_parent() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-corrected-child";
    let first_ingest = Utc
        .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
        .single()
        .unwrap();
    let corrected_ingest = first_ingest + chrono::Duration::seconds(1);

    let mut parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
    parent.ingested_at = Some(first_ingest);
    let mut child = make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 100);
    child.ingested_at = Some(first_ingest);
    let corrected_child = NormalizedSpan {
        gen_ai_cost_total: 0.0,
        gen_ai_usage_input_tokens: 0,
        gen_ai_usage_output_tokens: 0,
        gen_ai_usage_total_tokens: 0,
        ingested_at: Some(corrected_ingest),
        ..child.clone()
    };

    let conn = analytics.conn();
    insert_batch(&conn, &[parent, child]).expect("insert billed parent and child");
    let before = get_trace(&conn, project_id, trace_id)
        .expect("query")
        .expect("trace");
    assert_eq!(
        before.total_tokens, 100,
        "the billed child suppresses its parent"
    );
    assert_eq!(before.total_cost, 0.10);

    insert_batch(&conn, &[corrected_child]).expect("insert corrected child delivery");
    let after = get_trace(&conn, project_id, trace_id)
        .expect("query")
        .expect("trace");
    assert_eq!(
        after.total_tokens, 400,
        "the losing billed delivery must not keep suppressing the parent"
    );
    assert_eq!(after.total_cost, 0.40);
}

#[tokio::test]
async fn cost_only_generation_child_suppresses_its_parent() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-cost-only-child";
    let parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
    let child = make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 0);

    let conn = analytics.conn();
    insert_batch(&conn, &[parent, child]).expect("insert fixture");
    let trace = get_trace(&conn, project_id, trace_id)
        .expect("query")
        .expect("trace");
    assert_eq!(trace.total_tokens, 0);
    assert_eq!(
        trace.total_cost, 0.10,
        "cost without tokens is still billed and suppresses the parent"
    );
}

#[tokio::test]
async fn deleting_a_billed_child_reactivates_its_parent() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-deleted-child";
    let parent = make_generation_span(project_id, trace_id, "parent", None, 0.40, 400);
    let child = make_generation_span(project_id, trace_id, "child", Some("parent"), 0.10, 100);

    let conn = analytics.conn();
    insert_batch(&conn, &[parent, child]).expect("insert fixture");
    assert_eq!(
        get_trace(&conn, project_id, trace_id)
            .expect("query")
            .expect("trace")
            .total_tokens,
        100
    );

    delete_spans(
        &conn,
        project_id,
        &[(trace_id.to_string(), "child".to_string())],
    )
    .expect("delete child");
    let trace = get_trace(&conn, project_id, trace_id)
        .expect("query")
        .expect("trace");
    assert_eq!(trace.total_tokens, 400);
    assert_eq!(trace.total_cost, 0.40);
    let child_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM otel_spans
                 WHERE project_id = ? AND trace_id = ? AND span_id = 'child'",
            duckdb::params![project_id, trace_id],
            |row| row.get(0),
        )
        .expect("count child rows");
    assert_eq!(child_rows, 0);
}

/// Test non-nested generation spans (LangGraph/CrewAI pattern):
/// Agent -> Generation1, Agent -> Generation2 (siblings)
/// Both generations should be counted since neither is a parent of the other.
#[tokio::test]
async fn test_get_trace_sibling_generations_both_counted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-siblings";

    // LangGraph pattern: agent -> gen1, agent -> gen2 (siblings, not nested)
    // Both should be counted: $0.01 + $0.02 = $0.03
    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
        make_generation_span(project_id, trace_id, "gen-2", Some("agent-1"), 0.02, 2000),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Should count both sibling generations
    assert_eq!(
        trace.total_cost, 0.03,
        "Should count all sibling generations"
    );
    assert_eq!(trace.total_tokens, 3000, "Should count all sibling tokens");
}

/// Test deeply nested generations (3 levels):
/// Agent -> Gen1 -> Gen2 -> Gen3
/// Only Gen1 (root generation) should be counted.
#[tokio::test]
async fn test_get_trace_deeply_nested_only_leaf_counted() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-deep";

    // 3-level nesting: only Gen3 (leaf) should be counted
    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(project_id, trace_id, "gen-1", Some("agent-1"), 0.01, 1000),
        make_generation_span(project_id, trace_id, "gen-2", Some("gen-1"), 0.01, 1000),
        make_generation_span(project_id, trace_id, "gen-3", Some("gen-2"), 0.01, 1000),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Only leaf generation (gen-3) should be counted
    assert_eq!(trace.total_cost, 0.01, "Should only count leaf generation");
    assert_eq!(
        trace.total_tokens, 1000,
        "Should only count leaf generation tokens"
    );
}

/// Test mixed pattern: some nested, some siblings
/// Agent -> NestedParent -> NestedChild (nested)
/// Agent -> Standalone (sibling to NestedParent)
/// Should count NestedChild + Standalone
#[tokio::test]
async fn test_get_trace_mixed_nested_and_siblings() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-mixed";

    let spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        // Nested pair: parent-gen -> child-gen
        make_generation_span(
            project_id,
            trace_id,
            "parent-gen",
            Some("agent-1"),
            0.01,
            1000,
        ),
        make_generation_span(
            project_id,
            trace_id,
            "child-gen",
            Some("parent-gen"),
            0.01,
            1000,
        ),
        // Standalone sibling
        make_generation_span(
            project_id,
            trace_id,
            "standalone-gen",
            Some("agent-1"),
            0.02,
            2000,
        ),
    ];

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Should count child-gen (0.01) + standalone-gen (0.02) = 0.03
    // NOT parent-gen (excluded because it's parent of child-gen)
    assert_eq!(trace.total_cost, 0.03, "Should count leaf + standalone");
    assert_eq!(
        trace.total_tokens, 3000,
        "Should count leaf + standalone tokens"
    );
}

/// Test session aggregation with nested generations
#[tokio::test]
async fn test_get_session_nested_generations_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let session_id = "session-1";
    let trace_id = "trace-session";

    // Create spans with session_id
    let mut spans = vec![
        make_agent_span(project_id, trace_id, "agent-1", None),
        make_generation_span(
            project_id,
            trace_id,
            "parent-gen",
            Some("agent-1"),
            0.01,
            1000,
        ),
        make_generation_span(
            project_id,
            trace_id,
            "child-gen",
            Some("parent-gen"),
            0.01,
            1000,
        ),
    ];
    for span in &mut spans {
        span.session_id = Some(session_id.to_string());
    }

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let result = get_session(&conn, project_id, session_id).expect("Query should succeed");
    let session = result.expect("Session should exist");

    // Session aggregation should also only count leaf generations
    assert_eq!(session.total_cost, 0.01, "Session should not double-count");
    assert_eq!(
        session.total_tokens, 1000,
        "Session should not double-count tokens"
    );
}

/// A trace whose only span is transport-tagged (the documented "without the SDK"
/// path: BotocoreInstrumentor sets rpc.system, so the span is deliberately classified
/// as a plain span) must still appear in the default GenAI-only listing. It used to be
/// filtered out entirely, so those users saw nothing even though tokens and costs were
/// aggregated correctly.
///
/// This exercises the `include_nongenai: false` branch, which no other test covered -
/// the reason the original break slipped past the suite.
#[tokio::test]
async fn test_list_traces_includes_genai_trace_without_observation_span() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    let mut transport_only =
        make_generation_span(project_id, "trace-nosdk", "sp-1", None, 0.02, 40);
    // What the transport instrumentation produces: plain span, but real GenAI data.
    transport_only.observation_type = Some(ObservationType::Span);
    transport_only.gen_ai_system = Some("aws.bedrock".to_string());
    transport_only.gen_ai_request_model = Some("global.anthropic.claude-haiku-4-5".to_string());

    // A trace with neither observations nor GenAI attributes must stay hidden.
    let mut plain = make_generation_span(project_id, "trace-plain", "sp-2", None, 0.0, 0);
    plain.observation_type = Some(ObservationType::Span);
    plain.gen_ai_usage_total_tokens = 0;
    plain.gen_ai_usage_input_tokens = 0;
    plain.gen_ai_usage_output_tokens = 0;
    plain.gen_ai_cost_total = 0.0;

    {
        let conn = analytics.conn();
        insert_batch(&conn, &[transport_only, plain]).expect("Insert should succeed");
    }

    let params = ListTracesParams {
        project_id: ProjectId::from(project_id),
        page: 0,
        limit: 100,
        include_nongenai: false,
        ..Default::default()
    };
    let conn = analytics.conn();
    let (traces, _total) = list_traces(&conn, &params).expect("Query should succeed");

    let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
    assert!(
        ids.contains(&"trace-nosdk"),
        "a GenAI trace without an observation span must be listed, got {ids:?}"
    );
    assert!(
        !ids.contains(&"trace-plain"),
        "a trace with no GenAI evidence must stay hidden, got {ids:?}"
    );
}

/// Test list_traces with nested generations
#[tokio::test]
async fn test_list_traces_nested_generations_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Create two traces: one with nested generations, one with siblings
    let mut spans = vec![
        // Trace 1: nested generations (should count $0.01)
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
        // Trace 2: sibling generations (should count $0.03)
        make_agent_span(project_id, "trace-2", "agent-2", None),
        make_generation_span(project_id, "trace-2", "gen-a", Some("agent-2"), 0.01, 1000),
        make_generation_span(project_id, "trace-2", "gen-b", Some("agent-2"), 0.02, 2000),
    ];

    // Set timestamps for ordering
    let base_ts = Utc::now();
    spans[0].timestamp_start = base_ts;
    spans[1].timestamp_start = base_ts;
    spans[2].timestamp_start = base_ts;
    spans[3].timestamp_start = base_ts + chrono::Duration::seconds(1);
    spans[4].timestamp_start = base_ts + chrono::Duration::seconds(1);
    spans[5].timestamp_start = base_ts + chrono::Duration::seconds(1);

    {
        let conn = analytics.conn();
        insert_batch(&conn, &spans).expect("Insert should succeed");
    }

    let conn = analytics.conn();
    let params = ListTracesParams {
        project_id: ProjectId::from(project_id),
        page: 0,
        limit: 100,
        include_nongenai: true, // Test spans don't have gen_ai_system set
        ..Default::default()
    };
    let (traces, _total) = list_traces(&conn, &params).expect("Query should succeed");

    assert_eq!(traces.len(), 2, "Should have 2 traces");

    // Find traces by ID
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

/// Test cache tokens with nested generations (Strands pattern)
/// Cache tokens only exist on parent spans, not leaf spans.
/// Cache tokens should be summed from ALL generations (not leaf-only).
#[tokio::test]
async fn test_get_trace_cache_tokens_from_generation_leaf() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";
    let trace_id = "trace-cache";

    // Strands pattern:
    // - Parent "chat" span (generation) has cache tokens
    // - Child "chat us.amazon..." span is Span type (not Generation) in real data
    let parent = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "parent-chat".to_string(),
        parent_span_id: None,
        span_name: "chat".to_string(),
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

    // Real Strands: child is Span type, excluded by Path 2 (trace has generation)
    let child = NormalizedSpan {
        project_id: Some(project_id.to_string()),
        trace_id: trace_id.to_string(),
        span_id: "child-bedrock".to_string(),
        parent_span_id: Some("parent-chat".to_string()),
        span_name: "chat us.amazon.nova".to_string(),
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
    let result = get_trace(&conn, project_id, trace_id).expect("Query should succeed");
    let trace = result.expect("Trace should exist");

    // Parent generation is leaf (no generation child) - all its data counted
    assert_eq!(trace.input_tokens, 100, "Input tokens from generation leaf");
    assert_eq!(
        trace.output_tokens, 50,
        "Output tokens from generation leaf"
    );
    assert_eq!(
        trace.cache_read_tokens, 100,
        "Cache read from generation leaf"
    );
    assert_eq!(
        trace.cache_write_tokens, 200,
        "Cache write from generation leaf"
    );
    assert_eq!(trace.total_cost, 0.01, "Cost from generation leaf");
}

/// Test list_sessions with nested generations
#[tokio::test]
async fn test_list_sessions_nested_generations_no_double_count() {
    let (_temp_dir, analytics) = create_test_service().await;
    let project_id = "test-project";

    // Session 1: nested generations (should count $0.01)
    let mut spans1 = vec![
        make_agent_span(project_id, "trace-s1", "agent-1", None),
        make_generation_span(
            project_id,
            "trace-s1",
            "parent-gen",
            Some("agent-1"),
            0.01,
            1000,
        ),
        make_generation_span(
            project_id,
            "trace-s1",
            "child-gen",
            Some("parent-gen"),
            0.01,
            1000,
        ),
    ];
    for span in &mut spans1 {
        span.session_id = Some("session-1".to_string());
    }

    // Session 2: sibling generations (should count $0.03)
    let mut spans2 = vec![
        make_agent_span(project_id, "trace-s2", "agent-2", None),
        make_generation_span(project_id, "trace-s2", "gen-a", Some("agent-2"), 0.01, 1000),
        make_generation_span(project_id, "trace-s2", "gen-b", Some("agent-2"), 0.02, 2000),
    ];
    for span in &mut spans2 {
        span.session_id = Some("session-2".to_string());
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
    let params = ListSessionsParams {
        project_id: ProjectId::from(project_id),
        page: 0,
        limit: 100,
        ..Default::default()
    };
    let (sessions, _total) = list_sessions(&conn, &params).expect("Query should succeed");

    assert_eq!(sessions.len(), 2, "Should have 2 sessions");

    let session1 = sessions
        .iter()
        .find(|s| s.session_id == "session-1")
        .expect("Session 1 should exist");
    let session2 = sessions
        .iter()
        .find(|s| s.session_id == "session-2")
        .expect("Session 2 should exist");

    // Session 1: nested - should count only leaf ($0.01)
    assert_eq!(
        session1.total_cost, 0.01,
        "Session 1 should not double-count"
    );
    assert_eq!(
        session1.total_tokens, 1000,
        "Session 1 tokens should not double-count"
    );

    // Session 2: siblings - should count both ($0.03)
    assert_eq!(
        session2.total_cost, 0.03,
        "Session 2 should count both siblings"
    );
    assert_eq!(
        session2.total_tokens, 3000,
        "Session 2 tokens should count both"
    );
}
