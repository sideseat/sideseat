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
