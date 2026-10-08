// ----------------------------------------------------------------------------
// Test: Genuine repeated user message preserved (the reported bug)
// User asks the same question in trace 2 as in trace 1.
// The history re-send copy should be stripped but the genuine copy preserved.
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_genuine_repeat_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: user("Hello") → assistant("Hi")
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    // Trace 2: history re-send [user("Hello"), asst("Hi")] + genuine repeat user("Hello")
    // Framework re-sends full history as prefix of llm_request, then adds new message.
    // The new message happens to be "Hello" again (user asks the same question).
    let msg2 = json!([
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
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hello again!"}
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

    // Trace 1: user("Hello") + asst("Hi there") = 2
    // Trace 2: Cross-trace prefix marks first user("Hello") and asst("Hi there") as history.
    //   Within-trace dedup: user("Hello") has history copy + genuine copy → non-history wins.
    //   asst("Hi there") from llm_request is history-only → dropped.
    //   Result: user("Hello") + asst("Hello again!") = 2
    // Total: 4
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    // The genuine "Hello" from trace 2 MUST be preserved
    let hello_count = texts.iter().filter(|&&t| t == "Hello").count();
    assert_eq!(
        hello_count, 2,
        "Both 'Hello' messages (trace1 + trace2 genuine) must be preserved. Got: {:?}",
        texts
    );

    // The new response from trace 2 MUST be present
    assert!(
        texts.contains(&"Hello again!"),
        "asst('Hello again!') from trace2 must be preserved. Got: {:?}",
        texts
    );

    // History re-send of "Hi there" from trace2's llm_request should be dropped
    let hi_count = texts.iter().filter(|&&t| t == "Hi there").count();
    assert_eq!(
        hi_count, 1,
        "Only trace1's 'Hi there' should survive (trace2's is history). Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Trace endpoint view with genuine repeated content
// Simulates the exact bug scenario: viewing a single trace via the trace endpoint
// where the trace has messages with same content as prior traces.
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_retain_genuine_repeat_trace_view() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        }
    ]);

    // Trace 2: re-sends history + user says "Hello" again
    let msg2 = json!([
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
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi again"}
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

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    // The genuine "Hello" from trace2 MUST be present (this was the reported bug)
    assert!(
        texts.contains(&"Hello"),
        "Genuine 'Hello' from trace2 must be preserved in trace view. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Hi again"),
        "New response 'Hi again' from trace2 must be present. Got: {:?}",
        texts
    );
}

// ============================================================================
// LOGFIRE / OPENAI AGENTS: ASSISTANT PROMOTION IN CHOICELESS GENERATION SPANS
// ============================================================================
// Logfire stores LLM output as gen_ai.assistant.message (not gen_ai.choice).
// Without promotion, assistant messages sort by array index alongside inputs,
// causing incorrect ordering (system=0, assistant=1, user=2 → assistant before user).

#[test]
fn test_logfire_assistant_promoted_when_no_choice() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(2);

    // Generation span with parent, no gen_ai.choice — only gen_ai.*.message events
    let msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful."}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Here is the answer."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen-span",
        Some("parent-span"),
        &msgs.to_string(),
        t0,
        Some(t1),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect roles in order
    let roles: Vec<ChatRole> = result.messages.iter().map(|b| b.role).collect();

    // Assistant should come AFTER user (promoted to span_end timestamp)
    assert_eq!(
        roles,
        vec![ChatRole::System, ChatRole::User, ChatRole::Assistant],
        "Expected system -> user -> assistant, got {:?}",
        roles
    );

    // Verify the promoted assistant block has correct flags
    let assistant_block = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant)
        .expect("Should have assistant block");
    assert!(
        assistant_block.uses_span_end,
        "Promoted assistant should use span_end"
    );
    assert!(
        assistant_block.is_protected(),
        "Promoted assistant should be protected from history marking"
    );
}

#[test]
fn choiceless_generation_promotes_only_the_terminal_assistant_suffix() {
    let t0 = fixed_time();
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Search twice."}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call-1",
                    "name": "search",
                    "input": {"query": "same"}
                }]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": "first"
                }]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call-2",
                    "name": "search",
                    "input": {"query": "same"}
                }]
            }
        }
    ]);
    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "generation",
        Some("agent"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::seconds(1)),
    )];

    let blocks = classified_blocks_for_test(rows);
    let calls: Vec<_> = blocks.iter().filter(|block| block.is_tool_use()).collect();
    assert_eq!(calls.len(), 2);
    assert!(
        !calls[0].is_output_source(),
        "the call before a tool result is replayed request history"
    );
    assert!(
        calls[1].is_output_source(),
        "the terminal assistant call is this generation's output"
    );
    assert!(
        !calls[1].is_history,
        "a generation's own output must not be filtered as replayed history"
    );
    assert!(
        !calls[1].uses_span_end,
        "tool decisions retain event-time ordering"
    );
}

/// Logfire OpenAI 6.x emits an input-only lifetime span beside the completed streaming log.
///
/// That span is raw telemetry and remains stored, but it is not another conversation. The feed
/// projection must withdraw it narrowly: older producer shapes and errors still expose their input.
#[test]
fn logfire_openai_streaming_transport_wrapper_is_not_a_conversation() {
    let timestamp = fixed_time();
    let input = json!([
        {
            "source": {
                "attribute": {
                    "key": "gen_ai.input.messages",
                    "time": timestamp.to_rfc3339()
                }
            },
            "content": [
                {
                    "role": "system",
                    "parts": [{"type": "text", "content": "Answer briefly."}]
                },
                {
                    "role": "user",
                    "parts": [{"type": "text", "content": "What is boiling?"}]
                }
            ]
        }
    ])
    .to_string();

    let mut wrapper = make_span_row_with_timestamps(
        "stream-trace",
        "request-wrapper",
        None,
        &input,
        timestamp,
        Some(timestamp + chrono::Duration::seconds(1)),
    );
    wrapper.scope_name = Some("logfire.openai".to_string());
    wrapper.scope_version = Some("6.0.0b7".to_string());
    wrapper.span_name = Some("Chat Completion with 'gpt-5-nano'".to_string());

    let raw_before = wrapper.messages_json.clone();
    let result = process_spans(vec![wrapper.clone()], &FeedOptions::default());
    assert!(
        result.messages.is_empty(),
        "the transport wrapper must not become an incomplete conversation"
    );
    assert_eq!(
        wrapper.messages_json, raw_before,
        "read-time suppression must not mutate stored telemetry"
    );

    wrapper.scope_version = Some("5.9.9".to_string());
    let older = process_spans(vec![wrapper.clone()], &FeedOptions::default());
    assert_eq!(
        older.messages.len(),
        2,
        "an older input-only Logfire span is still a legitimate request view"
    );

    wrapper.scope_version = Some("6.0.0b7".to_string());
    wrapper.status_code = Some(status::ERROR.to_string());
    wrapper.exception_type = Some("openai.APIError".to_string());
    wrapper.exception_message = Some("stream failed".to_string());
    let failed = process_spans(vec![wrapper], &FeedOptions::default());
    assert!(
        failed
            .messages
            .iter()
            .any(|message| message.role == ChatRole::User),
        "a failed stream must retain its request context"
    );
}

#[test]
fn test_no_promotion_when_choice_exists() {
    // Verify promotion is suppressed when gen_ai.choice is present.
    // Uses classify_blocks directly to test the classification logic
    // without dedup/history phases interfering.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(2);

    let span_timestamps = std::collections::HashMap::from([(
        "gen-span".to_string(),
        super::dedup::SpanTimestamps {
            span_start: t0,
            span_end: Some(t1),
        },
    )]);

    // Build blocks manually: gen_ai.assistant.message + gen_ai.choice in same gen span
    let assistant_block = BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: "Previous response.".to_string(),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: "gen-span".to_string(),
        session_id: None,
        message_index: 1,
        entry_index: 0,
        parent_span_id: Some("parent-span".to_string()),
        span_path: vec!["parent-span".to_string(), "gen-span".to_string()],
        timestamp: t0,
        order_time: t0,
        occurrence_ordinal: 0,
        observation_type: Some("generation".to_string()),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        name: None,
        finish_reason: None,
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: Some("gen_ai.assistant.message".to_string()),
        source_attribute: None,
        category: sideseat_ports::types::MessageCategory::GenAIAssistantMessage,
        content_hash: "hash_prev".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
        is_rendering: false,
    };

    let choice_block = BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: "4".to_string(),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: "gen-span".to_string(),
        session_id: None,
        message_index: 3,
        entry_index: 0,
        parent_span_id: Some("parent-span".to_string()),
        span_path: vec!["parent-span".to_string(), "gen-span".to_string()],
        timestamp: t1,
        order_time: t1,
        occurrence_ordinal: 0,
        observation_type: Some("generation".to_string()),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        name: None,
        finish_reason: Some(FinishReason::Stop),
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: Some("gen_ai.choice".to_string()),
        source_attribute: None,
        category: sideseat_ports::types::MessageCategory::GenAIChoice,
        content_hash: "hash_4".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
        is_rendering: false,
    };

    let mut blocks = vec![assistant_block, choice_block];
    super::classify_blocks(&mut blocks, &span_timestamps);

    // gen_ai.choice should be classified normally (uses_span_end from is_protected)
    let choice = &blocks[1];
    assert!(choice.is_protected(), "gen_ai.choice should be protected");
    assert!(choice.uses_span_end, "gen_ai.choice should use span_end");

    // gen_ai.assistant.message should NOT be promoted (choice exists in this span)
    let asst = &blocks[0];
    assert_eq!(
        asst.category,
        sideseat_ports::types::MessageCategory::GenAIAssistantMessage,
        "gen_ai.assistant.message should keep original category when choice exists"
    );
    assert!(
        !asst.uses_span_end,
        "gen_ai.assistant.message should NOT use span_end when choice exists"
    );
}

// ============================================================================
// Role filter
// ============================================================================

/// The Gemini/ADK shape: a tool result arrives inside a message whose raw role is `user`, and
/// the pipeline derives the block's role as `tool` from its content.
fn gemini_tool_result_row() -> MessageSpanRow {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "user",
            "content": [
                {"functionResponse": {"name": "get_weather", "response": {"temp": 25}}}
            ]
        }
    }]);
    make_span_row(
        "trace-role",
        "span-role",
        None,
        &msg.to_string(),
        "[]",
        "[]",
    )
}

/// `?role=tool` must return a Gemini tool result.
///
/// The filter used to be applied to the raw message role before the per-block role was
/// derived, so `role=tool` dropped every Gemini and ADK tool result (raw role `user`) and
/// `role=user` returned blocks whose role is `tool`.
#[test]
fn role_filter_matches_the_derived_role_not_the_raw_one() {
    let rows = vec![gemini_tool_result_row()];

    let unfiltered = process_spans(rows.clone(), &FeedOptions::new());
    let derived: Vec<&str> = unfiltered
        .messages
        .iter()
        .map(|b| b.role.as_str())
        .collect();
    assert_eq!(
        derived,
        vec!["tool"],
        "precondition: the block's derived role is tool"
    );

    let as_tool = process_spans(
        rows.clone(),
        &FeedOptions::new().with_role(Some("tool".into())),
    );
    assert_eq!(
        as_tool.messages.len(),
        1,
        "role=tool must return the tool result; the raw message role is user"
    );

    let as_user = process_spans(rows, &FeedOptions::new().with_role(Some("user".into())));
    assert!(
        as_user.messages.is_empty(),
        "role=user must not return a block whose derived role is tool, got {:?}",
        as_user
            .messages
            .iter()
            .map(|b| b.role.as_str())
            .collect::<Vec<_>>()
    );
}

/// A time window must not change which messages are *history*.
///
/// The lower bound used to be passed to the message query, which removed the earlier spans that
/// history detection reads. With nothing to recognise a re-send against, the re-sent turns came
/// back as new messages - so narrowing a window could *increase* what a trace appeared to contain.
/// The window is a filter on the answer, applied after the pipeline has seen the context.
#[test]
fn a_time_window_only_removes_messages() {
    let earlier = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "first question"}
    }]);
    // The later span re-sends the first turn, as every framework does, plus a new one.
    let later = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "first question"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:05Z"}},
            "content": {"role": "user", "content": "second question"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "span1",
            None,
            &earlier.to_string(),
            fixed_time(),
            Some(fixed_time() + chrono::Duration::seconds(1)),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            None,
            &later.to_string(),
            fixed_time() + chrono::Duration::seconds(5),
            Some(fixed_time() + chrono::Duration::seconds(6)),
        ),
    ];

    let full = process_spans(rows.clone(), &FeedOptions::new());
    let unwindowed = process_spans(rows, &FeedOptions::new());
    let windowed = apply_time_window(
        &unwindowed,
        Some(fixed_time() + chrono::Duration::seconds(2)),
        None,
    );

    assert!(
        windowed.messages.len() <= full.messages.len(),
        "a window returned more messages ({}) than the unwindowed feed ({})",
        windowed.messages.len(),
        full.messages.len()
    );
    assert_eq!(
        windowed.metadata.block_count,
        windowed.messages.len(),
        "the window reported a count that does not match what it returned"
    );
    for block in &windowed.messages {
        assert!(
            block.timestamp >= fixed_time() + chrono::Duration::seconds(2),
            "a block outside the window was returned: {}",
            block.timestamp
        );
    }
}

/// The project feed must not split a response.
///
/// `process_feed` recognises a response by its blocks sharing a timestamp. When each block carried
/// its own birth time, a response whose text was timestamped at span end and whose tool call was
/// timestamped at event time stopped being one response, and a block from another response could
/// land between them.
///
/// Note this endpoint is newest-first, so a later tool result appearing *before* the earlier call
/// it answers is correct here - the ordering to check is within a response, and that responses do
/// not interleave.
#[test]
fn the_feed_keeps_a_response_together() {
    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:01Z"}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "looking that up"},
                    {"type": "tool_use", "id": "call-1", "name": "lookup", "input": {"q": "a"}}
                ]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:02Z"}},
            "content": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": "lookup",
                             "content": "answer"}]
            }
        }
    ]);

    // The span ends after the tool result, so the text's birth time falls after it.
    let start = "2025-01-01T00:00:00Z"
        .parse::<chrono::DateTime<Utc>>()
        .expect("valid timestamp");
    let mut row = make_span_row_with_timestamps(
        "trace1",
        "span1",
        None,
        &msg.to_string(),
        start,
        Some(start + chrono::Duration::seconds(4)),
    );
    row.ingested_at = start;

    let feed = process_feed(vec![row], &FeedOptions::new());
    let kinds: Vec<&str> = feed
        .messages
        .iter()
        .map(|b| b.entry_type.as_str())
        .collect();

    // The response's two blocks are adjacent and in the order the model produced them.
    let text_at = kinds
        .iter()
        .position(|k| *k == "text")
        .expect("the response's text");
    let call_at = kinds
        .iter()
        .position(|k| *k == "tool_use")
        .expect("the response's tool call");
    assert_eq!(
        call_at,
        text_at + 1,
        "the response was split, or its blocks were reordered: {kinds:?}"
    );

    // And every block of one response carries one timestamp, which is what keeps it together.
    let response_times: std::collections::BTreeSet<_> = feed
        .messages
        .iter()
        .filter(|b| b.entry_type == "text" || b.entry_type == "tool_use")
        .map(|b| b.timestamp)
        .collect();
    assert_eq!(
        response_times.len(),
        1,
        "the response's blocks reported {} different times",
        response_times.len()
    );
}

/// A span's input and its output are different responses, even at the same timestamp.
///
/// Attribute extraction gives every message of a span the span's start time, so keying a response
/// on time alone made `input.value` and `output.value` one unit. The earlier time was then
/// materialised onto both, and the completed output was reported as having happened when the span
/// started - which a time window could drop, and which is simply the wrong time to show.
#[test]
fn a_spans_input_and_output_are_not_one_response() {
    let start = fixed_time();
    let end = start + chrono::Duration::seconds(30);
    let msg = json!([
        {
            "source": {"attribute": {"key": "input.value", "time": start.to_rfc3339()}},
            "content": {"role": "user", "content": "what is the capital of France?"}
        },
        {
            // Structured output, which is what carries a span-end timestamp: a plain text
            // output.value is not classified as a completion and keeps the span's start time.
            "source": {"attribute": {"key": "output.value", "time": start.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "json", "data": {"json": {"capital": "Paris"}}}]
            }
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), start, Some(end));

    let result = process_spans(vec![row], &FeedOptions::new());
    let output = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant)
        .expect("the span's output");
    let input = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::User)
        .expect("the span's input");

    assert_eq!(
        input.timestamp, start,
        "the input belongs at the span's start"
    );
    assert_eq!(
        output.timestamp, end,
        "the output completed when the span did, and must not inherit the input's time"
    );
}

/// The bundled OTel event pair must be recognised as input and output.
///
/// The current conventions carry a turn as `gen_ai.input.messages` and `gen_ai.output.messages` on
/// one `gen_ai.client.inference.operation.details` event, so both arrive at the same instant. With
/// only the older event names classified, the output was not recognised as output: it shared a
/// response with the input, took the input's timestamp, and was not protected from history marking.
#[test]
fn the_bundled_otel_event_pair_is_classified() {
    let start = fixed_time();
    let end = start + chrono::Duration::seconds(20);
    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.input.messages", "time": start.to_rfc3339()}},
            "content": {"role": "user", "content": "what is the capital of France?"}
        },
        {
            "source": {"event": {"name": "gen_ai.output.messages", "time": start.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Paris."}
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), start, Some(end));
    let result = process_spans(vec![row], &FeedOptions::new());

    let input = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::User)
        .expect("the turn's input");
    let output = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant)
        .expect("the turn's output");

    assert_eq!(
        input.timestamp, start,
        "the input is timestamped at the event"
    );
    assert_eq!(
        output.timestamp, end,
        "the output completed when the span did, and must not inherit the input's time"
    );
}

/// A tool result never precedes the call it answers, even when both spans report the same instant.
///
/// A message index restarts at zero in every span, so between spans it means nothing: a generation
/// span whose response opens with text gives its call index 1, the tool span carrying the result
/// starts at 0, and ordering the tie by index put the answer before the question.
///
/// Settling it *at comparison time* by role was tried three ways and each broke a framework - per
/// pair it is cyclic, per response it merges ADK's turns, per span it interleaves Vercel's parallel
/// calls with their results; all three are recorded in `dedup.rs`. Settling it beforehand does work:
/// the result takes its position from its own call, which is a property of the block rather than of
/// the pair, so the key stays a set of values and the order stays total. Only a cross-span tie is
/// adjusted, which is why the ADK and Vercel shapes are unaffected - their calls and results share a
/// span.
#[test]
fn a_cross_span_tie_keeps_a_result_after_its_call() {
    let t = fixed_time();
    // The generation span: introductory text, then the call. The call is index 1.
    let generation = json!([
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
            "content": {"role": "assistant", "content": "let me look that up"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call-1", "name": "lookup",
                             "input": {"q": "a"}}]
            }
        }
    ]);
    // The tool span: the result, index 0, at the same instant.
    let tool = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": "lookup",
                         "content": "answer"}]
        }
    }]);

    let mut tool_row = make_span_row_with_timestamps(
        "trace1",
        "span2",
        Some("span1"),
        &tool.to_string(),
        t,
        Some(t),
    );
    tool_row.observation_type = Some("tool".to_string());

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &generation.to_string(), t, Some(t)),
        tool_row,
    ];

    let result = process_spans(rows, &FeedOptions::new());
    let kinds: Vec<&str> = result
        .messages
        .iter()
        .map(|b| b.entry_type.as_str())
        .collect();
    assert_eq!(
        kinds,
        vec!["text", "tool_use", "tool_result"],
        "the result took its position from the call it answers, so it follows it"
    );
}

/// A span delivered twice must be billed once.
///
/// The DuckDB message query reads the raw span table, where a re-ingested span appears twice, while
/// ClickHouse reads it with FINAL. Summing rows therefore doubled a conversation's tokens and cost
/// on one backend, even though the messages themselves are deduplicated and appear once.
#[test]
fn a_span_delivered_twice_is_counted_once() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "hello"}
    }]);
    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");

    let once = process_spans(vec![row.clone()], &FeedOptions::new());
    let twice = process_spans(vec![row.clone(), row], &FeedOptions::new());

    assert_eq!(
        twice.messages.len(),
        once.messages.len(),
        "the duplicate delivery added a message"
    );
    assert_eq!(
        twice.metadata.total_tokens, once.metadata.total_tokens,
        "the duplicate delivery was billed twice"
    );
    assert_eq!(twice.metadata.total_cost, once.metadata.total_cost);
    assert_eq!(twice.metadata.span_count, 1);
}
