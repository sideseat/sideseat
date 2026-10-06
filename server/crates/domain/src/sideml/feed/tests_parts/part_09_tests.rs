// ============================================================================
// CROSS-TRACE SESSION DEDUP (PREFIX STRIP ENGINE)
// ============================================================================

// ----------------------------------------------------------------------------
// Test: 3 ADK traces with growing prefix → only new content from each
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_accumulated_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Trace 1: user("NYC weather") → assistant("NYC sunny")
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        }
    ]);

    // Trace 2: user("NYC weather") + assistant("NYC sunny") [history] + user("London") → assistant("London rain")
    let trace2_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "London weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "London rain"}
        }
    ]);

    // Trace 3: all history + user("Tokyo") → assistant("Tokyo cloudy")
    let trace3_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "London weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "London rain"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Tokyo weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Tokyo cloudy"}
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
        &trace2_msg.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace3",
        "s3",
        None,
        &trace3_msg.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2, row3], &options);

    // Within each trace, assistant blocks from llm_request are input history.
    // Cross-trace prefix strip: removes user/tool blocks already seen in prior traces.
    // Trace 1: user("NYC") + asst("NYC sunny") from llm_response = 2
    // Trace 2: prefix [user("NYC")] stripped, asst("NYC sunny") filtered as input history
    //   → user("London") + asst("London rain") = 2
    // Trace 3: prefix [user("NYC"), user("London")] stripped, assistants filtered as input history
    //   → user("Tokyo") + asst("Tokyo cloudy") = 2
    // Total: 6
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        result.messages.len(),
        6,
        "Expected 6 blocks (2 per trace after prefix and input-history filtering). Got {} blocks: {:?}",
        result.messages.len(),
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Single trace is unchanged
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_single_trace_unchanged() {
    let t0 = fixed_time();

    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));

    let options = FeedOptions::default();
    let result_via_process = process_spans(vec![row.clone()], &options);
    let result_via_trace = process_trace_spans(vec![row], &options);

    assert_eq!(
        result_via_process.messages.len(),
        result_via_trace.messages.len(),
        "Single trace: process_spans should match process_trace_spans"
    );
}

// ----------------------------------------------------------------------------
// Test: Two traces with no overlapping content → 0 stripped
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_no_overlap() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Goodbye"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(
        result.messages.len(),
        2,
        "No overlap: both blocks should be preserved"
    );
}

// ----------------------------------------------------------------------------
// Test: Strands JS bundled gen_ai.input.messages — cross-trace history stripped
// ----------------------------------------------------------------------------
//
// Strands JS uses @opentelemetry/instrumentation-aws-sdk, which bundles all
// messages into a single gen_ai.input.messages event (shared timestamp). This
// means timestamp comparison cannot detect cross-trace history. The cross-
// trace prefix mechanism must handle it instead.

#[test]
fn test_cross_trace_strands_js_bundled_messages_deduped() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(60);

    // Trace 1: user1 → assistant1 (single turn)
    // Note: content values must be non-numeric strings to avoid normalize_content treating
    // them as JSON numbers (which produces empty content and drops the block).
    let trace1_msgs = json!([
        {
            // gen_ai.input.messages: just the user message (no history yet)
            "source": {"event": {"name": "gen_ai.input.messages", "time": t0.to_rfc3339()}},
            "content": [
                {"role": "user", "content": "What is 2+2?"}
            ]
        },
        {
            // gen_ai.choice: assistant response
            "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Two plus two is four"}
        }
    ]);

    // Trace 2: user1 + assistant1 replayed + new user2 → assistant2
    let trace2_msgs = json!([
        {
            // gen_ai.input.messages: includes full history from trace 1 + new user message
            // All share the same event timestamp, so timestamp comparison cannot help.
            "source": {"event": {"name": "gen_ai.input.messages", "time": t1.to_rfc3339()}},
            "content": [
                {"role": "user", "content": "What is 2+2?"},                  // replayed from trace 1
                {"role": "assistant", "content": "Two plus two is four"},      // replayed from trace 1
                {"role": "user", "content": "And 3+3?"}                       // new user message
            ]
        },
        {
            // gen_ai.choice: new assistant response
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Three plus three is six"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &trace1_msgs.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &trace2_msgs.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Expected: user1, assistant1, user2, assistant2 — no duplicates
    assert_eq!(
        result.messages.len(),
        4,
        "Should have 4 unique messages (cross-trace history stripped). Got: {}",
        result.messages.len()
    );

    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|b| {
            if let crate::sideml::types::ContentBlock::Text { text } = &b.content {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(texts.contains(&"What is 2+2?"), "user1 should appear once");
    assert!(
        texts.contains(&"Two plus two is four"),
        "assistant1 should appear once"
    );
    assert!(texts.contains(&"And 3+3?"), "user2 should appear");
    assert!(
        texts.contains(&"Three plus three is six"),
        "assistant2 should appear"
    );
}

// ----------------------------------------------------------------------------
// Test: Event-based (Strands Python) per-message events stay trace-independent
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_strands_independent() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Strands: unique event per trace, different content
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Question 1"}
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Answer 1"}
    }]);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Question 2"}
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Answer 2"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(
        result.messages.len(),
        4,
        "Strands: unique events per trace, all preserved. Got {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// Test: Pure replay trace → 0 new blocks
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_replay_fully_deduped() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "4"}
        }
    ]);

    // Trace2 replays identical content (re-execution)
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "4"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg.to_string(),
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

    // The re-sent request is history and is stripped; what the model produced again is a new answer.
    // This once asserted that trace2 contributed nothing, which held only because an answer of "4"
    // parsed as a number and was dropped - the same defect that lost a tool returning `395.0`.
    let trace1_count = result
        .messages
        .iter()
        .filter(|b| b.trace_id == "trace1")
        .count();
    let trace2: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.trace_id == "trace2")
        .map(|b| (b.role, b.entry_type.as_str()))
        .collect();
    assert_eq!(trace1_count, 2, "trace1 holds the question and the answer");
    assert_eq!(
        trace2,
        [(ChatRole::Assistant, "text")],
        "trace2 re-sent the question and answered again"
    );
}

// ----------------------------------------------------------------------------
// Test: System messages preserved per contributing trace
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_system_per_trace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        }
    ]);

    // Trace2: same system + history prefix + new content
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Thanks"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Welcome"}
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

    // Trace1: system + user + asst = 3
    // Trace2: system (preserved) + user("Thanks") + asst("Welcome") = 3
    // Input-source assistant content is marked as history within trace2,
    // so "Hi" from llm_request is already filtered by within-trace pipeline
    let system_count = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::System)
        .count();
    assert!(
        system_count >= 1,
        "At least one system message should be preserved. Got {}",
        system_count
    );

    // Check that new content from trace2 is present
    let has_thanks = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "Thanks"));
    assert!(has_thanks, "user('Thanks') should be present from trace2");

    let has_welcome = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "Welcome"));
    assert!(has_welcome, "asst('Welcome') should be present from trace2");
}

// ----------------------------------------------------------------------------
// ADK multi-span trace in a session
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_adk_multi_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: single generation span
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    // Trace 2: history + new question
    let trace2_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "How are you?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "I am fine"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &trace1_msg.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &trace2_msg.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace 1: user("Hello") + asst("Hi there") = 2
    // Trace 2: prefix [user("Hello")] stripped (asst "Hi there" already filtered by 4b)
    //   → user("How are you?") + asst("I am fine") = 2
    // Total: 4
    let has_how = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "How are you?"));
    assert!(has_how, "user('How are you?') from trace2 should survive");

    let has_fine = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "I am fine"));
    assert!(has_fine, "asst('I am fine') from trace2 should survive");
}

// ----------------------------------------------------------------------------
// Test: retain by trace_id simulates the trace endpoint view
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_retain_trace_view() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
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

    // Simulate trace endpoint: retain only trace2 blocks
    result.messages.retain(|b| b.trace_id == "trace2");

    // After prefix strip + retain: only trace2's NEW content
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"Second question"),
        "Should have 'Second question'. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Second answer"),
        "Should have 'Second answer'. Got: {:?}",
        texts
    );
    // History should NOT be present
    assert!(
        !texts.contains(&"First question"),
        "Should NOT have 'First question' (history). Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: same-timestamp traces keep first-seen order for prefix strip
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_same_timestamp_trace_ordering() {
    let t0 = fixed_time();

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
        }
    ]);

    // Same timestamps and reverse lexical trace IDs: ordering must follow first-seen row
    // order (trace-z first), not trace_id sort (trace-a first).
    let mut row1 = make_span_row_full(
        "trace-z-older",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace-a-newer",
        "s2",
        None,
        &msg2.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2], &options);

    // Simulate trace endpoint retain-by-trace behavior
    result.messages.retain(|b| b.trace_id == "trace-a-newer");
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"Second question"),
        "Expected 'Second question' in target trace. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Second answer"),
        "Expected 'Second answer' in target trace. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"First question"),
        "History prefix should be stripped from target trace. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"First answer"),
        "History prefix should be stripped from target trace. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: system blocks in prefix are transparent (do not break scan)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_system_prefix_transparent() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Q1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A1"}
        }
    ]);

    // Trace2 re-sends history (system + Q1 + A1 in llm_request) then adds new turn.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Q1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Q2"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A2"}
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
        texts.contains(&"Q2"),
        "New user turn should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"A2"),
        "New assistant output should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"Q1"),
        "History user message should be stripped despite leading system block. Got: {:?}",
        texts
    );
}
