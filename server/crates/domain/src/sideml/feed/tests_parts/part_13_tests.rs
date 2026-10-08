/// How far the bounded search reaches on the three-level shape, reported rather than assumed.
///
/// A construction with identical roots, identical middles, and unique leaves: `root_i -> middle_i ->
/// leaf_i`, replayed branch by branch in reverse. Two levels of interchangeable blocks rather than one.
#[test]
#[ignore]
fn probe_matcher_envelope_three_level() {
    for branches in [4usize, 6, 7, 8, 10, 12, 16] {
        let mut identities: Vec<(ChatRole, String)> = Vec::new();
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for _ in 0..branches {
            identities.push((ChatRole::Assistant, "root".to_string()));
        }
        for _ in 0..branches {
            identities.push((ChatRole::Assistant, "middle".to_string()));
        }
        for i in 0..branches {
            identities.push((ChatRole::Tool, format!("leaf{i}")));
            edges.push((i, branches + i));
            edges.push((branches + i, 2 * branches + i));
        }
        let borrowed: Vec<(ChatRole, &str)> = identities
            .iter()
            .map(|(role, hash)| (*role, hash.as_str()))
            .collect();
        let prior = prior_state(&borrowed, &edges);
        let mut replay: Vec<(ChatRole, &str)> = Vec::new();
        for i in (0..branches).rev() {
            replay.push(borrowed[i]);
            replay.push(borrowed[branches + i]);
            replay.push(borrowed[2 * branches + i]);
        }
        let start = std::time::Instant::now();
        let (matched, _) = prior.longest_matching_prefix(&replay);
        eprintln!(
            "THREE-LEVEL {branches:3} branches ({} blocks): matched {} in {:?}",
            replay.len(),
            matched.len(),
            start.elapsed()
        );
    }
}

/// How far the bounded search actually reaches, reported rather than assumed.
#[test]
#[ignore]
fn probe_matcher_envelope() {
    for branches in [9usize, 16, 24, 32, 48, 64] {
        let mut identities: Vec<(ChatRole, String)> = Vec::new();
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for _ in 0..branches {
            identities.push((ChatRole::Assistant, "call".to_string()));
        }
        for i in 0..branches {
            identities.push((ChatRole::Tool, format!("result{i}")));
            edges.push((i, branches + i));
        }
        let borrowed: Vec<(ChatRole, &str)> = identities
            .iter()
            .map(|(role, hash)| (*role, hash.as_str()))
            .collect();
        let prior = prior_state(&borrowed, &edges);
        let mut replay: Vec<(ChatRole, &str)> = Vec::new();
        for i in (0..branches).rev() {
            replay.push(borrowed[i]);
            replay.push(borrowed[branches + i]);
        }
        let start = std::time::Instant::now();
        let (matched, _) = prior.longest_matching_prefix(&replay);
        eprintln!(
            "ENVELOPE {branches:3} branches ({} blocks): matched {} in {:?}",
            replay.len(),
            matched.len(),
            start.elapsed()
        );
    }
}

/// When the search is cut short, the answer says so.
///
/// The budget is a resource guard, and a guard that silently changes the answer is the thing a caller
/// cannot reason about. So `longest_matching_prefix` reports whether it was exhaustive, and that travels to
/// `FeedMetadata::replay_matching_complete`: either the stripping is complete, or the response says it may
/// repeat history. Under-stripping is the safe direction - duplicated history rather than missing messages
/// - but only if the caller is told.
#[test]
fn an_incomplete_search_is_reported_as_incomplete() {
    // Three levels of interchangeable blocks, wide enough to exhaust the budget: identical roots,
    // identical middles, unique leaves, replayed branch by branch in reverse.
    const BRANCHES: usize = 12;
    let mut identities: Vec<(ChatRole, String)> = Vec::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for _ in 0..BRANCHES {
        identities.push((ChatRole::Assistant, "root".to_string()));
    }
    for _ in 0..BRANCHES {
        identities.push((ChatRole::Assistant, "middle".to_string()));
    }
    for i in 0..BRANCHES {
        identities.push((ChatRole::Tool, format!("leaf{i}")));
        edges.push((i, BRANCHES + i));
        edges.push((BRANCHES + i, 2 * BRANCHES + i));
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
        replay.push(borrowed[2 * BRANCHES + i]);
    }

    let (matched, exhaustive) = prior.longest_matching_prefix(&replay);
    assert!(
        !exhaustive,
        "this shape is meant to exhaust the budget; if it no longer does, widen it rather than delete \
         the test - the point is that the flag is reachable"
    );
    assert!(
        matched.len() < replay.len(),
        "and an exhausted search is exactly when the answer is short"
    );
    assert!(
        !matched.is_empty(),
        "but it still strips what it found, rather than giving up entirely"
    );
}

/// A span view is the normalized payload of that span, not a one-span trace reconstruction.
///
/// The ordinary history pass removes assistant messages from generation inputs because a wider trace or
/// session already has the authoritative output. With only the requested span in scope, that removal hid
/// context the span actually received and contradicted the span endpoint's contract.
#[test]
fn a_span_view_preserves_replayed_assistant_context() {
    let t0 = fixed_time();
    let messages = json!([
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "first question"}
        },
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "first answer"}
        },
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "second question"}
        },
        {
            "source": {"attribute": {"key": "llm.output_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "second answer"}
        }
    ]);
    let row = make_span_row_with_timestamps(
        "trace-span-context",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    );

    let span = process_span(vec![row.clone()], &FeedOptions::new());
    let span_text: Vec<&str> = span
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        span_text,
        [
            "first question",
            "first answer",
            "second question",
            "second answer"
        ]
    );

    let trace = process_spans(vec![row], &FeedOptions::new());
    assert!(
        !trace.messages.iter().any(
            |block| matches!(&block.content, ContentBlock::Text { text } if text == "first answer")
        ),
        "trace reconstruction still collapses input history"
    );
}

/// **Known limit**, asserted as it behaves: two spans starting at the same instant are ordered by their
/// span ids, and an id-less tool result whose call lands on the far side of that tie stays uncorrelated.
///
/// Between spans, document order is `(timestamp_start, trace_id, span_id)`, and correlation is a single
/// forward pass over it - so when a tool span and the generation span that called it share a start time
/// (ordinary with a millisecond clock) the pairing depends on two random bytes. With the ids the other way
/// round the same telemetry correlates, which the second half of this test shows.
///
/// The obvious repair - letting a result also claim a *following* call in a span that starts at the same
/// instant - was implemented and reverted. It changed `adk/tool_use` for the worse in every variant tried
/// (an equal alternative to the preceding rule, and a fallback used only when no preceding call exists):
/// ADK's tool and generation spans do tie, so the relaxation lets one result claim a call that a later
/// result needed, and results that *had* ids lost them - three of them, with their order changing to put
/// the results before their calls. Rules 3 and 4 have nothing but document order to go on, and relaxing
/// them where that order is arbitrary trades a rare mis-order for a common mis-pairing.
///
/// What a real fix needs is causal evidence that does not come from the span id: the ordering redesign's
/// partial order (`order_graph`), where a call→result edge is a constraint rather than a position. Recorded
/// here so the next attempt starts from the measurement rather than the idea.
#[test]
fn an_idless_result_is_correlated_only_when_span_ids_order_its_call_first() {
    let t = fixed_time();
    let call = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call-1", "name": "get_weather",
                         "input": {"city": "Paris"}}]
        }
    }]);
    let result = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "name": "get_weather", "content": "sunny"}]
        }
    }]);

    let feed_for = |tool_span: &str, gen_span: &str| {
        let mut rows = vec![
            make_span_row_full(
                "t1",
                tool_span,
                None,
                &result.to_string(),
                t,
                Some(t),
                Some("tool"),
            ),
            make_span_row_full(
                "t1",
                gen_span,
                None,
                &call.to_string(),
                t,
                Some(t),
                Some("generation"),
            ),
        ];
        // As the query delivers them: `ORDER BY timestamp_start, trace_id, span_id`. With the starts equal
        // the span id is the whole order, which is the point of this test.
        rows.sort_by(|a, b| a.span_id.cmp(&b.span_id));
        process_spans(rows, &FeedOptions::default())
            .messages
            .iter()
            .map(|b| (b.entry_type.clone(), b.tool_use_id.clone()))
            .collect::<Vec<_>>()
    };

    // The call's span sorts first: correlated, and the result follows its call.
    assert_eq!(
        feed_for("b-tool", "a-gen"),
        vec![
            ("tool_use".to_string(), Some("call-1".to_string())),
            ("tool_result".to_string(), Some("call-1".to_string())),
        ],
        "when document order puts the call first, the result answers it"
    );

    // The result's span sorts first: uncorrelated, and it precedes the call it answers. This is the limit.
    assert_eq!(
        feed_for("a-tool", "b-gen"),
        vec![
            ("tool_result".to_string(), None),
            ("tool_use".to_string(), Some("call-1".to_string())),
        ],
        "the same telemetry, ordered by span id the other way, leaves the result uncorrelated"
    );
}

/// Millisecond-quantized sibling timestamps must not let random span ids scramble a tool turn.
///
/// This is the shape emitted by a fast OpenTelemetry JavaScript request: the runtime gives the
/// first generation one millisecond and the tool/final-generation spans the next, while the database
/// necessarily falls back to `span_id` inside the tie. The ids below deliberately sort final before
/// tool. Payload causality is still sufficient to recover
/// user -> preamble -> call -> result -> final.
#[test]
fn an_unambiguous_quantized_sibling_tool_turn_ignores_span_id_order() {
    use super::order_graph::Constraints;

    let t = fixed_time();
    let tied = t + chrono::Duration::milliseconds(1);
    let first_generation = json!([
        {
            "source": {"attribute": {"key": "gen_ai.input.messages", "time": t}},
            "content": {"role": "user", "content": "What is the weather?"}
        },
        {
            "source": {"attribute": {"key": "gen_ai.output.messages", "time": t}},
            "content": {
                "role": "assistant",
                "content": "I will check.",
                "finish_reason": "tool_use"
            }
        }
    ]);
    let tool = json!([
        {
            "source": {"attribute": {"key": "gen_ai.tool.call.arguments", "time": t}},
            "content": {
                "role": "assistant",
                "content": {
                    "type": "tool_use",
                    "id": "call-weather",
                    "name": "get_weather",
                    "input": {"city": "London"}
                }
            }
        },
        {
            "source": {"attribute": {"key": "gen_ai.tool.call.result", "time": t}},
            "content": {
                "role": "tool",
                "content": {
                    "type": "tool_result",
                    "tool_use_id": "call-weather",
                    "content": "sunny"
                }
            }
        }
    ]);
    let final_generation = json!([{
        "source": {"attribute": {"key": "gen_ai.output.messages", "time": t}},
        "content": {
            "role": "assistant",
            "content": "It is sunny.",
            "finish_reason": "stop"
        }
    }]);

    let mut rows = vec![
        make_span_row_full(
            "trace-equal-time",
            "a-tool",
            Some("root"),
            &tool.to_string(),
            tied,
            Some(tied),
            Some("tool"),
        ),
        make_span_row_full(
            "trace-equal-time",
            "b-final",
            Some("root"),
            &final_generation.to_string(),
            tied,
            Some(tied),
            Some("generation"),
        ),
        make_span_row_full(
            "trace-equal-time",
            "z-preamble",
            Some("root"),
            &first_generation.to_string(),
            t,
            Some(t),
            Some("generation"),
        ),
    ];
    rows.sort_by(|a, b| a.span_id.cmp(&b.span_id));

    let result = super::process_spans_unfiltered_with(rows, Constraints::PRODUCTION);
    let shape: Vec<(ChatRole, &str)> = result
        .messages
        .iter()
        .map(|block| (block.role, block.entry_type.as_str()))
        .collect();
    assert_eq!(
        shape,
        vec![
            (ChatRole::User, "text"),
            (ChatRole::Assistant, "text"),
            (ChatRole::Assistant, "tool_use"),
            (ChatRole::Tool, "tool_result"),
            (ChatRole::Assistant, "text"),
        ]
    );
}

/// The feed is a projection of the resolved order, never a re-sort of it.
///
/// This is what "the resolver is the ordering authority" means for the one non-chronological view:
/// `sort_feed_newest_first` may regroup responses and reverse the groups, and it may not change the
/// order of two blocks within a response. The old implementation did - a second scalar tuple undid
/// whatever the resolver had improved, which is how `bedrock/converse` showed the answer *before* the
/// tool result it used.
///
/// Asserted as three properties of the projection, none of which needs plumbing to the pre-feed order:
/// every response's subsequence is unchanged (full fingerprint, not role/kind); each response is one
/// contiguous run; and the runs descend by `(order_time, trace_id)`, so an anchor tie between two
/// traces has one deterministic answer.
#[test]
fn the_feed_projects_the_resolved_order_without_resorting_it() {
    let t = fixed_time();
    // Two traces, interleaved anchors, one response with several blocks including a call and result -
    // enough structure that a re-sort of any term would be visible.
    let turn = |q: &str, id: &str, time: chrono::DateTime<chrono::Utc>| {
        serde_json::json!([
            {"source": {"event": {"name": "gen_ai.user.message", "time": time.to_rfc3339()}},
             "content": {"role": "user", "content": q}},
            {"source": {"event": {"name": "gen_ai.choice", "time": time.to_rfc3339()}},
             "content": {"role": "assistant", "content": [
                 {"type": "text", "text": format!("thinking about {q}")},
                 {"type": "tool_use", "id": id, "name": "look_up", "input": {"q": q}}
             ]}},
            {"source": {"event": {"name": "gen_ai.tool.message", "time": (time + chrono::Duration::seconds(1)).to_rfc3339()}},
             "content": {"role": "tool", "content": [
                 {"type": "tool_result", "tool_use_id": id, "content": format!("answer to {q}")}
             ]}},
        ])
        .to_string()
    };
    let rows = vec![
        make_span_row_with_timestamps(
            "trace-a",
            "span-a",
            None,
            &turn("alpha", "call_a", t),
            t,
            Some(t + chrono::Duration::seconds(2)),
        ),
        make_span_row_with_timestamps(
            "trace-b",
            "span-b",
            None,
            &turn("beta", "call_b", t + chrono::Duration::seconds(10)),
            t + chrono::Duration::seconds(10),
            Some(t + chrono::Duration::seconds(12)),
        ),
    ];

    let resolved = process_spans(rows, &FeedOptions::new()).messages;
    assert!(resolved.len() >= 6, "got {}", resolved.len());
    let fingerprint = |b: &BlockEntry| {
        format!(
            "{}/{}/{}/{}/{}#{}",
            b.trace_id, b.span_id, b.message_index, b.entry_index, b.entry_type, b.content_hash
        )
    };
    let response_of = |b: &BlockEntry| (b.order_time, b.trace_id.clone());

    let feed = sort_feed_newest_first(resolved.clone());

    // 1. Within each response, the subsequence is byte-for-byte the resolved one.
    let subsequence = |blocks: &[BlockEntry]| {
        let mut map: std::collections::BTreeMap<_, Vec<String>> = std::collections::BTreeMap::new();
        for b in blocks {
            map.entry(response_of(b)).or_default().push(fingerprint(b));
        }
        map
    };
    assert_eq!(
        subsequence(&resolved),
        subsequence(&feed),
        "the feed changed a response's internal order - it re-sorted instead of projecting"
    );

    // 2. Each response is one contiguous run.
    let mut seen = std::collections::HashSet::new();
    let mut current = None;
    for b in &feed {
        let key = response_of(b);
        if current.as_ref() != Some(&key) {
            assert!(
                seen.insert(key.clone()),
                "response {key:?} appears in two separate runs"
            );
            current = Some(key);
        }
    }

    // 3. Runs descend by (order_time, trace_id).
    let mut anchors: Vec<_> = Vec::new();
    for b in &feed {
        let key = response_of(b);
        if anchors.last() != Some(&key) {
            anchors.push(key);
        }
    }
    let mut sorted = anchors.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(anchors, sorted, "responses are not newest-first");
}

/// A *single* call re-sent once with a regenerated id is still one call.
///
/// The pair form of this is pinned above; the single form is the case review 25 found unguarded, and
/// it slips past the pair's own defence. Two executions are told apart from a re-sent pair by how many
/// calls of one shape a single response lists - but a lone call re-sent once lists its shape once in
/// *each* response, so that discriminator sees two "executions", and with the provider's regenerated id
/// trusted trace-wide the echo ranked as a second call. The guard the rank scope's comment always
/// claimed - only a **non-history** call ranks trace-wide - is what this test holds in place.
#[test]
fn a_resent_single_call_with_a_regenerated_id_is_still_one_call() {
    let t = fixed_time();
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "generate_image", "input": {"prompt": "a cat"}});
    let produced = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t.to_rfc3339()}},
        "content": {"role": "assistant", "content": [call("call_1")]}
    }]);
    // A later span re-sends the conversation; the framework regenerated the call id.
    let resent = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": (t + chrono::Duration::seconds(5)).to_rfc3339()}},
        "content": {"role": "assistant", "content": [call("regenerated_9")]}
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
        1,
        "the re-sent call must collapse onto the original, not rank as a second execution: {:?}",
        result
            .messages
            .iter()
            .filter(|b| b.entry_type == "tool_use")
            .map(|b| b.tool_use_id.as_deref())
            .collect::<Vec<_>>()
    );
}

/// A **stored** tool name that is not a string does not cost its siblings.
///
/// The column was read as `Vec<String>`, so one stored non-string - `["search", 7]`, from a producer that wrote
/// a number - failed the whole deserialisation and took the valid `"search"` with it. A malformed item poisoning
/// its siblings at the last possible moment, after storage had already accepted it.
///
/// The emission path keeps a non-string out now, so this shape can only arrive from a row written before that.
/// Which is exactly why it needs its own test: no fixture stores one, so nothing else exercises the read.
#[test]
fn a_stored_tool_name_that_is_not_a_string_costs_only_itself() {
    let row = make_span_row(
        "trace-1",
        "span-1",
        None,
        "[]",
        "[]",
        r#"["search", 7, "  ", "calculate"]"#,
    );
    let result = process_spans(vec![row], &FeedOptions::default());
    // Sorted: the feed deduplicates and orders tool names, which is its own concern - what this asserts is
    // that both survived.
    let mut names = result.tool_names.clone();
    names.sort();
    assert_eq!(
        names,
        vec!["calculate".to_string(), "search".to_string()],
        "the number and the blank name nothing; the two real names are kept"
    );
}

/// The declared shapes leave the retired canonicaliser nothing to do.
///
/// It ran *after* `normalize_tools` and wrapped whatever the assets had not recognised - a second vocabulary of
/// provider spellings downstream of the declared one. So the same tool was shown wrapped on the path that ran it
/// and raw on the path that did not (`normalize_tools_message` files its result straight into a
/// `ToolDefinitions` block). The claim now is that applying it to the declared output is the identity, which is
/// what makes removing it a statement rather than a hope: if a shape stops being declared, this fails.
#[test]
fn the_declared_shapes_leave_nothing_for_the_retired_canonicaliser() {
    let payloads = vec![
        serde_json::json!({"type": "function", "function": {"name": "a", "parameters": {"type": "object"}}}),
        serde_json::json!({"type": "function", "function": {"name": "a"}, "strict": true}),
        serde_json::json!({"name": "b", "description": "d", "input_schema": {"type": "object"}}),
        serde_json::json!({"type": "function", "name": "b", "inputSchema": {"type": "object"}}),
        serde_json::json!({"name": "b", "parameters": {"type": "object"}, "strict": false}),
        serde_json::json!({"toolSpec": {"name": "c", "inputSchema": {"json": {"type": "object"}}}}),
        serde_json::json!({"functionDeclarations": [{"name": "d1"}, {"name": "d2", "parameters": {}}]}),
        serde_json::json!({"function_declarations": [{"name": "e1"}]}),
        serde_json::json!({"name": "f", "parameter_definitions": {"q": {"type": "str", "required": true}}}),
        serde_json::json!({"name": "g"}),
        serde_json::json!({"description": "no name"}),
        serde_json::json!("a string"),
    ];
    for payload in payloads {
        let normalized = crate::sideml::tools::normalize_tools(&payload);
        let definitions = normalized.as_array().cloned().unwrap_or_default();
        assert!(
            !definitions.is_empty() || !payload.is_object(),
            "an object payload should yield at least a passthrough: {payload}"
        );
        for definition in definitions {
            assert_eq!(
                super::canonicalize_tool_definition(definition.clone()),
                definition,
                "the retired canonicaliser still changes declared output, for {payload}"
            );
        }
    }
}

/// The declared shapes state the schema wherever the retired canonicaliser could find one.
///
/// The identity check above cannot see this, and that is the whole reason this test exists: undeclaring a
/// spelling makes the plan emit a definition with **no** `parameters`, and the retired canonicaliser returns any
/// value carrying `function` untouched - so the identity still holds while the schema has silently gone. An
/// assertion that holds when the answer got worse is not a gate.
///
/// The three spellings are the retired chain's own vocabulary, which is what makes this an oracle rather than a
/// restatement of the asset.
#[test]
fn the_declared_shapes_state_every_schema_the_retired_canonicaliser_found() {
    let payloads = vec![
        serde_json::json!({"name": "a", "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}}),
        serde_json::json!({"type": "function", "name": "b", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}}),
        serde_json::json!({"name": "c", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}),
        serde_json::json!({"name": "d", "input_schema": {"type": "object"}, "strict": true}),
    ];
    for payload in payloads {
        let schema = ["parameters", "input_schema", "inputSchema"]
            .iter()
            .find_map(|member| payload.get(*member))
            .expect("every payload here carries a schema under one of the spellings");
        let normalized = crate::sideml::tools::normalize_tools(&payload);
        let definitions = normalized.as_array().cloned().unwrap_or_default();
        assert_eq!(definitions.len(), 1, "one definition, for {payload}");
        assert_eq!(
            definitions[0]
                .get("function")
                .and_then(|f| f.get("parameters")),
            Some(schema),
            "the declared shapes dropped the schema the retired canonicaliser found, for {payload}"
        );
        // And `strict` survives, since it says what the model may send.
        if let Some(strict) = payload.get("strict") {
            assert_eq!(definitions[0].get("strict"), Some(strict));
        }
    }
}

/// A generation span that both **received and produced** the same message: two barriers, not a product.
///
/// The dataflow class says everything a generation received precedes everything it produced. Where the two
/// sets overlap - the span re-sent a message it also emitted - that relation cannot be expressed by one
/// barrier without asserting `u -> barrier -> u`, so the code kept the *product* for those spans. The
/// product has two problems this case exhibits.
///
/// It is unbounded: a span re-sending a long history has hundreds of inputs, so the edge count is quadratic
/// in a span's message count - the exact growth the barrier exists to remove, still reachable through that
/// branch.
///
/// And for two or more shared units it **contradicts itself**: the product contains `u -> v` and `v -> u`
/// for every shared pair, so the resolver breaks a cycle the code manufactured and the order after it is a
/// deterministic guess rather than a derived one. The two-barrier construction omits exactly those pairs -
/// which is the honest reading, since a unit a span both received and produced is a replay, and one span's
/// dataflow says nothing about where two such units sit relative to each other.
///
/// No corpus fixture has an overlapping span, which is why `a_barrier_orders_exactly_as_pairwise_edges_do`
/// cannot see any of this and why the case is constructed here.
#[test]
fn an_overlapping_generation_span_is_ordered_without_a_manufactured_cycle() {
    use super::order_graph::{CYCLES_BROKEN_IN_TESTS, Constraints};

    // One generation span whose input side re-lists two assistant messages it also produced, so both
    // become units on both sides. The re-listing is a carrier with no declared direction: one declared to
    // hold what the span was sent precedes what it produced, so its copies are earlier occurrences and
    // never the produced units (`a_reply_takes_no_finish_from_the_request_that_preceded_it`).
    // The two produced messages come from **different** emission carriers, so contraction leaves them as
    // two units: two blocks of one `gen_ai.choice` are one emission and would contract into a single unit,
    // where the product's self-pair is skipped and there is no cycle to make.
    let replayed = json!([
        {"source": {"attribute": {"key": "app.conversation", "time": fixed_time()}},
         "content": {"role": "assistant", "content": "first answer"}},
        {"source": {"attribute": {"key": "app.conversation", "time": fixed_time()}},
         "content": {"role": "assistant", "content": "second answer"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": fixed_time()}},
         "content": {"role": "assistant", "content": "first answer"}},
        {"source": {"attribute": {"key": "output.value", "time": fixed_time()}},
         "content": {"role": "assistant", "content": "second answer"}}
    ]);
    let mut row = make_span_row(
        "trace-overlap",
        "span-1",
        None,
        &replayed.to_string(),
        "[]",
        "[]",
    );
    // The dataflow class reads only generation spans, since a model call is where a received message and a
    // produced one meet.
    row.observation_type = Some("generation".to_string());

    let count_cycles = |constraints: Constraints| -> (usize, usize) {
        CYCLES_BROKEN_IN_TESTS.with(|c| *c.borrow_mut() = 0);
        let result = super::process_spans_unfiltered_with(vec![row.clone()], constraints);
        (
            CYCLES_BROKEN_IN_TESTS.with(|c| *c.borrow()),
            result.messages.len(),
        )
    };

    let (barrier_cycles, barrier_blocks) = count_cycles(Constraints::PRODUCTION);
    let (pairwise_cycles, pairwise_blocks) = count_cycles(Constraints {
        pairwise_dataflow_edges: true,
        ..Constraints::PRODUCTION
    });

    assert!(
        barrier_blocks > 0 && barrier_blocks == pairwise_blocks,
        "both constructions must return the same blocks - this is about their order, not their membership: \
         {barrier_blocks} against {pairwise_blocks}"
    );
    assert_eq!(
        barrier_cycles, 0,
        "the two-barrier construction states only the pairs that can all hold, so nothing contradicts"
    );
    assert!(
        pairwise_cycles > 0,
        "the product asserts `u -> v` and `v -> u` for the shared pair, so it must break a cycle it made \
         itself - if this stops firing the case no longer exercises the overlap branch"
    );
}

/// A client that runs the tool loop itself reports every round on one generation span: the output
/// array holds the call and, after it, the answer the model wrote once the result came back. Both
/// rounds carry the one attribute's timestamp, and read as one response they left no room for the
/// tool span's result, which came back last - after the answer it produced.
#[test]
fn a_tool_result_sits_between_the_rounds_of_one_generation_span() {
    let t0 = fixed_time();
    let at = |ms: i64| t0 + chrono::Duration::milliseconds(ms);
    let generation = json!([
        {
            "source": {"attribute": {"key": "gen_ai.input.messages", "time": at(0).to_rfc3339()}},
            "content": [{"role": "user", "parts": [{"type": "text", "content": "Weather in Rome?"}]}]
        },
        {
            "source": {"attribute": {"key": "gen_ai.output.messages", "time": at(30).to_rfc3339()}},
            "content": [
                {"role": "assistant", "finish_reason": "stop", "parts": [
                    {"type": "tool_call", "id": "call-1", "name": "get_weather", "arguments": {"city": "Rome"}}
                ]},
                {"role": "assistant", "finish_reason": "stop", "parts": [
                    {"type": "text", "content": "Sunny, wear light layers."}
                ]}
            ]
        }
    ]);
    let tool = json!([{
        "source": {"attribute": {"key": "gen_ai.tool.call.result", "time": at(20).to_rfc3339()}},
        "content": {"role": "tool", "content": [
            {"type": "tool_result", "tool_use_id": "call-1", "name": "get_weather", "content": "sunny"}
        ]}
    }]);
    let mut tool_row = make_span_row_with_timestamps(
        "trace1",
        "tool",
        Some("generation"),
        &tool.to_string(),
        at(10),
        Some(at(20)),
    );
    tool_row.observation_type = Some("tool".to_string());
    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "generation",
            None,
            &generation.to_string(),
            at(0),
            Some(at(30)),
        ),
        tool_row,
    ];

    let kinds: Vec<String> = process_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .map(|b| b.entry_type.clone())
        .collect();

    assert_eq!(kinds, ["text", "tool_use", "tool_result", "text"]);
}

/// A choiceless generation re-lists earlier replies as `gen_ai.assistant.message` history, the same
/// event its own reply uses. A reply an earlier generation already stated is that history, so the
/// second turn shows it once - it used to be promoted to the second span's output and shown again.
#[test]
fn a_choiceless_generation_does_not_promote_a_reply_an_earlier_generation_stated() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(2);
    let event = |name: &str, role: &str, text: &str, at: DateTime<Utc>| {
        json!({
            "source": {"event": {"name": name, "time": at.to_rfc3339()}},
            "content": {"role": role, "content": text}
        })
    };
    let first = json!([
        event("gen_ai.user.message", "user", "Name a neighbourhood.", t0),
        event(
            "gen_ai.assistant.message",
            "assistant",
            "Stay in Chiado.",
            t0
        ),
    ]);
    let second = json!([
        event("gen_ai.user.message", "user", "Name a neighbourhood.", t1),
        event(
            "gen_ai.assistant.message",
            "assistant",
            "Stay in Chiado.",
            t1
        ),
        event("gen_ai.user.message", "user", "And a dish?", t1),
        event("gen_ai.assistant.message", "assistant", "Try bacalhau.", t1),
    ]);
    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "generation-1",
            Some("agent"),
            &first.to_string(),
            t0,
            Some(t0 + chrono::Duration::seconds(1)),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "generation-2",
            Some("agent"),
            &second.to_string(),
            t1,
            Some(t1 + chrono::Duration::seconds(1)),
        ),
    ];

    let texts: Vec<(ChatRole, String)> = process_spans(rows, &FeedOptions::default())
        .messages
        .iter()
        .map(|block| match &block.content {
            ContentBlock::Text { text } => (block.role, text.clone()),
            other => panic!("unexpected block {other:?}"),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            (ChatRole::User, "Name a neighbourhood.".to_string()),
            (ChatRole::Assistant, "Stay in Chiado.".to_string()),
            (ChatRole::User, "And a dish?".to_string()),
            (ChatRole::Assistant, "Try bacalhau.".to_string()),
        ],
        "each reply once, in the order the conversation happened"
    );
}

/// Log-carried messages are read as the span events they are: appended to the span's own, given roles by
/// their event names, and a turn reported both ways appears once.
#[test]
fn log_carried_messages_join_the_span_they_name() {
    let question = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"content": "Name a primary colour."}
    }]);
    let answer = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:01Z"}},
        "content": {"index": "0", "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "Red."}}
    }]);
    let mut row = make_span_row("trace1", "span1", None, &question.to_string(), "[]", "[]");
    row.observation_type = Some("generation".to_string());
    let mut logs = question.as_array().unwrap().clone();
    logs.extend(answer.as_array().unwrap().iter().cloned());
    row.log_messages_json = serde_json::Value::Array(logs).to_string();

    let result = process_spans(vec![row.clone()], &FeedOptions::new());
    let turns: Vec<(ChatRole, String)> = result
        .messages
        .iter()
        .map(|block| {
            let ContentBlock::Text { text } = &block.content else {
                panic!("text expected: {block:?}");
            };
            (block.role, text.clone())
        })
        .collect();
    assert_eq!(
        turns,
        vec![
            (ChatRole::User, "Name a primary colour.".to_string()),
            (ChatRole::Assistant, "Red.".to_string()),
        ]
    );

    // Unparseable log messages leave the span's own conversation intact rather than dropping it.
    row.log_messages_json = "not json".to_string();
    assert_eq!(
        process_spans(vec![row], &FeedOptions::new()).messages.len(),
        1
    );
}
