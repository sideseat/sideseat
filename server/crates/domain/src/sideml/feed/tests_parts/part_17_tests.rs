// ============================================================================
// Messages on both signals (`log_messages`)
// ============================================================================

/// A log record's copy of a span's message is that message, whenever it was stamped: matched one to one, so a
/// message the span holds twice and the records three times is three messages.
#[test]
fn a_log_record_s_copy_of_a_span_s_message_is_that_message() {
    let answer = |seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": {"role": "assistant", "content": "Red."}
        })
    };
    let answers = |span: Vec<serde_json::Value>, logs: Vec<serde_json::Value>| {
        let mut rows = streamed_call(span, None);
        rows[1].log_messages_json = serde_json::Value::Array(logs).to_string();
        let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
        assistant_texts(&process_span(rows, &FeedOptions::new())).len()
    };
    assert_eq!(answers(vec![answer(1)], vec![answer(2)]), 1);
    assert_eq!(
        answers(vec![answer(1), answer(3)], vec![answer(2), answer(4)]),
        2
    );
    assert_eq!(
        answers(
            vec![answer(1), answer(3)],
            vec![answer(2), answer(4), answer(5)]
        ),
        3
    );
}

/// A streamed response the records hold and the span does not keeps every piece: streams are matched segment by
/// segment, so a segment the span also holds is dropped whole and one only the records hold stays whole.
#[test]
fn a_stream_only_the_records_hold_stays_whole() {
    let mut rows = streamed_call(
        vec![
            streamed(json!("A"), "chunk", 3),
            streamed(json!("C"), "delta", 4),
        ],
        None,
    );
    rows[1].log_messages_json = json!([
        streamed(json!("A"), "chunk", 1),
        streamed(json!("B"), "delta", 2),
        streamed(json!("A"), "chunk", 3),
        streamed(json!("C"), "delta", 4),
    ])
    .to_string();
    assert_eq!(texts_of(rows), ["AB", "AC"]);
}

/// A span event and a log record of one message, stamped either side of the span's start: the copy kept takes
/// the later instant, which says the message was sent with this call.
#[test]
fn a_message_on_both_signals_keeps_the_later_instant() {
    let question = |seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.user.message", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": {"role": "user", "content": "Name a primary colour."}
        })
    };
    let answer = json!({
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:20Z"}},
        "content": {"role": "assistant", "content": "Red."}
    });
    let mut rows = streamed_call(vec![question(0), answer.clone()], Some("Red."));
    // The model call starts after the span event's instant and before the record's.
    rows[1].span_timestamp = fixed_time() + chrono::Duration::seconds(5);
    rows[1].log_messages_json = json!([question(10), answer]).to_string();
    let result = process_spans(rows, &FeedOptions::new());
    let user: Vec<&BlockEntry> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();
    assert_eq!(user.len(), 1, "{:?}", result.messages);
}

/// A span event bundling several messages and records holding one each are matched message by message.
#[test]
fn a_bundled_span_event_and_its_messages_records_are_one_copy() {
    let output = |content: serde_json::Value| {
        json!({
            "source": {"event": {"name": "gen_ai.output.messages", "time": "2025-01-01T00:00:02Z"}},
            "content": content
        })
    };
    let message = |text: &str| json!({"role": "assistant", "content": text});
    let mut rows = streamed_call(
        vec![output(json!([message("Red."), message("Blue.")]))],
        None,
    );
    rows[1].log_messages_json =
        json!([output(message("Red.")), output(message("Blue."))]).to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    assert_eq!(
        assistant_texts(&process_span(rows, &FeedOptions::new())),
        ["Red.", "Blue."]
    );
}

/// A stream's segment closes after the whole terminal reading, so a terminal bundling several messages is
/// matched whole: records whose terminal differs keep their own response intact.
#[test]
fn a_bundled_terminal_is_matched_whole() {
    let terminal = |texts: [&str; 2], seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.output.messages", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": texts.map(|text| json!({"role": "assistant", "content": text})),
            "stream": "delta"
        })
    };
    let mut rows = streamed_call(
        vec![streamed(json!("X"), "chunk", 1), terminal(["A", "B"], 2)],
        None,
    );
    rows[1].log_messages_json =
        json!([streamed(json!("X"), "chunk", 3), terminal(["A", "C"], 4)]).to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    let texts = assistant_texts(&process_span(rows, &FeedOptions::new()));
    assert_eq!(texts.concat(), "XABXAC", "{texts:?}");
}

/// A copy's instant moves only where the copies disagree on whether it was sent with this call, so distinct
/// messages keep their order.
#[test]
fn a_record_s_later_copy_does_not_reorder_the_span_s_messages() {
    let user = |text: &str, seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.user.message", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": {"role": "user", "content": text}
        })
    };
    let mut row = make_span_row(
        "trace1",
        "chat",
        None,
        &json!([user("A", 1), user("B", 2)]).to_string(),
        "[]",
        "[]",
    );
    row.log_messages_json = json!([user("A", 3)]).to_string();
    let result = process_spans(vec![row], &FeedOptions::new());
    let users: Vec<String> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["A", "B"]);
}

/// A stream the records matched only part of keeps the span's instants: its segments are ordered on one clock,
/// and moving one segment onto the records' would interleave it with the next.
#[test]
fn a_partially_matched_stream_keeps_the_span_s_clock() {
    let mut rows = streamed_call(
        vec![
            streamed(json!("A"), "chunk", 1),
            streamed(json!("B"), "delta", 6),
            streamed(json!("C"), "chunk", 8),
            streamed(json!("D"), "delta", 12),
        ],
        None,
    );
    rows[1].span_timestamp = fixed_time() + chrono::Duration::seconds(5);
    rows[1].log_messages_json = json!([
        streamed(json!("A"), "chunk", 10),
        streamed(json!("B"), "delta", 11),
    ])
    .to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    assert_eq!(
        assistant_texts(&process_span(rows, &FeedOptions::new())),
        ["AB", "CD"]
    );
}

/// A matched stream moves whole where its copies disagree on whether it was sent with this call, so its pieces
/// stay in the order they are joined in.
#[test]
fn a_matched_stream_moves_whole() {
    let mut rows = streamed_call(
        vec![
            streamed(json!("A"), "chunk", 1),
            streamed(json!("B"), "delta", 6),
        ],
        None,
    );
    rows[1].span_timestamp = fixed_time() + chrono::Duration::seconds(5);
    rows[1].log_messages_json = json!([
        streamed(json!("A"), "chunk", 10),
        streamed(json!("B"), "delta", 11),
    ])
    .to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    assert_eq!(
        assistant_texts(&process_span(rows, &FeedOptions::new())),
        ["AB"]
    );
}

/// Copies are one message whatever order a producer wrote their members in on each signal.
#[test]
fn copies_written_with_their_members_in_another_order_are_one_message() {
    let answer = |content: serde_json::Value| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:02Z"}},
            "content": content
        })
    };
    let mut rows = streamed_call(
        vec![answer(json!({"role": "assistant", "content": "Red."}))],
        None,
    );
    rows[1].log_messages_json =
        json!([answer(json!({"content": "Red.", "role": "assistant"}))]).to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    assert_eq!(
        assistant_texts(&process_span(rows, &FeedOptions::new())),
        ["Red."]
    );
}

/// A tool message ingest enriched with its span's tool name and call id, and the log record's copy of it that
/// never was, are one message: the span copy exceeds the record's only by the members enrichment writes. With a
/// text-valued input nothing downstream collapses the pair - the enriched copy is a call and the plain one a
/// text - so the record's copy showed as an extra assistant text beside the call.
#[test]
fn an_enriched_span_event_and_its_plain_record_are_one_message() {
    let event = |name: &str, seconds: u32, content: serde_json::Value| {
        json!({
            "source": {"event": {"name": name, "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": content
        })
    };
    // Text-valued and object-valued inputs: the span's copy as ingest enriched it, the record's as sent.
    for input in [json!("Milan"), json!({"city": "Milan"})] {
        let mut enriched =
            json!({"role": "tool_call", "tool_call_id": "c1", "name": "get_weather"});
        enriched["content"] = input.clone();
        let span_messages = json!([
            event("gen_ai.tool.message", 1, enriched),
            event(
                "gen_ai.choice",
                2,
                json!({"content": "Sunny", "tool_call_id": "c1"})
            )
        ]);
        let row = |logs: &str| {
            let mut row = make_span_row(
                "trace1",
                "tool",
                None,
                &span_messages.to_string(),
                "[]",
                "[]",
            );
            row.observation_type = Some("tool".to_string());
            row.log_messages_json = logs.to_string();
            row
        };
        let shown = |logs: &str| {
            process_span(vec![row(logs)], &FeedOptions::new())
                .messages
                .iter()
                .map(|b| {
                    (
                        b.role,
                        serde_json::to_string(&b.content).unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let logs = json!([
            event("gen_ai.tool.message", 1, json!({"content": input})),
            event("gen_ai.choice", 2, json!({"content": "Sunny"}))
        ]);
        assert_eq!(shown(&logs.to_string()), shown("[]"), "{input}");
    }
}

/// The records' clock may tie where the span's did not, and a tie falls back to the span's storage positions: a
/// move that would reorder the readings is not made, and the span's clock stays.
#[test]
fn a_move_that_would_reorder_the_stream_is_not_made() {
    let mut rows = streamed_call(
        vec![
            streamed(json!("B"), "chunk", 2),
            streamed(json!("A"), "chunk", 1),
            streamed(json!("C"), "delta", 6),
        ],
        None,
    );
    rows[1].span_timestamp = fixed_time() + chrono::Duration::seconds(5);
    rows[1].log_messages_json = json!([
        streamed(json!("A"), "chunk", 10),
        streamed(json!("B"), "chunk", 10),
        streamed(json!("C"), "delta", 11),
    ])
    .to_string();
    let rows: Vec<MessageSpanRow> = rows.into_iter().filter(|r| r.span_id == "model").collect();
    assert_eq!(
        assistant_texts(&process_span(rows, &FeedOptions::new())),
        ["ABC"]
    );
}

// ============================================================================
// Call identity: whose id a block carries
// ============================================================================

/// A call id read from a message's first content block is that block's: a sibling call that states no id of its
/// own does not take it, so two calls never share one id they were not both given.
#[test]
fn a_first_block_s_call_id_is_not_its_sibling_s() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [
                {"toolUse": {"toolUseId": "call-a", "name": "lookup", "input": {"city": "Paris"}}},
                {"text": "And the forecast:"},
                {"toolUse": {"name": "forecast", "input": {"city": "Tokyo"}}}
            ]
        }
    }]);
    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    let calls: Vec<(&str, Option<&str>)> = result
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { name, .. } => {
                Some((name.as_str(), block.tool_use_id.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2, "{:?}", result.messages);
    assert!(calls.contains(&("lookup", Some("call-a"))), "{calls:?}");
    let forecast = calls
        .iter()
        .find(|(name, _)| *name == "forecast")
        .expect("the second call");
    assert_ne!(
        forecast.1,
        Some("call-a"),
        "the sibling took the first block's id: {calls:?}"
    );
    let text = result
        .messages
        .iter()
        .find(|block| block.entry_type == "text")
        .expect("the text between them");
    assert_eq!(text.tool_use_id, None, "the text took the first block's id");
}

/// An id the message states under its own members is the message's: a call and the result beside it that names
/// none both carry it.
#[test]
fn a_message_s_stated_call_id_is_shared_by_its_blocks() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "tool_call_id": "call-a",
            "content": [
                {"type": "tool_use", "id": "call-a", "name": "lookup", "input": {}},
                {"type": "tool_result", "content": "ok"}
            ]
        }
    }]);
    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    let ids: Vec<(&str, Option<&str>)> = result
        .messages
        .iter()
        .map(|block| (block.entry_type.as_str(), block.tool_use_id.as_deref()))
        .collect();
    assert!(ids.contains(&("tool_use", Some("call-a"))), "{ids:?}");
    assert!(ids.contains(&("tool_result", Some("call-a"))), "{ids:?}");
}

/// A stream that was cut off joins what arrived: the call still arriving is left out, and the id its reading named
/// goes with it rather than passing to the settled call beside it, which named none.
#[test]
fn a_left_out_call_s_id_does_not_pass_to_the_call_a_join_keeps() {
    let rows = streamed_call(
        vec![
            streamed(
                json!([{"type": "tool_use", "name": "lookup", "input": {"city": "Paris"}}]),
                "settled_chunk",
                1,
            ),
            streamed(
                json!([{"toolUse": {"toolUseId": "call-a", "name": "forecast", "input": {}}}]),
                "chunk",
                2,
            ),
        ],
        None,
    );
    let result = process_spans(rows, &FeedOptions::new());
    let calls: Vec<(&str, Option<&str>)> = result
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { name, .. } => {
                Some((name.as_str(), block.tool_use_id.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert!(calls.iter().any(|(name, _)| *name == "lookup"), "{calls:?}");
    assert!(
        calls.iter().all(|(_, id)| *id != Some("call-a")),
        "a call took the left-out call's id: {calls:?}"
    );
}

/// Text beside a message's only call is not that call: the id the message read from the call's block stays the
/// call's.
#[test]
fn text_beside_a_lone_call_does_not_take_its_id() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [
                {"toolUse": {"toolUseId": "call-a", "name": "lookup", "input": {"city": "Paris"}}},
                {"text": "Looking it up."}
            ]
        }
    }]);
    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    let text = result
        .messages
        .iter()
        .find(|block| block.entry_type == "text")
        .expect("the text");
    assert_eq!(text.tool_use_id, None, "{:?}", result.messages);
}

/// A joined stream keeps an id its terminal states under its own members: the call and the result beside it
/// that names none both carry it, as they would unstreamed.
#[test]
fn a_joined_stream_keeps_its_stated_call_id() {
    let terminal = json!({
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:02Z"}},
        "content": {
            "role": "assistant",
            "tool_call_id": "call-a",
            "content": [
                {"type": "tool_use", "id": "call-a", "name": "lookup", "input": {}},
                {"type": "tool_result", "content": "ok"}
            ]
        },
        "stream": "delta"
    });
    let rows = streamed_call(
        vec![streamed(json!("Checking. "), "chunk", 1), terminal],
        None,
    );
    let result = process_spans(rows, &FeedOptions::new());
    let ids: Vec<(&str, Option<&str>)> = result
        .messages
        .iter()
        .map(|block| (block.entry_type.as_str(), block.tool_use_id.as_deref()))
        .collect();
    assert!(ids.contains(&("tool_result", Some("call-a"))), "{ids:?}");
}

/// A message on both signals is one message when its copies differ only in how a call's arguments were encoded:
/// one signal writes them as JSON text, the other as the object.
#[test]
fn copies_differing_only_in_argument_encoding_are_one_message() {
    let message = |arguments: serde_json::Value, seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.assistant.message", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": {
                "role": "assistant",
                "tool_calls": [{"id": "call-a", "type": "function", "function": {"name": "lookup", "arguments": arguments}}]
            }
        })
    };
    let mut row = make_span_row(
        "trace1",
        "model",
        None,
        &json!([message(json!(r#"{"city":"Paris"}"#), 1)]).to_string(),
        "[]",
        "[]",
    );
    row.observation_type = Some("generation".to_string());
    row.log_messages_json = json!([message(json!({"city": "Paris"}), 2)]).to_string();
    let result = process_span(vec![row], &FeedOptions::new());
    let calls: Vec<&BlockEntry> = result
        .messages
        .iter()
        .filter(|block| block.entry_type == "tool_use")
        .collect();
    assert_eq!(calls.len(), 1, "{:?}", result.messages);
    // One message, so the copy kept keeps its own instant: the span's.
    assert_eq!(
        calls[0].timestamp,
        fixed_time() + chrono::Duration::seconds(1)
    );
}

/// Copies of one call on both signals, stamped either side of the span's start, differing only in argument
/// encoding: one message, which takes the later instant - the one that says it was sent with this call.
#[test]
fn copies_differing_only_in_argument_encoding_keep_the_later_instant() {
    let message = |arguments: serde_json::Value, seconds: u32| {
        json!({
            "source": {"event": {"name": "gen_ai.assistant.message", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
            "content": {
                "role": "assistant",
                "tool_calls": [{"id": "call-a", "type": "function", "function": {"name": "lookup", "arguments": arguments}}]
            }
        })
    };
    let answer = json!({
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:20Z"}},
        "content": {"role": "assistant", "content": "Sunny."}
    });
    let mut rows = streamed_call(
        vec![message(json!(r#"{"city":"Paris"}"#), 0), answer.clone()],
        Some("Sunny."),
    );
    rows[1].span_timestamp = fixed_time() + chrono::Duration::seconds(5);
    rows[1].log_messages_json = json!([message(json!({"city": "Paris"}), 10), answer]).to_string();
    let result = process_spans(rows, &FeedOptions::new());
    let calls: Vec<&BlockEntry> = result
        .messages
        .iter()
        .filter(|block| block.entry_type == "tool_use")
        .collect();
    assert_eq!(calls.len(), 1, "{:?}", result.messages);
    assert_eq!(
        calls[0].timestamp,
        fixed_time() + chrono::Duration::seconds(10),
        "{:?}",
        calls
    );
}

/// A result whose id sits under the snake_case spelling of Converse's member answers that call, and the text beside
/// it does not: the block reads the spelling the vocabulary declares, as the message-level fallback does.
#[test]
fn the_block_an_id_was_read_from_keeps_it() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "user",
            "content": [
                {"toolResult": {"tool_use_id": "call-a", "content": [{"text": "ok"}]}},
                {"text": "after"}
            ]
        }
    }]);
    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    let ids: Vec<(&str, Option<&str>)> = result
        .messages
        .iter()
        .map(|block| (block.entry_type.as_str(), block.tool_use_id.as_deref()))
        .collect();
    assert!(ids.contains(&("tool_result", Some("call-a"))), "{ids:?}");
    // The block reads the snake_case spelling itself, so the message keeps no copy for the text to take.
    assert!(ids.contains(&("text", None)), "{ids:?}");
}

/// Nothing added to a message after it is read takes the id its lone call carries: not the error it reports,
/// not text a stream joins to it.
#[test]
fn what_joins_a_lone_call_later_does_not_take_its_id() {
    let call = json!({"toolUse": {"toolUseId": "call-a", "name": "lookup", "input": {}}});
    let with_error = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "assistant", "content": [call.clone()], "error": {"message": "rate limited"}}
    }]);
    let row = make_span_row("trace1", "span1", None, &with_error.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    assert!(
        result
            .messages
            .iter()
            .filter(|block| block.entry_type != "tool_use")
            .all(|block| block.tool_use_id.is_none()),
        "{:?}",
        result.messages
    );
    let terminal = json!({
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:02Z"}},
        "content": {"role": "assistant", "content": [call]},
        "stream": "delta"
    });
    let rows = streamed_call(
        vec![streamed(json!("Checking. "), "chunk", 1), terminal],
        None,
    );
    let result = process_spans(rows, &FeedOptions::new());
    assert!(
        result
            .messages
            .iter()
            .filter(|block| block.entry_type == "text")
            .all(|block| block.tool_use_id.is_none()),
        "{:?}",
        result.messages
    );
}

/// A tool result answering a call by an id its block carries is named after that call, whatever else its message
/// holds.
#[test]
fn a_result_beside_an_error_is_named_after_its_call() {
    let rows = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "c", "name": "lookup", "input": {}}]}
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {
                "role": "tool",
                "content": [{"toolResult": {"toolUseId": "c", "content": [{"text": "ok"}]}}],
                "error": {"message": "warning"}
            }
        }
    ]);
    let row = make_span_row("trace1", "span1", None, &rows.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::default());
    let answered = result
        .messages
        .iter()
        .find(|block| block.entry_type == "tool_result")
        .expect("the result");
    assert_eq!(
        answered.name.as_deref(),
        Some("lookup"),
        "{:?}",
        result.messages
    );
}
