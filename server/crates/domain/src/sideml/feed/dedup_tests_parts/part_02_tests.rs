#[test]
fn history_before_an_identical_new_output_is_not_its_lineage() {
    let t0 = utc(0);
    let t1 = utc(1);
    let mut history = make_test_block(
        "trace1",
        "generation1",
        ChatRole::Assistant,
        "same answer",
        t0,
    );
    history.is_history = true;
    let mut output = make_test_block(
        "trace1",
        "generation2",
        ChatRole::Assistant,
        "same answer",
        t1,
    );
    output.uses_span_end = true;
    let timestamps = HashMap::from([
        (
            "generation1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "generation2".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let (survivors, lineage) = process_dedup_with_lineage(vec![history, output], timestamps);
    assert_eq!(survivors.len(), 1);
    assert_eq!(lineage, vec![None, Some(0)]);
}

#[test]
fn one_call_republished_by_agent_and_tool_spans_keeps_one_ordinal() {
    let t_before = utc(-1);
    let t0 = utc(0);
    let t1 = utc(1);
    let mut generation = make_tool_use_block("trace1", "generation", "call-1", "search", t0);
    generation.observation_type = Some("generation".to_string());
    generation.source_type = "attribute".to_string();
    generation.source_attribute = Some("gen_ai.output.messages".to_string());

    let mut agent = generation.clone();
    agent.span_id = "agent".to_string();
    agent.timestamp = t_before;
    agent.observation_type = Some("agent".to_string());
    agent.source_attribute = Some("pydantic_ai.all_messages".to_string());
    let mut tool = generation.clone();
    tool.span_id = "tool".to_string();
    tool.observation_type = Some("tool".to_string());
    tool.source_attribute = Some("gen_ai.tool.call.arguments".to_string());
    let mut replay = generation.clone();
    replay.span_id = "next-generation".to_string();
    replay.timestamp = t1;
    replay.observation_type = Some("generation".to_string());
    replay.source_attribute = Some("gen_ai.input.messages".to_string());
    replay.is_history = true;

    assert_eq!(
        call_repeat_ordinals(&[agent, generation, tool, replay]),
        vec![0, 0, 0, 0]
    );
}

#[test]
fn one_generation_republishing_a_call_through_two_carriers_is_one_execution() {
    let t0 = utc(0);
    let mut attribute = make_tool_use_block("trace1", "generation", "call-1", "search", t0);
    attribute.observation_type = Some("generation".to_string());
    attribute.source_type = "attribute".to_string();
    attribute.source_attribute = Some("gen_ai.output.messages".to_string());
    attribute.message_index = 1;

    let mut event = attribute.clone();
    event.source_type = "event".to_string();
    event.source_attribute = None;
    event.event_name = Some("gen_ai.choice".to_string());
    event.message_index = 4;

    assert_eq!(
        call_repeat_ordinals(&[attribute.clone(), event.clone()]),
        vec![0, 0]
    );
    assert_eq!(
        call_repeat_ordinals(&[attribute.clone(), event.clone(), attribute, event]),
        vec![0, 0, 0, 0],
        "redelivering both carriers must not create another execution"
    );
}

#[test]
fn choiceless_generation_outputs_rank_distinct_ids_across_spans() {
    let t0 = utc(0);
    let t1 = utc(1);
    let mut first = make_tool_use_block("trace1", "generation-1", "call-1", "search", t0);
    first.observation_type = Some("generation".to_string());
    first.event_name = Some("gen_ai.assistant.message".to_string());
    first.category = MessageCategory::GenAIAssistantMessage;
    first.promoted_to_span_output = true;

    let mut second = first.clone();
    second.span_id = "generation-2".to_string();
    second.span_path = vec![second.span_id.clone()];
    second.timestamp = t1;
    second.order_time = t1;
    second.tool_use_id = Some("call-2".to_string());
    if let ContentBlock::ToolUse { id, .. } = &mut second.content {
        *id = Some("call-2".to_string());
    }

    assert_eq!(
        call_repeat_ordinals(&[first.clone(), first, second.clone(), second]),
        vec![0, 0, 1, 1],
        "redelivering each span must not hide the later execution"
    );
}

#[test]
fn identical_generation_text_from_distinct_executions_keeps_each_occurrence() {
    let t0 = utc(0);
    let t1 = utc(1);
    let mut first = make_test_block(
        "trace1",
        "generation-1",
        ChatRole::Assistant,
        "same answer",
        t0,
    );
    first.observation_type = Some("generation".to_string());
    first.source_type = "attribute".to_string();
    first.source_attribute = Some("gen_ai.output.messages".to_string());
    first.uses_span_end = true;
    first.finish_reason = Some(FinishReason::Stop);

    let mut second = first.clone();
    second.span_id = "generation-2".to_string();
    second.span_path = vec![second.span_id.clone()];
    second.timestamp = t1;
    second.order_time = t1;

    assert_eq!(
        call_repeat_ordinals(&[first.clone(), first.clone(), second.clone(), second.clone()]),
        vec![0, 0, 1, 1],
        "redelivery is one observation, while a second generation is a second answer"
    );

    let timestamps = HashMap::from([
        (
            first.span_id.clone(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            second.span_id.clone(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);
    let deduped = process_dedup(vec![first, second], timestamps);
    assert_eq!(
        deduped.len(),
        2,
        "content equality must not erase a separately executed model response"
    );
    assert_eq!(
        deduped
            .iter()
            .map(|block| block.timestamp)
            .collect::<Vec<_>>(),
        vec![t0, t1],
        "each occurrence keeps its own birth time instead of inheriting the first"
    );
}

#[test]
fn identical_requests_on_separate_top_level_agent_spans_are_separate_turns() {
    let t0 = utc(0);
    let t1 = utc(1);
    let mut first = make_test_block("trace1", "agent-1", ChatRole::User, "same request", t0);
    first.observation_type = Some("agent".to_string());
    first.span_name = Some("invoke_agent stable-agent".to_string());
    first.source_type = "attribute".to_string();
    first.source_attribute = Some("gen_ai.input.messages".to_string());
    first.parent_span_id = Some("session".to_string());
    first.span_path = vec!["session".to_string(), first.span_id.clone()];

    let mut nested_copy = first.clone();
    nested_copy.span_id = "subagent".to_string();
    nested_copy.parent_span_id = Some(first.span_id.clone());
    nested_copy.span_path = vec![
        "session".to_string(),
        first.span_id.clone(),
        nested_copy.span_id.clone(),
    ];
    let mut first_generation_copy = first.clone();
    first_generation_copy.span_id = "generation-1".to_string();
    first_generation_copy.observation_type = Some("generation".to_string());
    first_generation_copy.parent_span_id = Some(first.span_id.clone());
    first_generation_copy.span_path = vec![
        "session".to_string(),
        first.span_id.clone(),
        first_generation_copy.span_id.clone(),
    ];

    let mut second = first.clone();
    second.span_id = "agent-2".to_string();
    second.span_path = vec!["session".to_string(), second.span_id.clone()];
    second.timestamp = t1;
    second.order_time = t1;
    let mut second_generation_copy = second.clone();
    second_generation_copy.span_id = "generation-2".to_string();
    second_generation_copy.observation_type = Some("generation".to_string());
    second_generation_copy.parent_span_id = Some(second.span_id.clone());
    second_generation_copy.span_path = vec![
        "session".to_string(),
        second.span_id.clone(),
        second_generation_copy.span_id.clone(),
    ];

    assert_eq!(
        call_repeat_ordinals(&[
            first,
            nested_copy,
            first_generation_copy,
            second,
            second_generation_copy
        ]),
        vec![0, 0, 0, 1, 1],
        "descendants inherit their invocation, while a new top-level agent is a new user turn"
    );
}

#[test]
fn peer_agents_receiving_one_prompt_do_not_create_user_turns() {
    let agent = |span: &str, identity: &str, time: i64| {
        let mut block =
            make_test_block("trace1", span, ChatRole::User, "shared request", utc(time));
        block.observation_type = Some("agent".to_string());
        block.span_name = Some(format!("invoke_agent {identity}"));
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("gen_ai.input.messages".to_string());
        block.parent_span_id = Some("session".to_string());
        block.span_path = vec!["session".to_string(), span.to_string()];
        block
    };

    assert_eq!(
        call_repeat_ordinals(&[
            agent("researcher", "agent-a", 0),
            agent("architect", "agent-b", 1),
            agent("marketing", "agent-c", 2),
        ]),
        vec![0, 0, 0],
        "fan-out is one external user turn even though several agents receive it"
    );
}

#[test]
fn nested_agent_loop_nodes_do_not_create_user_turns() {
    let nested_agent = |span: &str, workflow: &str, time: i64| {
        let mut block =
            make_test_block("trace1", span, ChatRole::User, "shared request", utc(time));
        block.observation_type = Some("agent".to_string());
        block.span_name = Some("agent".to_string());
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("gen_ai.input.messages".to_string());
        block.parent_span_id = Some(workflow.to_string());
        block.span_path = vec![
            "trace-root".to_string(),
            workflow.to_string(),
            span.to_string(),
        ];
        block
    };

    assert_eq!(
        call_repeat_ordinals(&[
            nested_agent("agent-1", "loop-1", 0),
            nested_agent("agent-2", "loop-2", 1),
        ]),
        vec![0, 0],
        "nested workflow agents replay one external request rather than creating new turns"
    );
}

#[test]
fn an_ancestor_request_carrier_keeps_child_agents_in_the_same_turn() {
    let mut root = make_test_block(
        "trace1",
        "workflow",
        ChatRole::User,
        "shared request",
        utc(0),
    );
    root.observation_type = Some("chain".to_string());
    root.source_type = "attribute".to_string();
    root.source_attribute = Some("gen_ai.input.messages".to_string());
    root.span_path = vec![root.span_id.clone()];
    let root_id = root.span_id.clone();

    let child = |span: &str, time: i64| {
        let mut block =
            make_test_block("trace1", span, ChatRole::User, "shared request", utc(time));
        block.observation_type = Some("agent".to_string());
        block.span_name = Some("agent".to_string());
        block.source_type = "attribute".to_string();
        block.source_attribute = Some("gen_ai.input.messages".to_string());
        block.parent_span_id = Some(root_id.clone());
        block.span_path = vec![root_id.clone(), span.to_string()];
        block
    };

    assert_eq!(
        call_repeat_ordinals(&[root, child("agent-1", 1), child("agent-2", 2)]),
        vec![0, 0, 0],
        "a workflow's child agents consume its request rather than creating external requests"
    );
}

#[test]
fn a_generation_wrapper_inherits_its_leaf_execution() {
    let mut wrapper = make_test_block(
        "trace1",
        "wrapper",
        ChatRole::Assistant,
        "same answer",
        utc(0),
    );
    wrapper.observation_type = Some("generation".to_string());
    wrapper.source_type = "attribute".to_string();
    wrapper.source_attribute = Some("gen_ai.output.messages".to_string());
    wrapper.span_path = vec![wrapper.span_id.clone()];
    wrapper.uses_span_end = true;

    let mut leaf = wrapper.clone();
    leaf.span_id = "leaf".to_string();
    leaf.parent_span_id = Some(wrapper.span_id.clone());
    leaf.span_path = vec![wrapper.span_id.clone(), leaf.span_id.clone()];
    leaf.timestamp = utc(1);
    leaf.order_time = leaf.timestamp;

    assert_eq!(
        call_repeat_ordinals(&[wrapper, leaf]),
        vec![0, 0],
        "a generation wrapper and its model span are two carriers of one execution"
    );
}

#[test]
fn agent_output_copy_inherits_its_child_generation_occurrence() {
    let t0 = utc(0);
    let t1 = utc(1);
    let output = |agent: &str, generation: &str, timestamp: DateTime<Utc>| {
        let mut child = make_test_block(
            "trace1",
            generation,
            ChatRole::Assistant,
            "same answer",
            timestamp,
        );
        child.observation_type = Some("generation".to_string());
        child.source_type = "attribute".to_string();
        child.source_attribute = Some("gen_ai.output.messages".to_string());
        child.parent_span_id = Some(agent.to_string());
        child.span_path = vec![
            "session".to_string(),
            agent.to_string(),
            generation.to_string(),
        ];
        child.uses_span_end = true;
        child.finish_reason = Some(FinishReason::Stop);

        let mut parent = child.clone();
        parent.span_id = agent.to_string();
        parent.observation_type = Some("agent".to_string());
        parent.parent_span_id = Some("session".to_string());
        parent.span_path = vec!["session".to_string(), agent.to_string()];
        parent.finish_reason = None;
        (parent, child)
    };
    let (first_parent, first_child) = output("agent-1", "generation-1", t0);
    let (second_parent, second_child) = output("agent-2", "generation-2", t1);

    assert_eq!(
        call_repeat_ordinals(&[first_parent, first_child, second_parent, second_child]),
        vec![0, 0, 1, 1],
        "an accumulator copy must contract with the generation from its own invocation"
    );
}

#[test]
fn agent_tool_copies_inherit_their_child_execution_ordinal() {
    let invocation = |agent: &str, generation: &str, tool: &str, time: i64| {
        let child_time = utc(time);
        let mut child = make_tool_use_block("trace1", generation, "reused", "search", child_time);
        child.observation_type = Some("generation".to_string());
        child.source_type = "attribute".to_string();
        child.source_attribute = Some("gen_ai.output.messages".to_string());
        child.parent_span_id = Some(agent.to_string());
        child.span_path = vec![
            "session".to_string(),
            agent.to_string(),
            generation.to_string(),
        ];

        let mut parent_call = child.clone();
        parent_call.span_id = agent.to_string();
        parent_call.observation_type = Some("agent".to_string());
        parent_call.parent_span_id = Some("session".to_string());
        parent_call.span_path = vec!["session".to_string(), agent.to_string()];
        parent_call.timestamp = utc(time - 1);
        parent_call.order_time = parent_call.timestamp;

        let mut actual_result =
            make_tool_result_block("trace1", tool, "reused", "same result", utc(time + 1));
        actual_result.observation_type = Some("tool".to_string());
        actual_result.source_type = "attribute".to_string();
        actual_result.source_attribute = Some("gen_ai.tool.call.result".to_string());
        actual_result.parent_span_id = Some(agent.to_string());
        actual_result.span_path = vec!["session".to_string(), agent.to_string(), tool.to_string()];

        let mut parent_result = actual_result.clone();
        parent_result.span_id = agent.to_string();
        parent_result.observation_type = Some("agent".to_string());
        parent_result.source_attribute = Some("gen_ai.output.messages".to_string());
        parent_result.parent_span_id = Some("session".to_string());
        parent_result.span_path = vec!["session".to_string(), agent.to_string()];
        parent_result.timestamp = utc(time - 1);
        parent_result.order_time = parent_result.timestamp;

        vec![parent_call, parent_result, child, actual_result]
    };
    let blocks = invocation("agent-1", "generation-1", "tool-1", 10)
        .into_iter()
        .chain(invocation("agent-2", "generation-2", "tool-2", 20))
        .collect::<Vec<_>>();

    assert_eq!(
        call_repeat_ordinals(&blocks),
        vec![0, 0, 0, 0, 1, 1, 1, 1],
        "agent-span timestamps precede their children, but subtree identity still selects the right execution"
    );
}

#[test]
fn same_timestamp_result_after_its_call_does_not_create_a_prior_execution() {
    let t0 = utc(0);
    let mut call = make_tool_use_block("trace1", "generation", "call-1", "search", t0);
    call.observation_type = Some("generation".to_string());
    call.source_type = "event".to_string();
    call.event_name = Some("gen_ai.choice".to_string());
    call.message_index = 1;
    let mut result = make_tool_result_block("trace1", "generation", "call-1", "same result", t0);
    result.observation_type = Some("generation".to_string());
    result.source_type = "event".to_string();
    result.event_name = Some("gen_ai.tool.message".to_string());
    result.message_index = 2;

    assert_eq!(call_repeat_ordinals(&[call, result]), vec![0, 0]);
}

#[test]
fn ordered_adk_snapshot_preserves_reused_id_occurrences() {
    let t0 = utc(0);
    let mut call_0 = make_tool_use_block("trace1", "generation", "reused", "search", t0);
    call_0.observation_type = Some("generation".to_string());
    call_0.source_type = "attribute".to_string();
    call_0.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    call_0.position = PositionPath::root(2);
    call_0.message_index = 2;
    call_0.is_history = true;
    let mut result_0 = make_tool_result_block("trace1", "generation", "reused", "same result", t0);
    result_0.observation_type = Some("generation".to_string());
    result_0.source_type = "attribute".to_string();
    result_0.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    result_0.position = PositionPath::root(3);
    result_0.message_index = 3;
    let mut call_1 = call_0.clone();
    call_1.position = PositionPath::root(6);
    call_1.message_index = 6;
    let mut result_1 = result_0.clone();
    result_1.position = PositionPath::root(7);
    result_1.message_index = 7;
    result_1.is_history = true;
    let mut call_2 = call_0.clone();
    call_2.source_attribute = Some("gcp.vertex.agent.llm_response".to_string());
    call_2.position = PositionPath::root(10);
    call_2.message_index = 10;
    call_2.is_history = false;

    let blocks = vec![call_0, result_0, call_1, result_1, call_2];
    assert_eq!(call_repeat_ordinals(&blocks), vec![0, 0, 1, 1, 2]);
    let timestamps = HashMap::from([(
        "generation".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);
    assert_eq!(process_dedup(blocks, timestamps).len(), 5);
}

#[test]
fn ordered_snapshot_drops_an_occurrence_replayed_from_an_earlier_trace() {
    let t0 = utc(0);
    let mut replay = make_test_block("trace1", "generation", ChatRole::User, "same question", t0);
    replay.observation_type = Some("generation".to_string());
    replay.source_type = "attribute".to_string();
    replay.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
    replay.position = PositionPath::root(1);
    replay.is_history = true;
    replay.is_cross_trace_history = true;

    assert!(
        process_dedup(vec![replay], HashMap::new()).is_empty(),
        "position proves local repeats, not that an earlier trace's replay is new"
    );
}

#[test]
fn atomic_emission_marked_as_history_does_not_survive_on_position_alone() {
    let t0 = utc(0);
    let mut replay = make_tool_result_block("trace1", "generation", "old-call", "old result", t0);
    replay.observation_type = Some("generation".to_string());
    replay.event_name = Some("gen_ai.tool.message".to_string());
    replay.position = PositionPath::root(3);
    replay.is_history = true;

    assert!(
        process_dedup(vec![replay], HashMap::new()).is_empty(),
        "an atomic carrier proves multiplicity, not that a replay is current"
    );
}

#[test]
fn test_tool_result_no_tool_use_id_falls_back_to_content_hash() {
    // Without tool_use_id, identity uses content hash (existing behavior)
    let t0 = utc(0);

    let mut result1 = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: None,
            name: None,
            content: serde_json::json!("same content"),
            is_error: false,
        },
        ..make_tool_result_block("trace1", "span1", "", "unused", t0)
    };
    result1.tool_use_id = None;

    let mut result2 = BlockEntry {
        content: ContentBlock::ToolResult {
            tool_use_id: None,
            name: None,
            content: serde_json::json!("same content"),
            is_error: false,
        },
        ..make_tool_result_block("trace1", "span2", "", "unused", t0)
    };
    result2.tool_use_id = None;

    // Same content, no tool_use_id → same identity via content hash
    let id1 = MessageIdentity::from_block(&result1);
    let id2 = MessageIdentity::from_block(&result2);
    assert_eq!(id1, id2, "Same content without tool_use_id should match");

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
    assert_eq!(
        result.len(),
        1,
        "Same content without tool_use_id should dedup"
    );
}

#[test]
fn test_tool_result_same_id_same_observation_type_deduped() {
    // Same tool_use_id from same observation type → still deduped.
    // tool_use_id is identity, observation type is irrelevant.
    let t0 = utc(0);
    let t1 = utc(1);

    let mut r1 = make_tool_result_block("trace1", "span1", "call_1", "First result", t0);
    r1.observation_type = Some("generation".to_string());

    let mut r2 = make_tool_result_block("trace1", "span2", "call_1", "Second result", t1);
    r2.observation_type = Some("generation".to_string());

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
                span_start: t1,
                span_end: Some(t1),
            },
        ),
    ]);

    let result = process_dedup(vec![r1, r2], span_timestamps);

    // Same tool_use_id → same identity → deduped to 1
    assert_eq!(result.len(), 1);
}

#[test]
fn test_tool_result_without_matching_tool_use() {
    let t0 = utc(0);
    // Tool result with no matching tool_use in the data
    let tool_result = make_tool_result_block("trace1", "span1", "missing_call", "result", t0);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    // Should still work, using effective timestamp as fallback
    let result = process_dedup(vec![tool_result], span_timestamps);
    assert_eq!(result.len(), 1);
}

// ========================================================================
// ADVANCED DEDUPLICATION TESTS
// ========================================================================

#[test]
fn test_parallel_tool_calls_different_inputs_not_deduped() {
    // Multiple tool calls with DIFFERENT inputs should NOT be deduped
    // (even though they have the same tool name)
    let t0 = utc(0);

    // Create tool calls with same name but different inputs
    let mut tool1 = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
    tool1.content = ContentBlock::ToolUse {
        id: Some("call_1".to_string()),
        name: "search".to_string(),
        input: serde_json::json!({"query": "cats"}),
    };

    let mut tool2 = make_tool_use_block("trace1", "span1", "call_2", "search", t0);
    tool2.content = ContentBlock::ToolUse {
        id: Some("call_2".to_string()),
        name: "search".to_string(),
        input: serde_json::json!({"query": "dogs"}),
    };

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    let result = process_dedup(vec![tool1, tool2], span_timestamps);

    // Both should be kept (different inputs = different identities)
    assert_eq!(result.len(), 2);
}

#[test]
fn test_streaming_chunks_deduped() {
    // Same content appearing multiple times (streaming) should be deduped
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);

    let chunk1 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t0);
    let chunk2 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t1);
    let mut chunk3 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t2);
    chunk3.finish_reason = Some(FinishReason::Stop);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t2),
        },
    )]);

    let result = process_dedup(vec![chunk1, chunk2, chunk3], span_timestamps);

    // Should be deduped to single message (with finish_reason = highest quality)
    assert_eq!(result.len(), 1);
    assert!(result[0].finish_reason.is_some());
}

#[test]
fn test_different_roles_same_content_not_deduped() {
    // Same content but different roles should NOT be deduped
    let t0 = utc(0);

    let user_msg = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    let mut assistant_msg = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
    assistant_msg.finish_reason = Some(FinishReason::Stop);

    let span_timestamps = HashMap::from([(
        "span1".to_string(),
        SpanTimestamps {
            span_start: t0,
            span_end: Some(t0),
        },
    )]);

    let result = process_dedup(vec![user_msg, assistant_msg], span_timestamps);

    // Both should be kept (different roles = different identities)
    assert_eq!(result.len(), 2);
}

#[test]
fn test_history_at_multiple_depths_deduped() {
    // User message appears at root, child, and grandchild spans
    // Should be deduped to the earliest occurrence
    let t0 = utc(0);
    let t5 = utc(5);
    let t10 = utc(10);

    let root_msg = make_test_block("trace1", "root", ChatRole::User, "Hello", t0);
    let child_msg = make_test_block("trace1", "child", ChatRole::User, "Hello", t5);
    let grandchild_msg = make_test_block("trace1", "grandchild", ChatRole::User, "Hello", t10);

    let span_timestamps = HashMap::from([
        (
            "root".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "child".to_string(),
            SpanTimestamps {
                span_start: t5,
                span_end: Some(t5),
            },
        ),
        (
            "grandchild".to_string(),
            SpanTimestamps {
                span_start: t10,
                span_end: Some(t10),
            },
        ),
    ]);

    let result = process_dedup(vec![root_msg, child_msg, grandchild_msg], span_timestamps);

    // Should be deduped to single message from root span
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].span_id, "root");
}

#[test]
fn test_full_tool_chain_ordering() {
    // Complete tool chain: User -> Assistant+ToolUse -> ToolResult -> Assistant
    // Each step in separate spans with proper timestamps
    let t0 = utc(0);
    let t1 = utc(1);
    let t2 = utc(2);
    let t3 = utc(3);

    let user = make_test_block("trace1", "span_user", ChatRole::User, "Search for cats", t0);
    let tool_use = make_tool_use_block("trace1", "span_tool_use", "call_1", "search", t1);
    let tool_result =
        make_tool_result_block("trace1", "span_tool_result", "call_1", "Found cats", t2);

    let mut final_response = make_test_block(
        "trace1",
        "span_final",
        ChatRole::Assistant,
        "Here are the cats",
        t3,
    );
    final_response.finish_reason = Some(FinishReason::Stop);

    // Each span has its own timestamps - OUTPUT uses span_end
    let span_timestamps = HashMap::from([
        (
            "span_user".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        ),
        (
            "span_tool_use".to_string(),
            SpanTimestamps {
                span_start: t1,
                span_end: Some(t1), // Tool use span ends at t1
            },
        ),
        (
            "span_tool_result".to_string(),
            SpanTimestamps {
                span_start: t2,
                span_end: Some(t2),
            },
        ),
        (
            "span_final".to_string(),
            SpanTimestamps {
                span_start: t3,
                span_end: Some(t3),
            },
        ),
    ]);

    // Process in random order
    let result = process_dedup(
        vec![final_response, tool_result, user, tool_use],
        span_timestamps,
    );

    // Should be in correct order: User -> ToolUse -> ToolResult -> Final response
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, ChatRole::User);
    assert_eq!(result[1].entry_type, "tool_use");
    assert_eq!(result[2].entry_type, "tool_result");
    assert_eq!(result[3].role, ChatRole::Assistant);
    assert!(matches!(
        result[3].content,
        ContentBlock::Text { ref text } if text == "Here are the cats"
    ));
}

#[test]
fn test_uses_span_end_field_on_test_helpers() {
    let t0 = utc(0);

    // User message uses event_time (uses_span_end = false)
    let user = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
    assert!(!user.uses_span_end);

    // ToolUse uses event_time (uses_span_end = false) - the decision to call
    // a tool happens DURING generation, not at completion
    let tool_use = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
    assert!(!tool_use.uses_span_end);

    // ToolResult uses event_time (uses_span_end = false) unless from tool span
    let tool_result = make_tool_result_block("trace1", "span1", "call_1", "result", t0);
    assert!(!tool_result.uses_span_end);
}

#[test]
fn test_quality_scoring() {
    let t0 = utc(0);

    // Base block
    let base = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
    let base_quality = compute_quality(&base);

    // Block with finish_reason has higher quality
    let mut with_finish = base.clone();
    with_finish.finish_reason = Some(FinishReason::Stop);
    let with_finish_quality = compute_quality(&with_finish);
    assert!(with_finish_quality > base_quality);

    // Block with model info has higher quality
    let mut with_model = base.clone();
    with_model.model = Some("gpt-4".to_string());
    let with_model_quality = compute_quality(&with_model);
    assert!(with_model_quality > base_quality);

    // Event source has higher quality than attribute
    let mut from_event = base.clone();
    from_event.source_type = "event".to_string();
    let mut from_attribute = base;
    from_attribute.source_type = "attribute".to_string();
    assert!(compute_quality(&from_event) > compute_quality(&from_attribute));
}

/// Two copies of one attachment, one with its filename: they are one block, and the name survives
/// whichever copy wins on quality.
#[test]
fn a_deduplicated_attachment_keeps_the_filename_a_copy_had() {
    let t0 = utc(0);
    let document = |span: &str, name: Option<&str>| BlockEntry {
        content: ContentBlock::Document {
            media_type: Some("application/pdf".to_string()),
            name: name.map(str::to_string),
            source: "base64".to_string(),
            data: "JVBERi0xLjMKJcTl8uXrp/Og0MTGCg==".to_string(),
        },
        entry_type: "document".to_string(),
        role: ChatRole::User,
        ..make_tool_result_block("trace1", span, "", "unused", t0)
    };
    let spans = HashMap::from([
        ("named".to_string(), SpanTimestamps { span_start: t0, span_end: Some(t0) }),
        ("bare".to_string(), SpanTimestamps { span_start: t0, span_end: Some(t0) }),
    ]);

    for order in [[Some("task"), None], [None, Some("task")]] {
        let blocks = order
            .iter()
            .map(|name| document(if name.is_some() { "named" } else { "bare" }, *name))
            .collect();
        let survivors = process_dedup(blocks, spans.clone());

        let [survivor] = &survivors[..] else {
            panic!("one attachment, got {}", survivors.len());
        };
        assert!(
            matches!(&survivor.content, ContentBlock::Document { name: Some(name), .. } if name == "task"),
            "{:?}",
            survivor.content
        );
    }
}

/// Two equally good copies of an answer - the model call's and the agent span's re-listing - survive
/// as the model call's, whichever arrives first. Arrival order is the order of span ids where the
/// spans' clocks agree only to the millisecond, so it attributed the answer differently per capture.
#[test]
fn an_equally_good_answer_survives_as_the_model_calls() {
    let answer = |span: &str, observation: &str| {
        let mut block = make_test_block(
            "trace-1",
            span,
            ChatRole::Assistant,
            "Sunny in Rome.",
            Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        );
        block.event_name = Some("gen_ai.choice".to_string());
        block.category = MessageCategory::GenAIChoice;
        block.finish_reason = Some(FinishReason::Stop);
        block.observation_type = Some(observation.to_string());
        block
    };
    for agent_first in [true, false] {
        let mut blocks = vec![answer("chat", "generation"), answer("agent", "agent")];
        if agent_first {
            blocks.reverse();
        }
        let survivors = process_dedup(blocks, HashMap::new());
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].observation_type.as_deref(), Some("generation"));
    }
}
