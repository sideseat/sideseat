/// Regression #40: ToolUse from tool spans is INPUT, not OUTPUT.
///
/// Tool spans log tool invocation (INPUT). The tool_use is output only if it
/// comes from a gen_ai.choice event (the LLM's completion marker).
#[test]
fn test_regression_tool_use_from_tool_span_is_input() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Tool span with tool_use (logging the call)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}]
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "tool_span",
        Some("parent"),
        &tool_span_msg.to_string(),
        t0,
        Some(t_end),
        "tool", // Tool span
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The tool_use should be present and marked as INPUT
    let tool_use = result.messages.iter().find(|b| b.entry_type == "tool_use");
    assert!(tool_use.is_some(), "Tool use should be preserved");
    assert!(
        !tool_use.unwrap().uses_span_end,
        "ToolUse from tool span should be INPUT (not OUTPUT)"
    );
}

/// Regression #41: ToolResult from tool spans is OUTPUT, uses span_end for ordering.
///
/// Tool results from tool spans represent the actual tool execution result.
/// They should use span_end for effective timestamp (when tool finished).
#[test]
fn test_regression_tool_result_from_tool_span_uses_span_end() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Tool span with tool_result (actual execution)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result"}]
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "tool_span",
        Some("parent"),
        &tool_span_msg.to_string(),
        t0,
        Some(t_end),
        "tool", // Tool span
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The tool_result should be present and marked as OUTPUT
    let tool_result = result
        .messages
        .iter()
        .find(|b| b.entry_type == "tool_result");
    assert!(tool_result.is_some(), "Tool result should be preserved");
    assert!(
        tool_result.unwrap().uses_span_end,
        "ToolResult from tool span should be OUTPUT"
    );
}

/// Regression #42: Tool ordering - tool_use ALWAYS before tool_result.
///
/// Even when history copies of tool_results have earlier timestamps,
/// the ordering should still be: tool_use → tool_result.
/// This is achieved by:
/// 1. ToolResult from tool spans uses span_end (when tool finished)
/// 2. History copies are filtered and don't affect birth_time
#[test]
fn test_regression_tool_use_before_tool_result_with_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t4 = t0 + chrono::Duration::milliseconds(400);
    let t5 = t0 + chrono::Duration::milliseconds(500);

    // Generation span with tool_use (LLM decision)
    let gen_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // Tool span with tool_result (actual execution)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t4.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
        }
    }]);

    // Event loop span with history copy of tool_result (misleading early timestamp!)
    let t_early = t0 + chrono::Duration::milliseconds(50); // Very early!
    let history_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t_early.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
        }
    }]);

    let rows = vec![
        // Generation span (tool_use)
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("root"),
            &gen_span_msg.to_string(),
            t0,
            Some(t2), // span_end = T2
            "generation",
        ),
        // Tool span (actual tool_result)
        make_span_row_with_observation_type(
            "trace1",
            "tool_span",
            Some("gen_span"),
            &tool_span_msg.to_string(),
            t3,
            Some(t5), // span_end = T5
            "tool",
        ),
        // Event loop span (history copy with early timestamp)
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &history_span_msg.to_string(),
            t0,
            Some(t0 + chrono::Duration::seconds(10)),
            "span", // Non-generation span
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find the final tool_use and tool_result blocks
    let tool_use_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_use");
    let tool_result_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_result");

    assert!(tool_use_idx.is_some(), "Should have tool_use");
    assert!(tool_result_idx.is_some(), "Should have tool_result");

    // CRITICAL: tool_use must come BEFORE tool_result
    assert!(
        tool_use_idx.unwrap() < tool_result_idx.unwrap(),
        "tool_use (index {}) should come before tool_result (index {})",
        tool_use_idx.unwrap(),
        tool_result_idx.unwrap()
    );
}

/// Regression #43: Tool ordering in same span (Strands scenario).
///
/// In Strands and similar frameworks, tool_use and tool_result can appear
/// in the SAME generation span with different event timestamps:
/// - tool_use at T=100 (LLM decided to call tool)
/// - tool_result at T=200 (tool returned)
/// - final_response at T=300 (LLM finished with response)
/// - span_end at T=300
///
/// If tool_use incorrectly used span_end (T=300), it would sort AFTER
/// tool_result (T=200), breaking the conversation flow.
///
/// This test ensures tool_use uses event_time, not span_end.
#[test]
fn test_regression_tool_use_and_tool_result_same_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100); // tool_use event
    let t2 = t0 + chrono::Duration::milliseconds(200); // tool_result event
    let t3 = t0 + chrono::Duration::milliseconds(300); // final_response event
    let t_end = t3; // span_end

    // Single generation span with tool_use, tool_result, and final response
    // This mimics Strands behavior where all messages are in one span
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Here is your result"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have 3 blocks: tool_use, tool_result, final_response
    assert_eq!(result.messages.len(), 3, "Should have 3 blocks");

    // Find indices
    let tool_use_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_use");
    let tool_result_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_result");
    let text_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "text" && b.finish_reason.is_some());

    assert!(tool_use_idx.is_some(), "Should have tool_use");
    assert!(tool_result_idx.is_some(), "Should have tool_result");
    assert!(text_idx.is_some(), "Should have final text");

    // CRITICAL: Correct ordering must be: tool_use < tool_result < final_response
    assert!(
        tool_use_idx.unwrap() < tool_result_idx.unwrap(),
        "tool_use (idx {}) must come before tool_result (idx {})",
        tool_use_idx.unwrap(),
        tool_result_idx.unwrap()
    );
    assert!(
        tool_result_idx.unwrap() < text_idx.unwrap(),
        "tool_result (idx {}) must come before final text (idx {})",
        tool_result_idx.unwrap(),
        text_idx.unwrap()
    );

    // Verify uses_span_end classification
    let tool_use = &result.messages[tool_use_idx.unwrap()];
    let tool_result = &result.messages[tool_result_idx.unwrap()];
    let final_text = &result.messages[text_idx.unwrap()];

    // tool_use from gen_ai.assistant.message (not gen_ai.choice) should NOT be uses_span_end
    // because it doesn't have finish_reason and isn't a completion marker
    assert!(
        !tool_use.uses_span_end,
        "tool_use from gen_ai.assistant.message should NOT be uses_span_end"
    );

    // tool_result from generation span should NOT be uses_span_end
    // (only tool_result from tool spans is uses_span_end)
    assert!(
        !tool_result.uses_span_end,
        "tool_result from generation span should NOT be uses_span_end"
    );

    // final_text from gen_ai.choice with finish_reason should be uses_span_end
    assert!(
        final_text.uses_span_end,
        "final_text from gen_ai.choice should be uses_span_end"
    );
}

/// Regression #44: Parallel tool calls ordering (multiple tools same timestamp).
///
/// When LLM calls multiple tools in parallel, all tool_use blocks have
/// the same event_time. They should maintain their message_index order
/// and all come before their corresponding tool_results.
#[test]
fn test_regression_parallel_tools_same_span_ordering() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100); // both tool_use events
    let t2 = t0 + chrono::Duration::milliseconds(200); // both tool_result events
    let t3 = t0 + chrono::Duration::milliseconds(300); // final response
    let t_end = t3;

    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "temperature", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_2", "name": "precipitation", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "72F"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_2", "content": "20%"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Temperature: 72F, Precipitation: 20%"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect indices by type
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "tool_use")
        .collect();
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "tool_result")
        .collect();
    let final_text: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "text" && b.finish_reason.is_some())
        .collect();

    assert_eq!(tool_uses.len(), 2, "Should have 2 tool_use blocks");
    assert_eq!(tool_results.len(), 2, "Should have 2 tool_result blocks");
    assert_eq!(final_text.len(), 1, "Should have 1 final text block");

    // All tool_uses should come before all tool_results
    let max_tool_use_idx = tool_uses.iter().map(|(i, _)| *i).max().unwrap();
    let min_tool_result_idx = tool_results.iter().map(|(i, _)| *i).min().unwrap();
    assert!(
        max_tool_use_idx < min_tool_result_idx,
        "All tool_uses (max idx {}) must come before all tool_results (min idx {})",
        max_tool_use_idx,
        min_tool_result_idx
    );

    // All tool_results should come before final text
    let max_tool_result_idx = tool_results.iter().map(|(i, _)| *i).max().unwrap();
    let final_text_idx = final_text[0].0;
    assert!(
        max_tool_result_idx < final_text_idx,
        "All tool_results (max idx {}) must come before final text (idx {})",
        max_tool_result_idx,
        final_text_idx
    );
}

/// Regression #45: ToolUse without explicit completion marker uses event_time.
///
/// When tool_use comes from gen_ai.assistant.message (not gen_ai.choice),
/// it should use event_time for ordering, not span_end.
/// This is critical for correct tool ordering within a span.
#[test]
fn test_regression_tool_use_from_assistant_message_uses_event_time() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t_end = t0 + chrono::Duration::milliseconds(500); // Much later span_end

    // tool_use from gen_ai.assistant.message (not gen_ai.choice)
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {}}]
        }
    }]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    // Should NOT be uses_span_end (no gen_ai.choice event, no finish_reason)
    assert!(
        !block.uses_span_end,
        "tool_use from gen_ai.assistant.message should NOT be uses_span_end"
    );

    // Category should be GenAIAssistantMessage, not GenAIChoice
    assert_eq!(
        block.category,
        sideseat_ports::types::MessageCategory::GenAIAssistantMessage
    );
    assert!(
        block.is_output_source(),
        "a terminal assistant tool call is the choiceless generation's output"
    );
}

/// Regression #46: Intermediate assistant text from generation spans is filtered.
///
/// In Strands tool-use loops:
/// - Generation span produces intermediate text (gen_ai.assistant.message)
/// - Agent span produces final response (gen_ai.choice)
///
/// The intermediate text should be filtered to show only the final response.
/// This prevents duplicate/intermediate outputs during tool-use cycles.
#[test]
fn test_regression_intermediate_assistant_text_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span: intermediate assistant text (NOT the final response)
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Intermediate output during tool use"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {}}]
            }
        }
    ]);

    // Agent span: final response via gen_ai.choice
    let agent_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Final response after tools"}],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        // Agent span (root) with final choice
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        // Generation span with intermediate output
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("agent_span"),
            &gen_span_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have tool_use and final text, but NOT intermediate text
    let text_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "text" && b.role == ChatRole::Assistant)
        .collect();

    assert_eq!(
        text_blocks.len(),
        1,
        "Should have exactly 1 assistant text (final only)"
    );

    let final_text = text_blocks[0];
    assert_eq!(
        final_text.category,
        sideseat_ports::types::MessageCategory::GenAIChoice,
        "Should be from GenAIChoice (final response)"
    );
    assert!(
        matches!(&final_text.content, ContentBlock::Text { text } if text.contains("Final response")),
        "Should be the final response text"
    );

    // Tool use should still be present
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();
    assert_eq!(tool_uses.len(), 1, "Should have tool_use");
}

/// Regression #47: Multi-turn session history filtered from generation spans.
///
/// In Strands multi-turn sessions, generation spans contain FULL conversation history
/// including tool calls and results from previous turns. These should be filtered
/// so only the current turn's messages appear.
///
/// Key insight: Tool results in generation spans indicate session history is present.
/// Current turn output uses gen_ai.choice (GenAIChoice category) which is protected.
#[test]
fn test_regression_multi_turn_session_history_in_generation_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span contains:
    // 1. Previous turn tool calls (GenAIAssistantMessage) - HISTORY
    // 2. Previous turn tool results (GenAIToolMessage) - HISTORY
    // 3. Current turn tool calls (GenAIChoice) - CURRENT
    let gen_span_msg = json!([
        // Previous turn tool call (history)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "old_call_1", "name": "search", "input": {"query": "NYC"}}]
            }
        },
        // Previous turn tool result (history) - KEY SIGNAL for session history detection
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_call_id": "old_call_1",
                "content": "NYC weather: sunny"
            }
        },
        // Current turn tool call (gen_ai.choice = protected)
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "new_call_1", "name": "search", "input": {"query": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Agent span: current turn user message and final response
    let agent_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "LA weather"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "LA is sunny"}],
                "finish_reason": "stop"
            }
        }
    ]);

    // Event loop span: current turn tool result (execution output)
    let event_loop_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_call_id": "new_call_1",
            "content": "LA weather: sunny"
        }
    }]);

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
            "event_loop",
            Some("agent_span"),
            &event_loop_msg.to_string(),
            t0,
            Some(t3),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("event_loop"),
            &gen_span_msg.to_string(),
            t0,
            Some(t2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool_use blocks - should only have LA (current turn), not NYC (history)
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Should have exactly 1 tool_use (current turn only). Found: {:?}",
        tool_uses.iter().map(|b| &b.content).collect::<Vec<_>>()
    );

    // Verify it's the LA tool call (current turn)
    if let ContentBlock::ToolUse { input, .. } = &tool_uses[0].content {
        assert_eq!(
            input.get("query").and_then(|v| v.as_str()),
            Some("LA"),
            "Should be LA tool call (current turn)"
        );
    } else {
        panic!("Expected tool_use content block");
    }

    // Count tool_result blocks - should only have LA (current turn), not NYC (history)
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "Should have exactly 1 tool_result (current turn only)"
    );

    // Should have user message for current turn
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();
    assert_eq!(user_msgs.len(), 1, "Should have 1 user message");

    // Should have final assistant text
    let assistant_text: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant && b.entry_type == "text")
        .collect();
    assert_eq!(
        assistant_text.len(),
        1,
        "Should have 1 final assistant text"
    );
}

// ============================================================================
// REGRESSION TESTS FOR TIMESTAMP-BASED HISTORY DETECTION
// ============================================================================

/// Regression #48: Timestamp-based history detection.
///
/// Messages with timestamp < span_start in child generation spans should be
/// marked as history. This is the fundamental signal for detecting historical
/// context that was passed to the LLM.
#[test]
fn test_regression_timestamp_based_history_detection() {
    let t0 = fixed_time();
    let t_history = t0 - chrono::Duration::seconds(10); // Before span start
    let t_current = t0 + chrono::Duration::seconds(1); // After span start
    let t_end = t0 + chrono::Duration::seconds(2);

    // Generation span with both historical and current content
    let gen_span_msg = json!([
        // Historical message (timestamp before span start) - should be filtered
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t_history.to_rfc3339()}},
            "content": {"role": "user", "content": "Old question from history"}
        },
        // Current message (timestamp after span start) - should be kept
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_current.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": "Current response",
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen_span",
        Some("parent"),
        &gen_span_msg.to_string(),
        t0, // Span starts at t0
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Historical message should be filtered (timestamp < span_start)
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();
    assert!(
        user_msgs.is_empty(),
        "Historical user message (timestamp < span_start) should be filtered"
    );

    // Current response should be preserved (protected by gen_ai.choice)
    let assistant_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        assistant_msgs.len(),
        1,
        "Current response should be preserved"
    );
}

/// Regression #49: Child spans with different content preserved.
///
/// When parent and child spans have genuinely different content,
/// both should be preserved. The history detection should NOT filter
/// new content just because it doesn't exist in parent spans.
#[test]
fn test_regression_child_span_new_content_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Parent span with one message
    let parent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Question in parent"}
    }]);

    // Child span with different (new) content - NOT a history copy
    let child_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Response in child",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "parent",
            None,
            &parent_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("parent"),
            &child_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Both messages should be preserved (different content)
    assert_eq!(
        result.messages.len(),
        2,
        "Both parent and child content should be preserved when different"
    );

    let roles: Vec<_> = result.messages.iter().map(|m| m.role).collect();
    assert!(roles.contains(&ChatRole::User), "User message should exist");
    assert!(
        roles.contains(&ChatRole::Assistant),
        "Assistant message should exist"
    );
}
