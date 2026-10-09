// Trace 74045ce6aae9e6765f8017028a23ecfe demonstrates this pattern:
//   - Tool spans: raw {path, base64, mimeType}
//   - Generation span: {type: "content", value: [{type: "text"}, {type: "image-data"}]}

#[test]
fn test_regression_vercel_to_model_output_tool_result_dedup() {
    // Vercel AI SDK: toModelOutput transforms content before re-sending to the model.
    // The raw tool result in the tool span differs from the transformed version in the
    // next generation span's ai.prompt.messages attribute.
    //
    // Trace 74045ce6aae9e6765f8017028a23ecfe demonstrates this pattern.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    // Root span (ai.generateText): wraps the whole multi-step flow
    let root_msg = json!([]);

    // First generation span (ai.generateText.doGenerate): produces tool_use
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Read the image"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "tooluse_ABC", "name": "image_reader", "input": {"path": "/tmp/image.png"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool span (ai.toolCall): raw execute() result as attribute
    let tool_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "tooluse_ABC",
            "content": {"path": "/tmp/image.png", "base64": "iVBOR...", "mimeType": "image/png"}
        }
    }]);

    // Second generation span: tool result re-sent (toModelOutput format) + final answer
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC",
                "content": {"type": "content", "value": [
                    {"type": "text", "text": "Image: /tmp/image.png"},
                    {"type": "image-data", "data": "iVBOR...", "mediaType": "image/png"}
                ]}
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool_span",
            Some("root"),
            &tool_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "toModelOutput-transformed tool result should dedup with raw version. Found {}",
        tool_results.len()
    );

    // Should keep the tool span version (actual execution)
    assert_eq!(
        tool_results[0].observation_type.as_deref(),
        Some("tool"),
        "Should prefer tool span version over generation span copy"
    );
}

#[test]
fn test_regression_vercel_to_model_output_multiple_tools_dedup() {
    // 3 parallel tool calls: each produces a raw result in its tool span and
    // a toModelOutput-transformed copy in the next generation span.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    let root_msg = json!([]);

    // First generation span: 3 parallel tool_use calls
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Generate 3 images of a dog"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_1", "name": "image_reader", "input": {"path": "/img1.png"}},
                    {"type": "tool_use", "id": "call_2", "name": "image_reader", "input": {"path": "/img2.png"}},
                    {"type": "tool_use", "id": "call_3", "name": "image_reader", "input": {"path": "/img3.png"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // 3 tool spans with raw results (attribute source, matching real Vercel data)
    let tool1_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": {"path": "/img1.png", "data": "AAA"}}
    }]);
    let tool2_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_2", "content": {"path": "/img2.png", "data": "BBB"}}
    }]);
    let tool3_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_3", "content": {"path": "/img3.png", "data": "CCC"}}
    }]);

    // Second generation span: all 3 re-sent with toModelOutput format + answer
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img1.png"}]}}
        },
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_2",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img2.png"}]}}
        },
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_3",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img3.png"}]}}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Image 1 is best.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "t1",
            Some("root"),
            &tool1_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_tool_span_row(
            "trace1",
            "t2",
            Some("root"),
            &tool2_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_tool_span_row(
            "trace1",
            "t3",
            Some("root"),
            &tool3_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        3,
        "3 tool calls should produce 3 results (not 6). Found {}",
        tool_results.len()
    );

    // All should be from tool spans
    for tr in &tool_results {
        assert_eq!(
            tr.observation_type.as_deref(),
            Some("tool"),
            "All tool results should come from tool spans, not generation span"
        );
    }
}

// ============================================================================
// TIMESTAMP MATERIALIZATION
// ============================================================================
// Attribute-sourced messages inherit span start time as their raw timestamp,
// but their actual production time is span_end (for output blocks).
// After feed processing, block.timestamp must reflect the birth/effective time.
//
// Trace 74045ce6aae9e6765f8017028a23ecfe: final assistant response on root span
// had timestamp = span start (10:03:08) instead of span end (10:03:17).

#[test]
fn test_regression_timestamp_materialized_for_root_span_output() {
    // Vercel AI SDK multi-step: root span wraps gen1 → tools → gen2.
    // The final assistant response appears as an attribute on the root span,
    // inheriting span_start as its timestamp. After processing, its timestamp
    // must be materialized to span_end (when the response was actually produced).
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(3);
    let t2 = t0 + chrono::Duration::seconds(5);
    let t3 = t0 + chrono::Duration::seconds(10);

    // Root span (ai.generateText): final response as attribute
    let root_msg = json!([{
        "source": {"attribute": {"key": "ai.response.text", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
    }]);

    // First gen span: user prompt + tool_use output
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Describe the image"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "image_reader", "input": {"path": "/img.png"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool span: raw result
    let tool_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": {"path": "/img.png", "data": "AAA"}}
    }]);

    // Second gen span: transformed tool result + final answer (gen_ai.choice)
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img.png"}]}}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool1",
            Some("root"),
            &tool_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find the final assistant response (finish_reason=stop)
    let final_assistant: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant && m.finish_reason == Some(FinishReason::Stop))
        .collect();

    assert_eq!(
        final_assistant.len(),
        1,
        "Should have exactly one final assistant response"
    );

    // The key assertion: timestamp must NOT be span start (t0).
    // It must be materialized to the effective time (span_end = t3).
    assert_ne!(
        final_assistant[0].timestamp, t0,
        "Final assistant timestamp must not be raw span start time"
    );
    assert_eq!(
        final_assistant[0].timestamp, t3,
        "Final assistant timestamp should be span_end (when response was produced)"
    );

    // Tool results should have span_end timestamps too (tool execution completion)
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(tool_results.len(), 1);
    assert_eq!(
        tool_results[0].timestamp, t2,
        "Tool result timestamp should be tool span_end (execution completion)"
    );

    // Verify chronological ordering: user → tool_use → tool_result → final assistant
    let timestamps: Vec<_> = result.messages.iter().map(|m| m.timestamp).collect();
    for window in timestamps.windows(2) {
        assert!(
            window[0] <= window[1],
            "Messages must be in chronological order: {:?} should be <= {:?}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn test_regression_different_tool_use_ids_preserved() {
    // Different tool_use_ids = different logical tool executions → both preserved.
    // This guards against over-merging: only same tool_use_id triggers dedup.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "First result"}
    }]);

    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_2", "content": "Second result"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        2,
        "Different tool_use_ids should both be preserved. Found {}",
        tool_results.len()
    );
}

// ============================================================================
// PR 1: WITHIN-TRACE ADK SUPPORT TESTS
// ============================================================================

/// In an ADK multi-span trace, ordered assistant input preserves earlier turns and output survives.
///
/// Agent root + 2 generation children.
/// span1 input (llm_request): [sys, userA, asstB_old, userC]
/// span1 output (gen_ai.choice): [toolD(tool_use)]
/// span2 input (llm_request): [sys, userA, asstB_old, userC, toolD, resultE]
/// span2 output (gen_ai.choice): [asstG(stop)]
///
/// Key assertion: the ordered request snapshot proves that `asstB_old` is a distinct earlier turn.
/// Protected gen_ai.choice output (toolD, asstG) survives and duplicate copies collapse.
#[test]
fn adk_multi_span_preserves_ordered_input_history_and_deduplicates() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    let span1_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "Previous answer from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Now do something"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "calculator", "input": {"op": "add"}}]
            }
        }
    ]);

    let span1 = make_span_row_full(
        "trace1",
        "gen_span1",
        Some("agent_root"),
        &span1_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let span2_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "assistant", "content": "Previous answer from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "user", "content": "Now do something"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "calculator", "input": {"op": "add"}}]
            }
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "4"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:09Z"}},
            "content": {
                "role": "assistant",
                "content": "The answer is 4",
                "finish_reason": "stop"
            }
        }
    ]);

    let span2 = make_span_row_full(
        "trace1",
        "gen_span2",
        Some("agent_root"),
        &span2_msgs.to_string(),
        t0 + dur(6),
        Some(t0 + dur(9)),
        Some("generation"),
    );

    let rows = vec![agent_root, span1, span2];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // ADK's ordered request snapshot is a conversation witness, so its earlier assistant turn
    // remains visible. Repeated snapshots still collapse it to one block.
    let old_assistant_count = result
        .messages
        .iter()
        .filter(|m| {
            matches!(&m.content, ContentBlock::Text { text, .. } if text == "Previous answer from history")
        })
        .count();
    assert_eq!(
        old_assistant_count, 1,
        "ordered input history should preserve one copy of the earlier assistant turn"
    );

    // toolD should survive (from span1 output via gen_ai.choice, protected)
    let tool_uses: Vec<_> = result.messages.iter().filter(|m| m.is_tool_use()).collect();
    assert_eq!(
        tool_uses.len(),
        1,
        "toolD should survive from output source"
    );

    // Final answer should be present (protected by gen_ai.choice + finish_reason)
    let final_answer = result.messages.iter().any(
        |m| matches!(&m.content, ContentBlock::Text { text, .. } if text == "The answer is 4"),
    );
    assert!(final_answer, "Final assistant answer should be present");

    // Verify no empty result
    assert!(
        result.messages.len() >= 3,
        "At minimum: toolD, asstG, plus user context. Got {} blocks",
        result.messages.len()
    );
}

/// Input-attribute history detection does not affect event-based frameworks such as Strands.
/// Events have their own protection mechanism (gen_ai.choice is protected).
#[test]
fn input_attribute_history_does_not_affect_strands_events() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    // Strands pattern: events on generation span
    let gen_msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {
                "role": "assistant",
                "content": "Hi there!",
                "finish_reason": "stop"
            }
        }
    ]);

    let gen_span = make_span_row_full(
        "trace1",
        "gen_span",
        Some("agent_root"),
        &gen_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(3)),
        Some("generation"),
    );

    let rows = vec![agent_root, gen_span];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // gen_ai.choice assistant text is protected and remains current.
    let assistant_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        assistant_blocks.len(),
        1,
        "Strands gen_ai.choice should survive (protected)"
    );
}

/// An ordered request snapshot preserves its earlier assistant turn and the current response.
#[test]
fn ordered_input_assistant_and_output_source_assistant_both_survive() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    // Non-root generation span with both input and output assistant text
    let gen_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "Old response from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": "New response from this turn",
                "finish_reason": "stop"
            }
        }
    ]);

    let gen_span = make_span_row_full(
        "trace1",
        "gen_span",
        Some("agent_root"),
        &gen_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let rows = vec![agent_root, gen_span];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The old response is a distinct earlier turn in ADK's ordered request snapshot.
    let old = result.messages.iter().any(|m| {
        matches!(&m.content, ContentBlock::Text { text, .. } if text == "Old response from history")
    });
    assert!(
        old,
        "ordered input history should preserve the earlier assistant turn"
    );

    // "New response" should survive (output-source, has finish_reason → protected)
    let new = result.messages.iter().any(|m| {
        matches!(&m.content, ContentBlock::Text { text, .. } if text == "New response from this turn")
    });
    assert!(
        new,
        "Output-source assistant with finish_reason should survive"
    );
}

/// An input-source tool call is history, so the output-source copy wins deduplication.
#[test]
fn test_tool_use_input_vs_output_source_quality() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(20)),
        Some("agent"),
    );

    // span1 output: gen_ai.choice with tool_use
    let span1_msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Do something"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_abc", "name": "my_tool", "input": {"x": 1}}]
            }
        }
    ]);

    let span1 = make_span_row_full(
        "trace1",
        "gen_span1",
        Some("agent_root"),
        &span1_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(3)),
        Some("generation"),
    );

    // span2 input: llm_request re-sends the tool_use as history
    let span2_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_abc", "name": "my_tool", "input": {"x": 1}}]
            }
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "tool", "tool_use_id": "call_abc", "content": "result_val"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:09Z"}},
            "content": {
                "role": "assistant",
                "content": "Done!",
                "finish_reason": "stop"
            }
        }
    ]);

    let span2 = make_span_row_full(
        "trace1",
        "gen_span2",
        Some("agent_root"),
        &span2_msgs.to_string(),
        t0 + dur(6),
        Some(t0 + dur(9)),
        Some("generation"),
    );

    let rows = vec![agent_root, span1, span2];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // tool_use should survive (from span1's gen_ai.choice, protected)
    let tool_uses: Vec<_> = result.messages.iter().filter(|m| m.is_tool_use()).collect();
    assert_eq!(tool_uses.len(), 1, "Exactly one tool_use should survive");

    // The surviving tool_use should be output-sourced (from gen_ai.choice event)
    assert!(
        tool_uses[0].is_output_event() || tool_uses[0].is_protected(),
        "Surviving tool_use should be from output/protected source"
    );
}

/// is_input_source and is_output_source classification propagates through pipeline.
/// Verify that blocks from ADK llm_request get is_input_source() = true.
#[test]
fn test_source_classification_propagates_to_blocks() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "input msg"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": "output msg",
                "finish_reason": "stop"
            }
        }
    ]);

    let row = make_span_row_full(
        "trace1",
        "span1",
        None, // root
        &msgs.to_string(),
        t0,
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 2);

    let user_block = &result.messages[0];
    assert_eq!(user_block.role, ChatRole::User);
    assert!(
        user_block.is_input_source(),
        "User block from llm_request should be input-source. source_attribute={:?}",
        user_block.source_attribute
    );

    let asst_block = &result.messages[1];
    assert_eq!(asst_block.role, ChatRole::Assistant);
    assert!(
        asst_block.is_output_source(),
        "Assistant block from llm_response should be output-source. source_attribute={:?}",
        asst_block.source_attribute
    );
}
