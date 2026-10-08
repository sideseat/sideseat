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
