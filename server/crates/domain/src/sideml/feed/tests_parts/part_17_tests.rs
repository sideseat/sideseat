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
