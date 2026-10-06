// ----------------------------------------------------------------------------
// Additional helper tests for specific edge cases
// ----------------------------------------------------------------------------

#[test]
fn test_tool_result_text_vs_array_normalization() {
    // Tool result content can be string, object, or array
    // All forms representing same data should produce same identity
    let t0 = fixed_time();

    // Form 1: plain string
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "Result text"}
    }]);

    // Form 2: array with text block
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": [{"type": "text", "text": "Result text"}]}
    }]);

    let row1 =
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0));
    let row2 = make_span_row_with_timestamps(
        "trace1",
        "span2",
        Some("span1"),
        &msg2.to_string(),
        t0,
        Some(t0),
    );

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2], &options);

    // Both forms should deduplicate to 1
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Tool)
        .collect();

    // This test documents current behavior - it may fail if normalization isn't implemented
    assert_eq!(
        tool_results.len(),
        1,
        "Tool results with same semantic content should deduplicate. Found {}:\n{:?}",
        tool_results.len(),
        tool_results
            .iter()
            .map(|m| format!("span={}, hash={}", m.span_id, m.content_hash))
            .collect::<Vec<_>>()
    );
}

// History detection for input events on non-generation spans.
// Tests for GenAI input events from non-generation spans being marked as history.
// This catches cross-trace session history that Strands includes in event loop spans.

/// Helper to create a span row with specific observation_type
fn make_span_row_with_observation_type(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    span_start: chrono::DateTime<Utc>,
    span_end: Option<chrono::DateTime<Utc>>,
    observation_type: &str,
) -> MessageSpanRow {
    let mut row = make_span_row_with_timestamps(
        trace_id,
        span_id,
        parent_span_id,
        messages_json,
        span_start,
        span_end,
    );
    row.observation_type = Some(observation_type.to_string());
    row
}

// ----------------------------------------------------------------------------
// ISSUE 29: Session History in Event Loop Spans
// ----------------------------------------------------------------------------
// Strands includes previous session turns in event loop spans (observation_type="span").
// These should be filtered as history.

#[test]
fn test_regression_session_history_in_event_loop_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Root agent span with current request
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in London?"}
        }
    ]);

    // Event loop span with session history (previous NYC request)
    // This is what Strands does - accumulates all previous turns
    let event_loop_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in NYC?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC is sunny today."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in London?"}
        }
    ]);

    // Generation span with actual LLM output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {"role": "assistant", "content": "London is rainy today.", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t2),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t2),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("event_loop"),
            &gen_msg.to_string(),
            t1,
            Some(t2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain NYC messages (session history)
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("NYC")),
        "Session history (NYC) should be filtered. Found: {:?}",
        texts
    );

    // Should only have: system, user (London), assistant (London response)
    assert_eq!(
        result.messages.len(),
        3,
        "Should have 3 messages (system, user, assistant). Found {}:\n{:?}",
        result.messages.len(),
        texts
    );
}

// ----------------------------------------------------------------------------
// ISSUE 30: Generation Span Messages Not Filtered
// ----------------------------------------------------------------------------
// Messages from generation spans should NOT be filtered, even if they have
// GenAI input event names.

#[test]
fn test_regression_generation_span_messages_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Generation span with user input and assistant output
    let gen_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type("trace1", "root", None, "[]", t0, Some(t1), "agent"),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("root"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Both messages from generation span should be preserved
    assert_eq!(
        result.messages.len(),
        2,
        "Messages from generation span should be preserved. Found {}",
        result.messages.len()
    );

    let roles: Vec<_> = result.messages.iter().map(|m| m.role).collect();
    assert!(roles.contains(&ChatRole::User));
    assert!(roles.contains(&ChatRole::Assistant));
}

// ----------------------------------------------------------------------------
// ISSUE 31: Root Span Messages Not Filtered
// ----------------------------------------------------------------------------
// Messages from root spans should NOT be filtered, regardless of event name.

#[test]
fn test_regression_root_span_messages_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root span with user input (even though it has GenAI input event name)
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Root span user message"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Response", "finish_reason": "stop"}
        }
    ]);

    // Even with observation_type="span", root should not be filtered
    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "root",
        None,
        &root_msg.to_string(),
        t0,
        Some(t1),
        "span",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Root span messages should be preserved
    assert_eq!(
        result.messages.len(),
        2,
        "Root span messages should be preserved. Found {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 32: Chain Span History Filtered
// ----------------------------------------------------------------------------
// GenAI input events from chain spans (observation_type="chain") should be filtered.

#[test]
fn test_regression_chain_span_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root span
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Current request"}
    }]);

    // Chain span with accumulated history
    let chain_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Previous request"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Previous response"}
        }
    ]);

    // Generation span with output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Current response", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "chain",
            Some("root"),
            &chain_msg.to_string(),
            t0,
            Some(t1),
            "chain",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("chain"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain "Previous" messages from chain span
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Previous")),
        "Chain span history should be filtered. Found: {:?}",
        texts
    );

    // Should have: user (Current request), assistant (Current response)
    assert_eq!(result.messages.len(), 2);
}

// ----------------------------------------------------------------------------
// ISSUE 33: Agent Span History Filtered
// ----------------------------------------------------------------------------
// GenAI input events from agent spans (observation_type="agent") in non-root
// position should be filtered.

#[test]
fn test_regression_nested_agent_span_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root agent span with current request
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Main request"}
    }]);

    // Nested agent span (sub-agent) with history
    let sub_agent_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Sub-agent history"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Sub-agent previous response"}
        }
    ]);

    // Generation span with final output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Final response", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "sub_agent",
            Some("root"),
            &sub_agent_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("sub_agent"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain sub-agent history
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Sub-agent")),
        "Nested agent span history should be filtered. Found: {:?}",
        texts
    );

    // Should have: user (Main request), assistant (Final response)
    assert_eq!(result.messages.len(), 2);
}

// ----------------------------------------------------------------------------
// ISSUE 34: Output Events Not Filtered
// ----------------------------------------------------------------------------
// gen_ai.choice events should NOT be filtered even from non-generation spans.

#[test]
fn test_regression_output_events_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Span with both input and output events
    let span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History input"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Current output", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type("trace1", "root", None, "[]", t0, Some(t1), "agent"),
        make_span_row_with_observation_type(
            "trace1",
            "span",
            Some("root"),
            &span_msg.to_string(),
            t0,
            Some(t1),
            "span",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // gen_ai.choice output should be preserved (only input filtered)
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.iter().any(|t| t.contains("Current output")),
        "Output events should be preserved. Found: {:?}",
        texts
    );

    // The root holds no messages, so this span is the conversation's root and its input is the only
    // copy of the question: it is the turn, not a replay of one.
    assert!(
        texts.iter().any(|t| t.contains("History input")),
        "the only copy of the user's turn must be kept. Found: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// ISSUE 35: Multi-Turn Session History (Real-World Strands Pattern)
// ----------------------------------------------------------------------------
// Simulates a Strands trace with multiple previous session turns.

#[test]
fn test_regression_multi_turn_session_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(3);

    // Root agent span with current request
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Third question"}
        }
    ]);

    // Event loop span with ALL previous session turns
    let event_loop_msg = json!([
        // Turn 1
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        // Turn 2
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
        },
        // Current turn (also appears here)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Third question"}
        }
    ]);

    // Generation span with current output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Third answer", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t_end),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t_end),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("event_loop"),
            &gen_msg.to_string(),
            t2,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain first/second turn history
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("First")),
        "First turn should be filtered. Found: {:?}",
        texts
    );
    assert!(
        !texts.iter().any(|t| t.contains("Second")),
        "Second turn should be filtered. Found: {:?}",
        texts
    );

    // Should have: system, user (Third question), assistant (Third answer)
    assert_eq!(
        result.messages.len(),
        3,
        "Should have 3 messages for current turn. Found {}:\n{:?}",
        result.messages.len(),
        texts
    );
}

/// Regression #36: System message should appear before user message when timestamps are equal.
///
/// Some frameworks (Strands) record system and user messages with the same timestamp.
/// Semantic ordering should ensure System comes first (sets context), then User (provides input).
#[test]
fn test_regression_system_before_user_same_timestamp() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Both messages have the exact same timestamp
    // System comes first in the array (message_index 0)
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "span1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(result.messages.len(), 2, "Should have 2 messages");

    // System should be first (message_index 0), User second (message_index 1)
    assert_eq!(
        result.messages[0].role,
        ChatRole::System,
        "First message should be System, got {:?}",
        result.messages[0].role
    );
    assert_eq!(
        result.messages[1].role,
        ChatRole::User,
        "Second message should be User, got {:?}",
        result.messages[1].role
    );

    // Verify the content
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text } if text.contains("helpful assistant")),
        "System message content mismatch"
    );
    assert!(
        matches!(&result.messages[1].content, ContentBlock::Text { text } if text == "Hello"),
        "User message content mismatch"
    );
}

// ============================================================================
// OUTPUT CLASSIFICATION AND HISTORY PROTECTION TESTS
// ============================================================================

/// Regression #37: gen_ai.choice events are ALWAYS protected from history marking.
///
/// Even if a gen_ai.choice event appears in a non-generation span with a parent,
/// it should NOT be marked as history. This protects actual LLM outputs.
#[test]
fn test_regression_gen_ai_choice_never_history() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // A gen_ai.choice event in a span with parent - should NOT be marked as history
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
        "content": {"role": "assistant", "content": "LLM response", "finish_reason": "stop"}
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "child_span",
        Some("parent"), // Has parent
        &messages.to_string(),
        t0,
        Some(t_end),
        "span", // Non-generation span type subject to accumulator-history filtering.
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The gen_ai.choice event should be preserved (protected from history)
    assert_eq!(
        result.messages.len(),
        1,
        "gen_ai.choice should not be filtered"
    );
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "LLM response"),
        "gen_ai.choice content should be preserved"
    );
}

/// Regression #38: gen_ai.assistant.message events CAN be marked as history.
///
/// Unlike gen_ai.choice (actual LLM output), gen_ai.assistant.message is used for
/// history re-sends. These SHOULD be marked as history when in non-generation spans.
#[test]
fn test_regression_gen_ai_assistant_message_can_be_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Root span with current request (has gen_ai.choice output)
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Current request"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Current response", "finish_reason": "stop"}
        }
    ]);

    // Event loop span with history (gen_ai.assistant.message - NOT gen_ai.choice)
    let event_loop_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Previous response from history"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t_end),
            "span", // Non-generation span
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have: user (Current request), assistant (Current response)
    // Should NOT have: "Previous response from history"
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Previous response")),
        "gen_ai.assistant.message should be filtered as history. Found: {:?}",
        texts
    );
    assert!(
        texts.iter().any(|t| t.contains("Current response")),
        "gen_ai.choice should be preserved. Found: {:?}",
        texts
    );
}

/// Regression #39: uses_span_end field is correctly set for different block types.
///
/// Verifies the output classification rules:
/// - gen_ai.choice → uses_span_end = true
/// - Assistant text → uses_span_end = true
/// - ToolUse from non-tool span → uses_span_end = true
/// - User message → uses_span_end = false
/// - Tool result → uses_span_end = false
#[test]
fn test_regression_uses_span_end_classification() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Mix of different message types
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "User message"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Response text"},
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "span1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find blocks by type and verify uses_span_end
    let user_block = result.messages.iter().find(|b| b.role == ChatRole::User);
    let assistant_text = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant && b.entry_type == "text");
    let tool_use = result.messages.iter().find(|b| b.entry_type == "tool_use");

    assert!(user_block.is_some(), "Should have user message");
    assert!(assistant_text.is_some(), "Should have assistant text");
    assert!(tool_use.is_some(), "Should have tool_use");

    // Verify uses_span_end flags
    assert!(
        !user_block.unwrap().uses_span_end,
        "User message should NOT be output"
    );
    assert!(
        assistant_text.unwrap().uses_span_end,
        "Assistant text from gen_ai.choice should be output"
    );
    // NOTE: ToolUse ALWAYS uses event_time (uses_span_end=false), even from gen_ai.choice events.
    // This is critical for correct ordering: ToolUse at event_time T=100 must sort BEFORE
    // ToolResult at span_end T=200. If ToolUse used span_end, it would sort AFTER ToolResult.
    assert!(
        !tool_use.unwrap().uses_span_end,
        "ToolUse should use event_time (not output) for correct ordering"
    );
}
