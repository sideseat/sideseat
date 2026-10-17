// ============================================================================
// Redaction markers (`redaction`): what reaches the copy that finally stands for a statement
// ============================================================================

/// A copy of another tool winning for a while does not decide what reaches the copy that stays: the report of a
/// lookup marker reaches the best lookup result whether or not a write result held the place in between.
#[test]
fn a_report_reaches_the_settled_survivor_past_another_tool() {
    let result_row = |span: &str, content: &str, tool: &str, rank: u8| {
        let marker = content == MARKER;
        let block = json!({"type": "tool_result", "tool_use_id": "call-1", "name": tool, "content": content, "provider_executed": marker});
        // The best copy is the one an event carried.
        let source = if rank > 1 {
            json!({"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:00Z"}})
        } else {
            json!({"attribute": {"key": "output", "time": "2025-01-01T00:00:00Z"}})
        };
        let messages = json!([{
            "source": source,
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", span, None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        if marker {
            row.status_code = Some(status::ERROR.to_string());
        }
        if rank > 0 {
            row.observation_type = Some("tool".to_string());
        }
        row
    };
    let marker = result_row("span1", MARKER, "lookup", 0);
    let write = result_row("span2", "Wrote it.", "write", 1);
    let lookup = result_row("span3", "Found it.", "lookup", 2);
    for rows in [
        vec![marker.clone(), write.clone(), lookup.clone()],
        vec![lookup.clone(), write.clone(), marker.clone()],
        vec![write.clone(), marker.clone(), lookup.clone()],
    ] {
        let found: Vec<BlockEntry> = process_spans(rows, &FeedOptions::new())
            .messages
            .into_iter()
            .filter(|b| {
                serde_json::to_string(&b.content)
                    .unwrap_or_default()
                    .contains("Found it.")
            })
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].is_error, "{found:?}");
        assert!(
            matches!(
                found[0].content,
                ContentBlock::ToolResult {
                    provider_executed: true,
                    ..
                }
            ),
            "{found:?}"
        );
    }
}

/// Copies of one call naming two tools between them are decided as before markers, by an order no third copy can
/// undo: whichever order a lookup value, a write value and a lookup marker of equal quality arrive in, one copy
/// survives.
#[test]
fn a_tie_across_tools_is_decided_alike_in_every_order() {
    // The order the copies arrive in follows their times and spans, so both are permuted.
    let result_row = |span: &str, content: &str, tool: &str, kind: Option<&str>, second: usize| {
        let block = json!({"type": "tool_result", "tool_use_id": "call-1", "name": tool, "content": content});
        let messages = json!([{
            "source": {"attribute": {"key": "output", "time": format!("2025-01-01T00:00:0{second}Z")}},
            "content": {"role": "tool", "content": [block]}
        }]);
        let mut row = make_span_row("trace1", span, None, &messages.to_string(), "[]", "[]");
        row.scope_name = Some(SCRUBBING_SCOPE.to_string());
        row.observation_type = kind.map(str::to_string);
        row
    };
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let survivors: Vec<Vec<String>> = orders
        .iter()
        .map(|order| {
            let at = |copy: usize| order.iter().position(|&i| i == copy).unwrap_or(0) + 1;
            let span = |copy: usize| format!("span{}", at(copy));
            let rows = vec![
                result_row(&span(0), "Found it.", "lookup", None, at(0)),
                result_row(&span(1), "Wrote it.", "write", Some("generation"), at(1)),
                result_row(&span(2), MARKER, "lookup", Some("agent"), at(2)),
            ];
            process_spans(rows, &FeedOptions::new())
                .messages
                .iter()
                .filter(|b| b.entry_type == "tool_result")
                .map(|b| serde_json::to_string(&b.content).unwrap_or_default())
                .collect()
        })
        .collect();
    assert!(survivors.windows(2).all(|w| w[0] == w[1]), "{survivors:?}");
    assert_eq!(survivors[0].len(), 1, "{survivors:?}");
}

/// A result a rule declared `autogen.function_execution_result` keeps the call id it states; JSON of the same
/// shape that declares nothing is data.
#[test]
fn a_declared_function_result_keeps_its_call_id_and_undeclared_json_stays_data() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let at = |key: &str| json!({"attribute": {"key": key, "time": t0.to_rfc3339()}});
    let declared = json!({"type": "autogen.function_execution_result", "call_id": "B", "name": "lookup", "content": "ok", "is_error": false});
    let undeclared =
        json!({"call_id": "C", "name": "lookup", "content": "ok", "is_error": false, "extra": 1});
    let messages = json!([
        {"source": at("llm.input_messages"), "content": {"role": "tool", "content": [declared]}},
        {"source": at("llm.output_messages"), "content": {"role": "assistant", "content": [undeclared]}}
    ]);
    let row = make_span_row_with_timestamps(
        "trace-results",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    );
    let results: Vec<Option<String>> = process_span(vec![row], &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results, [Some("B".to_string())]);
}

// ----------------------------------------------------------------------------
// Test: the deprecated content events' members read as the request and the response they are
// ----------------------------------------------------------------------------

fn member_event(
    name: &str,
    at: chrono::DateTime<Utc>,
    content: serde_json::Value,
) -> serde_json::Value {
    json!({"source": {"event": {"name": name, "time": at.to_rfc3339()}}, "content": content})
}

/// A child generation's response read from a deprecated completion event's member is its output: an agent parent
/// that re-lists the question does not make the answer history, finish reason or none.
#[test]
fn a_completion_member_is_its_generation_s_output() {
    let start = fixed_time();
    let parent = make_span_row_full(
        "trace1",
        "agent",
        None,
        &json!([member_event(
            "gen_ai.user.message",
            start,
            json!({"role": "user", "content": "What is 3+3?"})
        )])
        .to_string(),
        start,
        Some(start + chrono::Duration::seconds(3)),
        Some("agent"),
    );
    let child = make_span_row_with_timestamps(
        "trace1",
        "call",
        Some("agent"),
        &json!([
            member_event(
                "gen_ai.prompt",
                start,
                json!({"role": "user", "content": "What is 3+3?"})
            ),
            member_event(
                "gen_ai.completion",
                start + chrono::Duration::seconds(1),
                json!({"role": "assistant", "content": "3+3 is 6."})
            ),
        ])
        .to_string(),
        start,
        Some(start + chrono::Duration::seconds(2)),
    );
    let texts: Vec<String> = process_trace_spans(vec![parent, child], &FeedOptions::new())
        .messages
        .iter()
        .map(|b| {
            format!(
                "{}:{}",
                b.role.as_str(),
                serde_json::to_string(&b.content).unwrap_or_default()
            )
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("assistant:") && t.contains("3+3 is 6.")),
        "the answer was lost: {texts:?}"
    );
}

/// A deprecated prompt event's member re-lists the whole conversation at request time: in a session, a later trace's
/// copy of an earlier trace's turns is recognised by its place in the replayed prefix and not shown twice.
#[test]
fn a_prompt_member_replays_an_earlier_trace_once() {
    let first = fixed_time();
    let second = first + chrono::Duration::seconds(60);
    let turn = |trace: &str, at: chrono::DateTime<Utc>, prompt: serde_json::Value, answer: &str| {
        let mut row = make_span_row_with_timestamps(
            trace,
            &format!("{trace}-call"),
            None,
            &json!([
                member_event("gen_ai.prompt", at, prompt),
                member_event(
                    "gen_ai.completion",
                    at + chrono::Duration::seconds(1),
                    json!({"role": "assistant", "content": answer})
                ),
            ])
            .to_string(),
            at,
            Some(at + chrono::Duration::seconds(2)),
        );
        row.session_id = Some("session".to_string());
        row
    };
    let rows = vec![
        turn(
            "trace1",
            first,
            json!([{"role": "user", "content": "Q1"}]),
            "A1",
        ),
        turn(
            "trace2",
            second,
            json!([
                {"role": "user", "content": "Q1"},
                {"role": "assistant", "content": "A1"},
                {"role": "user", "content": "Q2"}
            ]),
            "A2",
        ),
    ];
    let texts: Vec<String> = process_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            crate::sideml::types::ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        ["Q1", "A1", "Q2", "A2"],
        "the session shows each turn once, in order"
    );
}

/// A bare prompt is one question, reported under the deprecated event's own name, never a replayed conversation: the
/// same question asked in two traces of a session is shown twice.
#[test]
fn a_bare_prompt_asked_twice_is_shown_twice() {
    let first = fixed_time();
    let second = first + chrono::Duration::seconds(60);
    let turn = |trace: &str, at: chrono::DateTime<Utc>, answer: &str| {
        let mut row = make_span_row_with_timestamps(
            trace,
            &format!("{trace}-call"),
            None,
            &json!([
                member_event(
                    "gen_ai.content.prompt",
                    at,
                    json!({"role": "user", "content": "Q"})
                ),
                member_event(
                    "gen_ai.completion",
                    at + chrono::Duration::seconds(1),
                    json!({"role": "assistant", "content": answer})
                ),
            ])
            .to_string(),
            at,
            Some(at + chrono::Duration::seconds(2)),
        );
        row.session_id = Some("session".to_string());
        row
    };
    let texts: Vec<String> = process_spans(
        vec![turn("trace1", first, "A1"), turn("trace2", second, "A2")],
        &FeedOptions::new(),
    )
    .messages
    .iter()
    .filter_map(|b| match &b.content {
        crate::sideml::types::ContentBlock::Text { text, .. } => Some(text.clone()),
        _ => None,
    })
    .collect();
    assert_eq!(texts, ["Q", "A1", "Q", "A2"]);
}

/// Every message of a trace carries its trace's session, as the store resolves it: a child that names another
/// session links its messages to the root's, the session the trace is in.
#[test]
fn a_childs_messages_carry_its_traces_session() {
    let turn = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "a question"}
    }]);
    let mut child = make_span_row(
        "trace1",
        "child",
        Some("root"),
        &turn.to_string(),
        "[]",
        "[]",
    );
    child.session_id = Some("child-session".to_string());
    let options = FeedOptions::new().with_session_of_trace(std::collections::HashMap::from([(
        "trace1".to_string(),
        "root-session".to_string(),
    )]));
    let feed = process_feed(vec![child], &options);
    assert!(!feed.messages.is_empty());
    assert!(
        feed.messages
            .iter()
            .all(|block| block.session_id.as_deref() == Some("root-session")),
        "{:?}",
        feed.messages
            .iter()
            .map(|block| block.session_id.clone())
            .collect::<Vec<_>>()
    );
}
