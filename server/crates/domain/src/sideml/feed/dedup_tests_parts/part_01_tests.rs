use super::*;
use crate::sideml::types::FinishReason;
use chrono::TimeZone;
use sideseat_ports::types::MessageCategory;

fn make_test_block(
    trace_id: &str,
    span_id: &str,
    role: ChatRole,
    text: &str,
    timestamp: DateTime<Utc>,
) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: text.to_string(),
        },
        role,
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec![span_id.to_string()],
        timestamp,
        order_time: timestamp,
        occurrence_ordinal: 0,
        observation_type: None,
        model: None,
        provider: None,
        name: None,
        finish_reason: None,
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: None,
        source_attribute: None,
        category: MessageCategory::GenAIUserMessage,
        content_hash: "test".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
    }
}

fn make_tool_use_block(
    trace_id: &str,
    span_id: &str,
    call_id: &str,
    name: &str,
    timestamp: DateTime<Utc>,
) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "tool_use".to_string(),
        content: ContentBlock::ToolUse {
            id: Some(call_id.to_string()),
            name: name.to_string(),
            input: serde_json::json!({}),
        },
        role: ChatRole::Assistant,
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec![span_id.to_string()],
        timestamp,
        order_time: timestamp,
        occurrence_ordinal: 0,
        observation_type: None,
        model: None,
        provider: None,
        name: None,
        finish_reason: Some(FinishReason::ToolUse),
        tool_use_id: Some(call_id.to_string()),
        tool_name: Some(name.to_string()),
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: None,
        source_attribute: None,
        category: MessageCategory::GenAIChoice,
        content_hash: "test".to_string(),
        is_semantic: true,
        // ToolUse uses event_time (not span_end) - the decision to call a tool
        // happens DURING generation, not at completion. See classify::uses_span_end().
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
    }
}

fn make_tool_result_block(
    trace_id: &str,
    span_id: &str,
    tool_use_id: &str,
    content: &str,
    timestamp: DateTime<Utc>,
) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "tool_result".to_string(),
        content: ContentBlock::ToolResult {
            tool_use_id: Some(tool_use_id.to_string()),
            name: None,
            content: serde_json::json!(content),
            is_error: false,
        },
        role: ChatRole::Tool,
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec![span_id.to_string()],
        timestamp,
        order_time: timestamp,
        occurrence_ordinal: 0,
        observation_type: None,
        model: None,
        provider: None,
        name: None,
        finish_reason: None,
        tool_use_id: Some(tool_use_id.to_string()),
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: None,
        source_attribute: None,
        category: MessageCategory::GenAIToolMessage,
        content_hash: "test".to_string(),
        is_semantic: true,
        uses_span_end: false, // Tool results are INPUT
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
    }
}

/// A framework re-sending a result without the provider's id is the same result.
///
/// Identity keyed on "the id when present, else the content" made these two copies two identities,
/// so nothing collapsed them: ADK showed each forecast twice, once with an id and once without,
/// suppressed only by the order-sensitive cross-trace prefix scan.
#[test]
fn an_idless_result_is_the_same_result_as_its_id_bearing_copy() {
    let span_timestamps = HashMap::new();
    let with_id = make_tool_result_block("t1", "s1", "call_1", "Sunny, 22C", utc(100));
    let mut without_id = make_tool_result_block("t1", "s1", "call_1", "Sunny, 22C", utc(200));
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut without_id.content {
        *tool_use_id = None;
    }
    without_id.tool_use_id = None;

    let result = process_dedup(vec![with_id, without_id], span_timestamps);
    assert_eq!(
        result.len(),
        1,
        "an id-less re-send of a result is that result, not a second one: {:?}",
        result.iter().map(|b| &b.content).collect::<Vec<_>>()
    );
}

/// Content is only evidence where it names *one* result. Three calls that return identical bytes
/// are three results - a provider issues one id per result, so two ids are two results whatever
/// their content, and the symmetric closure over content collapsed all three of
/// `agent-framework/image_gen`'s generated images into one.
#[test]
fn identical_content_under_distinct_ids_stays_distinct() {
    let span_timestamps = HashMap::new();
    let blocks = vec![
        make_tool_result_block("t1", "s1", "call_a", "same bytes", utc(100)),
        make_tool_result_block("t1", "s1", "call_b", "same bytes", utc(101)),
        make_tool_result_block("t1", "s1", "call_c", "same bytes", utc(102)),
    ];
    let result = process_dedup(blocks, span_timestamps);
    assert_eq!(
        result.len(),
        3,
        "three ids are three results, however identical their content"
    );
}

/// And an id-less copy whose content matches several id-bearing results names none of them, so it
/// is left alone rather than attached to an arbitrary one.
#[test]
fn an_idless_result_matching_several_is_left_alone() {
    let span_timestamps = HashMap::new();
    let mut idless = make_tool_result_block("t1", "s1", "call_x", "same bytes", utc(103));
    if let ContentBlock::ToolResult { tool_use_id, .. } = &mut idless.content {
        *tool_use_id = None;
    }
    idless.tool_use_id = None;
    let blocks = vec![
        make_tool_result_block("t1", "s1", "call_a", "same bytes", utc(100)),
        make_tool_result_block("t1", "s1", "call_b", "same bytes", utc(101)),
        idless,
    ];
    let result = process_dedup(blocks, span_timestamps);
    assert_eq!(
        result.len(),
        3,
        "ambiguous content must not be attached to an arbitrary call"
    );
}

fn utc(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).unwrap()
}

/// Tool payloads keep the empty/absent distinction: "no matches" is an answer.
#[test]
fn tool_identity_distinguishes_an_empty_collection_from_a_missing_one() {
    use serde_json::json;

    assert_ne!(
        normalize_json_for_hash(&json!({})),
        normalize_json_for_hash(&json!({"results": []})),
        "a tool result saying \"no matches\" is not the same as one saying nothing"
    );
    assert_ne!(
        normalize_json_for_hash(&json!({"q": "x"})),
        normalize_json_for_hash(&json!({"q": "x", "filters": []})),
        "two tool calls whose arguments differ must stay distinct"
    );
    // The structured normalizer is the one that treats them as one, and only for a
    // schema-shaped answer.
    assert_eq!(
        normalize_structured_json_for_hash(&json!({})),
        normalize_structured_json_for_hash(&json!({"results": []})),
    );
}

/// An executed call reports the optional parameters its framework injected as `null`.
#[test]
fn a_null_argument_is_the_argument_left_out() {
    use serde_json::json;
    let call = |input: &serde_json::Value| compute_tool_call_hash("get_weather", input);

    let made = json!({"city": "Barcelona", "days": 2});
    assert_eq!(
        call(&made),
        call(&json!({"instructions_override": null, "city": "Barcelona", "days": 2})),
    );
    assert_ne!(
        call(&made),
        call(&json!({"city": "Barcelona", "days": 2, "filters": []})),
        "an empty collection is still an argument"
    );
    assert_ne!(
        call(&json!({"place": {"city": "Barcelona"}})),
        call(&json!({"place": {"city": "Barcelona", "region": null}})),
        "a null inside an argument's value is part of that value"
    );
}

#[test]
fn identity_ignores_schema_filled_empty_fields() {
    use serde_json::json;

    let normalize_json_for_hash = normalize_structured_json_for_hash;

    // The Vercel case: the outer span reports the schema-shaped object, the inner span the
    // model's raw one. Same answer, and the only difference is a field the model left out.
    let raw = json!({"name": "Jane", "age": 28, "skills": ["admin"]});
    let schema_filled = json!({
        "name": "Jane", "age": 28, "skills": ["admin"],
        "contacts": [], "notes": null, "meta": {}, "title": "  "
    });
    assert_eq!(
        normalize_json_for_hash(&raw),
        normalize_json_for_hash(&schema_filled),
        "a field with no value must not make the same answer look like two"
    );

    // Nested, and an array whose members are all empty.
    assert_eq!(
        normalize_json_for_hash(&json!({"a": {"b": 1}})),
        normalize_json_for_hash(&json!({"a": {"b": 1, "c": []}, "d": [{}, null]})),
    );

    // A populated field still distinguishes. Notably `false` and `0` are values, not blanks.
    for populated in [
        json!({"name": "Jane", "age": 28, "skills": ["admin"], "contacts": ["x"]}),
        json!({"name": "Jane", "age": 29, "skills": ["admin"]}),
        json!({"name": "Jane", "age": 28, "skills": ["admin"], "active": false}),
        json!({"name": "Jane", "age": 28, "skills": ["admin"], "count": 0}),
    ] {
        assert_ne!(
            normalize_json_for_hash(&raw),
            normalize_json_for_hash(&populated),
            "{populated} carries a value and must stay distinct"
        );
    }
}

// ========================================================================
// BIRTH TIME TESTS
// ========================================================================

#[test]
fn test_birth_time_uses_earliest_occurrence() {
    // Same content appears at T=0 and T=5, birth_time should be T=0
    let t0 = utc(0);
    let t5 = utc(5);

    let block1 = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    let block2 = make_test_block("trace1", "span2", ChatRole::User, "Hello", t5);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t5,
                span_end: Some(t5),
            },
        ),
    ]);

    let birth_map = build_birth_times(&[block1.clone(), block2], &[0, 0], &span_timestamps);

    // Both should have birth_time = T=0
    let birth1 = get_birth_time(&block1, 0, &birth_map, &span_timestamps);
    assert_eq!(birth1, t0);
}

#[test]
fn test_output_uses_effective_timestamp() {
    let t0 = utc(0);
    let t5 = utc(5);

    let mut block = make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t0);
    block.finish_reason = Some(FinishReason::Stop);
    block.uses_span_end = true; // Mark as OUTPUT for effective timestamp calculation

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t5),
        },
    )]);

    // OUTPUT blocks use span_end for effective timestamp
    let effective = effective_timestamp(&block, &span_timestamps);
    assert_eq!(effective, t5);
}

#[test]
fn test_timestamp_materialized_for_output_blocks() {
    // After process_dedup, output blocks should have their timestamp updated
    // to span_end (not the raw event time which equals span start for attributes).
    let t_start = utc(0);
    let t_end = utc(10);

    let mut block = make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t_start);
    block.finish_reason = Some(FinishReason::Stop);
    block.uses_span_end = true;
    block.source_type = "attribute".to_string();

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t_start,
            span_end: Some(t_end),
        },
    )]);

    // Before: timestamp is span start (raw attribute time)
    assert_eq!(block.timestamp, t_start);

    let result = process_dedup(vec![block], span_timestamps);

    // After: timestamp materialized to span_end (effective/birth time)
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].timestamp, t_end,
        "Output block timestamp should be materialized to span_end, not raw event time"
    );
}

#[test]
fn test_tool_result_uses_own_birth_time() {
    // Tool results now use content-based identity and their own birth time
    let t0 = utc(0);
    let t5 = utc(5);

    let tool_use = make_tool_use_block("trace1", "span1", "call_123", "search", t0);
    let tool_result = make_tool_result_block("trace1", "span2", "call_123", "result", t5);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t5,
                span_end: Some(t5),
            },
        ),
    ]);

    let birth_map = build_birth_times(&[tool_use, tool_result.clone()], &[0, 0], &span_timestamps);

    // Tool result uses its own birth time (content-based)
    let birth = get_birth_time(&tool_result, 0, &birth_map, &span_timestamps);
    assert_eq!(birth, t5);
}

// ========================================================================
// DEDUPLICATION TESTS
// ========================================================================

#[test]
fn test_history_collapsed_to_first_occurrence() {
    let t0 = utc(0);
    let t5 = utc(5);

    let original = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    let history = make_test_block("trace1", "span2", ChatRole::User, "Hello", t5);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t5,
                span_end: Some(t5),
            },
        ),
    ]);

    let result = process_dedup(vec![original, history], span_timestamps);

    // Should dedupe to single message
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].span_id, "span1"); // Keep original
}

#[test]
fn test_same_content_different_traces_both_kept() {
    let t0 = utc(0);

    let block1 = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    let block2 = make_test_block("trace2", "span2", ChatRole::User, "Hello", t0);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![block1, block2], span_timestamps);

    // Different traces = different identities = both kept
    assert_eq!(result.len(), 2);
}

#[test]
fn test_enriched_version_preferred() {
    let t0 = utc(0);

    // Plain text block
    let mut plain = make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t0);
    plain.source_type = "attribute".to_string();

    // Same content but from event source (higher quality)
    let mut enriched = make_test_block("trace1", "span2", ChatRole::Assistant, "Response", t0);
    enriched.finish_reason = Some(FinishReason::Stop);
    enriched.source_type = "event".to_string();

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![plain, enriched], span_timestamps);

    // Should keep enriched version (has finish_reason and event source)
    assert_eq!(result.len(), 1);
    assert!(result[0].finish_reason.is_some());
}

// ========================================================================
// ORDERING TESTS
// ========================================================================

#[test]
fn test_tool_use_before_tool_result() {
    let t0 = utc(0);

    let mut tool_use = make_tool_use_block("trace1", "span1", "call_123", "search", t0);
    tool_use.message_index = 0;

    let mut tool_result = make_tool_result_block("trace1", "span2", "call_123", "result", t0);
    tool_result.message_index = 1; // Comes after tool_use

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    // Process in reverse order
    let result = process_dedup(vec![tool_result, tool_use], span_timestamps);

    // ToolUse should come before Tool result (by message_index)
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].entry_type, "tool_use");
    assert_eq!(result[1].entry_type, "tool_result");
}

#[test]
fn test_same_batch_ordering_text_before_tool_use() {
    // When text and tool_use are from the same span with same timestamp,
    // they should preserve message_index order regardless of uses_span_end
    let t0 = utc(0);
    let t_end = utc(1);

    // Text with finish_reason (uses_span_end=true, effective_time=t_end)
    let mut text = make_test_block("trace1", "span1", ChatRole::Assistant, "I'll search", t0);
    text.message_index = 0;
    text.finish_reason = Some(FinishReason::ToolUse);
    text.uses_span_end = true; // Would use span_end for birth time

    // ToolUse (uses_span_end=false, effective_time=t0)
    let mut tool_use = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
    tool_use.message_index = 1;
    tool_use.uses_span_end = false; // Uses event_time for birth time

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t_end),
        },
    )]);

    // Even though text uses span_end (t_end=1) and tool_use uses event_time (t0=0),
    // they're from the same batch (same span, same timestamp) so text should come first
    let result = process_dedup(vec![tool_use, text], span_timestamps);

    assert_eq!(result.len(), 2);
    // Text (message_index=0) should come before tool_use (message_index=1)
    assert_eq!(result[0].entry_type, "text");
    assert_eq!(result[1].entry_type, "tool_use");
}

#[test]
fn test_conversation_order() {
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);
    let t3 = utc(3);

    let user_msg = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

    let mut assistant_msg =
        make_test_block("trace1", "span1", ChatRole::Assistant, "Hi there!", t1);
    assistant_msg.finish_reason = Some(FinishReason::Stop);

    let user_msg2 = make_test_block("trace1", "span2", ChatRole::User, "Search for X", t2);

    let tool_use = make_tool_use_block("trace1", "span2", "call_123", "search", t3);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t1),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t2,
                span_end: Some(t3),
            },
        ),
    ]);

    // Process in random order
    let result = process_dedup(
        vec![tool_use, user_msg, user_msg2, assistant_msg],
        span_timestamps,
    );

    // Should be in conversation order
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, ChatRole::User);
    assert!(matches!(
        result[0].content,
        ContentBlock::Text { ref text } if text == "Hello"
    ));
    assert_eq!(result[1].role, ChatRole::Assistant);
    assert_eq!(result[2].role, ChatRole::User);
    assert!(matches!(
        result[2].content,
        ContentBlock::Text { ref text } if text == "Search for X"
    ));
    assert_eq!(result[3].entry_type, "tool_use");
}

// ========================================================================
// IDENTITY TESTS
// ========================================================================

#[test]
fn test_identity_tool_call() {
    let t0 = utc(0);
    let block = make_tool_use_block("trace1", "span1", "call_123", "search", t0);

    let identity = MessageIdentity::from_block(&block);
    assert!(matches!(
        identity,
        MessageIdentity::ToolCall {
            trace_id,
            content_hash: _,
        } if trace_id == "trace1"
    ));
}

#[test]
fn test_identity_tool_result() {
    let t0 = utc(0);
    let block = make_tool_result_block("trace1", "span1", "call_123", "result", t0);

    let identity = MessageIdentity::from_block(&block);
    assert!(matches!(
        identity,
        MessageIdentity::ToolResult {
            trace_id,
            identity_hash: _,
        } if trace_id == "trace1"
    ));
}

#[test]
fn two_executions_of_one_shape_on_their_own_spans_are_two_calls() {
    // Two calls of the same shape, each on its own span, each with the provider's own id, are two
    // **executions** - not one call seen twice.
    //
    // This test previously asserted the opposite, on the grounds that a history re-send regenerates
    // ids. That reasoning is sound for a re-send and wrong here, and the corpus settled it:
    // `agent-framework/tool_use` really does call `temperature_forecast{New York City, 3}` twice with
    // two ids, and the trace showed **one** call beside **two** answers. Same shape in
    // `openai-agents/tool_use`, both `mcp_tools` suites and both `subagents` suites - six fixtures.
    //
    // What separates the two cases is how many calls of the shape *one response* lists: two
    // executions each list it once in their own emission, while a re-sent pair lists it twice in one
    // emission, where position is the evidence and the ids are not.
    // `a_resent_pair_of_identical_calls_is_still_one_pair` holds that other side.
    let t0 = utc(0);

    // An execution's own report, not a re-send: the trace-wide id rank only trusts an id from an
    // emission carrier, which is how a genuine second execution and a snapshot's regenerated echo
    // are told apart (`a_resent_single_call_with_a_regenerated_id_is_still_one_call` holds the
    // other side).
    let mut tool1 = make_tool_use_block("trace1", "span1", "call_111", "search", t0);
    tool1.event_name = Some("gen_ai.choice".to_string());
    tool1.observation_type = Some("generation".to_string());
    let mut tool2 = make_tool_use_block("trace1", "span2", "call_222", "search", t0);
    tool2.event_name = Some("gen_ai.choice".to_string());
    tool2.observation_type = Some("generation".to_string());

    // The *identity* is still content-based: the id decides the repeat rank, never the identity,
    // because a re-send may regenerate it.
    assert_eq!(
        MessageIdentity::from_block(&tool1),
        MessageIdentity::from_block(&tool2)
    );

    // And should be deduplicated
    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![tool1, tool2], span_timestamps);
    assert_eq!(
        result.len(),
        2,
        "two executions with two provider ids must both survive"
    );
}

#[test]
fn a_reused_provider_id_still_distinguishes_sequential_executions() {
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);
    let t3 = utc(3);
    let mut call1 = make_tool_use_block("trace1", "generation1", "reused", "search", t0);
    call1.event_name = Some("gen_ai.choice".to_string());
    call1.observation_type = Some("generation".to_string());
    let result1 = make_tool_result_block("trace1", "tool1", "reused", "same result", t1);
    let mut call2 = make_tool_use_block("trace1", "generation2", "reused", "search", t2);
    call2.event_name = Some("gen_ai.choice".to_string());
    call2.observation_type = Some("generation".to_string());
    let result2 = make_tool_result_block("trace1", "tool2", "reused", "same result", t3);

    let blocks = vec![call1, result1, call2, result2];
    assert_eq!(
        call_repeat_ordinals(&blocks),
        vec![0, 0, 1, 1],
        "a response occurrence, not global id uniqueness, proves the second execution"
    );

    let timestamps = blocks
        .iter()
        .map(|block| {
            (
                block.span_id.clone(),
                SpanTimestamps {
                    span_start: block.timestamp,
                    span_end: Some(block.timestamp),
                },
            )
        })
        .collect();
    let result = process_dedup(blocks, timestamps);
    assert_eq!(result.iter().filter(|block| block.is_tool_use()).count(), 2);
    assert_eq!(
        result.iter().filter(|block| block.is_tool_result()).count(),
        2
    );
}

#[test]
fn replay_before_an_identical_call_keeps_the_new_execution_distinct() {
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);
    let mut old_call = make_tool_use_block("trace1", "generation2", "reused", "search", t0);
    old_call.source_type = "attribute".to_string();
    old_call.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    old_call.observation_type = Some("generation".to_string());
    let mut old_result =
        make_tool_result_block("trace1", "generation2", "reused", "same result", t0);
    old_result.source_type = "attribute".to_string();
    old_result.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    old_result.observation_type = Some("generation".to_string());
    let mut new_call = make_tool_use_block("trace1", "generation2", "reused", "search", t1);
    new_call.source_type = "attribute".to_string();
    new_call.source_attribute = Some("gcp.vertex.agent.llm_response".to_string());
    new_call.observation_type = Some("generation".to_string());
    let mut new_result =
        make_tool_result_block("trace1", "generation3", "reused", "same result", t2);
    new_result.source_type = "attribute".to_string();
    new_result.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    new_result.observation_type = Some("generation".to_string());

    assert_eq!(
        call_repeat_ordinals(&[old_call, old_result, new_call, new_result]),
        vec![0, 0, 1, 1],
        "history before an output is a previous occurrence, not a copy of the new call"
    );

    let mut prior_result =
        make_tool_result_block("trace2", "generation2", "reused", "same result", t0);
    prior_result.source_type = "attribute".to_string();
    prior_result.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    prior_result.observation_type = Some("generation".to_string());
    let mut next_call = make_tool_use_block("trace2", "generation2", "reused", "search", t1);
    next_call.source_type = "attribute".to_string();
    next_call.source_attribute = Some("gcp.vertex.agent.llm_response".to_string());
    next_call.observation_type = Some("generation".to_string());
    let mut next_result =
        make_tool_result_block("trace2", "generation3", "reused", "same result", t2);
    next_result.source_type = "attribute".to_string();
    next_result.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    next_result.observation_type = Some("generation".to_string());
    assert_eq!(
        call_repeat_ordinals(&[prior_result, next_call, next_result]),
        vec![0, 1, 1],
        "a result before the next call proves that it belongs to an earlier execution"
    );
}

#[test]
fn test_identity_regular() {
    let t0 = utc(0);
    let block = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

    let identity = MessageIdentity::from_block(&block);
    assert!(matches!(
        identity,
        MessageIdentity::Regular {
            trace_id,
            role: ChatRole::User,
            ..
        } if trace_id == "trace1"
    ));
}

// ========================================================================
// EDGE CASE TESTS
// ========================================================================

#[test]
fn test_empty_blocks() {
    let result = process_dedup(vec![], HashMap::new());
    assert!(result.is_empty());
}

#[test]
fn test_single_block() {
    let t0 = utc(0);
    let block = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    let result = process_dedup(vec![block], span_timestamps);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_tool_result_same_id_different_content_deduped() {
    // Same tool_use_id → same identity, regardless of content format.
    // Covers Vercel AI SDK toModelOutput: raw execute() output vs transformed format.
    let t0 = utc(0);
    let t1 = utc(1);

    // Raw tool result from tool span
    let mut raw = make_tool_result_block("trace1", "tool_span", "call_123", "raw result data", t0);
    raw.observation_type = Some("tool".to_string());

    // Transformed tool result from generation span (different content, same tool_use_id)
    let mut transformed = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: Some("call_123".to_string()),
            name: None,
            content: serde_json::json!({"type": "content", "value": [{"type": "text", "text": "transformed result"}]}),
            is_error: false,
        },
        span_id: "gen_span".to_string(),
        timestamp: t1,
        observation_type: Some("generation".to_string()),
        ..make_tool_result_block("trace1", "gen_span", "call_123", "", t1)
    };
    transformed.model = Some("claude-haiku".to_string());

    let span_timestamps = HashMap::from([
        (
            "tool_span".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "gen_span".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let result = process_dedup(vec![raw, transformed], span_timestamps);

    // Same tool_use_id → deduped to 1 (tool_use_id is identity, not content)
    assert_eq!(result.len(), 1);
    // Tool span version wins via FROM_TOOL_SPAN quality bonus
    assert_eq!(result[0].span_id, "tool_span");
}

#[test]
fn test_tool_result_different_ids_not_deduped_by_tool_id() {
    // Different tool_use_ids should NOT be merged even if both are tool results
    let t0 = utc(0);

    let result1 = make_tool_result_block("trace1", "span1", "call_111", "result A", t0);
    let result2 = make_tool_result_block("trace1", "span2", "call_222", "result B", t0);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![result1, result2], span_timestamps);

    // Both should be kept (different tool_use_ids = different identities)
    assert_eq!(result.len(), 2);
}
