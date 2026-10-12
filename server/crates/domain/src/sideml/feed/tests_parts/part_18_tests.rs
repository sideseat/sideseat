// ============================================================================
// Redaction markers (`redaction`): a scrubbed copy yields to its unscrubbed twin
// ============================================================================

/// The scope a producer's declared marker is gated on, as the embedded assets declare it.
const SCRUBBING_SCOPE: &str = "logfire.openai_agents";
const MARKER: &str = "[Scrubbed due to 'credential']";

/// One span's row, delivered once: a call's result holding `result`, and an answer holding `answer`.
fn delivery(scope: &str, result: &str, answer: &str) -> MessageSpanRow {
    let messages = json!([
        {
            "source": {"attribute": {"key": "input", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call-1", "name": "lookup", "input": {}}]}
        },
        {
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": "lookup", "content": result}]}
        },
        {
            "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "assistant", "content": answer}
        }
    ]);
    let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    row.scope_name = Some(scope.to_string());
    row
}

fn shown(rows: Vec<MessageSpanRow>) -> Vec<String> {
    process_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter(|b| b.entry_type != "tool_use")
        .map(|b| serde_json::to_string(&b.content).unwrap_or_default())
        .collect()
}

/// A span delivered scrubbed and unscrubbed shows the unscrubbed copy of each statement, whichever delivery
/// came first: the producer's own marker decides, never arrival order or a hash.
#[test]
fn a_scrubbed_delivery_yields_to_the_unscrubbed_one_in_either_order() {
    let scrubbed = delivery(SCRUBBING_SCOPE, MARKER, MARKER);
    let plain = delivery(
        SCRUBBING_SCOPE,
        "Missing credentials.",
        "No image was generated.",
    );
    for rows in [
        vec![scrubbed.clone(), plain.clone()],
        vec![plain.clone(), scrubbed.clone()],
    ] {
        let shown = shown(rows);
        assert!(shown.iter().all(|c| !c.contains("Scrubbed")), "{shown:?}");
        assert!(
            shown.iter().any(|c| c.contains("Missing credentials.")),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|c| c.contains("No image was generated.")),
            "{shown:?}"
        );
    }
}

/// Where no unscrubbed twin exists, or every copy is a marker, or none is, nothing changes: a span delivered only
/// scrubbed stays scrubbed, and two deliveries of values keep what they kept before.
#[test]
fn without_an_unscrubbed_twin_a_marker_stays() {
    let scrubbed = delivery(SCRUBBING_SCOPE, MARKER, MARKER);
    let only = shown(vec![scrubbed.clone()]);
    assert_eq!(
        only.iter().filter(|c| c.contains("Scrubbed")).count(),
        2,
        "{only:?}"
    );
    let both = shown(vec![scrubbed.clone(), scrubbed]);
    assert_eq!(both, only, "two scrubbed deliveries show what one does");
    let first = delivery(SCRUBBING_SCOPE, "One.", "First.");
    let second = delivery(SCRUBBING_SCOPE, "Two.", "Second.");
    let values = shown(vec![first, second]);
    assert!(
        values.iter().any(|c| c.contains("First.")) && values.iter().any(|c| c.contains("Second.")),
        "two deliveries of values keep both answers: {values:?}"
    );
}

/// Text that quotes a marker inside something longer is a value, not a marker, and keeps its place.
#[test]
fn text_quoting_a_marker_is_a_value() {
    let quoting = format!("The log line read {MARKER} in full.");
    let first = delivery(SCRUBBING_SCOPE, "Missing credentials.", &quoting);
    let second = delivery(
        SCRUBBING_SCOPE,
        "Missing credentials.",
        "No image was generated.",
    );
    let shown = shown(vec![first, second]);
    assert!(
        shown.iter().any(|c| c.contains("The log line read")),
        "{shown:?}"
    );
}

/// Two copies of one result the dedup identity joins - one call id - though they sit at different places: the
/// copy holding the value survives the quality tie, not the producer's marker, in either order.
#[test]
fn a_tie_between_a_result_and_its_marker_copy_keeps_the_result() {
    let scrubbed = {
        let mut row = delivery(SCRUBBING_SCOPE, MARKER, "Done.");
        // One more message first, so the scrubbed result is not at its twin's position.
        let mut messages: Vec<serde_json::Value> =
            serde_json::from_str(&row.messages_json).expect("JSON");
        messages.insert(
            0,
            json!({
                "source": {"attribute": {"key": "preamble", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "user", "content": "Go."}
            }),
        );
        row.messages_json = serde_json::Value::Array(messages).to_string();
        row
    };
    let plain = delivery(SCRUBBING_SCOPE, "Missing credentials.", "Done.");
    for rows in [
        vec![scrubbed.clone(), plain.clone()],
        vec![plain.clone(), scrubbed.clone()],
    ] {
        let shown = shown(rows);
        let results: Vec<&String> = shown.iter().filter(|c| c.contains("tool_result")).collect();
        assert_eq!(results.len(), 1, "{shown:?}");
        assert!(results[0].contains("Missing credentials."), "{shown:?}");
    }
}

/// Two statements one place holds - a span delivered twice with different values there - are ordered by what
/// they say, not by a hash of it: the same answer in either order of arrival, the lesser content first.
#[test]
fn statements_at_one_place_are_ordered_by_their_content() {
    let first = delivery("other.scope", "One.", "Alpha.");
    let second = delivery("other.scope", "One.", "Beta.");
    let answers = |rows: Vec<MessageSpanRow>| -> Vec<String> {
        shown(rows)
            .into_iter()
            .filter(|c| c.contains("Alpha.") || c.contains("Beta."))
            .collect()
    };
    let forward = answers(vec![first.clone(), second.clone()]);
    assert_eq!(forward, answers(vec![second, first]));
    assert_eq!(forward.len(), 2, "{forward:?}");
    assert!(forward[0].contains("Alpha."), "{forward:?}");
}

/// Two copies at one place differing only in what no view shows - a thinking block's signature - are still
/// ordered by the rule: the same answer in either order of arrival.
#[test]
fn copies_differing_only_in_an_unshown_member_keep_one_order() {
    let reasoning = |signature: &str| {
        let messages = json!([{
            "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Weighing it.", "signature": signature},
                {"type": "text", "text": "Done."}
            ]}
        }]);
        make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]")
    };
    let hashes = |rows: Vec<MessageSpanRow>| -> Vec<String> {
        process_spans(rows, &FeedOptions::new())
            .messages
            .iter()
            .map(|b| b.content_hash.clone())
            .collect()
    };
    let (first, second) = (reasoning("sig-one"), reasoning("sig-two"));
    assert_eq!(
        hashes(vec![first.clone(), second.clone()]),
        hashes(vec![second, first])
    );
}

/// What only the scrubbed copy says besides its content passes to the copy that stays: a failure the marker copy
/// reports is the result's failure, though the unscrubbed copy left it unsaid.
#[test]
fn the_scrubbed_copy_s_failure_passes_to_the_twin_that_stays() {
    let delivery_with = |content: &str, is_error: bool| {
        let messages = json!([
            {
                "source": {"attribute": {"key": "input", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call-1", "name": "lookup", "input": {}}]}
            },
            {
                "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": "lookup", "content": content, "is_error": is_error}]}
            }
        ]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    let scrubbed = delivery_with(MARKER, true);
    let plain = delivery_with("Missing credentials.", false);
    for rows in [
        vec![scrubbed.clone(), plain.clone()],
        vec![plain.clone(), scrubbed.clone()],
    ] {
        let result = process_spans(rows, &FeedOptions::new());
        let results: Vec<&BlockEntry> = result
            .messages
            .iter()
            .filter(|b| b.entry_type == "tool_result")
            .collect();
        assert_eq!(results.len(), 1, "{:?}", result.messages);
        match &results[0].content {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(content, &json!("Missing credentials."));
                assert!(*is_error, "the failure the scrubbed copy reported was lost");
            }
            other => panic!("not a result: {other:?}"),
        }
        // Its identity is the observation's as it was read - what a later trace's replay of it matches.
        let alone = process_spans(vec![plain.clone()], &FeedOptions::new());
        let read = alone
            .messages
            .iter()
            .find(|b| b.entry_type == "tool_result")
            .expect("the result");
        assert_eq!(results[0].content_hash, read.content_hash);
    }
}

/// Where one place holds several unscrubbed values beside a marker copy, what only the marker copy says goes to
/// each of them - every one is that statement - whatever order the deliveries arrived in.
#[test]
fn what_a_marker_copy_says_goes_to_every_twin() {
    let answer = |text: &str, finish: Option<&str>| {
        let mut content = json!({"role": "assistant", "content": text});
        if let Some(finish) = finish {
            content["finish_reason"] = json!(finish);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
            "content": content
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    let finished = |rows: Vec<MessageSpanRow>| -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = process_spans(rows, &FeedOptions::new())
            .messages
            .iter()
            .filter(|b| b.entry_type == "text")
            .map(|b| {
                (
                    serde_json::to_string(&b.content).unwrap_or_default(),
                    b.finish_reason.is_some(),
                )
            })
            .collect();
        out.sort();
        out
    };
    let (alpha, beta, marker) = (
        answer("Alpha.", None),
        answer("Beta.", None),
        answer(MARKER, Some("stop")),
    );
    let forward = finished(vec![alpha.clone(), beta.clone(), marker.clone()]);
    assert_eq!(forward, finished(vec![beta, alpha, marker]));
    assert!(
        forward.len() == 2 && forward.iter().all(|(_, finish)| *finish),
        "{forward:?}"
    );
}

/// Joined by deduplication rather than by place, the copy holding the value adopts the failure the marker copy
/// reported, and keeps the identity it was read with.
#[test]
fn a_failure_adopted_across_places_keeps_the_survivor_s_identity() {
    let result_row = |preamble: bool, content: &str, is_error: bool| {
        // The scrubbed delivery reports all it can besides its content: a failed span and a provider's own run.
        let reported = preamble;
        let mut messages = vec![
            json!({
                "source": {"attribute": {"key": "input", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call-1", "name": "lookup", "input": {}}]}
            }),
            json!({
                "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
                "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": "lookup", "content": content, "is_error": is_error, "provider_executed": reported}]}
            }),
        ];
        if preamble {
            messages.insert(
                0,
                json!({
                    "source": {"attribute": {"key": "preamble", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": "Go."}
                }),
            );
        }
        let mut row = make_span_row(
            "trace1",
            "span1",
            None,
            &serde_json::Value::Array(messages).to_string(),
            "[]",
            "[]",
        );
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        if reported {
            row.status_code = Some(status::ERROR.to_string());
        }
        row
    };
    let result_of = |rows: Vec<MessageSpanRow>| {
        process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .find(|b| b.entry_type == "tool_result")
            .expect("the result")
    };
    let read = result_of(vec![result_row(false, "Missing credentials.", false)]);
    for rows in [
        vec![
            result_row(true, MARKER, true),
            result_row(false, "Missing credentials.", false),
        ],
        vec![
            result_row(false, "Missing credentials.", false),
            result_row(true, MARKER, true),
        ],
    ] {
        let survivor = result_of(rows);
        assert!(
            matches!(&survivor.content, ContentBlock::ToolResult { is_error: true, content, .. } if content == &json!("Missing credentials.")),
            "{survivor:?}"
        );
        assert_eq!(survivor.content_hash, read.content_hash);
        assert!(survivor.is_error, "{survivor:?}");
        assert_eq!(survivor.status_code.as_deref(), Some(status::ERROR));
        assert!(
            matches!(
                survivor.content,
                ContentBlock::ToolResult {
                    provider_executed: true,
                    ..
                }
            ),
            "{survivor:?}"
        );
    }
}

/// A marker copy naming one call and an unscrubbed copy naming another are two statements, though one place
/// holds them: neither yields, and neither takes the other's failure.
#[test]
fn copies_naming_different_calls_do_not_yield() {
    let result = |id: &str, content: &str, is_error: bool| {
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": id, "name": "lookup", "content": content, "is_error": is_error}]}
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    let rows = vec![
        result("call-a", MARKER, true),
        result("call-b", "Fine.", false),
    ];
    let results: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
        .messages
        .into_iter()
        .filter(|b| b.entry_type == "tool_result")
        .collect();
    assert_eq!(results.len(), 2, "{results:?}");
    let b = results
        .iter()
        .find(|r| r.tool_use_id.as_deref() == Some("call-b"))
        .expect("call-b's result");
    assert!(
        matches!(
            &b.content,
            ContentBlock::ToolResult {
                is_error: false,
                ..
            }
        ),
        "{b:?}"
    );
}

/// Marker copies that disagree on how the response finished pass no finish on; copies that agree pass theirs -
/// in either order of arrival.
#[test]
fn marker_copies_pass_on_only_what_they_agree_on() {
    let answer = |text: &str, finish: Option<&str>| {
        let mut content = json!({"role": "assistant", "content": text});
        if let Some(finish) = finish {
            content["finish_reason"] = json!(finish);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
            "content": content
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    let finish_of = |rows: Vec<MessageSpanRow>| {
        process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .find(|b| b.entry_type == "text")
            .and_then(|b| b.finish_reason)
    };
    let plain = answer("Done.", None);
    let (stop, length) = (answer(MARKER, Some("stop")), answer(MARKER, Some("length")));
    assert_eq!(
        finish_of(vec![stop.clone(), length.clone(), plain.clone()]),
        None
    );
    assert_eq!(
        finish_of(vec![length.clone(), stop.clone(), plain.clone()]),
        None
    );
    let agreed = finish_of(vec![stop.clone(), stop.clone(), plain.clone()]);
    assert!(agreed.is_some());
    assert_eq!(agreed, finish_of(vec![plain, stop.clone(), stop]));
}

/// One place holding copies of two calls, scrubbed and not, with a copy naming none: each scrubbed copy speaks to
/// the copy naming its own call, and the copy naming none hears only what speaks to every copy - in any order.
#[test]
fn marker_copies_speak_to_their_own_call_first() {
    let result = |id: Option<&str>, content: &str, is_error: bool| {
        let mut block = json!({"type": "tool_result", "name": "lookup", "content": content, "is_error": is_error});
        if let Some(id) = id {
            block["tool_use_id"] = json!(id);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    let rows = [
        result(Some("call-a"), MARKER, true),
        result(Some("call-b"), MARKER, false),
        result(Some("call-a"), "A done.", false),
        result(Some("call-b"), "B done.", false),
        result(None, "Something done.", false),
    ];
    let failed_of = |rows: Vec<MessageSpanRow>| -> Vec<(Option<String>, bool)> {
        let mut out: Vec<(Option<String>, bool)> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "tool_result")
            .map(|b| {
                let failed = matches!(b.content, ContentBlock::ToolResult { is_error: true, .. });
                (b.tool_use_id.clone(), failed)
            })
            .collect();
        out.sort();
        out
    };
    let forward = failed_of(rows.to_vec());
    let mut reversed = rows.to_vec();
    reversed.reverse();
    assert_eq!(forward, failed_of(reversed));
    assert!(
        forward.contains(&(Some("call-a".to_string()), true)),
        "{forward:?}"
    );
    assert!(
        forward.contains(&(Some("call-b".to_string()), false)),
        "{forward:?}"
    );
    assert!(
        forward.iter().all(|(id, failed)| id.is_some() || !failed),
        "{forward:?}"
    );
}

/// A marker copy naming a call - at its message or in its result - that the unscrubbed copy does not name is not
/// shown to be the same statement: it does not yield, and both stay as they were read.
#[test]
fn a_marker_naming_a_call_its_twin_does_not_name_stays() {
    let result = |content: &str, call: Option<&str>| {
        let mut message = json!({"role": "tool", "content": [{"type": "tool_result", "name": "lookup", "content": content}]});
        if let Some(call) = call {
            message["tool_call_id"] = json!(call);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": message
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    for rows in [
        vec![result(MARKER, Some("call-a")), result("Done.", None)],
        vec![result("Done.", None), result(MARKER, Some("call-a"))],
    ] {
        let results = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "tool_result")
            .count();
        assert_eq!(results, 2);
    }
}

/// Copies naming different tools are different statements, though one place holds them: a scrubbed `lookup`
/// result's failure does not pass to an unscrubbed `write` result.
#[test]
fn copies_naming_different_tools_do_not_yield() {
    // The tool named in the result, at its message, or in the result with one call id on both copies.
    let result =
        |tool: &str, content: &str, is_error: bool, at_message: bool, call: Option<&str>| {
            let mut block =
                json!({"type": "tool_result", "content": content, "is_error": is_error});
            let mut message = json!({"role": "tool"});
            if at_message {
                message["name"] = json!(tool);
            } else {
                block["name"] = json!(tool);
            }
            if let Some(call) = call {
                block["tool_use_id"] = json!(call);
            }
            message["content"] = json!([block]);
            let messages = json!([{
                "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
                "content": message
            }]);
            let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
            row.scope_name = Some(SCRUBBING_SCOPE.to_string());
            row
        };
    for (at_message, call) in [(false, None), (true, None), (false, Some("call-1"))] {
        for (rows, first) in [
            (
                vec![
                    result("lookup", MARKER, true, at_message, call),
                    result("write", "Written.", false, at_message, call),
                ],
                MARKER,
            ),
            (
                vec![
                    result("write", "Written.", false, at_message, call),
                    result("lookup", MARKER, true, at_message, call),
                ],
                "Written.",
            ),
        ] {
            let results: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
                .messages
                .into_iter()
                .filter(|b| b.entry_type == "tool_result")
                .collect();
            if call.is_none() {
                // Two statements: both stay, and `write` takes nothing from `lookup`.
                assert_eq!(results.len(), 2, "{results:?}");
                assert!(
                    results.iter().any(|r| matches!(&r.content,
                        ContentBlock::ToolResult { content, is_error: false, .. } if content == &json!("Written."))),
                    "{results:?}"
                );
            } else {
                // Joined by their call id, the copy kept is the earlier one - as before markers: the marker
                // preference does not reach across two tools.
                assert_eq!(results.len(), 1, "{results:?}");
                assert!(
                    matches!(&results[0].content, ContentBlock::ToolResult { content, .. } if content == &json!(first)),
                    "{results:?}"
                );
            }
        }
    }
}

/// A marker copy and an unscrubbed copy from two senders are two statements, though one place holds them: the
/// marker stays, and the other sender takes nothing from it.
#[test]
fn copies_from_different_senders_do_not_yield() {
    let said = |sender: &str, text: &str, finish: Option<&str>| {
        let mut content = json!({"role": "assistant", "name": sender, "content": text});
        if let Some(finish) = finish {
            content["finish_reason"] = json!(finish);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
            "content": content
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    for rows in [
        vec![
            said("alice", MARKER, Some("stop")),
            said("bob", "Hello.", None),
        ],
        vec![
            said("bob", "Hello.", None),
            said("alice", MARKER, Some("stop")),
        ],
    ] {
        let texts: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "text")
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        let bob = texts
            .iter()
            .find(|b| b.name.as_deref() == Some("bob"))
            .expect("bob's text");
        assert_eq!(bob.finish_reason, None, "{bob:?}");
    }
}

/// A message of a span's input is not the same statement as the message at the same position of its output: a
/// marker read from one carrier does not yield to, nor pass its finish to, a text read from another.
#[test]
fn copies_read_from_different_carriers_do_not_yield() {
    let said = |carrier: &str, text: &str, finish: Option<&str>| {
        let mut content = json!({"role": "assistant", "content": text});
        if let Some(finish) = finish {
            content["finish_reason"] = json!(finish);
        }
        let messages = json!([{
            "source": {"attribute": {"key": carrier, "time": "2025-01-01T00:00:00Z"}},
            "content": content
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    for rows in [
        vec![
            said("gen_ai.input.messages", MARKER, Some("stop")),
            said("gen_ai.output.messages", "Fresh answer.", None),
        ],
        vec![
            said("gen_ai.output.messages", "Fresh answer.", None),
            said("gen_ai.input.messages", MARKER, Some("stop")),
        ],
    ] {
        let texts: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "text")
            .collect();
        assert!(
            texts.iter().any(|b| serde_json::to_string(&b.content)
                .unwrap_or_default()
                .contains("Scrubbed")),
            "{texts:?}"
        );
        let fresh = texts
            .iter()
            .find(|b| {
                serde_json::to_string(&b.content)
                    .unwrap_or_default()
                    .contains("Fresh answer.")
            })
            .expect("the output's text");
        assert_eq!(fresh.finish_reason, None, "{fresh:?}");
    }
}

/// One span's row holding one answer, finished or not, at the place a delivered answer always sits.
fn answer_delivery(text: &str, finish: Option<&str>) -> MessageSpanRow {
    let mut content = json!({"role": "assistant", "content": text});
    if let Some(finish) = finish {
        content["finish_reason"] = json!(finish);
    }
    let messages = json!([{
        "source": {"attribute": {"key": "answer", "time": "2025-01-01T00:00:00Z"}},
        "content": content
    }]);
    let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    row.scope_name = Some(SCRUBBING_SCOPE.to_string());
    row
}

/// A finish only a marker states passes to an unscrubbed copy only when no other unscrubbed copy contradicts it:
/// otherwise which unscrubbed copy survived the quality tie - arrival order - would decide the finish shown.
#[test]
fn a_marker_s_finish_does_not_contradict_an_unscrubbed_copy_s() {
    let finished = answer_delivery("Done.", Some("stop"));
    let unfinished = answer_delivery("Done.", None);
    let marker = answer_delivery(MARKER, Some("length"));
    let mut seen = Vec::new();
    for rows in [
        vec![finished.clone(), unfinished.clone(), marker.clone()],
        vec![unfinished.clone(), finished.clone(), marker.clone()],
        vec![marker.clone(), unfinished.clone(), finished.clone()],
    ] {
        let texts: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "text")
            .collect();
        assert!(
            texts.iter().all(|b| !serde_json::to_string(&b.content)
                .unwrap_or_default()
                .contains("Scrubbed")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .all(|b| b.finish_reason != Some(FinishReason::Length)),
            "{texts:?}"
        );
        seen.push(texts.iter().map(|b| b.finish_reason).collect::<Vec<_>>());
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
}

/// A marker delivered by a failed span passes the span's failure to its unscrubbed twin, as a failed result
/// passes its own: withdrawing the marker must not withdraw the failure it reported.
#[test]
fn a_marker_s_span_failure_passes_to_its_twin() {
    let mut failed = answer_delivery(MARKER, None);
    failed.status_code = Some(status::ERROR.to_string());
    let plain = answer_delivery("Done.", None);
    for rows in [
        vec![failed.clone(), plain.clone()],
        vec![plain.clone(), failed.clone()],
    ] {
        let texts: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "text")
            .collect();
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert!(texts[0].is_error, "{texts:?}");
        assert_eq!(texts[0].status_code.as_deref(), Some(status::ERROR));
    }
}

/// A result a marker says the provider ran keeps saying so once its unscrubbed twin stands for it.
#[test]
fn a_marker_s_provider_execution_passes_to_its_twin() {
    let result = |content: &str, by_provider: bool| {
        let mut block = json!({"type": "tool_result", "tool_use_id": "call-1", "name": "lookup", "content": content});
        if by_provider {
            block["provider_executed"] = json!(true);
        }
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row
    };
    for rows in [
        vec![result(MARKER, true), result("Found it.", false)],
        vec![result("Found it.", false), result(MARKER, true)],
    ] {
        let results: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| b.entry_type == "tool_result")
            .collect();
        assert_eq!(results.len(), 1, "{results:?}");
        assert!(
            matches!(
                results[0].content,
                crate::sideml::ContentBlock::ToolResult {
                    provider_executed: true,
                    ..
                }
            ),
            "{results:?}"
        );
    }
}

/// What a marker reported survives every merge after the one that took it: a better copy - the tool span's own -
/// replacing the one that adopted the report keeps it too, whichever order the three copies arrive in.
#[test]
fn a_marker_s_report_survives_a_later_replacement() {
    let result_row = |span: &str, preamble: bool, content: &str, from_tool: bool, tool: &str| {
        let marker = content == MARKER;
        let mut messages = vec![json!({
            "source": {"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-1", "name": tool, "content": content, "provider_executed": marker}]}
        })];
        if preamble {
            messages.insert(
                0,
                json!({
                    "source": {"attribute": {"key": "preamble", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": "Go."}
                }),
            );
        }
        let mut row = make_span_row(
            "trace1",
            span,
            None,
            &serde_json::Value::Array(messages).to_string(),
            "[]",
            "[]",
        );
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        if marker {
            row.status_code = Some(status::ERROR.to_string());
        }
        if from_tool {
            row.observation_type = Some("tool".to_string());
        }
        row
    };
    // The marker at another place than the plain copy - dedup merges them - and at the same place - the marker
    // yields before dedup, its report taken by the plain copy.
    for elsewhere in [true, false] {
        let marker = result_row("span1", elsewhere, MARKER, false, "lookup");
        let plain = result_row("span1", false, "Missing credentials.", false, "lookup");
        let better = result_row("span2", false, "Missing credentials!", true, "lookup");
        for rows in [
            vec![marker.clone(), plain.clone(), better.clone()],
            vec![plain.clone(), marker.clone(), better.clone()],
            vec![better.clone(), marker.clone(), plain.clone()],
        ] {
            let results: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
                .messages
                .into_iter()
                .filter(|b| b.entry_type == "tool_result")
                .collect();
            assert_eq!(results.len(), 1, "{results:?}");
            let survivor = &results[0];
            assert!(survivor.is_error, "{survivor:?}");
            assert_eq!(survivor.status_code.as_deref(), Some(status::ERROR));
            assert!(
                matches!(
                    survivor.content,
                    ContentBlock::ToolResult {
                        provider_executed: true,
                        ..
                    }
                ),
                "{survivor:?}"
            );
        }
    }
    // A marker naming another tool reports nothing of this one, though both name one call - nor through an unnamed
    // copy that took its report before this one replaced it.
    let unnamed = result_row("span1", false, "Missing credentials.", false, "");
    let lookup = result_row("span1", true, MARKER, false, "lookup");
    let write = result_row("span3", false, "Wrote it.", true, "write");
    for rows in [
        vec![
            result_row("span1", false, "Wrote it.", false, "write"),
            result_row("span2", false, MARKER, false, "lookup"),
        ],
        vec![
            result_row("span2", false, MARKER, false, "lookup"),
            result_row("span1", false, "Wrote it.", false, "write"),
        ],
        vec![lookup.clone(), unnamed.clone(), write.clone()],
        vec![unnamed.clone(), lookup.clone(), write.clone()],
        vec![write.clone(), lookup.clone(), unnamed.clone()],
    ] {
        let written: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| {
                serde_json::to_string(&b.content)
                    .unwrap_or_default()
                    .contains("Wrote it.")
            })
            .collect();
        assert!(!written.is_empty());
        for block in &written {
            assert!(!block.is_error, "{block:?}");
            assert!(
                !matches!(
                    block.content,
                    ContentBlock::ToolResult {
                        provider_executed: true,
                        ..
                    }
                ),
                "{block:?}"
            );
        }
    }
}
