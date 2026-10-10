// ============================================================================
// Streamed responses: chunks and the reading that ends them (`stream`)
// ============================================================================

/// One streamed reading: an assistant message as a `gen_ai.choice` event, `seconds` after the fixed time, with
/// the stream part its rule declared.
fn streamed(content: serde_json::Value, mark: &str, seconds: u32) -> serde_json::Value {
    json!({
        "source": {"event": {"name": "gen_ai.choice", "time": format!("2025-01-01T00:00:{seconds:02}Z")}},
        "content": {"role": "assistant", "content": content},
        "stream": mark
    })
}

/// A model call's span below a model-call step, holding `readings`; the step records `recorded` as its
/// response, or nothing.
fn streamed_call(readings: Vec<serde_json::Value>, recorded: Option<&str>) -> Vec<MessageSpanRow> {
    let mut step = make_span_row(
        "trace1",
        "step",
        None,
        &recorded
            .map(|text| {
                json!([{
                    "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
                    "content": {"role": "assistant", "content": text}
                }])
            })
            .unwrap_or(json!([]))
            .to_string(),
        "[]",
        "[]",
    );
    step.observation_type = Some("generation".to_string());
    let mut model = make_span_row(
        "trace1",
        "model",
        Some("step"),
        &serde_json::Value::Array(readings).to_string(),
        "[]",
        "[]",
    );
    model.observation_type = Some("generation".to_string());
    vec![step, model]
}

fn assistant_texts(result: &FeedResult) -> Vec<String> {
    result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant)
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_aggregate_terminal_is_the_response_and_its_chunks_restate_it() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "chunk", 2),
            streamed(json!("Paris"), "aggregate", 3),
        ],
        None,
    );
    assert_eq!(
        assistant_texts(&process_spans(rows, &FeedOptions::new())),
        ["Paris"]
    );
}

#[test]
fn a_delta_terminal_is_the_last_piece_of_the_response() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "delta", 2),
        ],
        None,
    );
    assert_eq!(
        assistant_texts(&process_spans(rows, &FeedOptions::new())),
        ["Paris"]
    );
}

#[test]
fn an_unknown_terminal_equal_to_the_recorded_response_is_an_aggregate() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "chunk", 2),
            streamed(json!("Paris"), "unknown", 3),
        ],
        Some("Paris"),
    );
    assert_eq!(
        assistant_texts(&process_spans(rows, &FeedOptions::new())),
        ["Paris"]
    );
}

#[test]
fn an_unknown_terminal_whose_join_is_the_recorded_response_is_a_delta() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "unknown", 2),
        ],
        Some("Paris"),
    );
    assert_eq!(
        assistant_texts(&process_spans(rows, &FeedOptions::new())),
        ["Paris"]
    );
}

/// Nothing says what the terminal holds - the step recorded another response, or there is no step - so every
/// reading stays as it was recorded rather than a guess being shown.
#[test]
fn an_unknown_terminal_no_evidence_resolves_keeps_every_reading() {
    let readings = || {
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "unknown", 2),
        ]
    };
    let disagreeing = process_spans(streamed_call(readings(), Some("Rome")), &FeedOptions::new());
    assert_eq!(assistant_texts(&disagreeing), ["Par", "is", "Rome"]);
    let alone = process_spans(streamed_call(readings(), None), &FeedOptions::new());
    assert_eq!(assistant_texts(&alone), ["Par", "is"]);
}

/// A stream that ended before any terminal is what arrived, joined, with no finish reason.
#[test]
fn an_interrupted_stream_is_its_chunks_joined() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is "), "chunk", 2),
            streamed(json!("is sunny"), "chunk", 3),
        ],
        None,
    );
    let result = process_spans(rows, &FeedOptions::new());
    assert_eq!(assistant_texts(&result), ["Paris is sunny"]);
    assert!(result.messages.iter().all(|b| b.finish_reason.is_none()));
}

/// A chunk's call may still be missing arguments, so it is no call: not in a join, not where the chunk stays.
#[test]
fn a_chunk_s_tool_call_is_never_an_execution() {
    let partial = json!([{"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Pa"}}]);
    for rows in [
        streamed_call(vec![streamed(partial.clone(), "chunk", 1)], None),
        streamed_call(
            vec![
                streamed(partial.clone(), "chunk", 1),
                streamed(json!("done"), "unknown", 2),
            ],
            Some("other"),
        ),
    ] {
        let result = process_spans(rows, &FeedOptions::new());
        assert!(
            !result.messages.iter().any(|b| b.is_tool_use()),
            "{:?}",
            result.messages
        );
    }
}

/// Reasoning joins with reasoning and keeps the one signature its pieces carry; text joins with text.
#[test]
fn a_joined_response_keeps_reasoning_and_text_apart() {
    let rows = streamed_call(
        vec![
            streamed(
                json!([{"type": "thinking", "thinking": "Weigh "}]),
                "chunk",
                1,
            ),
            streamed(
                json!([{"type": "thinking", "thinking": "it.", "signature": "sig"}]),
                "chunk",
                2,
            ),
            streamed(json!([{"type": "text", "text": "Pack "}]), "chunk", 3),
            streamed(json!([{"type": "text", "text": "light."}]), "delta", 4),
        ],
        None,
    );
    let result = process_spans(rows, &FeedOptions::new());
    let blocks: Vec<&ContentBlock> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant)
        .map(|b| &b.content)
        .collect();
    assert!(
        matches!(
            blocks.as_slice(),
            [
                ContentBlock::Thinking { text, signature: Some(signature) },
                ContentBlock::Text { text: answer, .. },
            ] if text == "Weigh it." && signature == "sig" && answer == "Pack light."
        ),
        "{blocks:?}"
    );
}

/// A span view reads a stream the same way: a declared terminal resolves it, and an unknown one, whose evidence is
/// on another span, leaves the terminal beside the chunks joined.
#[test]
fn a_span_view_reads_a_stream_as_one_response() {
    let model_only = |readings| -> Vec<MessageSpanRow> {
        streamed_call(readings, Some("Paris"))
            .into_iter()
            .filter(|r| r.span_id == "model")
            .collect()
    };
    let declared = model_only(vec![
        streamed(json!("Par"), "chunk", 1),
        streamed(json!("is"), "chunk", 2),
        streamed(json!("Paris"), "aggregate", 3),
    ]);
    assert_eq!(
        assistant_texts(&process_span(declared, &FeedOptions::new())),
        ["Paris"]
    );
    let unknown = model_only(vec![
        streamed(json!("Par"), "chunk", 1),
        streamed(json!("is"), "chunk", 2),
        streamed(json!("Paris"), "unknown", 3),
    ]);
    assert_eq!(
        assistant_texts(&process_span(unknown, &FeedOptions::new())),
        ["Paris", "Paris"]
    );
}

/// Unresolved, the chunks are still one response so far: a lone whitespace chunk is joined with its neighbours
/// rather than shown as an empty message of its own.
#[test]
fn an_unresolved_stream_shows_its_chunks_joined() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!(" "), "chunk", 2),
            streamed(json!("is"), "chunk", 3),
            streamed(json!("Paris"), "unknown", 4),
        ],
        Some("Rome"),
    );
    assert_eq!(
        assistant_texts(&process_spans(rows, &FeedOptions::new())),
        ["Par is", "Paris", "Rome"]
    );
}

/// A span delivered twice is two rows holding one stream: read as one, its readings are not joined twice.
#[test]
fn a_stream_delivered_twice_is_read_once() {
    let readings = || {
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "unknown", 2),
        ]
    };
    let once = streamed_call(readings(), Some("Paris"));
    let mut twice = once.clone();
    twice.extend(once.iter().cloned());
    assert_eq!(
        assistant_texts(&process_spans(twice, &FeedOptions::new())),
        assistant_texts(&process_spans(once, &FeedOptions::new()))
    );
}

fn assistant_blocks(result: &FeedResult) -> Vec<ContentBlock> {
    result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant)
        .map(|b| b.content.clone())
        .collect()
}

fn call(id: &str, city: &str) -> serde_json::Value {
    json!({"type": "tool_use", "id": id, "name": "get_weather", "input": {"city": city}})
}

/// A terminal's text joins the chunks before it even where the terminal also makes a call, and the call stays one.
#[test]
fn a_delta_terminal_s_text_joins_its_chunks_beside_its_call() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(
                json!([{"type": "text", "text": "is"}, call("call_1", "Paris")]),
                "delta",
                2,
            ),
        ],
        None,
    );
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(
            blocks.as_slice(),
            [ContentBlock::Text { text, .. }, ContentBlock::ToolUse { id: Some(id), .. }]
                if text == "Paris" && id == "call_1"
        ),
        "{blocks:?}"
    );
}

/// The joined response keeps the order its pieces came in: the chunks' text joins the terminal's first text,
/// before its calls, and the text after the calls stays after them.
#[test]
fn a_joined_response_keeps_its_text_and_calls_in_order() {
    let rows = streamed_call(
        vec![
            streamed(json!("A"), "chunk", 1),
            streamed(
                json!([
                    {"type": "text", "text": "B"},
                    call("call_1", "Paris"),
                    call("call_2", "Rome"),
                    {"type": "text", "text": "C"}
                ]),
                "delta",
                2,
            ),
        ],
        None,
    );
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(
            blocks.as_slice(),
            [
                ContentBlock::Text { text: first, .. },
                ContentBlock::ToolUse { id: Some(one), .. },
                ContentBlock::ToolUse { id: Some(two), .. },
                ContentBlock::Text { text: last, .. },
            ] if first == "AB" && one == "call_1" && two == "call_2" && last == "C"
        ),
        "{blocks:?}"
    );
}

/// A chunk's whitespace is the response's, even beside a call that is no call yet: it is kept in the join, and
/// the join is what the recorded response is compared with.
#[test]
fn a_chunk_s_whitespace_beside_a_partial_call_stays_in_the_response() {
    let readings = |end: &str| {
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(
                json!([{"type": "text", "text": " "}, call("call_1", "Pa")]),
                "chunk",
                2,
            ),
            streamed(json!("is"), end, 3),
        ]
    };
    let declared = process_spans(streamed_call(readings("delta"), None), &FeedOptions::new());
    assert_eq!(assistant_texts(&declared), ["Par is"]);
    let resolved = process_spans(
        streamed_call(readings("unknown"), Some("Par is")),
        &FeedOptions::new(),
    );
    assert_eq!(assistant_texts(&resolved), ["Par is"]);
}

/// Whitespace that is blank beside the rest of the joined response is dropped from it, as from any one message.
#[test]
fn a_joined_response_shows_no_blank_text() {
    let rows = streamed_call(
        vec![
            streamed(json!(" "), "chunk", 1),
            streamed(
                json!([{"type": "thinking", "thinking": "Weigh it."}]),
                "chunk",
                2,
            ),
        ],
        None,
    );
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(blocks.as_slice(), [ContentBlock::Thinking { text, .. }] if text == "Weigh it."),
        "{blocks:?}"
    );
}

/// A chunk's whitespace beside its reasoning is the response's too: the text after the reasoning still follows it.
#[test]
fn a_chunk_s_whitespace_beside_its_reasoning_stays_in_the_response() {
    let rows = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(
                json!([{"type": "text", "text": " "}, {"type": "thinking", "thinking": "Weigh."}]),
                "chunk",
                2,
            ),
            streamed(json!("is"), "delta", 3),
        ],
        None,
    );
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(
            blocks.as_slice(),
            [
                ContentBlock::Text { text: before, .. },
                ContentBlock::Thinking { text: reasoning, .. },
                ContentBlock::Text { text: after, .. },
            ] if before == "Par " && reasoning == "Weigh." && after == "is"
        ),
        "{blocks:?}"
    );
}

fn recorded_output(text: &str) -> serde_json::Value {
    json!({
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": text}
    })
}

/// "Par" then an unknown "is": "Paris" where the evidence resolves it, and its pieces beside the evidence where not.
fn par_is() -> Vec<serde_json::Value> {
    vec![
        streamed(json!("Par"), "chunk", 1),
        streamed(json!("is"), "unknown", 2),
    ]
}

fn texts_of(rows: Vec<MessageSpanRow>) -> Vec<String> {
    assistant_texts(&process_spans(rows, &FeedOptions::new()))
}

/// The evidence is every row of the generation: delivered twice with two responses, it states neither; delivered
/// twice with one, it states that one, wherever in each row it sits.
#[test]
fn a_generation_delivered_twice_is_evidence_only_where_its_rows_agree() {
    let with_second_row = |messages: serde_json::Value| {
        let mut rows = streamed_call(par_is(), Some("Paris"));
        let mut second = rows[0].clone();
        second.messages_json = messages.to_string();
        rows.push(second);
        rows
    };
    assert_eq!(
        texts_of(with_second_row(json!([recorded_output("Rome")]))),
        ["Par", "is", "Paris", "Rome"]
    );
    let question = json!({
        "source": {"attribute": {"key": "gen_ai.input.messages", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "The capital of France?"}
    });
    assert_eq!(
        texts_of(with_second_row(json!([question, recorded_output("Paris")]))),
        ["Paris"]
    );
}

/// The span tree is every span the view holds, including one whose messages could not be read.
#[test]
fn the_evidence_is_found_through_a_span_whose_messages_are_unreadable() {
    let mut rows = streamed_call(par_is(), Some("Paris"));
    rows[1].parent_span_id = Some("middle".to_string());
    rows.push(make_span_row(
        "trace1",
        "middle",
        Some("step"),
        "not json",
        "[]",
        "[]",
    ));
    assert_eq!(texts_of(rows), ["Paris"]);
}

/// A span that is its own parent is not its own ancestor: its own output is no evidence for its stream.
#[test]
fn a_span_that_is_its_own_parent_is_no_evidence_for_itself() {
    let mut readings = par_is();
    readings.push(recorded_output("Paris"));
    let mut rows: Vec<MessageSpanRow> = streamed_call(readings, None)
        .into_iter()
        .filter(|r| r.span_id == "model")
        .collect();
    rows[0].parent_span_id = Some("model".to_string());
    assert_eq!(texts_of(rows), ["Par", "is", "Paris"]);
}

/// A span whose rows disagree on what it is cannot be walked through either way, so the order the rows arrive in
/// decides nothing.
#[test]
fn a_span_its_rows_disagree_on_is_no_path_to_evidence() {
    let with_middle = |generation_first: bool| {
        let mut rows = streamed_call(par_is(), Some("Paris"));
        rows[1].parent_span_id = Some("middle".to_string());
        let plain = make_span_row("trace1", "middle", Some("step"), "[]", "[]", "[]");
        let mut generation = plain.clone();
        generation.observation_type = Some("generation".to_string());
        match generation_first {
            true => rows.extend([generation, plain]),
            false => rows.extend([plain, generation]),
        }
        rows
    };
    assert_eq!(texts_of(with_middle(true)), ["Par", "is", "Paris"]);
    assert_eq!(texts_of(with_middle(false)), ["Par", "is", "Paris"]);
}

/// A stream written on both signals is two deliveries of it, each read on its own: never one stream of both, which
/// would join every piece twice. Both deliveries show, as an answer written on both signals does.
#[test]
fn a_stream_on_span_events_and_log_records_is_read_on_each() {
    let on_both = |readings: Vec<serde_json::Value>| {
        let mut rows = streamed_call(readings.clone(), None);
        rows[1].log_messages_json = serde_json::Value::Array(readings).to_string();
        rows
    };
    for end in ["chunk", "delta"] {
        assert_eq!(
            texts_of(on_both(vec![
                streamed(json!("A"), "chunk", 1),
                streamed(json!("B"), end, 2),
            ])),
            ["AB", "AB"],
            "{end}"
        );
    }
}

/// The evidence is read per signal too: one answer written on both is two statements of it, and two pieces on two
/// signals are two answers, never one joined across them.
#[test]
fn a_generation_s_answer_on_both_signals_is_two_statements_of_it() {
    let choice = |text: &str| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:30Z"}},
            "content": {"role": "assistant", "content": text}
        })
    };
    let answered = |span: &str, log: &str| {
        let mut rows = streamed_call(par_is(), None);
        rows[0].messages_json = json!([choice(span)]).to_string();
        rows[0].log_messages_json = json!([choice(log)]).to_string();
        texts_of(rows)
    };
    assert_eq!(answered("Paris", "Paris"), ["Paris"]);
    assert_eq!(answered("Par", "is"), ["Par", "is"]);
}

/// Two output messages of one generation are two statements of its response, even on one signal and one carrier:
/// equal, they state it; never are they one response joined from both.
#[test]
fn a_generation_s_two_answers_are_two_statements_never_one_joined() {
    let choice = |text: &str| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:30Z"}},
            "content": {"role": "assistant", "content": text}
        })
    };
    let answered = |answers: Vec<serde_json::Value>, readings: Vec<serde_json::Value>| {
        let mut rows = streamed_call(readings, None);
        rows[0].messages_json = serde_json::Value::Array(answers).to_string();
        texts_of(rows)
    };
    let par_is_paris = || {
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "chunk", 2),
            streamed(json!("Paris"), "unknown", 3),
        ]
    };
    assert_eq!(
        answered(vec![choice("Paris"), choice("Paris")], par_is_paris()),
        ["Paris"]
    );
    assert_eq!(
        answered(vec![choice("Par"), choice("is")], par_is()),
        ["Par", "is"]
    );
}

/// A terminal that does not begin with its chunks restates none of them, so it is no aggregate even where the
/// generation recorded it: a cancelled call records the last piece it saw.
#[test]
fn a_terminal_that_does_not_restate_its_chunks_is_no_aggregate() {
    assert_eq!(texts_of(streamed_call(par_is(), Some("is"))), ["Par", "is"]);
}

/// The evidence is compared as it is shown: a recorded message holding "Par" and "is" as two blocks shows two, so
/// it never resolves a stream into a "Paris" shown beside them.
#[test]
fn a_recorded_response_is_compared_as_it_is_shown() {
    let mut rows = streamed_call(par_is(), None);
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": [
            {"type": "text", "text": "Par"},
            {"type": "text", "text": "is"}
        ]}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Par", "is"]);
}

/// A generation whose messages cannot be read recorded nothing to resolve by.
#[test]
fn a_generation_whose_messages_are_unreadable_is_no_evidence() {
    let mut rows = streamed_call(par_is(), None);
    rows[0].messages_json = "not json".to_string();
    assert_eq!(texts_of(rows), ["Par", "is"]);
}

/// A terminal holding nothing restates no chunk, so it can only be their end: the response is the chunks, with
/// the finish reason it states - in a span view too, which sees no evidence.
#[test]
fn a_terminal_holding_nothing_ends_its_chunks() {
    let rows: Vec<MessageSpanRow> = streamed_call(
        vec![
            streamed(json!("Par"), "chunk", 1),
            streamed(json!("is"), "chunk", 2),
            json!({
                "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
                "content": {"role": "assistant", "content": [], "finish_reason": "stop"},
                "stream": "unknown"
            }),
        ],
        None,
    )
    .into_iter()
    .filter(|r| r.span_id == "model")
    .collect();
    let result = process_span(rows, &FeedOptions::new());
    assert_eq!(assistant_texts(&result), ["Paris"]);
    assert!(
        result
            .messages
            .iter()
            .all(|b| b.finish_reason == Some(crate::sideml::types::FinishReason::Stop)),
        "{:?}",
        result.messages
    );
}

/// A record of the call that drops reasoning's signature still states the response the stream ended with.
#[test]
fn evidence_without_the_signature_still_states_a_signed_response() {
    let terminal = json!([
        {"type": "thinking", "thinking": "Weigh it.", "signature": "sig"},
        {"type": "text", "text": "Pack light."}
    ]);
    let mut rows = streamed_call(
        vec![
            streamed(
                json!([{"type": "thinking", "thinking": "Weigh "}]),
                "chunk",
                1,
            ),
            streamed(json!([{"type": "thinking", "thinking": "it."}]), "chunk", 2),
            streamed(json!([{"type": "text", "text": "Pack "}]), "chunk", 3),
            streamed(terminal, "unknown", 4),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Weigh it."},
            {"type": "text", "text": "Pack light."}
        ]}
    }])
    .to_string();
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(
            blocks.as_slice(),
            [
                ContentBlock::Thinking { text, signature: Some(signature) },
                ContentBlock::Text { text: answer, .. },
            ] if text == "Weigh it." && signature == "sig" && answer == "Pack light."
        ),
        "{blocks:?}"
    );
}

/// A signature ends the reasoning it signs: reasoning streamed after it is a block of its own.
#[test]
fn a_signature_ends_the_reasoning_it_signs() {
    let thinking = |text: &str, signature: Option<&str>| {
        let mut block = json!({"type": "thinking", "thinking": text});
        if let Some(signature) = signature {
            block["signature"] = json!(signature);
        }
        json!([block])
    };
    let rows = streamed_call(
        vec![
            streamed(thinking("Weigh ", None), "chunk", 1),
            streamed(thinking("", Some("s1")), "chunk", 2),
            streamed(thinking("Pack ", None), "chunk", 3),
            streamed(thinking("", Some("s2")), "chunk", 4),
            streamed(json!("Light."), "delta", 5),
        ],
        None,
    );
    let blocks = assistant_blocks(&process_spans(rows, &FeedOptions::new()));
    assert!(
        matches!(
            blocks.as_slice(),
            [
                ContentBlock::Thinking { text: first, signature: Some(one) },
                ContentBlock::Thinking { text: second, signature: Some(two) },
                ContentBlock::Text { text: answer, .. },
            ] if first == "Weigh " && one == "s1" && second == "Pack " && two == "s2" && answer == "Light."
        ),
        "{blocks:?}"
    );
}
