// ============================================================================
// Streamed responses: the calls a stream's chunks hold (`stream.partial_calls`)
// ============================================================================

/// A chunk whose calls the producer marks as finished carries calls of the response: once each, however many
/// readings hold one.
#[test]
fn a_settled_chunk_s_call_is_a_call_once() {
    let call = |id: Option<&str>| match id {
        Some(id) => {
            json!({"type": "tool_use", "id": id, "name": "get_weather", "input": {"city": "Milan"}})
        }
        None => json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}}),
    };
    let finish = |content: serde_json::Value, mark: &str| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {"role": "assistant", "content": content, "finish_reason": "tool_calls"},
            "stream": mark
        })
    };
    let calls = |readings: Vec<serde_json::Value>| {
        let result = process_spans(streamed_call(readings, None), &FeedOptions::new());
        result.messages.iter().filter(|b| b.is_tool_use()).count()
    };
    let settled = |content: serde_json::Value, seconds| streamed(content, "settled_chunk", seconds);
    // Followed by a terminal holding nothing: the chunk's call is the call.
    assert_eq!(
        calls(vec![
            streamed(json!("Checking."), "chunk", 1),
            settled(json!([call(Some("c1"))]), 2),
            finish(json!([]), "delta"),
        ]),
        1
    );
    // Not marked settled, it may be partial, and is none.
    assert_eq!(
        calls(vec![
            streamed(json!([call(Some("c1"))]), "chunk", 1),
            finish(json!([]), "delta"),
        ]),
        0
    );
    // The terminal holds it too - by id, or by name and arguments where neither names one - and in an
    // unresolved stream beside the terminal as well: one call.
    for (id, end) in [
        (Some("c1"), "delta"),
        (None, "delta"),
        (Some("c1"), "unknown"),
    ] {
        assert_eq!(
            calls(vec![
                settled(json!([call(id)]), 1),
                finish(json!([call(id)]), end),
            ]),
            1,
            "{id:?} {end}"
        );
    }
    // Two settled chunks holding one call: one.
    assert_eq!(
        calls(vec![
            settled(json!([call(Some("c1"))]), 1),
            settled(json!([call(Some("c1"))]), 2),
            finish(json!([]), "delta"),
        ]),
        1
    );
}

/// Two alike calls in one settled chunk are two calls; a later chunk's copy of one, or the terminal's, takes one.
#[test]
fn alike_calls_in_one_settled_chunk_are_two() {
    let call = json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
    let calls = |readings: Vec<serde_json::Value>| {
        let result = process_spans(streamed_call(readings, None), &FeedOptions::new());
        result.messages.iter().filter(|b| b.is_tool_use()).count()
    };
    let end = |content: serde_json::Value| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {"role": "assistant", "content": content, "finish_reason": "tool_calls"},
            "stream": "delta"
        })
    };
    let two = json!([call.clone(), call.clone()]);
    assert_eq!(
        calls(vec![
            streamed(two.clone(), "settled_chunk", 1),
            end(json!([]))
        ]),
        2
    );
    assert_eq!(
        calls(vec![
            streamed(two.clone(), "settled_chunk", 1),
            streamed(json!([call.clone()]), "settled_chunk", 2),
            end(json!([])),
        ]),
        2
    );
    assert_eq!(
        calls(vec![streamed(two, "settled_chunk", 1), end(json!([call]))]),
        2
    );
}

/// One call several readings hold is one call, however the copies fall: two settled chunks and the terminal
/// holding it once each show it once.
#[test]
fn a_call_held_by_several_readings_is_shown_as_often_as_one_holds_it() {
    let call = json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
    let calls = |readings: Vec<serde_json::Value>| {
        let result = process_spans(streamed_call(readings, None), &FeedOptions::new());
        result.messages.iter().filter(|b| b.is_tool_use()).count()
    };
    let end = |content: serde_json::Value| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {"role": "assistant", "content": content, "finish_reason": "tool_calls"},
            "stream": "delta"
        })
    };
    let settled = |content: serde_json::Value, seconds| streamed(content, "settled_chunk", seconds);
    assert_eq!(
        calls(vec![
            settled(json!([call.clone()]), 1),
            settled(json!([call.clone()]), 2),
            end(json!([call.clone()])),
        ]),
        1
    );
    assert_eq!(
        calls(vec![
            settled(json!([call.clone()]), 1),
            settled(json!([call.clone(), call.clone()]), 2),
            end(json!([call])),
        ]),
        2
    );
}

/// A call naming no id is any call of its name and arguments: beside the ids of alike calls it adds a call only
/// where one reading holds more of them than there are ids.
#[test]
fn an_idless_call_beside_alike_calls_with_ids_is_one_of_them() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let calls = |readings: Vec<serde_json::Value>| {
        let result = process_spans(streamed_call(readings, None), &FeedOptions::new());
        result.messages.iter().filter(|b| b.is_tool_use()).count()
    };
    let end = |content: serde_json::Value| {
        json!({
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {"role": "assistant", "content": content, "finish_reason": "tool_calls"},
            "stream": "delta"
        })
    };
    let settled = |content: serde_json::Value, seconds| streamed(content, "settled_chunk", seconds);
    for (readings, expected) in [
        (
            vec![
                settled(json!([call(None)]), 1),
                settled(json!([call(Some("c1")), call(Some("c2"))]), 2),
                end(json!([])),
            ],
            2,
        ),
        (
            vec![
                settled(json!([call(None), call(None)]), 1),
                end(json!([call(Some("c1")), call(Some("c2"))])),
            ],
            2,
        ),
        (
            vec![
                settled(json!([call(None), call(None), call(None)]), 1),
                end(json!([call(Some("c1")), call(Some("c2"))])),
            ],
            3,
        ),
        (
            vec![
                settled(json!([call(Some("c1"))]), 1),
                settled(json!([call(None)]), 2),
                end(json!([])),
            ],
            1,
        ),
        (
            vec![
                settled(json!([call(Some("c1")), call(Some("c2"))]), 1),
                end(json!([call(None)])),
            ],
            2,
        ),
        (
            vec![
                settled(json!([call(Some("c1"))]), 1),
                end(json!([call(Some("c2"))])),
            ],
            2,
        ),
    ] {
        assert_eq!(calls(readings), expected);
    }
}

/// Every small arrangement of one call's copies - a terminal and two settled chunks, each holding some of the ids
/// `c1`, `c2` and up to two calls naming none - shows the fewest calls consistent with them: a reading's calls are
/// distinct, two readings' alike calls copies, so as many as there are ids, or as one reading holds, if more -
/// whether the stream is joined into the terminal or shown beside it, unresolved - and keeps every id they name.
#[test]
fn a_call_s_copies_show_the_fewest_calls_consistent_with_them() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    // A reading: which of c1 and c2 it holds, and how many calls naming none.
    let readings: Vec<(Vec<&str>, usize)> = [vec![], vec!["c1"], vec!["c2"], vec!["c1", "c2"]]
        .into_iter()
        .flat_map(|ids| (0..=2).map(move |anonymous| (ids.clone(), anonymous)))
        .collect();
    let content = |(ids, anonymous): &(Vec<&str>, usize)| {
        let mut calls: Vec<serde_json::Value> = ids.iter().map(|id| call(Some(id))).collect();
        calls.extend((0..*anonymous).map(|_| call(None)));
        serde_json::Value::Array(calls)
    };
    for (terminal, end) in readings
        .iter()
        .flat_map(|terminal| ["delta", "unknown"].map(|end| (terminal, end)))
    {
        for first in &readings {
            for second in &readings {
                let ids: std::collections::BTreeSet<&str> = [terminal, first, second]
                    .iter()
                    .flat_map(|(ids, _)| ids.iter().copied())
                    .collect();
                let most = [terminal, first, second]
                    .iter()
                    .map(|(ids, anonymous)| ids.len() + anonymous)
                    .max()
                    .unwrap_or(0);
                let rows = streamed_call(
                    vec![
                        streamed(content(first), "settled_chunk", 1),
                        streamed(content(second), "settled_chunk", 2),
                        json!({
                            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
                            "content": {"role": "assistant", "content": content(terminal)},
                            "stream": end
                        }),
                    ],
                    None,
                );
                let result = process_span(
                    rows.into_iter().filter(|r| r.span_id == "model").collect(),
                    &FeedOptions::new(),
                );
                let shown: Vec<Option<&str>> = result
                    .messages
                    .iter()
                    .filter_map(|b| match &b.content {
                        ContentBlock::ToolUse { id, .. } => Some(id.as_deref()),
                        _ => None,
                    })
                    .collect();
                let case =
                    format!("{end} terminal {terminal:?}, chunks {first:?} {second:?}: {shown:?}");
                assert_eq!(shown.len(), ids.len().max(most), "{case}");
                // Every id a reading names is kept, so a result naming it finds its call.
                let kept: std::collections::BTreeSet<&str> =
                    shown.iter().flatten().copied().collect();
                assert_eq!(kept, ids, "{case}");
            }
        }
    }
}

/// The evidence is compared with the terminal as it is shown, its call carrying the id the chunk's copy names.
#[test]
fn an_unknown_terminal_is_compared_with_the_ids_its_calls_take() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let mut rows = streamed_call(
        vec![
            streamed(
                json!([call(Some("c1")), {"type": "text", "text": "Par"}]),
                "settled_chunk",
                1,
            ),
            streamed(
                json!([call(None), {"type": "text", "text": "Paris"}]),
                "unknown",
                2,
            ),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": [call(Some("c1")), {"type": "text", "text": "Paris"}]}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Paris"]);
}

/// A call keeps the place it first appears, taking the id a later copy names: chunks holding it unnamed before
/// their text, then named, restate a terminal that holds it named before the whole text.
#[test]
fn a_call_keeps_the_place_it_first_appears() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let whole = || json!([call(Some("c1")), {"type": "text", "text": "Paris"}]);
    let mut rows = streamed_call(
        vec![
            streamed(
                json!([call(None), {"type": "text", "text": "Par"}]),
                "settled_chunk",
                1,
            ),
            streamed(json!([call(Some("c1"))]), "settled_chunk", 2),
            streamed(whole(), "unknown", 3),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": whole()}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Paris"]);
}

/// A chunk's call naming no id takes the id the terminal's copy names before the chunks are compared with it.
#[test]
fn a_chunk_s_unnamed_call_takes_the_terminal_s_id_before_it_is_compared() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let whole = || json!([call(Some("c1")), {"type": "text", "text": "Paris"}]);
    let mut rows = streamed_call(
        vec![
            streamed(
                json!([call(None), {"type": "text", "text": "Par"}]),
                "settled_chunk",
                1,
            ),
            streamed(whole(), "unknown", 2),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": whole()}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Paris"]);
}

/// Which of several alike calls an unnamed copy is cannot be told, so a terminal restates its chunks where it
/// holds every call they do, wherever among alike ones.
#[test]
fn an_unnamed_copy_among_alike_calls_is_restated_by_any_of_them() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let whole = || json!([call(Some("c1")), call(Some("c2")), {"type": "text", "text": "Paris"}]);
    let mut rows = streamed_call(
        vec![
            streamed(json!([call(None)]), "settled_chunk", 1),
            streamed(
                json!([call(Some("c2")), {"type": "text", "text": "Par"}]),
                "settled_chunk",
                2,
            ),
            streamed(whole(), "unknown", 3),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": whole()}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Paris"]);
}

/// A terminal's unnamed calls agree with the named calls the evidence states, in its order, whichever ids their
/// chunks' copies would lend them.
#[test]
fn unnamed_terminal_calls_agree_with_the_named_ones_the_evidence_states() {
    let call = |id: Option<&str>| {
        let mut call =
            json!({"type": "tool_use", "name": "get_weather", "input": {"city": "Milan"}});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        call
    };
    let mut rows = streamed_call(
        vec![
            streamed(json!([call(Some("c2"))]), "settled_chunk", 1),
            streamed(
                json!([call(Some("c1")), {"type": "text", "text": "Par"}]),
                "settled_chunk",
                2,
            ),
            streamed(
                json!([call(None), call(None), {"type": "text", "text": "Paris"}]),
                "unknown",
                3,
            ),
        ],
        None,
    );
    rows[0].messages_json = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": "2025-01-01T00:00:30Z"}},
        "content": {"role": "assistant", "content": [call(Some("c1")), call(Some("c2")), {"type": "text", "text": "Paris"}]}
    }])
    .to_string();
    assert_eq!(texts_of(rows), ["Paris"]);
}
