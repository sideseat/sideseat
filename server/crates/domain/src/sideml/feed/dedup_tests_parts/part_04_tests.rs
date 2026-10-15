#[test]
fn test_tool_result_same_id_different_content_deduped() {
    // Same tool_use_id → same identity, regardless of content format.
    // Covers Vercel AI SDK toModelOutput: raw execute() output vs transformed format.
    let t0 = utc(0);
    let t1 = utc(1);

    // Raw tool result from tool span
    let mut raw = make_tool_result_block("trace1", "tool_span", "call_123", "raw result data", t0);
    raw.observation_type = Some("tool".to_string());

    // Transformed tool result from generation span (different content, same tool_use_id)
    let mut transformed = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: Some("call_123".to_string()),
            name: None,
            content: serde_json::json!({"type": "content", "value": [{"type": "text", "text": "transformed result"}]}),
            is_error: false,
            provider_executed: false,
        },
        span_id: "gen_span".to_string(),
        timestamp: t1,
        observation_type: Some("generation".to_string()),
        ..make_tool_result_block("trace1", "gen_span", "call_123", "", t1)
    };
    transformed.model = Some("claude-haiku".to_string());

    let span_timestamps = HashMap::from([
        (
            "tool_span".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "gen_span".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let result = process_dedup(vec![raw, transformed], span_timestamps);

    // Same tool_use_id → deduped to 1 (tool_use_id is identity, not content)
    assert_eq!(result.len(), 1);
    // Tool span version wins via FROM_TOOL_SPAN quality bonus
    assert_eq!(result[0].span_id, "tool_span");
}

#[test]
fn test_tool_result_different_ids_not_deduped_by_tool_id() {
    // Different tool_use_ids should NOT be merged even if both are tool results
    let t0 = utc(0);

    let result1 = make_tool_result_block("trace1", "span1", "call_111", "result A", t0);
    let result2 = make_tool_result_block("trace1", "span2", "call_222", "result B", t0);

    let span_timestamps = HashMap::from([
        (
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span2".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
    ]);

    let result = process_dedup(vec![result1, result2], span_timestamps);

    // Both should be kept (different tool_use_ids = different identities)
    assert_eq!(result.len(), 2);
}

/// The band space is bounded below `own_output`'s bit: a trace and carrier past 2^14 delta spans folds the
/// overflow onto its last band - a deliberate bound, so adversarial span counts degrade within their own
/// group instead of colliding with another pass's namespace.
#[test]
fn delta_bands_past_their_space_fold_onto_the_last_band() {
    const LAST_BAND: usize = (1 << 14) - 1;
    let threads: Vec<String> = (0..=LAST_BAND + 1).map(|n| format!("th-{n}")).collect();
    let spans: Vec<String> = (0..=LAST_BAND + 1).map(|n| format!("s{n}")).collect();
    let blocks: Vec<BlockEntry> = (0..=LAST_BAND + 1)
        .map(|n| {
            threaded_delta_block(
                &threads[n],
                &spans[n],
                0,
                "Tokens left: 1",
                utc(100 + n as i64),
            )
        })
        .collect();
    let survivors = process_dedup(blocks, HashMap::new());
    assert_eq!(
        survivors.len(),
        LAST_BAND + 1,
        "the span past the band space folds onto the last band"
    );
}

/// A resumption restates the thread's whole history, not only the previous delta: after `[A]` and `[B]`, a
/// resumed turn says `[A, B, C]`, and A and B fold onto the originals while only C is new. Compared against
/// the previous delta alone, the restated A and B each showed twice.
#[test]
fn a_resumption_restates_the_thread_s_whole_history() {
    let survivors = process_dedup(
        vec![
            delta_block("s1", 0, "Env: /tmp", utc(100)),
            delta_block("s2", 0, "Tokens left: 2", utc(200)),
            delta_block("s3", 0, "Env: /tmp", utc(300)),
            delta_block("s3", 1, "Tokens left: 2", utc(300)),
            delta_block("s3", 2, "Tokens left: 1", utc(300)),
        ],
        HashMap::new(),
    );
    assert_eq!(survivors.len(), 3, "{survivors:?}");
}

/// A restated member lands exactly on its original's ordinal, rank included: after `[A]` and `[A]` - two
/// statements, the equal lists being fresh deliveries - a resumption `[A, A, B]` restates both. Its second A
/// sits at a different position than the original did, so inheriting only the band left it one rank off and
/// a fourth statement appeared.
#[test]
fn a_restated_member_lands_on_its_original_s_rank() {
    let survivors = process_dedup(
        vec![
            delta_block("s1", 0, "Tokens left: 1", utc(100)),
            delta_block("s2", 0, "Tokens left: 1", utc(200)),
            delta_block("s3", 0, "Tokens left: 1", utc(300)),
            delta_block("s3", 1, "Tokens left: 1", utc(300)),
            delta_block("s3", 2, "Env: /tmp", utc(300)),
        ],
        HashMap::new(),
    );
    assert_eq!(survivors.len(), 3, "{survivors:?}");
}

/// A result's identity keeps the distinctions the canonical form keeps: a one-element list left standing is
/// the tool's own list, and a part with citations or a non-text sibling says more than its text - each hashed
/// as joined text merged distinct results. A list of plain text parts is still its joined text.
#[test]
fn a_result_identity_keeps_what_the_canonical_form_keeps() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let text = |t: &str| serde_json::json!({"type": "text", "text": t});
    assert_ne!(
        hash(serde_json::json!(["ok"])),
        hash(serde_json::json!("ok")),
        "the tool's own list of one"
    );
    assert_eq!(
        hash(serde_json::json!([text("ok")])),
        hash(serde_json::json!("ok")),
        "a plain text part is its text"
    );
    assert_ne!(
        hash(serde_json::json!([text("a"), {"type": "image", "source": "url", "data": "u"}])),
        hash(serde_json::json!([text("a"), text("b")]))
    );
    let cited = serde_json::json!({"type": "text", "text": "a", "citations": [{"kind": "url"}]});
    assert_ne!(
        hash(serde_json::json!([cited, text("b")])),
        hash(serde_json::json!([text("a"), text("b")]))
    );
    assert_eq!(
        hash(serde_json::json!([text("a"), text("b")])),
        hash(serde_json::json!("a\nb")),
        "plain text parts are their joined text"
    );
}

/// Every spelling the canonical form treats as one result hashes as one, and nothing it keeps apart merges:
/// a json part is its data beside siblings as alone, a typed object standing as the content is the list holding
/// it, and a text with citations is not its text.
#[test]
fn a_result_identity_follows_the_canonical_form() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let datum = serde_json::json!({"city": "Paris"});
    let part = serde_json::json!({"type": "json", "data": datum.clone()});
    assert_eq!(hash(serde_json::json!([part.clone()])), hash(datum.clone()));
    assert_eq!(hash(part.clone()), hash(datum.clone()));
    assert_ne!(
        hash(serde_json::json!([datum.clone()])),
        hash(datum.clone())
    );
    let text = serde_json::json!({"type": "text", "text": "a"});
    assert_eq!(
        hash(serde_json::json!([text.clone(), part])),
        hash(serde_json::json!([text, datum])),
        "a json sibling is its data, wrapped or not"
    );
    let cited = |source: &str| serde_json::json!({"type": "text", "text": "a", "citations": [{"kind": "url", "source": source}]});
    assert_eq!(hash(cited("s")), hash(serde_json::json!([cited("s")])));
    assert_ne!(hash(cited("s")), hash(cited("t")));
    assert_ne!(hash(cited("s")), hash(serde_json::json!("a")));
    let extra = serde_json::json!({"type": "json", "data": {"ok": true}, "note": 1});
    assert_ne!(hash(extra), hash(serde_json::json!({"ok": true})));
}

/// A json part is its data only where the data is plain - nothing the content chain reads as a block - and a
/// returned list is its elements however it was wrapped: a json-wrapped value spelled like an image is not the
/// image, and one list returned through two readers is one answer.
#[test]
fn a_result_identity_unwraps_only_plain_data() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let image =
        serde_json::json!({"type": "image", "source": "url", "data": "https://example.com/a.png"});
    let text = serde_json::json!({"type": "text", "text": "done"});
    assert_ne!(
        hash(serde_json::json!([{"type": "json", "data": image.clone()}, text.clone()])),
        hash(serde_json::json!([image, text])),
        "returned JSON spelled like an image is not the image"
    );
    let media = serde_json::json!({"mime_type": "image/png", "data": "AAAA"});
    assert_ne!(
        hash(serde_json::json!([{"type": "json", "data": media.clone()}])),
        hash(media),
        "an untyped shape a case reads as a block keeps its part"
    );
    let city = serde_json::json!({"city": "Paris"});
    assert_eq!(
        hash(serde_json::json!([{"type": "json", "data": [city.clone()]}])),
        hash(serde_json::json!([city])),
        "a returned list of one, wrapped or listed"
    );
    assert_eq!(
        hash(serde_json::json!([{"type": "json", "data": ["a", "b"]}])),
        hash(serde_json::json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}])),
        "a returned list of strings is its texts"
    );
    assert_eq!(
        hash(serde_json::json!([{"type": "json", "data": ["a"]}])),
        hash(serde_json::json!(["a"]))
    );
    assert_ne!(hash(serde_json::json!(["a"])), hash(serde_json::json!("a")));
}

/// The identity guards hold under a declared structured-value wrapper, for a reader's `unknown` primitive
/// elements, and at any depth a parse produces.
#[test]
fn a_result_identity_guards_wrappers_primitives_and_depth() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let image = serde_json::json!({"type": "image", "source": "url", "data": "u"});
    let text = serde_json::json!({"type": "text", "text": "done"});
    assert_ne!(
        hash(serde_json::json!([{"type": "json", "data": {"json": image.clone()}}, text.clone()])),
        hash(serde_json::json!([image, text]))
    );
    assert_eq!(
        hash(serde_json::json!([{"type": "unknown", "raw": 7}])),
        hash(serde_json::json!([{"type": "json", "data": [7]}]))
    );
    let mut deep = serde_json::json!({"city": "Paris"});
    for _ in 0..9 {
        deep = serde_json::json!([deep]);
    }
    assert_eq!(
        hash(deep.clone()),
        hash(serde_json::json!([{"type": "json", "data": deep}]))
    );
}

/// A json part around a declared wrapper keeps its distinction from the bare value, as the canonical form does.
#[test]
fn a_wrapped_value_in_a_json_part_is_not_the_bare_value() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let city = serde_json::json!({"city": "Paris"});
    assert_ne!(
        hash(serde_json::json!([{"type": "json", "data": {"json": city.clone()}}])),
        hash(city)
    );
}

/// A scalar result and its replay through a json part are one result, as they were before the canonical form.
#[test]
fn a_scalar_result_and_its_wrapped_replay_hash_alike() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    for scalar in [
        serde_json::json!(7),
        serde_json::json!(true),
        serde_json::json!(null),
    ] {
        assert_eq!(
            hash(scalar.clone()),
            hash(serde_json::json!([{"type": "json", "data": scalar}]))
        );
    }
}

/// A declared wrapper's value is the value however it is listed, and a scalar is the text it renders as - one
/// producer states a result both ways.
#[test]
fn a_result_identity_reads_wrappers_as_values_and_scalars_as_their_text() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let city = serde_json::json!({"city": "Paris"});
    assert_eq!(
        hash(serde_json::json!({"json": [city.clone()]})),
        hash(serde_json::json!([city]))
    );
    assert_eq!(
        hash(serde_json::json!({"json": 7})),
        hash(serde_json::json!({"type": "json", "data": 7}))
    );
    for (scalar, text) in [
        (serde_json::json!(7), "7"),
        (serde_json::json!(true), "true"),
        (serde_json::json!(null), "null"),
    ] {
        let text = serde_json::json!(text);
        assert_eq!(
            hash(serde_json::json!({"type": "json", "data": scalar.clone()})),
            hash(text.clone())
        );
        assert_eq!(
            hash(serde_json::json!([{"type": "json", "data": scalar.clone()}])),
            hash(text.clone())
        );
        assert_eq!(hash(scalar), hash(text));
    }
}

/// A declared wrapper around block-shaped data is returned JSON, as a json part around it is - neither is the
/// blocks it is spelled like.
#[test]
fn a_wrapped_block_list_is_returned_json_not_the_blocks() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let image = serde_json::json!({"type": "image", "source": "url", "data": "u"});
    assert_ne!(
        hash(serde_json::json!({"json": [image.clone()]})),
        hash(serde_json::json!([image.clone()]))
    );
    assert_ne!(
        hash(serde_json::json!({"type": "json", "data": [image.clone()]})),
        hash(serde_json::json!([image]))
    );
}

/// A declared wrapper around block-shaped data is a json part by another spelling, and hashes as one.
#[test]
fn a_wrapped_block_list_hashes_as_its_json_part() {
    let hash = |content: serde_json::Value| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        super::identity::hash_tool_result_content_into(&content, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let image = serde_json::json!({"type": "image", "source": "url", "data": "u"});
    assert_eq!(
        hash(serde_json::json!({"json": [image.clone()]})),
        hash(serde_json::json!([{"type": "json", "data": [image]}]))
    );
}

/// A call only a later request re-sends - its own response recorded none of it - is kept where a current result
/// answers it: the result says the call happened. Not where a current copy of the call exists (the re-send folds onto
/// it), where the result answers it in another trace, or where nothing current answers it.
#[test]
fn a_re_sent_call_that_a_current_result_answers_is_kept() {
    let t0 = utc(0);
    let request = |trace: &str| {
        let mut call = make_tool_use_block(trace, "second", "call-1", "get_weather", t0);
        call.observation_type = Some("generation".to_string());
        call.source_type = "attribute".to_string();
        call.source_attribute = Some("gen_ai.prompt.2.content".to_string());
        call.message_index = 2;
        call.is_history = true;
        call
    };
    let result = |trace: &str| {
        let mut result = make_tool_result_block(trace, "second", "call-1", "Sunny.", t0);
        result.observation_type = Some("generation".to_string());
        result.source_type = "attribute".to_string();
        result.source_attribute = Some("gen_ai.prompt.3.content".to_string());
        result.message_index = 3;
        result
    };
    let calls = |blocks: Vec<BlockEntry>| {
        process_dedup(blocks, HashMap::new())
            .iter()
            .filter(|b| matches!(b.content, ContentBlock::ToolUse { .. }))
            .map(|b| b.span_id.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(calls(vec![request("trace1"), result("trace1")]), ["second"]);

    let mut reported = make_tool_use_block("trace1", "first", "call-1", "get_weather", t0);
    reported.observation_type = Some("generation".to_string());
    reported.source_type = "attribute".to_string();
    reported.source_attribute = Some("gen_ai.completion.0.content".to_string());
    assert_eq!(
        calls(vec![reported, request("trace1"), result("trace1")]),
        ["first"],
        "a call its response reported is that copy, once"
    );
    assert!(
        calls(vec![request("trace1"), result("trace2")]).is_empty(),
        "a result in another trace answers another call"
    );
    assert!(
        calls(vec![request("trace1")]).is_empty(),
        "nothing current answers it"
    );

    // The same call carried in from another trace's history, under that trace's id, is that trace's call: keeping
    // the local copy must not bring it back.
    let mut carried = make_tool_use_block("trace1", "second", "old-id", "get_weather", t0);
    carried.observation_type = Some("generation".to_string());
    carried.source_type = "attribute".to_string();
    carried.source_attribute = Some("gen_ai.prompt.0.content".to_string());
    carried.is_history = true;
    carried.is_cross_trace_history = true;
    let kept: Vec<Option<String>> = process_dedup(
        vec![carried, request("trace1"), result("trace1")],
        HashMap::new(),
    )
    .iter()
    .filter_map(|b| match &b.content {
        ContentBlock::ToolUse { id, .. } => Some(id.clone()),
        _ => None,
    })
    .collect();
    assert_eq!(
        kept,
        [Some("call-1".to_string())],
        "only the local copy, under its own id"
    );
}
