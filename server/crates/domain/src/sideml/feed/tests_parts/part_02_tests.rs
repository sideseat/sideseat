
// ----------------------------------------------------------------------------
// ISSUE 1: Historical Context Leaking as New Messages
// ----------------------------------------------------------------------------
// When an LLM span includes conversation history from previous turns,
// those messages appear as separate entries in the feed.

#[test]
fn test_regression_historical_context_not_leaked() {
    // Scenario: Agent trace where user asks about LA, but history includes NYC data
    // The NYC messages should NOT appear in the feed for the LA request
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Root span: user asks about LA
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in Los Angeles?"}
        }
    ]);

    // Child LLM span includes history from previous turn (NYC) as context
    // This is what the LLM received, but shouldn't be in final feed
    let child_msg = json!([
        // Historical context (previous turn about NYC)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in New York?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "New York is sunny today."}
        },
        // Current turn
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in Los Angeles?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Los Angeles is warm and sunny.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t2)),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("root"),
            &child_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT include NYC messages - they're historical context
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("New York")),
        "Historical NYC messages should not appear in feed. Found: {:?}",
        texts
    );

    // Should only have: system, user (LA question), assistant (LA answer)
    assert_eq!(result.messages.len(), 3);
}

// ----------------------------------------------------------------------------
// ISSUE 2: Tool Results Not Deduplicating Due to Structure Differences
// ----------------------------------------------------------------------------
// Same tool result appears with different JSON structures in different spans,
// causing hash mismatch and duplicate entries.

#[test]
fn test_regression_tool_result_structure_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Tool execution span: result as direct object
    let tool_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_123",
                "content": {"result": "sunny", "temp": 25}
            }
        }
    ]);

    // Chat span receives tool result as array with type wrapper
    let chat_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_123",
                "content": [{"type": "json", "data": {"json": {"result": "sunny", "temp": 25}}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather is sunny.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_tool_span_row(
            "trace1",
            "tool_span",
            Some("chat_span"),
            &tool_span_msg.to_string(),
            t1,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "chat_span",
            Some("root"),
            &chat_span_msg.to_string(),
            t0,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool results
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    // Should be 1 tool result, not 2
    assert_eq!(
        tool_results.len(),
        1,
        "Same tool result with different structure should deduplicate. Found {} instead of 1",
        tool_results.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 3: Text Messages Not Deduplicating Due to Whitespace
// ----------------------------------------------------------------------------
// Same text with trailing newline hashes differently, causing duplicates.

#[test]
fn test_regression_text_whitespace_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Child span: response without trailing newline
    let child_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Hello, world!", "finish_reason": "stop"}
    }]);

    // Root span: aggregated response with trailing newline
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Hello, world!\n", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t1)),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("root"),
            &child_msg.to_string(),
            t0,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count assistant responses
    let assistant_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .collect();

    // Should be 1 response, not 2
    assert_eq!(
        assistant_msgs.len(),
        1,
        "Same text with/without trailing newline should deduplicate. Found {}",
        assistant_msgs.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 4: Wrong Message Ordering (Tool Use After Tool Result)
// ----------------------------------------------------------------------------
// Tool use blocks appear after tool result blocks in the output.

#[test]
fn test_regression_tool_ordering() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);

    // LLM span with tool use output
    let llm_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Search for cats"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "cats"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Second LLM span: receives tool result as history, outputs final response
    // The tool result is recorded here even though tool execution happened elsewhere
    let llm2_msg = json!([
        // History includes the tool result
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Found cats!"}
        },
        // Final output
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Here are the cats!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "llm1",
            Some("root"),
            &llm_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "llm2",
            Some("root"),
            &llm2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find positions
    let tool_use_pos = result
        .messages
        .iter()
        .position(|m| m.entry_type == "tool_use");
    let tool_result_pos = result
        .messages
        .iter()
        .position(|m| m.entry_type == "tool_result");

    assert!(
        tool_use_pos.is_some() && tool_result_pos.is_some(),
        "Should have both tool_use and tool_result. Types found: {:?}",
        result
            .messages
            .iter()
            .map(|m| &m.entry_type)
            .collect::<Vec<_>>()
    );

    assert!(
        tool_use_pos.unwrap() < tool_result_pos.unwrap(),
        "tool_use (pos {}) should come before tool_result (pos {})",
        tool_use_pos.unwrap(),
        tool_result_pos.unwrap()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 5: Spurious Json Block from Tool Input
// ----------------------------------------------------------------------------
// Tool input appears as a separate Json message entry.
// This matches the pattern seen in trace 959d2590050265486b5f3a55ae3e2b71
// where span 2983bb7075c0d081 has a Json block with {"city": "Los Angeles", "days": 7}

#[test]
fn test_regression_no_spurious_tool_input_block() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Tool span with Strands-style events:
    // - tool_handler.invoke event with input params
    // - tool_handler.result event with output
    let tool_msg = json!([
        {
            "source": {"event": {"name": "tool_handler.invoke", "time": t0.to_rfc3339()}},
            "content": {"city": "Los Angeles", "days": 7}
        },
        {
            "source": {"event": {"name": "tool_handler.result", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC",
                "content": {"json": {"result": "sunny"}}
            }
        }
    ]);

    let rows = vec![make_tool_span_row(
        "trace1",
        "tool1",
        Some("root"),
        &tool_msg.to_string(),
        t0,
        Some(t1),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect all block types
    let block_types: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.entry_type.as_str())
        .collect();

    // Should NOT have json or text blocks for tool input params
    let non_tool_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type != "tool_result" && m.role != ChatRole::Tool)
        .collect();

    assert!(
        non_tool_blocks.is_empty(),
        "Tool span should only produce tool_result, not extra blocks. Found: {:?}",
        block_types
    );
}

// ----------------------------------------------------------------------------
// ISSUE 6: Tool Results with Same tool_use_id but Different Content Hash
// ----------------------------------------------------------------------------
// Tool results referencing same tool_use_id should deduplicate even if structure differs.

#[test]
fn test_regression_tool_result_same_id_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Tool span result
    let tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "tooluse_ABC123",
            "content": "The forecast shows sunny weather."
        }
    }]);

    // Chat span receives same result (recorded again as input history)
    let chat_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC123",
                "content": [{"type": "text", "text": "The forecast shows sunny weather."}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "It will be sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_tool_span_row(
            "trace1",
            "tool1",
            Some("chat1"),
            &tool_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "chat1",
            Some("root"),
            &chat_msg.to_string(),
            t0,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool results for this tool_use_id
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.tool_use_id.as_deref() == Some("tooluse_ABC123") && m.role == ChatRole::Tool)
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "Same tool result (same tool_use_id) should appear once. Found {}",
        tool_results.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 7: Intermediate Agent Loop Outputs Appearing in Feed
// ----------------------------------------------------------------------------
// Outputs from intermediate agent loop iterations shouldn't appear.

#[test]
fn test_regression_no_intermediate_loop_outputs() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    // First loop iteration (intermediate, not final)
    let loop1_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Get weather for LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "weather", "input": {}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Second loop iteration (final)
    let loop2_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Sunny"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather in LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    // Root span with final output only
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Get weather for LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather in LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t3)),
        make_span_row_with_timestamps(
            "trace1",
            "loop1",
            Some("root"),
            &loop1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "loop2",
            Some("root"),
            &loop2_msg.to_string(),
            t1,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count final assistant text responses (not tool_use)
    let final_responses: Vec<_> = result
        .messages
        .iter()
        .filter(|m| {
            m.role == ChatRole::Assistant && m.entry_type == "text" && m.finish_reason.is_some()
        })
        .collect();

    // Should have exactly 1 final response
    assert_eq!(
        final_responses.len(),
        1,
        "Should have exactly 1 final response. Found {}",
        final_responses.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 8: History Assistant Messages Without finish_reason
// ----------------------------------------------------------------------------
// Historical assistant messages lack finish_reason, affecting birth time computation.

#[test]
fn test_regression_history_assistant_detection() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // First span: original conversation
    let first_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!", "finish_reason": "stop"}
        }
    ]);

    // Second span: includes history WITHOUT finish_reason
    let second_msg = json!([
        // History - note: NO finish_reason (stripped when re-sent)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!"}
        },
        // New turn
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "How are you?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "I'm doing well!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "span1",
            None,
            &first_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &second_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count "Hi there!" assistant messages
    let hi_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|m| {
            m.role == ChatRole::Assistant
                && matches!(&m.content, ContentBlock::Text { text } if text == "Hi there!")
        })
        .collect();

    // Should deduplicate to 1
    assert_eq!(
        hi_messages.len(),
        1,
        "History assistant message should deduplicate. Found {}",
        hi_messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 9: Cross-Span Duplicate Tool Use Events
// ----------------------------------------------------------------------------
// Same tool use appears in multiple spans with different timing.

#[test]
fn test_regression_cross_span_tool_use_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // LLM span decides to use tool
    let llm_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_xyz", "name": "search", "input": {"q": "test"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // Tool span receives the same tool use as input context
    let tool_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_xyz", "name": "search", "input": {"q": "test"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_xyz", "content": "Results"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "llm",
            Some("root"),
            &llm_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool",
            Some("llm"),
            &tool_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool_use blocks with this ID
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_use" && m.tool_use_id.as_deref() == Some("call_xyz"))
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Same tool_use across spans should deduplicate. Found {}",
        tool_uses.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 10: Full Agent Trace Integration Test
// ----------------------------------------------------------------------------
// Simulates the exact scenario from trace 959d2590050265486b5f3a55ae3e2b71

#[test]
fn test_regression_full_agent_trace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t4 = t0 + chrono::Duration::milliseconds(400);

    // Root agent span
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {"role": "assistant", "content": "LA is sunny!\n", "finish_reason": "stop"}
        }
    ]);

    // First LLM call - decides to use tool
    let llm1_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool execution
    // Note: In tool spans, gen_ai.choice is used for tool OUTPUT (result)
    // gen_ai.tool.message would be INPUT which is not what we want here
    let tool_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.input", "time": t2.to_rfc3339()}},
            "content": {"city": "LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Sunny, 25C"}
        }
    ]);

    // Second LLM call - produces final response
    let llm2_msg = json!([
        // History
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "LA"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_1",
                "content": [{"type": "text", "text": "Sunny, 25C"}]
            }
        },
        // New output
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {"role": "assistant", "content": "LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t4)),
        make_span_row_with_timestamps(
            "trace1",
            "llm1",
            Some("root"),
            &llm1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool",
            Some("llm1"),
            &tool_msg.to_string(),
            t1,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "llm2",
            Some("root"),
            &llm2_msg.to_string(),
            t2,
            Some(t4),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Expected conversation flow:
    // 1. System message
    // 2. User: "Weather in LA?"
    // 3. Assistant: [tool_use: get_weather]
    // 4. Tool: "Sunny, 25C"
    // 5. Assistant: "LA is sunny!"

    // Verify no duplicates
    assert_eq!(
        result.messages.len(),
        5,
        "Should have exactly 5 messages. Found {}:\n{:?}",
        result.messages.len(),
        result
            .messages
            .iter()
            .map(|m| format!("{}: {:?}", m.entry_type, m.role))
            .collect::<Vec<_>>()
    );

    // Verify correct ordering - note: system and user have same timestamp,
    // so order between them may vary. Important is semantic ordering:
    // user/system → tool_use → tool_result → assistant_text
    let types: Vec<_> = result
        .messages
        .iter()
        .map(|m| (m.role, m.entry_type.as_str()))
        .collect();

    // First two should be user and system (order may vary as they have same timestamp)
    let first_two: std::collections::HashSet<_> = types[0..2].iter().collect();
    assert!(
        first_two.contains(&(ChatRole::User, "text"))
            && first_two.contains(&(ChatRole::System, "text")),
        "First two should be user and system"
    );
    assert_eq!(
        types[2],
        (ChatRole::Assistant, "tool_use"),
        "Third should be tool_use"
    );
    assert_eq!(
        types[3],
        (ChatRole::Tool, "tool_result"),
        "Fourth should be tool_result"
    );
    assert_eq!(
        types[4],
        (ChatRole::Assistant, "text"),
        "Fifth should be assistant text"
    );

    // No spurious json blocks
    let json_blocks = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "json")
        .count();
    assert_eq!(json_blocks, 0, "Should have no spurious json blocks");
}
