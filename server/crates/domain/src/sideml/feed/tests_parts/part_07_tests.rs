
/// Regression #56: User/System messages in non-root generation spans filtered.
///
/// In Strands-like traces with agent spans:
/// - Root agent span has authoritative current-turn messages
/// - Child generation spans receive full context (history + current) for LLM
///
/// User/system messages in child generation spans are history context copies
/// and should be filtered when agent spans exist.
#[test]
fn test_regression_generation_span_history_user_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Agent span (root) with current turn messages - use event-based format
    let agent_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Current question: What is 2+2?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The answer is 4", "finish_reason": "stop"}
        }
    ]);

    // Generation span with history context (copies of previous turns)
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History question from turn 1"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History question from turn 2"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "Current thinking"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("agent_span"),
            &gen_span_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count user messages
    let user_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();

    assert_eq!(
        user_messages.len(),
        1,
        "Should have exactly 1 user message (from root agent span), got {} user messages, {} total blocks",
        user_messages.len(),
        result.messages.len()
    );

    // Verify it's the current turn question
    assert!(
        matches!(&user_messages[0].content, ContentBlock::Text { text } if text.contains("Current question")),
        "Should be the current turn user message"
    );

    // Verify history user messages were filtered
    let has_history = result.messages.iter().any(
        |b| matches!(&b.content, ContentBlock::Text { text } if text.contains("History question")),
    );
    assert!(
        !has_history,
        "History user messages should be filtered from generation span"
    );
}

/// Regression #57: Multi-turn session with thinking - history filtered per trace.
///
/// Each trace (turn) has an agent span and generation span. History in
/// generation spans should be filtered, keeping only protected content.
#[test]
fn test_regression_multi_turn_thinking_session() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Turn 1: Simple question - no history
    let turn1_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 1", "finish_reason": "stop"}}
    ]);
    let turn1_gen = json!([
        {"source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}], "finish_reason": "stop"}}
    ]);

    // Turn 2: Has history from turn 1 in generation span
    let turn2_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t1.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}}, "content": {"role": "user", "content": "Question 2"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 2", "finish_reason": "stop"}}
    ]);
    let turn2_gen = json!([
        // History thinking (no finish_reason) - should be filtered
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}]}},
        // History user - should be filtered
        {"source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        // Current thinking (protected) - should be kept
        {"source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q2"}], "finish_reason": "stop"}}
    ]);

    // Turn 3: Has history from turns 1 and 2 in generation span
    let turn3_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t2.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 3"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 3", "finish_reason": "stop"}}
    ]);
    let turn3_gen = json!([
        // History thinking from Q1 and Q2 - should be filtered
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}]}},
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q2"}]}},
        // History users - should be filtered
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 2"}},
        // Current thinking (protected) - should be kept
        {"source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q3"}], "finish_reason": "stop"}}
    ]);

    let mut rows = vec![
        // Turn 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &turn1_agent.to_string(),
            t0,
            Some(t0 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &turn1_gen.to_string(),
            t0,
            Some(t0 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
        // Turn 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &turn2_agent.to_string(),
            t1,
            Some(t1 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &turn2_gen.to_string(),
            t1,
            Some(t1 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
        // Turn 3
        make_span_row_full(
            "trace3",
            "agent3",
            None,
            &turn3_agent.to_string(),
            t2,
            Some(t2 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace3",
            "gen3",
            Some("agent3"),
            &turn3_gen.to_string(),
            t2,
            Some(t2 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
    ];

    // Add session_id to group all traces as one conversation
    for row in &mut rows {
        row.session_id = Some("test-session".to_string());
    }

    let options = FeedOptions::default();
    let result = process_feed(rows, &options);

    // Should have 3 thinking blocks (one per turn)
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "thinking")
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        3,
        "Should have exactly 3 thinking blocks (one per turn), got {}",
        thinking_blocks.len()
    );

    // Should have 3 user messages (one per turn)
    let user_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();

    assert_eq!(
        user_messages.len(),
        3,
        "Should have exactly 3 user messages (one per turn), got {}",
        user_messages.len()
    );

    // Total should be 12 blocks: 3 * (system + user + thinking + response)
    assert_eq!(
        result.messages.len(),
        12,
        "Should have 12 total blocks (4 per turn), got {}",
        result.messages.len()
    );
}

/// Regression #58: Protected content never filtered regardless of timestamp.
///
/// Even if a message has timestamp < span_start, if it's protected
/// (gen_ai.choice, finish_reason), it should NOT be filtered.
#[test]
fn test_regression_protected_ignores_timestamp() {
    let t0 = fixed_time();
    let t_before = t0 - chrono::Duration::seconds(10);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Protected message with timestamp before span start
    // Should NOT be filtered because it's protected
    let gen_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_before.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Response from LLM",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen",
        Some("parent"),
        &gen_span_msg.to_string(),
        t0, // Span starts AFTER message timestamp
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Protected message should be preserved despite timestamp
    assert_eq!(
        result.messages.len(),
        1,
        "Protected message should be preserved even with timestamp < span_start"
    );
    assert!(
        result.messages[0].finish_reason.is_some(),
        "Should have finish_reason (protected)"
    );
}

// ----------------------------------------------------------------------------
// ADK Thinking Block Recognition
// ----------------------------------------------------------------------------
// ADK sends thinking content as {"text": "...", "thought": true} which should
// be normalized to ContentBlock::Thinking, not ContentBlock::Unknown.

#[test]
fn test_regression_adk_thinking_blocks_recognized() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t_end = t0 + chrono::Duration::seconds(5);

    // ADK-style messages with thought blocks using event-based format
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "user",
            "content": [{"type": "text", "text": "Solve this logic puzzle"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [
                {"text": "Let me think step by step...", "thought": true},
                {"type": "text", "text": "The answer is Alice=Green, Bob=Blue, Carol=Red."}
            ],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert!(
        result.messages.len() >= 3,
        "Expected user + thinking + assistant text, got {}",
        result.messages.len()
    );

    // Find the thinking block
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| matches!(m.content, ContentBlock::Thinking { .. }))
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        1,
        "Should have exactly one thinking block"
    );

    if let ContentBlock::Thinking { ref text, .. } = thinking_blocks[0].content {
        assert_eq!(text, "Let me think step by step...");
    } else {
        panic!("Expected Thinking content block");
    }

    // Verify no unknown blocks (the thought block should not be unknown)
    let unknown_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| matches!(m.content, ContentBlock::Unknown { .. }))
        .collect();

    assert_eq!(
        unknown_blocks.len(),
        0,
        "ADK thought blocks should not produce Unknown content blocks"
    );
}

#[test]
fn test_regression_tool_use_flow_complete() {
    // Verifies the basic tool_use -> tool_result -> assistant flow
    // across all framework patterns
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(5);

    let messages = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "user",
            "content": [{"type": "text", "text": "What is the weather in NYC?"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": (t0 + chrono::Duration::seconds(1)).to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "call_123",
                "name": "get_weather",
                "input": {"city": "NYC"}
            }],
            "finish_reason": "tool_use"
        }
    }, {
        "source": {"event": {"name": "gen_ai.tool.message", "time": (t0 + chrono::Duration::seconds(2)).to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "call_123",
            "content": [{"type": "tool_result", "tool_use_id": "call_123", "content": "Sunny, 75F"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": (t0 + chrono::Duration::seconds(3)).to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "The weather in NYC is sunny and 75F!"}],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Verify message count and order
    assert!(
        result.messages.len() >= 4,
        "Expected at least 4 messages (user, tool_use, tool_result, assistant), got {}",
        result.messages.len()
    );

    // Verify roles in order
    let roles: Vec<ChatRole> = result.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles[0], ChatRole::User, "First should be user");

    // Find tool_use and tool_result
    let has_tool_use = result
        .messages
        .iter()
        .any(|m| matches!(m.content, ContentBlock::ToolUse { .. }));
    let has_tool_result = result
        .messages
        .iter()
        .any(|m| matches!(m.content, ContentBlock::ToolResult { .. }));

    assert!(has_tool_use, "Should have tool_use block");
    assert!(has_tool_result, "Should have tool_result block");

    // Verify tool_use comes before tool_result
    let tool_use_idx = result
        .messages
        .iter()
        .position(|m| matches!(m.content, ContentBlock::ToolUse { .. }))
        .unwrap();
    let tool_result_idx = result
        .messages
        .iter()
        .position(|m| matches!(m.content, ContentBlock::ToolResult { .. }))
        .unwrap();
    assert!(
        tool_use_idx < tool_result_idx,
        "tool_use should come before tool_result"
    );

    // Verify last message is assistant text
    let last = result.messages.last().unwrap();
    assert_eq!(last.role, ChatRole::Assistant, "Last should be assistant");
    if let ContentBlock::Text { ref text } = last.content {
        assert!(text.contains("sunny"), "Should contain weather response");
    } else {
        panic!("Last message should be text");
    }

    // No duplicates
    let hashes: Vec<_> = result.messages.iter().map(|m| &m.content_hash).collect();
    let unique_hashes: std::collections::HashSet<_> = hashes.iter().collect();
    assert_eq!(
        hashes.len(),
        unique_hashes.len(),
        "Should have no duplicate content hashes"
    );
}

// ============================================================================
// ERROR MESSAGE TESTS
// ============================================================================

#[test]
fn test_error_messages_from_status_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Error span with exception_message -> should get error ParsedMessage
    let mut error_row = make_span_row_full(
        "t1",
        "error-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    error_row.status_code = Some("ERROR".to_string());
    error_row.exception_message = Some("Input is too long".into());

    // Normal span -> should NOT get error ParsedMessage
    let normal_row = make_span_row_full(
        "t1",
        "normal-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );

    let rows = vec![error_row, normal_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    // Should have exactly one error block
    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    assert_eq!(
        error_blocks[0].finish_reason,
        Some(super::super::types::FinishReason::Error)
    );
    assert!(
        !error_blocks[0].is_history,
        "Error blocks should not be marked as history"
    );
}

#[test]
fn test_error_messages_leaf_only() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root error span (has error child -> should NOT get error message)
    let mut root_row =
        make_span_row_full("t1", "root-span", None, "[]", t0, Some(t1), Some("agent"));
    root_row.status_code = Some("ERROR".to_string());
    root_row.exception_message = Some("Root error".to_string());

    // Leaf error span (no error children -> should get error message)
    let mut leaf_row = make_span_row_full(
        "t1",
        "leaf-span",
        Some("root-span"),
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    leaf_row.status_code = Some("ERROR".to_string());
    leaf_row.exception_message = Some("Leaf error".to_string());

    let rows = vec![root_row, leaf_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only leaf error should produce a block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert_eq!(text, "Leaf error"),
        _ => panic!("Expected Text content block"),
    }
}

#[test]
fn test_error_messages_no_status_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Error span without exception fields -> should NOT get error block
    let mut error_row = make_span_row_full(
        "t1",
        "error-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    error_row.status_code = Some("ERROR".to_string());
    // exception fields are all None

    let rows = vec![error_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    assert!(
        result.messages.is_empty(),
        "No error block when exception fields are empty"
    );
}

// ============================================================================
// COMPOSE ERROR TEXT TESTS
// ============================================================================

#[test]
fn test_compose_error_text_type_and_message() {
    let result = compose_error_text(Some("ValueError"), Some("bad input"), None);
    assert_eq!(result, Some("ValueError: bad input".to_string()));
}

#[test]
fn test_compose_error_text_message_only() {
    let result = compose_error_text(None, Some("bad input"), None);
    assert_eq!(result, Some("bad input".to_string()));
}

#[test]
fn test_compose_error_text_type_only() {
    let result = compose_error_text(Some("ValueError"), None, None);
    assert_eq!(result, Some("ValueError".to_string()));
}

#[test]
fn test_compose_error_text_all_none() {
    let result = compose_error_text(None, None, None);
    assert_eq!(result, None);
}

#[test]
fn test_compose_error_text_all_empty() {
    let result = compose_error_text(Some(""), Some(""), Some(""));
    assert_eq!(result, None);
}

#[test]
fn test_compose_error_text_stacktrace_only() {
    let result = compose_error_text(None, None, Some("at main.py:1"));
    assert_eq!(result, Some("```\nat main.py:1\n```".to_string()));
}

#[test]
fn test_compose_error_text_message_and_stacktrace() {
    let result = compose_error_text(None, Some("bad input"), Some("at main.py:1"));
    assert_eq!(
        result,
        Some("bad input\n\n```\nat main.py:1\n```".to_string())
    );
}

#[test]
fn test_compose_error_text_all_fields() {
    let result = compose_error_text(Some("ValueError"), Some("bad input"), Some("at main.py:1"));
    assert_eq!(
        result,
        Some("ValueError: bad input\n\n```\nat main.py:1\n```".to_string())
    );
}

// ============================================================================
// EXCEPTION FIELD REGRESSION TESTS
// ============================================================================

/// Regression: error span with only status_message (no exception fields)
/// should NOT produce an error block. OTEL SDKs propagate status_message
/// up the span tree — only exception events carry real error details.
#[test]
fn test_error_span_with_only_status_code_no_exception_fields() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("agent"));
    row.status_code = Some("ERROR".to_string());
    // No exception fields set — simulates SDK-propagated error status

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    assert!(
        result.messages.is_empty(),
        "No error block for status_code=ERROR without exception fields"
    );
}

/// Regression: exception_type + exception_message produce "Type: Message" header
#[test]
fn test_error_block_with_exception_type_and_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("generation"));
    row.status_code = Some("ERROR".to_string());
    row.exception_type = Some("ValueError".to_string());
    row.exception_message = Some("bad input".to_string());

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert_eq!(text, "ValueError: bad input"),
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: exception with stacktrace renders markdown code block
#[test]
fn test_error_block_with_stacktrace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("generation"));
    row.status_code = Some("ERROR".to_string());
    row.exception_type = Some("RuntimeError".to_string());
    row.exception_message = Some("crash".to_string());
    row.exception_stacktrace = Some("Traceback:\n  File main.py".to_string());

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    match &error_blocks[0].content {
        ContentBlock::Text { text } => {
            assert!(text.starts_with("RuntimeError: crash"));
            assert!(text.contains("```\nTraceback:\n  File main.py\n```"));
        }
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: parent error span with error child should NOT produce error block
/// (leaf detection) even when parent has exception fields.
#[test]
fn test_error_block_only_on_leaf_with_exception_fields() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut parent = make_span_row_full("t1", "parent", None, "[]", t0, Some(t1), Some("agent"));
    parent.status_code = Some("ERROR".to_string());
    parent.exception_type = Some("ValueError".to_string());
    parent.exception_message = Some("parent error".to_string());

    let mut child = make_span_row_full(
        "t1",
        "child",
        Some("parent"),
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    child.status_code = Some("ERROR".to_string());
    child.exception_type = Some("ValueError".to_string());
    child.exception_message = Some("child error".to_string());

    let rows = vec![parent, child];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only leaf error should produce block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert!(text.contains("child error")),
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: two independent error leaf spans (broken hierarchy) should
/// both produce error blocks when both have exception fields.
#[test]
fn test_error_blocks_for_independent_leaf_spans() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root agent span with exception event
    let mut root = make_span_row_full("t1", "root", None, "[]", t0, Some(t1), Some("agent"));
    root.status_code = Some("ERROR".to_string());
    root.exception_type = Some("RuntimeError".to_string());
    root.exception_message = Some("root exception".to_string());

    // Detached leaf span (parent not in trace) — only SDK-propagated status
    let mut detached = make_span_row_full(
        "t1",
        "detached",
        Some("missing-parent"),
        "[]",
        t0,
        Some(t1),
        Some("span"),
    );
    detached.status_code = Some("ERROR".to_string());
    // No exception fields — just propagated error status

    let rows = vec![root, detached];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only the span with exception fields should produce an error block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert!(text.contains("root exception")),
        _ => panic!("Expected Text content block"),
    }
}

// ============================================================================
// VERCEL AI SDK toModelOutput DEDUPLICATION
// ============================================================================
// Vercel AI SDK's toModelOutput transforms tool result content before re-sending
// to the model in the next generation span. The raw execute() output appears in
// the tool span, while the transformed format appears in the generation span's
// ai.prompt.messages. These have different content hashes but the same tool_use_id.
//
