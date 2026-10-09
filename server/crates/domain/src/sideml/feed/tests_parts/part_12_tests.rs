/// The feed must keep a trace whole when only its root span names the session.
///
/// Several frameworks record the session id on the root span alone. Grouping by each row's own id
/// then split a conversation: the root joined the session group and its children a trace group, so
/// history detection ran on the halves separately and a re-sent turn survived in one of them.
#[test]
fn the_feed_groups_a_trace_by_its_root_session_id() {
    let first = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "first question"}
    }]);
    // The child re-sends the first turn, as a generation span does, and adds nothing new.
    let child = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "first question"}
    }]);

    let mut root = make_span_row("trace1", "root", None, &first.to_string(), "[]", "[]");
    root.session_id = Some("session-1".to_string());
    // No session id on the child, which is what the frameworks in question emit.
    let child_row = make_span_row(
        "trace1",
        "child",
        Some("root"),
        &child.to_string(),
        "[]",
        "[]",
    );

    let feed = process_feed(vec![root, child_row], &FeedOptions::new());
    let users: Vec<&str> = feed
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .map(|b| b.span_id.as_str())
        .collect();
    assert_eq!(
        users.len(),
        1,
        "the re-sent turn survived, so the trace was processed as two conversations: {users:?}"
    );
}

/// A session's cost covers every trace in it, including one that only re-sent an earlier turn.
///
/// The multi-trace path added a trace's tokens only when it contributed a message the feed kept, so
/// a trace whose content was all history counted as free. It still called the model. The response
/// documents its totals as covering the spans in scope, and that is what they now do.
#[test]
fn a_replayed_trace_still_counts_towards_the_session() {
    let first = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "the question"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "the answer"}
        }
    ]);
    // The second trace re-sends the same turn and adds nothing.
    let replay = first.clone();

    let mut a = make_span_row("trace1", "span1", None, &first.to_string(), "[]", "[]");
    a.session_id = Some("session-1".to_string());
    let mut b = make_span_row("trace2", "span2", None, &replay.to_string(), "[]", "[]");
    b.session_id = Some("session-1".to_string());
    b.span_timestamp = a.span_timestamp + chrono::Duration::seconds(10);
    let per_trace_tokens = a.total_tokens;
    let per_trace_cost = a.cost_total;

    let result = process_spans(vec![a, b], &FeedOptions::new());

    assert_eq!(
        result.metadata.total_tokens,
        per_trace_tokens * 2,
        "the replayed trace called the model and must be counted"
    );
    assert!(
        (result.metadata.total_cost - per_trace_cost * 2.0).abs() < f64::EPSILON,
        "the replayed trace's cost is missing: {}",
        result.metadata.total_cost
    );
}

/// A span delivered twice is billed once, in a session exactly as in a trace.
///
/// The DuckDB session query reads the raw span table, so a retried OTLP delivery is two rows for
/// one span; ClickHouse reads it with FINAL and returns one. The single-trace path collapsed them
/// in `compute_metadata` and the multi-trace path summed rows, so a session reported double the
/// tokens of the traces it contains - and the two backends disagreed with each other.
#[test]
fn a_span_delivered_twice_is_counted_once_in_a_session() {
    let turn = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "the question"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "the answer"}
        }
    ]);
    let second = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:10Z"}},
            "content": {"role": "user", "content": "a second question"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:11Z"}},
            "content": {"role": "assistant", "content": "a second answer"}
        }
    ]);

    let mut a = make_span_row("trace1", "span1", None, &turn.to_string(), "[]", "[]");
    a.session_id = Some("session-1".to_string());
    // The same span, delivered again: identical trace and span id, as a retry produces.
    let redelivered = a.clone();
    let mut b = make_span_row("trace2", "span2", None, &second.to_string(), "[]", "[]");
    b.session_id = Some("session-1".to_string());
    b.span_timestamp = a.span_timestamp + chrono::Duration::seconds(10);
    let per_span_tokens = a.total_tokens;
    let per_span_cost = a.cost_total;

    let result = process_spans(vec![a, redelivered, b], &FeedOptions::new());

    assert_eq!(
        result.metadata.total_tokens,
        per_span_tokens * 2,
        "the retried delivery was billed a second time"
    );
    assert!(
        (result.metadata.total_cost - per_span_cost * 2.0).abs() < f64::EPSILON,
        "the retried delivery was charged twice: {}",
        result.metadata.total_cost
    );
    assert_eq!(
        result.metadata.span_count, 2,
        "one span delivered twice is still one span"
    );
}

/// Two identical calls in one response are two calls.
///
/// A tool call's identity ignores the provider's call id, because history re-sends regenerate ids and
/// the same call would otherwise appear twice. The cost was that a model asking for the same thing
/// twice in one response came back as one call - `crewai/mcp_tools` really does retry an identical
/// MCP call after a validation error, and the feed showed one call, one error, and an apology with
/// nothing to explain it. Within one response the ids are unambiguous, so each call takes the rank of
/// its id and that rank joins its identity.
#[test]
fn two_identical_calls_in_one_response_both_survive() {
    let t = fixed_time();
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "call_1", "name": "generate_image", "input": {"prompt": "a cat"}},
            {"type": "tool_use", "id": "call_2", "name": "generate_image", "input": {"prompt": "a cat"}}
        ]}
    }]);
    let row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::new());
    let ids: Vec<&str> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .filter_map(|b| b.tool_use_id.as_deref())
        .collect();
    assert_eq!(
        ids,
        vec!["call_1", "call_2"],
        "a response asking for the same thing twice must show both calls"
    );
}

/// ...and a history re-send of that pair is still one pair, whatever its ids became.
///
/// This is the reason ids are not simply part of the identity: a framework re-sending its history
/// regenerates them, and keying on the id would show every past call again on every turn. The rank is
/// per response, so the re-sent pair ranks 0 and 1 again and collapses onto the original pair.
#[test]
fn a_resent_pair_of_identical_calls_is_still_one_pair() {
    let t = fixed_time();
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "generate_image", "input": {"prompt": "a cat"}});
    // The generation span emits the pair; a later span re-sends it as history with new ids.
    let produced = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [call("call_1"), call("call_2")]}
    }]);
    let resent = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": (t + chrono::Duration::seconds(5)).to_rfc3339()}},
        "content": {"role": "assistant", "content": [call("regenerated_9"), call("regenerated_10")]}
    }]);

    let first = make_span_row("trace1", "span1", None, &produced.to_string(), "[]", "[]");
    let mut second = make_span_row("trace1", "span2", None, &resent.to_string(), "[]", "[]");
    second.span_timestamp = first.span_timestamp + chrono::Duration::seconds(5);

    let result = process_spans(vec![first, second], &FeedOptions::new());
    let calls = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .count();
    assert_eq!(
        calls,
        2,
        "the re-sent pair must collapse onto the original pair, not add to it: {:?}",
        result
            .messages
            .iter()
            .filter(|b| b.entry_type == "tool_use")
            .map(|b| b.tool_use_id.as_deref())
            .collect::<Vec<_>>()
    );
}

/// Two identical calls with *no* ids are still two calls.
///
/// This is what the position buys over the id-based rank it replaced: a framework that reports tool
/// calls without ids gave the rank nothing to work with, so a model asking for the same thing twice
/// came back as one call. The position is structure the payload stated, so it distinguishes them
/// whether or not ids were sent.
#[test]
fn two_identical_idless_calls_in_one_response_both_survive() {
    let t = fixed_time();
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [
            {"type": "tool_use", "name": "generate_image", "input": {"prompt": "a cat"}},
            {"type": "tool_use", "name": "generate_image", "input": {"prompt": "a cat"}}
        ]}
    }]);
    let row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::new());
    let calls = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .count();
    assert_eq!(
        calls, 2,
        "both id-less calls must survive: their positions differ even though nothing else does"
    );
}

/// A block records the route it was read by, and blocks of one payload never share a position.
///
/// The property everything else rests on. `gen_ai.input.messages` is an expandable array source, so
/// each entry's position names the array and its index - which is what tells two entries apart when
/// their content does not.
#[test]
fn every_block_carries_a_distinct_position_within_its_payload() {
    let t = fixed_time();
    let messages = json!([{
        "source": {"attribute": {"key": "gen_ai.input.messages", "time": t.to_rfc3339()}},
        "content": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "first question"},
            {"role": "user", "content": "second question"}
        ]
    }]);
    let row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::new());

    let positions: Vec<String> = result
        .messages
        .iter()
        .map(|b| b.position.to_string())
        .collect();
    assert_eq!(
        positions.len(),
        3,
        "the three entries must survive: {positions:?}"
    );
    let mut unique = positions.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        positions.len(),
        "two blocks of one payload share a position: {positions:?}"
    );
    // Route: observation 0 of the stored list, entry N of its array, content block 0.
    assert_eq!(
        unique,
        vec![
            "0.0.0".to_string(),
            "0.1.0".to_string(),
            "0.2.0".to_string()
        ],
        "a position must name the route it was read by: {positions:?}"
    );
}

/// A framework re-listing its own messages in one payload is not two calls.
///
/// LangChain's `output.value` carries accumulated state: the same tool call appears twice in one
/// attribute, at different positions, describing one call. Position alone therefore cannot decide a
/// repeat - ranking by it turned every such echo into a second call. The provider's id is the
/// evidence when there is one, and position is the fallback for payloads that carry no ids.
#[test]
fn an_echoed_call_within_one_payload_is_one_call() {
    let t = fixed_time();
    let call = json!({
        "type": "tool_use", "id": "tooluse_same", "name": "generate_image",
        "input": {"prompt": "a cat"}
    });
    // One carrier, the same call listed twice - accumulated state, not a repeat.
    let messages = json!([{
        "source": {"attribute": {"key": "output.value", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [call.clone(), call]}
    }]);
    let row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::new());
    let calls = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .count();
    assert_eq!(
        calls, 1,
        "the same id twice in one payload is one call, whatever its positions"
    );
}

/// With no ids, the *carrier* decides whether two identical calls are two calls.
///
/// The pair in `two_identical_idless_calls_in_one_response_both_survive` arrives in a
/// `gen_ai.choice` event - one emission, so two positions are two calls. The same pair in
/// `output.value` is accumulated framework state, which re-lists what it already said, so two
/// positions there describe one call. Nothing but the carrier distinguishes these two tests, which is
/// the point: the judgement is declared per carrier in `sideml::carrier` rather than guessed from
/// content.
#[test]
fn an_idless_echo_in_accumulated_state_is_one_call() {
    let t = fixed_time();
    let call = json!({"type": "tool_use", "name": "generate_image", "input": {"prompt": "a cat"}});
    let messages = json!([{
        "source": {"attribute": {"key": "output.value", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [call.clone(), call]}
    }]);
    let row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    let result = process_spans(vec![row], &FeedOptions::new());
    let calls = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .count();
    assert_eq!(
        calls, 1,
        "accumulated state re-lists itself, so two id-less positions there are one call"
    );
}

/// The number a block *reports* must not be the number it *sorts* by.
///
/// The two fields hold the same value, so no view can tell them apart today - which is exactly why a
/// later edit could quietly go back to reading `timestamp` and nothing would fail. Here the display
/// timestamps are rewritten into the reverse of the sort order, and the answer has to be unmoved.
/// The whole point of separating them is that the anchor can change without the reported time
/// following, and that only holds while the ordering path reads `order_time` alone.
#[test]
fn the_displayed_time_does_not_decide_the_order() {
    let first = fixed_time();
    let second = first + chrono::Duration::seconds(30);
    let turn = |question: &str, answer: &str, t: chrono::DateTime<Utc>| {
        json!([
            {"source": {"event": {"name": "gen_ai.user.message", "time": t.to_rfc3339()}},
             "content": {"role": "user", "content": question}},
            {"source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
             "content": {"role": "assistant", "content": answer}},
        ])
        .to_string()
    };
    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "span1",
            None,
            &turn("first question", "first answer", first),
            first,
            Some(first + chrono::Duration::seconds(1)),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            None,
            &turn("second question", "second answer", second),
            second,
            Some(second + chrono::Duration::seconds(1)),
        ),
    ];

    let blocks = process_spans(rows, &FeedOptions::new()).messages;
    assert!(
        blocks.len() >= 4,
        "need several blocks across two responses for an order to be observable, got {}",
        blocks.len()
    );
    let texts = |blocks: &[BlockEntry]| -> Vec<String> {
        blocks.iter().map(|b| b.content_hash.clone()).collect()
    };
    let expected = texts(&sort_feed_newest_first(blocks.clone()));

    // Ascending over a newest-first answer, so a sort that read them would hand back the reverse -
    // and spread far enough apart that no tie-break could mask it.
    let mut misreported = sort_feed_newest_first(blocks);
    for (i, block) in misreported.iter_mut().enumerate() {
        block.timestamp = first + chrono::Duration::hours(i as i64);
    }
    assert_eq!(
        texts(&sort_feed_newest_first(misreported)),
        expected,
        "the feed's order came from the displayed timestamp - the two fields are conflated again"
    );
}

// ----------------------------------------------------------------------------
// Test: a replay in a different valid order is still a replay
// ----------------------------------------------------------------------------

/// The provider's serialisation of parallel tool calls is a *different linearisation* of one turn.
///
/// A model that calls two tools at once emits them together, and the results come back as they come:
/// `call1, call2, result1, result2`. A conversation history is a flat message list, so the next turn
/// re-sends that turn as `call1, result1, call2, result2` - every call immediately followed by its own
/// result. Both orders satisfy the same constraints (each result after its own call; the two pairs
/// unordered against each other), so the second is not new content, it is the same turn written down
/// differently.
///
/// Matching that against a stored sequence with one forward cursor fails at the second call - it sits
/// *behind* the cursor, which already passed it to reach `result1` - and since a mismatch ends the
/// prefix, that call, its result and everything after leak into the session as duplicates. Matching
/// against the relation accepts it, because a call and the other call's result are incomparable.
#[test]
fn a_replay_that_reorders_incomparable_blocks_is_still_stripped() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(30);
    let t2 = t0 + chrono::Duration::seconds(60);

    // Turn 1, first call: the model emits both tool calls in one response.
    let span1 = json!([
        {"source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}}, "content": {"role": "user", "content": "Weather in NYC and LA?"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
         "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call_a", "name": "weather", "input": {"city": "NYC"}}, {"type": "tool_use", "id": "call_b", "name": "weather", "input": {"city": "LA"}}], "finish_reason": "tool_use"}}
    ]);

    // Turn 1, second call: the results come back, in the order they completed.
    let span2 = json!([
        {"source": {"attribute": {"key": "llm.input_messages", "time": t1.to_rfc3339()}}, "content": {"role": "user", "content": "Weather in NYC and LA?"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call_a", "name": "weather", "input": {"city": "NYC"}}, {"type": "tool_use", "id": "call_b", "name": "weather", "input": {"city": "LA"}}]}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t1.to_rfc3339()}}, "content": {"role": "tool", "tool_call_id": "call_a", "content": "NYC is sunny"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t1.to_rfc3339()}}, "content": {"role": "tool", "tool_call_id": "call_b", "content": "LA is warm"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
         "content": {"role": "assistant", "content": "NYC is sunny and LA is warm"}}
    ]);

    // Turn 2 replays turn 1 the way a provider writes a history down: each call beside its own result.
    let span3 = json!([
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Weather in NYC and LA?"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call_a", "name": "weather", "input": {"city": "NYC"}}]}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "tool", "tool_call_id": "call_a", "content": "NYC is sunny"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "call_b", "name": "weather", "input": {"city": "LA"}}]}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "tool", "tool_call_id": "call_b", "content": "LA is warm"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": "NYC is sunny and LA is warm"}},
        {"source": {"attribute": {"key": "llm.input_messages", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "And tomorrow?"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
         "content": {"role": "assistant", "content": "Tomorrow looks similar"}}
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &span1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace1",
            "s2",
            None,
            &span2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s3",
            None,
            &span3.to_string(),
            t2,
            Some(t2),
            Some("generation"),
        ),
    ];

    let result = process_spans(rows, &FeedOptions::default());
    let shape: Vec<(crate::sideml::types::ChatRole, &str)> = result
        .messages
        .iter()
        .map(|b| (b.role, b.entry_type.as_str()))
        .collect();

    let calls: Vec<&str> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .filter_map(|b| b.tool_use_id.as_deref())
        .collect();
    assert_eq!(
        calls.len(),
        2,
        "each call once, not once per order it was written in: {:?}",
        shape
    );
    let results = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_result")
        .count();
    assert_eq!(results, 2, "and each result once: {:?}", shape);
    assert_eq!(
        result.messages.len(),
        8,
        "the question, two calls, two results, the first answer, the new question, the second: {:?}",
        shape
    );
}

// ----------------------------------------------------------------------------
// The replay matcher, against shapes no captured fixture contains
// ----------------------------------------------------------------------------

/// Build prior state from `(role, hash)` identities and a relation over them, as one trace.
#[cfg(test)]
fn prior_state(
    identities: &[(ChatRole, &str)],
    edges: &[(usize, usize)],
) -> super::CrossTracePrefixState {
    let t0 = fixed_time();
    let transcript: Vec<BlockEntry> = identities
        .iter()
        .enumerate()
        .map(|(i, &(role, hash))| BlockEntry {
            scope_version: None,
            span_name: None,
            scope_name: None,
            position: PositionPath::default(),
            entry_type: "text".to_string(),
            content: ContentBlock::Text {
                text: hash.to_string(),
                citations: Vec::new(),
            },
            role,
            trace_id: "trace1".to_string(),
            span_id: "s1".to_string(),
            session_id: None,
            message_index: i as i32,
            entry_index: 0,
            parent_span_id: None,
            span_path: vec!["s1".to_string()],
            timestamp: t0,
            order_time: t0,
            occurrence_ordinal: 0,
            observation_type: Some("generation".to_string()),
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "attribute".to_string(),
            event_name: None,
            source_attribute: Some("llm.input_messages".to_string()),
            category: sideseat_ports::types::MessageCategory::GenAIUserMessage,
            content_hash: hash.to_string(),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
            is_cross_trace_history: false,
            tool_use_id_correlated: false,
            promoted_to_span_output: false,
            is_rendering: false,
        })
        .collect();
    let mut state = super::CrossTracePrefixState::default();
    state.push_trace(
        &transcript,
        super::order_graph::Precedence::from_edges(identities.len(), edges),
    );
    state
}

/// Two interchangeable results, and the choice between them decides whether the rest strips.
///
/// A counterexample to greedy replay matching, and the reason the matcher searches. Two unordered
/// branches give `callA -> resultA` and `callB -> resultB`, and both results carry the *same* identity -
/// two tools that each answered `"ok"`, which is ordinary. Replayed as `callB, resultB, callA, resultA`,
/// a valid linear extension, taking the first permitted candidate for `resultB` claims `resultA`; that
/// assignment then requires `callA` to have come earlier, so `callA` is refused and everything after it
/// is duplicated in the session view. Only the order constraints distinguish the two choices.
#[test]
fn interchangeable_results_do_not_end_the_prefix() {
    // 0: callA  1: resultA  2: callB  3: resultB, with each result after its own call and the two
    // branches unordered against each other.
    let prior = prior_state(
        &[
            (ChatRole::Assistant, "callA"),
            (ChatRole::Tool, "ok"),
            (ChatRole::Assistant, "callB"),
            (ChatRole::Tool, "ok"),
        ],
        &[(0, 1), (2, 3)],
    );

    let replay = [
        (ChatRole::Assistant, "callB"),
        (ChatRole::Tool, "ok"),
        (ChatRole::Assistant, "callA"),
        (ChatRole::Tool, "ok"),
    ];
    let (matched, _) = prior.longest_matching_prefix(&replay);
    assert_eq!(
        matched.len(),
        replay.len(),
        "the whole replay is a linear extension of the prior order, so all of it is history"
    );
    let mut consumed = matched.clone();
    consumed.sort_unstable();
    consumed.dedup();
    assert_eq!(
        consumed.len(),
        matched.len(),
        "and each block claimed a distinct occurrence"
    );
}

/// A replay that contradicts the order is *not* history, however identical its content.
///
/// The other side of the property: without this the matcher could strip anything that merely looks alike,
/// which would delete real messages rather than duplicate them.
#[test]
fn a_replay_that_contradicts_the_order_is_not_stripped() {
    // A tool result cannot precede the call it answers.
    let prior = prior_state(
        &[(ChatRole::Assistant, "call"), (ChatRole::Tool, "ok")],
        &[(0, 1)],
    );
    let (matched, _) =
        prior.longest_matching_prefix(&[(ChatRole::Tool, "ok"), (ChatRole::Assistant, "call")]);
    assert_eq!(
        matched.len(),
        1,
        "the result matches, and the call after it contradicts the evidence, so the prefix ends"
    );
}

/// Every linear extension of every relation over four blocks is recognised as a replay of it.
///
/// The completeness property, over *all* shapes at this size rather than over chosen examples. Enumerated
/// exhaustively: every one of the 64 edge sets over four blocks (each a DAG by construction, since only
/// forward pairs are offered), against every assignment of the blocks to a two-symbol identity alphabet
/// (16 of them), against every one of the 24 orders - keeping the orders that satisfy the constraints and
/// requiring each to match in full and injectively.
///
/// Identities repeat by design: interchangeable candidates are what make the choice non-obvious, and
/// both defects found here were about them. Choosing greedily among them fails the four-block
/// counterexample; choosing in stored order fails his ten-branch one, which
/// `ten_interchangeable_branches_replayed_in_reverse_are_fully_stripped` covers at a size this
/// enumeration cannot reach.
#[test]
fn every_linear_extension_of_every_small_relation_is_fully_stripped() {
    const BLOCKS: usize = 4;
    // The pairs an acyclic relation may contain when nodes are numbered in topological order.
    let forward_pairs: Vec<(usize, usize)> = (0..BLOCKS)
        .flat_map(|a| ((a + 1)..BLOCKS).map(move |b| (a, b)))
        .collect();

    fn permutations(n: usize) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        let mut current: Vec<usize> = (0..n).collect();
        fn go(current: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
            if k == current.len() {
                out.push(current.clone());
                return;
            }
            for i in k..current.len() {
                current.swap(k, i);
                go(current, k + 1, out);
                current.swap(k, i);
            }
        }
        go(&mut current, 0, &mut out);
        out
    }
    let orders = permutations(BLOCKS);

    let mut shapes = 0;
    let mut extensions = 0;
    for edge_mask in 0u32..(1 << forward_pairs.len()) {
        let edges: Vec<(usize, usize)> = forward_pairs
            .iter()
            .enumerate()
            .filter(|(i, _)| edge_mask & (1 << i) != 0)
            .map(|(_, &pair)| pair)
            .collect();

        for identity_mask in 0u32..(1 << BLOCKS) {
            // Two symbols, so blocks collide in identity as soon as the mask repeats a bit.
            let identities: Vec<(ChatRole, &str)> = (0..BLOCKS)
                .map(|i| {
                    if identity_mask & (1 << i) == 0 {
                        (ChatRole::Tool, "ok")
                    } else {
                        (ChatRole::Assistant, "call")
                    }
                })
                .collect();
            let prior = prior_state(&identities, &edges);
            shapes += 1;

            for order in &orders {
                let position: Vec<usize> = {
                    let mut p = vec![0; order.len()];
                    for (at, &block) in order.iter().enumerate() {
                        p[block] = at;
                    }
                    p
                };
                // Only a linear extension is a replay of this turn; anything else contradicts it.
                if edges.iter().any(|&(a, b)| position[a] > position[b]) {
                    continue;
                }
                extensions += 1;

                let replay: Vec<(ChatRole, &str)> = order.iter().map(|&i| identities[i]).collect();
                let (matched, _) = prior.longest_matching_prefix(&replay);
                assert_eq!(
                    matched.len(),
                    replay.len(),
                    "order {order:?} satisfies every constraint in {edges:?} with identities \
                     {identity_mask:04b}, so it is a replay - matched {} of {}",
                    matched.len(),
                    replay.len()
                );
                let mut distinct = matched.clone();
                distinct.sort_unstable();
                distinct.dedup();
                assert_eq!(distinct.len(), matched.len(), "matched injectively");
            }
        }
    }
    assert_eq!(
        shapes,
        64 * 16,
        "every relation and identity assignment was built"
    );
    assert!(
        extensions > 5_000,
        "only {extensions} linear extensions were checked, which is too few to be exhaustive"
    );
}

/// Ten interchangeable results, replayed in reverse: the shape that exhausts a naive search.
///
/// A counterexample about the *budget* rather than the matching rule. Ten
/// independent branches `call_i -> result_i` where every result carries the same identity - ten tools
/// that each answered `"ok"` - replayed branch by branch in reverse order. Every step then offers ten
/// permitted candidates that differ only in which call they answer, so a search that tries them in
/// stored order picks wrong nine times out of ten and spends its whole budget backtracking; it gives up
/// part way and the tail of the old turn is duplicated in the session view.
///
/// The fix is the order of exploration: try the candidate with the fewest unmatched ancestors first,
/// which is the one whose call the replay has just matched. Nothing about what is *permitted* changes,
/// so a shape the heuristic guesses wrong is still found by backtracking.
#[test]
fn ten_interchangeable_branches_replayed_in_reverse_are_fully_stripped() {
    const BRANCHES: usize = 10;

    // 2i: call_i (unique identity), 2i+1: result_i (all identical), with each result after its own call.
    let mut identities: Vec<(ChatRole, String)> = Vec::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for i in 0..BRANCHES {
        identities.push((ChatRole::Assistant, format!("call{i}")));
        identities.push((ChatRole::Tool, "ok".to_string()));
        edges.push((2 * i, 2 * i + 1));
    }
    let borrowed: Vec<(ChatRole, &str)> = identities
        .iter()
        .map(|(role, hash)| (*role, hash.as_str()))
        .collect();
    let prior = prior_state(&borrowed, &edges);

    // Reverse by branch, each call still before its own result: a valid linear extension.
    let mut replay: Vec<(ChatRole, &str)> = Vec::new();
    for i in (0..BRANCHES).rev() {
        replay.push(borrowed[2 * i]);
        replay.push(borrowed[2 * i + 1]);
    }

    let (matched, _) = prior.longest_matching_prefix(&replay);
    assert_eq!(
        matched.len(),
        replay.len(),
        "the whole replay is a linear extension, so all {} blocks are history; matched {}",
        replay.len(),
        matched.len()
    );
    let mut distinct = matched.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), matched.len(), "and injectively");
}

/// Nine identical calls with distinct results, replayed in reverse: the budget's counterexample.
///
/// The mirror counterexample. A tool call's identity deliberately excludes
/// the provider's call id, so nine calls of the same tool with the same input are *one* identity - which
/// is what a model retrying the same call produces. Their results differ. Replayed branch by branch in
/// reverse, every step offers nine permitted candidates for the call, and "fewest unmatched ancestors"
/// cannot separate them: a call has no ancestors at all, so they tie.
///
/// The disambiguation has to come from the blocks whose identity is *unique* - the results - which is why
/// the matcher matches in order of ambiguity rather than in replay order.
#[test]
fn nine_identical_calls_with_distinct_results_are_fully_stripped() {
    const BRANCHES: usize = 9;

    let mut identities: Vec<(ChatRole, String)> = Vec::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    // All calls first, then all results, as a transcript records them - and `call_i -> result_i`.
    for _ in 0..BRANCHES {
        identities.push((ChatRole::Assistant, "call".to_string()));
    }
    for i in 0..BRANCHES {
        identities.push((ChatRole::Tool, format!("result{i}")));
        edges.push((i, BRANCHES + i));
    }
    let borrowed: Vec<(ChatRole, &str)> = identities
        .iter()
        .map(|(role, hash)| (*role, hash.as_str()))
        .collect();
    let prior = prior_state(&borrowed, &edges);

    let mut replay: Vec<(ChatRole, &str)> = Vec::new();
    for i in (0..BRANCHES).rev() {
        replay.push(borrowed[i]);
        replay.push(borrowed[BRANCHES + i]);
    }

    let (matched, _) = prior.longest_matching_prefix(&replay);
    assert_eq!(
        matched.len(),
        replay.len(),
        "every constraint is satisfied, so all {} blocks are history; matched {}",
        replay.len(),
        matched.len()
    );
}
