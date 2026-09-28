#[test]
fn test_tool_result_no_tool_use_id_falls_back_to_content_hash() {
    // Without tool_use_id, identity uses content hash (existing behavior)
    let t0 = utc(0);

    let mut result1 = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: None,
            name: None,
            content: serde_json::json!("same content"),
            is_error: false,
        },
        ..make_tool_result_block("trace1", "span1", "", "unused", t0)
    };
    result1.tool_use_id = None;

    let mut result2 = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: None,
            name: None,
            content: serde_json::json!("same content"),
            is_error: false,
        },
        ..make_tool_result_block("trace1", "span2", "", "unused", t0)
    };
    result2.tool_use_id = None;

    // Same content, no tool_use_id → same identity via content hash
    let id1 = MessageIdentity::from_block(&result1);
    let id2 = MessageIdentity::from_block(&result2);
    assert_eq!(id1, id2, "Same content without tool_use_id should match");

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
    assert_eq!(
        result.len(),
        1,
        "Same content without tool_use_id should dedup"
    );
}

#[test]
fn test_tool_result_same_id_same_observation_type_deduped() {
    // Same tool_use_id from same observation type → still deduped.
    // tool_use_id is identity, observation type is irrelevant.
    let t0 = utc(0);
    let t1 = utc(1);

    let mut r1 = make_tool_result_block("trace1", "span1", "call_1", "First result", t0);
    r1.observation_type = Some("generation".to_string());

    let mut r2 = make_tool_result_block("trace1", "span2", "call_1", "Second result", t1);
    r2.observation_type = Some("generation".to_string());

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
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let result = process_dedup(vec![r1, r2], span_timestamps);

    // Same tool_use_id → same identity → deduped to 1
    assert_eq!(result.len(), 1);
}

#[test]
fn test_tool_result_without_matching_tool_use() {
    let t0 = utc(0);
    // Tool result with no matching tool_use in the data
    let tool_result = make_tool_result_block("trace1", "span1", "missing_call", "result", t0);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    // Should still work, using effective timestamp as fallback
    let result = process_dedup(vec![tool_result], span_timestamps);
    assert_eq!(result.len(), 1);
}

// ========================================================================
// ADVANCED DEDUPLICATION TESTS
// ========================================================================

#[test]
fn test_parallel_tool_calls_different_inputs_not_deduped() {
    // Multiple tool calls with DIFFERENT inputs should NOT be deduped
    // (even though they have the same tool name)
    let t0 = utc(0);

    // Create tool calls with same name but different inputs
    let mut tool1 = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
    tool1.content = ContentBlock::ToolUse {
        id: Some("call_1".to_string()),
        name: "search".to_string(),
        input: serde_json::json!({"query": "cats"}),
    };

    let mut tool2 = make_tool_use_block("trace1", "span1", "call_2", "search", t0);
    tool2.content = ContentBlock::ToolUse {
        id: Some("call_2".to_string()),
        name: "search".to_string(),
        input: serde_json::json!({"query": "dogs"}),
    };

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    let result = process_dedup(vec![tool1, tool2], span_timestamps);

    // Both should be kept (different inputs = different identities)
    assert_eq!(result.len(), 2);
}

#[test]
fn test_streaming_chunks_deduped() {
    // Same content appearing multiple times (streaming) should be deduped
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);

    let chunk1 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t0);
    let chunk2 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t1);
    let mut chunk3 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t2);
    chunk3.finish_reason = Some(FinishReason::Stop);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t2),
        },
    )]);

    let result = process_dedup(vec![chunk1, chunk2, chunk3], span_timestamps);

    // Should be deduped to single message (with finish_reason = highest quality)
    assert_eq!(result.len(), 1);
    assert!(result[0].finish_reason.is_some());
}

#[test]
fn test_different_roles_same_content_not_deduped() {
    // Same content but different roles should NOT be deduped
    let t0 = utc(0);

    let user_msg = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    let mut assistant_msg = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
    assistant_msg.finish_reason = Some(FinishReason::Stop);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    let result = process_dedup(vec![user_msg, assistant_msg], span_timestamps);

    // Both should be kept (different roles = different identities)
    assert_eq!(result.len(), 2);
}

#[test]
fn test_history_at_multiple_depths_deduped() {
    // User message appears at root, child, and grandchild spans
    // Should be deduped to the earliest occurrence
    let t0 = utc(0);
    let t5 = utc(5);
    let t10 = utc(10);

    let root_msg = make_test_block("trace1", "root", ChatRole::User, "Hello", t0);
    let child_msg = make_test_block("trace1", "child", ChatRole::User, "Hello", t5);
    let grandchild_msg = make_test_block("trace1", "grandchild", ChatRole::User, "Hello", t10);

    let span_timestamps = HashMap::from([
        (
            "root".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "child".to_string(),
            SpanTimestamps {
                span_start: t5,
                span_end: Some(t5),
            },
        ),
        (
            "grandchild".to_string(),
            SpanTimestamps {
                span_start: t10,
                span_end: Some(t10),
            },
        ),
    ]);

    let result = process_dedup(vec![root_msg, child_msg, grandchild_msg], span_timestamps);

    // Should be deduped to single message from root span
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].span_id, "root");
}

#[test]
fn test_full_tool_chain_ordering() {
    // Complete tool chain: User -> Assistant+ToolUse -> ToolResult -> Assistant
    // Each step in separate spans with proper timestamps
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);
    let t3 = utc(3);

    let user = make_test_block("trace1", "span_user", ChatRole::User, "Search for cats", t0);
    let tool_use = make_tool_use_block("trace1", "span_tool_use", "call_1", "search", t1);
    let tool_result =
        make_tool_result_block("trace1", "span_tool_result", "call_1", "Found cats", t2);

    let mut final_response = make_test_block(
        "trace1",
        "span_final",
        ChatRole::Assistant,
        "Here are the cats",
        t3,
    );
    final_response.finish_reason = Some(FinishReason::Stop);

    // Each span has its own timestamps - OUTPUT uses span_end
    let span_timestamps = HashMap::from([
        (
            "span_user".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span_tool_use".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1), // Tool use span ends at t1
            },
        ),
        (
            "span_tool_result".to_string(),
            SpanTimestamps {
                span_start: t2,
                span_end: Some(t2),
            },
        ),
        (
            "span_final".to_string(),
            SpanTimestamps {
                span_start: t3,
                span_end: Some(t3),
            },
        ),
    ]);

    // Process in random order
    let result = process_dedup(
        vec![final_response, tool_result, user, tool_use],
        span_timestamps,
    );

    // Should be in correct order: User -> ToolUse -> ToolResult -> Final response
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, ChatRole::User);
    assert_eq!(result[1].entry_type, "tool_use");
    assert_eq!(result[2].entry_type, "tool_result");
    assert_eq!(result[3].role, ChatRole::Assistant);
    assert!(matches!(
        result[3].content,
        ContentBlock::Text { ref text } if text == "Here are the cats"
    ));
}

#[test]
fn test_uses_span_end_field_on_test_helpers() {
    let t0 = utc(0);

    // User message uses event_time (uses_span_end = false)
    let user = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    assert!(!user.uses_span_end);

    // ToolUse uses event_time (uses_span_end = false) - the decision to call
    // a tool happens DURING generation, not at completion
    let tool_use = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
    assert!(!tool_use.uses_span_end);

    // ToolResult uses event_time (uses_span_end = false) unless from tool span
    let tool_result = make_tool_result_block("trace1", "span1", "call_1", "result", t0);
    assert!(!tool_result.uses_span_end);
}

#[test]
fn test_quality_scoring() {
    let t0 = utc(0);

    // Base block
    let base = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
    let base_quality = compute_quality(&base);

    // Block with finish_reason has higher quality
    let mut with_finish = base.clone();
    with_finish.finish_reason = Some(FinishReason::Stop);
    let with_finish_quality = compute_quality(&with_finish);
    assert!(with_finish_quality > base_quality);

    // Block with model info has higher quality
    let mut with_model = base.clone();
    with_model.model = Some("gpt-4".to_string());
    let with_model_quality = compute_quality(&with_model);
    assert!(with_model_quality > base_quality);

    // Event source has higher quality than attribute
    let mut from_event = base.clone();
    from_event.source_type = "event".to_string();
    let mut from_attribute = base;
    from_attribute.source_type = "attribute".to_string();
    assert!(compute_quality(&from_event) > compute_quality(&from_attribute));
}
