// ----------------------------------------------------------------------------
// Test: accumulated history matching allows gaps (subsequence, not strict)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_subsequence_match() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace1 accumulated non-system sequence includes an output block ("B")
    // that won't be replayed in trace2 input prefix.
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "B"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D1"}
        }
    ]);

    // Trace2 replays A and C as history, but "B" is absent from prefix.
    // Strict matching would stop at C; subsequence matching should still strip C.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "E"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "F"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D2"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2], &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"E"),
        "New content should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"F"),
        "New content should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"D2"),
        "New assistant response should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"A"),
        "History A should be stripped. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"C"),
        "History C should be stripped even with accumulated gap. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: cross-trace prefix scan applies per span (not only trace start)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_resets_per_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Prior trace contributes A/B to accumulated history.
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "B"}
        }
    ]);

    // Target trace span1 starts with new content C.
    let trace2_span1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D"}
        }
    ]);

    // Target trace span2 replays A/C, then adds E.
    // A should be stripped via cross-trace prefix even though it appears
    // in the second span, not at trace start.
    let trace2_span2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "E"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "F"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &trace1_msg.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &trace2_span1.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace2",
        "s3",
        None,
        &trace2_span2.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2, row3], &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"C"),
        "New C should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"E"),
        "New E should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"F"),
        "Final F should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"A"),
        "Cross-trace replay A should be stripped in span2 prefix. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Replay trace contributes 0 tool defs
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_replay_no_tool_defs() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Use the tool"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Done"}
        }
    ]);

    let tool_defs =
        json!([{"type": "function", "function": {"name": "my_tool", "parameters": {}}}])
            .to_string();

    // Trace2 is pure replay
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Use the tool"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Done"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    row1.tool_definitions_json = tool_defs.clone();

    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    row2.tool_definitions_json = tool_defs;

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2], &options);

    // Both traces contribute (guard prevents marking for pure replay).
    // Tool defs are deduplicated by name, so still 1 unique tool.
    assert_eq!(
        result.tool_definitions.len(),
        1,
        "Should have exactly 1 unique tool def after dedup. Got {}",
        result.tool_definitions.len()
    );
}

// ----------------------------------------------------------------------------
// Test: Repeated content AFTER prefix break is safe
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_repeated_content_safe() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: user("yes") + asst("confirmed")
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "confirmed"}
        }
    ]);

    // Trace 2: different prefix + user("yes") after break
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "new question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "done"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace2 prefix: "new question" NOT in accumulated → STOP immediately → 0 stripped
    // So trace2 keeps: user("new question") + user("yes") + asst("done")
    let yes_count = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "yes"))
        .count();
    assert!(
        yes_count >= 2,
        "Both 'yes' should be preserved (different contexts). Found {}",
        yes_count
    );
}

// ----------------------------------------------------------------------------
// Test: cross-trace prefix matching is role-sensitive
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_role_sensitive() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: assistant says "yes"
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Question 1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    // Trace 2: user says "yes" as new input (same content, different role)
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Acknowledged"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"yes"),
        "User 'yes' in trace2 must not be stripped by assistant 'yes' from trace1. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Acknowledged"),
        "Trace2 assistant output should remain. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: mixed event+attribute duplicates keep event copy in target trace
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_mixed_source_event_survives() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1 contributes "repeat" to accumulated history.
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "first"}
        }
    ]);

    // Trace 2 has the same user text from both event and attribute sources.
    // Cross-trace prefix must only mark the attribute copy as history.
    let msg2 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "fresh"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let repeat_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "repeat"))
        .collect();

    assert_eq!(
        repeat_blocks.len(),
        1,
        "Exactly one 'repeat' should survive in trace2 after dedup. Got {:?}",
        result
            .messages
            .iter()
            .filter_map(|b| match &b.content {
                ContentBlock::Text { text } => Some((text.as_str(), b.source_type.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        repeat_blocks[0].source_type,
        source_type::EVENT,
        "Event-sourced user message should win over attribute history copy"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "fresh")),
        "New assistant output should remain"
    );
}

// ----------------------------------------------------------------------------
// Test: repeated matches are bounded by accumulated occurrence count
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_occurrence_count_bounded() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Trace 1 contributes one "ping".
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a1"}
        }
    ]);

    // Trace 2 contributes a second "ping" after a prefix break.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "barrier"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a2"}
        }
    ]);

    // Trace 3 starts with three "ping" entries.
    // Only first two should be stripped (bounded by accumulated count = 2).
    let msg3 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "tail"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a3"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace3",
        "s3",
        None,
        &msg3.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2, row3], &options);
    result.messages.retain(|b| b.trace_id == "trace3");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    let ping_count = texts.iter().filter(|&&t| t == "ping").count();
    assert_eq!(
        ping_count, 1,
        "Only one non-history 'ping' should remain in trace3. Got {:?}",
        texts
    );
    assert!(
        texts.contains(&"tail"),
        "Content after prefix break should remain. Got {:?}",
        texts
    );
    assert!(
        texts.contains(&"a3"),
        "New assistant output should remain. Got {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Strands traces with "yes" in both (different turns) → both preserved
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_strands_repeated_yes() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Strands: unique events per trace
    let msg1 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Do you agree?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Confirm again?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // "Do you agree?" not in accumulated → STOP immediately → 0 stripped from trace2
    assert_eq!(
        result.messages.len(),
        4,
        "Strands: all 4 blocks preserved. Got {}",
        result.messages.len()
    );

    let yes_count = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "yes"))
        .count();
    assert_eq!(yes_count, 2, "Both 'yes' should be preserved");
}

// ----------------------------------------------------------------------------
// Test: Multi-trace detection routing check
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_multi_trace_detection() {
    let t0 = fixed_time();

    // Single trace: should go through process_trace_spans path
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);

    let single_row =
        make_span_row_with_timestamps("trace1", "s1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();

    // Single trace
    let r1 = process_spans(vec![single_row.clone()], &options);
    let r2 = process_trace_spans(vec![single_row], &options);
    assert_eq!(r1.messages.len(), r2.messages.len());

    // Two traces with same content
    let t1 = t0 + chrono::Duration::seconds(10);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Different"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let r3 = process_spans(rows, &options);
    // Multi-trace path: both unique → both preserved
    assert_eq!(r3.messages.len(), 2);
}
