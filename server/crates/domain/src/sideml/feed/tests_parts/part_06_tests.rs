/// Regression #50: Tool_use preserved when intermediate text is filtered.
///
/// In generation spans, intermediate assistant text should be filtered
/// but tool_use blocks should be preserved. This is because tool_use
/// represents actual LLM output (a decision to call a tool).
#[test]
fn test_regression_tool_use_preserved_text_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span with intermediate text AND tool_use
    let gen_span_msg = json!([
        // Intermediate text (should be filtered)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Let me search for that"}]
            }
        },
        // Tool use (should be preserved - it's actual output)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"query": "test"}}]
            }
        }
    ]);

    // Agent span with final response (triggers has_agent_spans and provides final output)
    let agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Here are the results",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent",
            None,
            &agent_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("agent"),
            &gen_span_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Tool_use should be preserved
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();
    assert_eq!(
        tool_uses.len(),
        1,
        "Tool_use should be preserved even when intermediate text is filtered"
    );

    // Intermediate text should be filtered, but final response (gen_ai.choice) preserved
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "text" && b.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        texts.len(),
        1,
        "Should have 1 text (final response), intermediate filtered"
    );
    assert!(
        matches!(&texts[0].content, ContentBlock::Text { text, .. } if text.contains("results")),
        "Should be final response, not intermediate text"
    );
}

/// Regression #51: Multi-turn session history with tool operations.
///
/// In multi-turn sessions (Strands-like), generation spans contain full
/// conversation history including tool calls and results from previous turns.
/// These should be filtered, keeping only current turn output.
///
/// This test specifically covers the original bug where trace
/// e91ae2156c0bb2242e34b507c68374e1 (LA weather) was showing NYC tool
/// calls/results from previous turns.
#[test]
fn test_regression_multi_turn_tool_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span contains:
    // 1. Previous turn: user question about NYC
    // 2. Previous turn: tool call for NYC
    // 3. Previous turn: tool result for NYC
    // 4. Previous turn: assistant response about NYC
    // 5. Current turn: user question about LA
    // 6. Current turn: tool call for LA (gen_ai.choice - protected)
    let gen_span_msg = json!([
        // PREVIOUS TURN (all should be filtered)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in NYC?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "old_call", "name": "weather", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
            "content": {"role": "tool", "tool_call_id": "old_call", "content": "NYC: Rainy"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC is rainy today."}
        },
        // CURRENT TURN
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "new_call", "name": "weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Agent span with current turn user message
    let agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "What's the weather in LA?"}
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent",
            None,
            &agent_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("agent"),
            &gen_span_msg.to_string(),
            t1, // Span starts AFTER previous turn timestamps (t0)
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Check that NYC content is NOT present
    let all_text: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !all_text.iter().any(|t| t.contains("NYC")),
        "NYC messages from previous turn should NOT appear. Found: {:?}",
        all_text
    );

    // Check that LA tool_use IS present
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Should have exactly 1 tool_use (LA, current turn)"
    );

    if let ContentBlock::ToolUse { input, .. } = &tool_uses[0].content {
        assert_eq!(
            input.get("city").and_then(|v| v.as_str()),
            Some("LA"),
            "Tool_use should be for LA (current turn), not NYC (history)"
        );
    }

    // Should have only LA user message (from agent span)
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(user_msgs.len(), 1, "Should have exactly 1 user message");
    assert!(
        user_msgs[0].contains("LA"),
        "User message should be about LA"
    );
}

/// Regression #52: Cross-trace tool_use_id contamination in sessions.
///
/// When processing a session with multiple traces, the orphan tool_result
/// detection should work per-trace. Tool_use_ids from previous traces should
/// NOT be considered "current" for subsequent traces.
///
/// This test simulates a session where:
/// - Trace 1: Has tool_use with ID "call_old"
/// - Trace 2: Has tool_result with ID "call_old" (session history from trace 1),
///   AND tool_use/result with ID "call_new" (current turn)
///
/// The "call_old" tool_result in trace 2 should be filtered as orphan.
#[test]
fn test_regression_cross_trace_tool_id_contamination() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(5);
    let t2 = t1 + chrono::Duration::seconds(1);
    let t_end1 = t0 + chrono::Duration::seconds(4);
    let t_end2 = t1 + chrono::Duration::seconds(2);

    // TRACE 1: Has tool_use with ID "call_old"
    let trace1_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_old", "name": "search", "input": {"q": "NYC"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // TRACE 2: Generation span with session history (call_old) and current turn (call_new)
    let trace2_gen_msg = json!([
        // Session history from trace 1 (should be filtered as orphan)
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {"role": "tool", "tool_call_id": "call_old", "content": "NYC weather"}
        },
        // Current turn (protected by gen_ai.choice)
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_new", "name": "search", "input": {"q": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let trace2_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "What's the weather in LA?"}
    }]);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &trace1_agent_msg.to_string(),
            t0,
            Some(t_end1),
            Some("agent"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &trace2_agent_msg.to_string(),
            t1,
            Some(t_end2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen_msg.to_string(),
            t1,
            Some(t_end2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Filter to trace2 messages only
    let trace2_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.trace_id == "trace2")
        .collect();

    // Should have: user message + current tool_use (call_new)
    // Should NOT have: old tool_result (call_old)
    let tool_results: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert!(
        tool_results.is_empty(),
        "Session history tool_result (call_old) should be filtered. Found: {:?}",
        tool_results
            .iter()
            .map(|m| &m.tool_use_id)
            .collect::<Vec<_>>()
    );

    // Current turn tool_use should be present
    let tool_uses: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Current turn tool_use (call_new) should be present"
    );

    if let ContentBlock::ToolUse { id, .. } = &tool_uses[0].content {
        assert_eq!(
            id.as_deref(),
            Some("call_new"),
            "Should be current turn tool_use"
        );
    }
}

/// Regression #53: Tool_use before tool_result ordering in multi-trace sessions.
///
/// When processing a session with multiple traces, tool_uses must appear
/// before their corresponding tool_results. This tests that orphan tool_results
/// (from session history) are filtered, maintaining correct ordering.
#[test]
fn test_regression_session_tool_ordering_across_traces() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);
    let t4 = t0 + chrono::Duration::seconds(4);
    let t5 = t0 + chrono::Duration::seconds(5);

    // TRACE 1: Complete tool cycle
    let trace1_gen_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "weather", "input": {"city": "NYC"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);
    let trace1_tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "NYC: Sunny"}]}
    }]);

    // TRACE 2: Has session history (call_1 result) + new tool cycle (call_2)
    let trace2_gen_msg = json!([
        // Session history - old tool_result (should be filtered)
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t3.to_rfc3339()}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "NYC: Sunny"}]}
        },
        // Current turn - new tool_use
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_2", "name": "weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);
    let trace2_tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t5.to_rfc3339()}},
        "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_2", "content": "LA: Warm"}]}
    }]);

    // Agent spans need content for has_agent_spans detection
    let trace1_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {"role": "assistant", "content": "NYC weather ready.", "finish_reason": "stop"}
    }]);
    let trace2_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t5.to_rfc3339()}},
        "content": {"role": "assistant", "content": "LA weather ready.", "finish_reason": "stop"}
    }]);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &trace1_agent_msg.to_string(),
            t0,
            Some(t2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &trace1_gen_msg.to_string(),
            t0,
            Some(t2),
            "generation",
        ),
        make_span_row_full(
            "trace1",
            "tool1",
            Some("agent1"),
            &trace1_tool_msg.to_string(),
            t1,
            Some(t2),
            Some("tool"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &trace2_agent_msg.to_string(),
            t3,
            Some(t5),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen_msg.to_string(),
            t3,
            Some(t5),
            "generation",
        ),
        make_span_row_full(
            "trace2",
            "tool2",
            Some("agent2"),
            &trace2_tool_msg.to_string(),
            t4,
            Some(t5),
            Some("tool"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Check trace 2 specifically - should have call_2 tool_use before call_2 tool_result
    let trace2_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.trace_id == "trace2")
        .collect();

    let tool_uses: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_use")
        .collect();
    let tool_results: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    // Should have exactly 1 tool_use (call_2) and 1 tool_result (call_2)
    assert_eq!(
        tool_uses.len(),
        1,
        "Trace 2 should have 1 tool_use (call_2)"
    );
    assert_eq!(
        tool_results.len(),
        1,
        "Trace 2 should have 1 tool_result (call_2, not call_1)"
    );

    // Verify it's call_2, not call_1 (session history)
    assert_eq!(
        tool_results[0].tool_use_id.as_deref(),
        Some("call_2"),
        "Tool result should be call_2 (current turn), not call_1 (session history)"
    );

    // Verify ordering: tool_use before tool_result in the full result
    let trace2_tool_positions: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.trace_id == "trace2" && (m.entry_type == "tool_use" || m.entry_type == "tool_result")
        })
        .map(|(i, m)| (i, &m.entry_type))
        .collect();

    assert!(
        trace2_tool_positions.len() >= 2,
        "Should have both tool_use and tool_result"
    );

    // Find positions
    let tool_use_pos = trace2_tool_positions
        .iter()
        .find(|(_, t)| *t == "tool_use")
        .map(|(i, _)| i);
    let tool_result_pos = trace2_tool_positions
        .iter()
        .find(|(_, t)| *t == "tool_result")
        .map(|(i, _)| i);

    assert!(
        tool_use_pos < tool_result_pos,
        "Tool_use should come before tool_result. Positions: use={:?}, result={:?}",
        tool_use_pos,
        tool_result_pos
    );
}

/// Regression #54: Session with multiple traces - each trace isolated.
///
/// When processing a session, history detection must work per-trace.
/// Tool operations from trace N should not affect filtering in trace N+1.
#[test]
fn test_regression_session_per_trace_isolation() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(5);
    let t2 = t0 + chrono::Duration::seconds(10);
    let t_end0 = t0 + chrono::Duration::seconds(4);
    let t_end1 = t1 + chrono::Duration::seconds(4);
    let t_end2 = t2 + chrono::Duration::seconds(4);

    // Three traces, each with their own tool cycle
    // The tool_use_ids are different in each trace
    let make_trace_msgs = |call_id: &str, city: &str, time: chrono::DateTime<Utc>| {
        let gen_msg = json!([{
            "source": {"event": {"name": "gen_ai.choice", "time": time.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": call_id, "name": "weather", "input": {"city": city}}],
                "finish_reason": "tool_use"
            }
        }]);
        let tool_msg = json!([{
            "source": {"event": {"name": "gen_ai.tool.message", "time": (time + chrono::Duration::seconds(1)).to_rfc3339()}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": call_id, "content": format!("{}: Weather data", city)}]}
        }]);
        (gen_msg, tool_msg)
    };

    let (trace1_gen, trace1_tool) = make_trace_msgs("call_a", "NYC", t0);
    let (trace2_gen, trace2_tool) = make_trace_msgs("call_b", "LA", t1);
    let (trace3_gen, trace3_tool) = make_trace_msgs("call_c", "Chicago", t2);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            "[]",
            t0,
            Some(t_end0),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &trace1_gen.to_string(),
            t0,
            Some(t_end0),
            "generation",
        ),
        make_span_row_full(
            "trace1",
            "tool1",
            Some("agent1"),
            &trace1_tool.to_string(),
            t0,
            Some(t_end0),
            Some("tool"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            "[]",
            t1,
            Some(t_end1),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen.to_string(),
            t1,
            Some(t_end1),
            "generation",
        ),
        make_span_row_full(
            "trace2",
            "tool2",
            Some("agent2"),
            &trace2_tool.to_string(),
            t1,
            Some(t_end1),
            Some("tool"),
        ),
        // Trace 3
        make_span_row_full(
            "trace3",
            "agent3",
            None,
            "[]",
            t2,
            Some(t_end2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace3",
            "gen3",
            Some("agent3"),
            &trace3_gen.to_string(),
            t2,
            Some(t_end2),
            "generation",
        ),
        make_span_row_full(
            "trace3",
            "tool3",
            Some("agent3"),
            &trace3_tool.to_string(),
            t2,
            Some(t_end2),
            Some("tool"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Each trace should have its own tool_use and tool_result
    for (trace_id, expected_call_id) in [
        ("trace1", "call_a"),
        ("trace2", "call_b"),
        ("trace3", "call_c"),
    ] {
        let trace_msgs: Vec<_> = result
            .messages
            .iter()
            .filter(|m| m.trace_id == trace_id)
            .collect();

        let tool_uses: Vec<_> = trace_msgs
            .iter()
            .filter(|m| m.entry_type == "tool_use")
            .collect();
        let tool_results: Vec<_> = trace_msgs
            .iter()
            .filter(|m| m.entry_type == "tool_result")
            .collect();

        assert_eq!(
            tool_uses.len(),
            1,
            "{} should have exactly 1 tool_use",
            trace_id
        );
        assert_eq!(
            tool_results.len(),
            1,
            "{} should have exactly 1 tool_result",
            trace_id
        );

        // Verify correct call_id
        if let ContentBlock::ToolUse { id, .. } = &tool_uses[0].content {
            assert_eq!(
                id.as_deref(),
                Some(expected_call_id),
                "{} tool_use should have id {}",
                trace_id,
                expected_call_id
            );
        }
        assert_eq!(
            tool_results[0].tool_use_id.as_deref(),
            Some(expected_call_id),
            "{} tool_result should have id {}",
            trace_id,
            expected_call_id
        );
    }
}

/// Regression #55: Thinking blocks without protection are filtered as history.
///
/// In multi-turn sessions, thinking blocks from previous turns are re-sent as
/// history context. These have GenAIAssistantMessage category (not GenAIChoice)
/// and no finish_reason, so they should be filtered.
///
/// Only thinking blocks with protection markers (GenAIChoice, finish_reason)
/// should be preserved.
#[test]
fn test_regression_thinking_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span with:
    // - History thinking (GenAIAssistantMessage, no finish_reason) - should be FILTERED
    // - Current thinking (GenAIChoice, finish_reason) - should be KEPT
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "History thinking from previous turn"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "Current turn thinking"}],
                "finish_reason": "stop"
            }
        }
    ]);

    // Agent span (root) to trigger history detection
    let agent_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Final response"}],
            "finish_reason": "stop"
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

    // Count thinking blocks
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "thinking")
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        1,
        "Should have exactly 1 thinking block (current turn only)"
    );

    // Verify it's the current turn thinking (protected)
    let thinking = thinking_blocks[0];
    assert!(
        thinking.finish_reason.is_some(),
        "Preserved thinking should have finish_reason (protected)"
    );
    assert!(
        matches!(&thinking.content, ContentBlock::Thinking { text, .. } if text.contains("Current turn")),
        "Should be the current turn thinking"
    );
}
