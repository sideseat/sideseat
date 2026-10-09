/// A result keeps the failure a dropped copy reported. OpenTelemetry's botocore instrumentation carries a
/// failed tool's result as a tool message, without a status, and inside the next request's user turn, with
/// one; the copy that won on quality read as a success.
#[test]
fn a_result_keeps_the_failure_a_dropped_copy_reported() {
    let current = make_tool_result_block("t1", "s1", "call_1", "No seats", utc(100));
    let mut resent = make_tool_result_block("t1", "s1", "call_1", "No seats", utc(100));
    resent.is_history = true;
    if let ContentBlock::ToolResult { is_error, .. } = &mut resent.content {
        *is_error = true;
    }
    let result = process_dedup(vec![current, resent], HashMap::new());
    assert_eq!(result.len(), 1);
    assert!(
        matches!(
            result[0].content,
            ContentBlock::ToolResult { is_error: true, .. }
        ),
        "{:?}",
        result[0].content
    );
}

fn anonymous(mut block: BlockEntry, name: Option<&str>) -> BlockEntry {
    if let ContentBlock::ToolResult {
        tool_use_id,
        name: named,
        ..
    } = &mut block.content
    {
        *tool_use_id = None;
        *named = name.map(str::to_string);
    }
    block.tool_use_id = None;
    block
}

/// A copy that names neither the call nor the tool - a later request re-sending a past result - is the
/// one named result of its content, and only when there is exactly one.
#[test]
fn an_anonymous_result_is_the_one_named_result_it_repeats() {
    let named = || {
        anonymous(
            make_tool_result_block("t1", "s1", "call_1", "Sunny, 22C", utc(100)),
            Some("get_weather"),
        )
    };
    let resent = |id: &str, content: &str| {
        let mut block = anonymous(
            make_tool_result_block("t1", "s2", id, content, utc(200)),
            None,
        );
        // In a later request's input: the provenance a re-send has.
        block.source_attribute = Some("gen_ai.input.messages".to_string());
        block.event_name = None;
        assert!(block.is_input_source());
        block
    };
    let result = process_dedup(
        vec![named(), resent("call_1", "Sunny, 22C")],
        HashMap::new(),
    );
    assert_eq!(
        result.len(),
        1,
        "{:?}",
        result.iter().map(|b| &b.content).collect::<Vec<_>>()
    );
    assert!(
        matches!(&result[0].content, ContentBlock::ToolResult { name: Some(n), .. } if n == "get_weather"),
        "the named copy survives: {:?}",
        result[0].content
    );

    // Two tools answering alike leave an anonymous copy naming neither.
    let blocks = vec![
        anonymous(
            make_tool_result_block("t1", "s1", "a", "ok", utc(100)),
            Some("book"),
        ),
        anonymous(
            make_tool_result_block("t1", "s1", "b", "ok", utc(101)),
            Some("cancel"),
        ),
        resent("c", "ok"),
    ];
    assert_eq!(process_dedup(blocks, HashMap::new()).len(), 3);

    // And an anonymous result a span produced itself is a result of its own, not a copy.
    let produced = anonymous(
        make_tool_result_block("t1", "s2", "call_2", "Sunny, 22C", utc(200)),
        None,
    );
    assert_eq!(
        process_dedup(vec![named(), produced], HashMap::new()).len(),
        2
    );
}

/// The result a tool span ran keeps the call id that only a re-listing elsewhere carried. The tool span's
/// copy wins on quality - it is the execution - and it was never told the model's id.
#[test]
fn a_result_keeps_the_id_a_dropped_copy_carried() {
    let mut executed = anonymous(
        make_tool_result_block("t1", "tool", "call_1", "Sunny", utc(200)),
        Some("weather"),
    );
    executed.observation_type = Some("tool".to_string());
    executed.source_attribute = Some("output.value".to_string());
    let mut relisted = make_tool_result_block("t1", "agent", "call_1", "Sunny", utc(100));
    if let ContentBlock::ToolResult { name, .. } = &mut relisted.content {
        *name = Some("weather".to_string());
    }
    relisted.observation_type = Some("agent".to_string());
    let result = process_dedup(vec![relisted, executed], HashMap::new());
    assert_eq!(result.len(), 1, "{:?}", result);
    assert_eq!(result[0].span_id, "tool", "the execution's copy survives");
    assert!(
        matches!(
            &result[0].content,
            ContentBlock::ToolResult { tool_use_id: Some(id), .. } if id == "call_1"
        ),
        "{:?}",
        result[0].content
    );
    assert_eq!(result[0].tool_use_id.as_deref(), Some("call_1"));
}

/// A history copy naming the call stays to be merged into the current id-less result it is: otherwise
/// the history filter drops the only copy with the id before the two are known to be one.
#[test]
fn a_current_result_takes_the_id_of_its_history_copy() {
    let executed = anonymous(
        make_tool_result_block("t1", "tool", "call_1", "Sunny", utc(200)),
        Some("weather"),
    );
    let mut relisted = make_tool_result_block("t1", "agent", "call_1", "Sunny", utc(100));
    if let ContentBlock::ToolResult { name, .. } = &mut relisted.content {
        *name = Some("weather".to_string());
    }
    relisted.is_history = true;
    let call = make_tool_use_block("t1", "tool", "call_1", "weather", utc(150));
    let result = process_dedup(
        vec![relisted.clone(), executed.clone(), call],
        HashMap::new(),
    );
    let results: Vec<_> = result.iter().filter(|b| b.is_tool_result()).collect();
    assert_eq!(results.len(), 1, "{:?}", result);
    assert_eq!(results[0].span_id, "tool");
    assert_eq!(results[0].tool_use_id.as_deref(), Some("call_1"));

    // An earlier turn's result with the same text answers an earlier call: the new result takes no id.
    let result = process_dedup(vec![relisted, executed], HashMap::new());
    assert_eq!(result.len(), 1, "{:?}", result);
    assert_eq!(result[0].tool_use_id, None, "{:?}", result);

    // A history copy nothing current is a copy of is still history.
    let mut past = make_tool_result_block("t1", "agent", "call_9", "Cloudy", utc(100));
    past.is_history = true;
    assert!(process_dedup(vec![past], HashMap::new()).is_empty());
}

/// A call keeps the provider's id a dropped copy carried; a survivor with an id of its own keeps that one.
#[test]
fn a_call_keeps_the_id_a_dropped_copy_carried() {
    let mut survivor = make_tool_use_block("t1", "model", "unused", "weather", utc(100));
    if let ContentBlock::ToolUse { id, .. } = &mut survivor.content {
        *id = None;
    }
    survivor.tool_use_id = None;
    survivor.model = Some("m".to_string());
    let carrier = make_tool_use_block("t1", "agent", "call_1", "weather", utc(150));
    let result = process_dedup(vec![survivor, carrier], HashMap::new());
    assert_eq!(result.len(), 1, "{:?}", result);
    assert_eq!(result[0].span_id, "model");
    assert_eq!(result[0].tool_use_id.as_deref(), Some("call_1"));
    assert!(matches!(
        &result[0].content,
        ContentBlock::ToolUse { id: Some(id), .. } if id == "call_1"
    ));

    let mut own = make_tool_use_block("t1", "model", "call_own", "weather", utc(100));
    own.model = Some("m".to_string());
    let other = make_tool_use_block("t1", "agent", "call_1", "weather", utc(150));
    let result = process_dedup(vec![own, other], HashMap::new());
    assert!(
        result
            .iter()
            .any(|b| b.tool_use_id.as_deref() == Some("call_own")),
        "an id the survivor carries is never replaced: {:?}",
        result
    );
}

/// A reply listed with how it finished by a whole-conversation carrier keeps that finish when an enclosing
/// span's re-listing is the copy that survives.
#[test]
fn a_reply_keeps_the_finish_a_dropped_copy_stated() {
    let mut listed = make_test_block(
        "t1",
        "conversation",
        ChatRole::Assistant,
        "Sunny.",
        utc(100),
    );
    listed.finish_reason = Some(crate::sideml::types::FinishReason::Stop);
    listed.is_history = true;
    let relisted = make_test_block("t1", "agent", ChatRole::Assistant, "Sunny.", utc(100));
    let result = process_dedup(vec![relisted, listed], HashMap::new());
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].finish_reason,
        Some(crate::sideml::types::FinishReason::Stop)
    );
}

/// A reply takes a finish only from a copy of the same occurrence. The previous turn's identical answer,
/// replayed as history before this one was produced, is not this reply's lineage, and how it finished
/// says nothing about how this one did.
#[test]
fn a_reply_takes_no_finish_from_an_earlier_turn_it_repeats() {
    let mut history = make_test_block(
        "trace1",
        "generation1",
        ChatRole::Assistant,
        "same answer",
        utc(0),
    );
    history.is_history = true;
    history.finish_reason = Some(crate::sideml::types::FinishReason::Length);
    let mut output = make_test_block(
        "trace1",
        "generation2",
        ChatRole::Assistant,
        "same answer",
        utc(1),
    );
    output.uses_span_end = true;
    let timestamps = HashMap::from([
        (
            "generation1".to_string(),
            SpanTimestamps {
                span_start: utc(0),
                span_end: Some(utc(0)),
            },
        ),
        (
            "generation2".to_string(),
            SpanTimestamps {
                span_start: utc(1),
                span_end: Some(utc(1)),
            },
        ),
    ]);
    let (survivors, lineage) = process_dedup_with_lineage(vec![history, output], timestamps);
    assert_eq!(lineage, vec![None, Some(0)]);
    assert_eq!(survivors[0].finish_reason, None);
}

/// What a span was sent precedes what it produced, even where the span starts and ends at one instant. The
/// earlier `OK`, sent with `length`, is history on the same span that answers `OK` again: it is not that
/// answer's lineage, and lends it no finish.
#[test]
fn a_reply_takes_no_finish_from_the_request_that_preceded_it() {
    let t = utc(1);
    let mut sent = make_test_block("trace1", "generation", ChatRole::Assistant, "OK", t);
    sent.is_history = true;
    sent.finish_reason = Some(crate::sideml::types::FinishReason::Length);
    sent.source_type = "attribute".to_string();
    sent.source_attribute = Some("gen_ai.input.messages".to_string());
    let mut reply = make_test_block("trace1", "generation", ChatRole::Assistant, "OK", t);
    reply.uses_span_end = true;
    reply.source_type = "attribute".to_string();
    reply.source_attribute = Some("gen_ai.output.messages".to_string());
    reply.message_index = 1;
    let timestamps = HashMap::from([(
        "generation".to_string(),
        SpanTimestamps {
            span_start: t,
            span_end: Some(t),
        },
    )]);
    let (survivors, lineage) = process_dedup_with_lineage(vec![sent, reply], timestamps);
    assert_eq!(survivors.len(), 1);
    assert_eq!(lineage, vec![None, Some(0)]);
    assert_eq!(survivors[0].finish_reason, None);
}

/// Two turns of withheld reasoning are two blocks, though both texts are empty: only their signatures, which
/// no view serialises, tell them apart. Two copies of one turn - a response and the next request re-sending
/// it - are one. The same holds for visible reasoning thought twice in the same words.
#[test]
fn reasoning_is_told_apart_by_its_signature() {
    let thinking = |span: &str, text: &str, signature: &str, at: i64| {
        let mut block = make_test_block("t1", span, ChatRole::Assistant, "", utc(at));
        block.entry_type = "thinking".to_string();
        block.content = ContentBlock::Thinking {
            text: text.to_string(),
            signature: Some(signature.to_string()),
        };
        block
    };
    for text in ["", "Check the forecast first."] {
        let mut resent = thinking("s3", text, "sig-a", 300);
        resent.is_history = true;
        let blocks = vec![
            thinking("s1", text, "sig-a", 100),
            thinking("s2", text, "sig-b", 200),
            resent,
        ];
        let result = process_dedup(blocks, HashMap::new());
        let signatures: Vec<Option<&str>> = result
            .iter()
            .map(|b| match &b.content {
                ContentBlock::Thinking { signature, .. } => signature.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(signatures, [Some("sig-a"), Some("sig-b")], "text {text:?}");
    }
}

/// One response reported twice, once by a carrier that drops the reasoning's signature: the unsigned copy
/// is the signed block, which keeps its signature. Where two signed blocks could be the one it copies,
/// it is left alone, as is a copy whose message holds anything else.
#[test]
fn an_unsigned_copy_of_reasoning_is_the_one_signed_block_it_copies() {
    let response = |span: &str, signature: Option<&str>, answer: &str| {
        let mut reasoning = make_test_block("t1", span, ChatRole::Assistant, "", utc(100));
        reasoning.entry_type = "thinking".to_string();
        reasoning.content = ContentBlock::Thinking {
            text: String::new(),
            signature: signature.map(str::to_string),
        };
        let mut text = make_test_block("t1", span, ChatRole::Assistant, answer, utc(100));
        text.entry_index = 1;
        vec![reasoning, text]
    };
    let signatures = |blocks: &[BlockEntry]| -> Vec<Option<String>> {
        let mut found: Vec<Option<String>> = blocks
            .iter()
            .filter_map(|b| match &b.content {
                ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
                _ => None,
            })
            .collect();
        found.sort();
        found
    };

    let unique = [
        response("chat", None, "Paris."),
        response("node", Some("sig-a"), "Paris."),
    ]
    .concat();
    let result = process_dedup(unique, HashMap::new());
    assert_eq!(signatures(&result), [Some("sig-a".to_string())]);

    let ambiguous = [
        response("chat", None, "Paris."),
        response("node", Some("sig-a"), "Paris."),
        response("other", Some("sig-b"), "Paris."),
    ]
    .concat();
    let result = process_dedup(ambiguous, HashMap::new());
    assert_eq!(
        signatures(&result),
        [None, Some("sig-a".to_string()), Some("sig-b".to_string())]
    );

    let another_turn = [
        response("chat", None, "Paris."),
        response("node", Some("sig-a"), "Rome."),
    ]
    .concat();
    let result = process_dedup(another_turn, HashMap::new());
    assert_eq!(signatures(&result), [None, Some("sig-a".to_string())]);
}

/// A response re-sent with a part its own report left out keeps that part, though only the re-send has it;
/// a re-send of two calls' parts says whose it is for neither, and the part stays history.
#[test]
fn a_part_only_a_re_sent_response_carries_is_kept() {
    let reported = |span: &str, text: &str| {
        let mut block = make_test_block("t1", span, ChatRole::Assistant, text, utc(100));
        block.observation_type = Some("generation".to_string());
        block.promoted_to_span_output = true;
        block
    };
    let resent = |span: &str, entry: i32, content: ContentBlock| {
        let mut block = make_test_block("t1", span, ChatRole::Assistant, "", utc(200));
        block.observation_type = Some("generation".to_string());
        block.is_history = true;
        block.message_index = 1;
        block.entry_index = entry;
        block.entry_type = content.block_type().to_string();
        block.content = content;
        block
    };
    let reasoning = || ContentBlock::Thinking {
        text: String::new(),
        signature: Some("sig".to_string()),
    };
    let text = |t: &str| ContentBlock::Text {
        text: t.to_string(),
    };
    let kept = |blocks: Vec<BlockEntry>| {
        process_dedup(blocks, HashMap::new())
            .iter()
            .any(|b| matches!(b.content, ContentBlock::Thinking { .. }))
    };

    assert!(kept(vec![
        reported("s1", "Paris."),
        resent("s2", 0, reasoning()),
        resent("s2", 1, text("Paris.")),
    ]));
    assert!(!kept(vec![
        reported("s1", "Paris."),
        reported("s3", "Rome."),
        resent("s2", 0, reasoning()),
        resent("s2", 1, text("Paris.")),
        resent("s2", 2, text("Rome.")),
    ]));
}

/// Two turns a client ran on one span, each reasoning without text, rank 0 and 1 in that emission; their
/// signed copies on two node spans each rank 0. Each unsigned copy is its turn's signed block, rank and all,
/// so neither survives twice.
#[test]
fn an_unsigned_copy_takes_the_rank_of_the_signed_block_it_copies() {
    use crate::sideml::provenance::PositionPath;
    let part = |span: &str, message: i32, entry: i32, content: ContentBlock, emitted: bool| {
        let mut block = make_test_block(
            "t1",
            span,
            ChatRole::Assistant,
            "",
            utc(100 + message as i64),
        );
        block.message_index = message;
        block.entry_index = entry;
        block.entry_type = content.block_type().to_string();
        block.content = content;
        block.position = PositionPath::root(message as usize).child_index(entry as usize);
        if emitted {
            block.event_name = Some("gen_ai.choice".to_string());
            block.observation_type = Some("generation".to_string());
        }
        block
    };
    let reasoning = |signature: Option<&str>| ContentBlock::Thinking {
        text: String::new(),
        signature: signature.map(str::to_string),
    };
    let text = |t: &str| ContentBlock::Text {
        text: t.to_string(),
    };
    let blocks = vec![
        part("chat", 0, 0, reasoning(None), true),
        part("chat", 0, 1, text("Paris."), true),
        part("chat", 1, 0, reasoning(None), true),
        part("chat", 1, 1, text("Rome."), true),
        part("node-1", 0, 0, reasoning(Some("sig-a")), false),
        part("node-1", 0, 1, text("Paris."), false),
        part("node-2", 0, 0, reasoning(Some("sig-b")), false),
        part("node-2", 0, 1, text("Rome."), false),
    ];
    let mut signatures: Vec<Option<String>> = process_dedup(blocks, HashMap::new())
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
            _ => None,
        })
        .collect();
    signatures.sort();
    assert_eq!(
        signatures,
        [Some("sig-a".to_string()), Some("sig-b".to_string())]
    );
}
