
/// The declared member vocabulary answers exactly as the three retired lists did.
///
/// All three questions, because a member usually answers more than one and the lists had drifted: which member
/// holds a value's content (ordered), which mean it is message-shaped, and which mean it is a content block.
#[test]
fn the_declared_members_reproduce_the_lists_they_replaced() {
    use sideseat_domain::sideml::{is_plain_data_value, is_plain_data_value_legacy};

    let plan = &sideseat_domain::rules::ruleset().message_members;

    // The ordered content chain: the first member the value *has*, not the first holding something.
    let chain: Vec<&str> = plan.content_in_order().collect();
    assert_eq!(
        chain,
        vec![
            "content",
            "contents",
            "message",
            "parts",
            "text",
            "object",
            "arguments"
        ],
        "the declared order is the retired chain's"
    );
    let first_present = |value: &serde_json::Value| -> Option<serde_json::Value> {
        plan.content_in_order()
            .find_map(|member| value.get(member))
            .cloned()
    };
    assert_eq!(
        first_present(&serde_json::json!({"content": null, "parts": ["x"]})),
        Some(serde_json::json!(null)),
        "a null content outranks a populated later member, which a set could not say"
    );
    assert_eq!(
        first_present(&serde_json::json!({"parts": ["x"], "text": "y"})),
        Some(serde_json::json!(["x"])),
        "the earlier member wins"
    );
    assert_eq!(
        first_present(&serde_json::json!({"unrelated": 1})),
        None,
        "a value with none of them has no declared content member"
    );

    // Message-shaped, against the retired list, over the shapes that distinguish the two answers.
    let shapes = [
        serde_json::json!({}),
        serde_json::json!({"name": "Jane", "age": 28}),
        serde_json::json!({"role": "user", "content": "hi"}),
        serde_json::json!({"contents": []}),
        serde_json::json!({"parts": []}),
        serde_json::json!({"choices": []}),
        serde_json::json!({"generations": []}),
        serde_json::json!({"toolUse": {}}),
        serde_json::json!({"reasoningContent": {}}),
        serde_json::json!({"finishReason": "stop"}),
        serde_json::json!({"finish_reason": "stop"}),
        serde_json::json!({"toolCalls": []}),
        serde_json::json!({"type": "text"}),
        serde_json::json!({"arguments": "{}"}),
        serde_json::json!({"object": {"a": 1}}),
        serde_json::json!("a string"),
        serde_json::json!([1, 2]),
        serde_json::json!(null),
    ];
    for shape in &shapes {
        assert_eq!(
            is_plain_data_value(shape),
            is_plain_data_value_legacy(shape),
            "the declared vocabulary disagrees about whether this is bare data: {shape}"
        );
    }

    // The message-shape vocabulary as a set too, in both directions - the same reason: a member missing from it
    // makes a turn look like bare data to be wrapped, and one added makes a producer's own JSON look like a
    // message with no blocks. Comparing only the *predicate* over chosen shapes cannot see a member neither
    // shape carries.
    let mut retired_shape: std::collections::BTreeSet<&str> =
        sideseat_domain::sideml::test_support::message_structure_keys_legacy()
            .iter()
            .copied()
            .collect();
    // **One deliberate divergence, recorded here rather than blessed by regenerating anything.** The retired list
    // held `functionCall` and `functionResponse` as message-shaped and their snake_case twins as content-block
    // only - the same provider's part, two answers, decided by spelling. So `{"function_call": {}}` was bare data
    // to be wrapped and `{"functionCall": {}}` was a message with a call member and no content member, which
    // normalises to a message with no blocks. The declarations make the pair one family on the *evidenced*
    // spelling's vector: measured, giving the family the message-shaped flag changes 92 of 33,139 corpus objects,
    // and the camelCase spelling appears in none of them - so following the evidenced one changes no corpus answer
    // and removes the disagreement.
    assert!(
        retired_shape.remove("functionCall") && retired_shape.remove("functionResponse"),
        "the retired list no longer holds the two spellings this divergence is about"
    );
    let declared_shape: std::collections::BTreeSet<&str> = plan.message_shaped_members().collect();
    assert_eq!(
        declared_shape, retired_shape,
        "the declared message-shape vocabulary is not exactly the retired list, beyond the one recorded divergence"
    );

    // The content-block vocabulary, as a **set**: the declared one must mean exactly what the retired list
    // meant, since a member missing from it turns a malformed block into plain structured output and one added
    // to it turns plain data into an unknown block.
    let retired: std::collections::BTreeSet<&str> =
        sideseat_domain::sideml::test_support::provider_content_fields_legacy()
            .iter()
            .copied()
            .collect();
    for member in &retired {
        assert!(
            plan.any_means_content_block(std::iter::once(&member.to_string())),
            "the retired list means `{member}` is a content-block member and the declared vocabulary does not"
        );
    }
    // And nothing beyond it, checked from the other side: every declared member that means "content block" is
    // in the retired list.
    for member in sideseat_domain::rules::ruleset()
        .message_members
        .content_block_members()
    {
        assert!(
            retired.contains(member),
            "the declared vocabulary means `{member}` is a content-block member and the retired list did not"
        );
    }

    // And the recognition as the normaliser reads it. Only a member no earlier case claims can be exercised
    // this way - most of the vocabulary is read by a provider handler first, which is the point of the
    // recognition step being a last resort - so the set comparison above is what covers the rest.
    for (what, block, expected_type) in [
        (
            "a member that means content block, unrecognised",
            serde_json::json!({"toolCallId": "x"}),
            "unknown",
        ),
        (
            "the same with an extra member, which is what makes a block malformed rather than unrecognised",
            serde_json::json!({"toolCallId": "x", "extra": 1}),
            "unknown",
        ),
        (
            "no such member, so plain structured output",
            serde_json::json!({"name": "Jane", "age": 28}),
            "json",
        ),
    ] {
        let out = sideseat_domain::sideml::test_support::normalize_content_block(&block)
            .unwrap_or_else(|| panic!("{what}: nothing normalised {block}"));
        assert_eq!(
            out.get("type").and_then(|t| t.as_str()),
            Some(expected_type),
            "{what}: {out}"
        );
    }
}

/// A container event whose payload is **unreadable** keeps its raw form; one that is merely empty does not.
///
/// `raw: "replace"` says the event's own body is not a message, because its attributes are. Applied on the
/// declaration alone, a container whose declared reads all *fail* produced no messages **and** suppressed the
/// raw form - so the event vanished, indistinguishable on the ingest path from one never emitted, with nothing
/// recorded anywhere. The counterexample is `gen_ai.input.messages = "{"` on the inference-details event.
///
/// Replacement now depends on something having read the event, and the raw form is kept when a declared carrier
/// was **present** and yielded nothing. Present, not merely unread: a container carrying nothing a rule names
/// is an ordinary empty container, and keeping its raw form would put a message in the feed whose content is
/// whatever unrelated attributes the producer attached - noise a user sees, guarding against a loss that did
/// not happen.
#[test]
fn an_unreadable_container_event_keeps_its_raw_form() {
    let container = |attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>| {
        extract_message_from_event(
            &Event {
                name: "gen_ai.client.inference.operation.details".to_string(),
                time_unix_nano: 1_702_400_000_000_000_000,
                attributes: attrs,
                dropped_attributes_count: 0,
            },
            "",
            &HashMap::new(),
            false,
        )
    };

    // The declared carrier exists but does not parse.
    let unreadable = container(vec![make_kv("gen_ai.input.messages", "{")]);
    assert_eq!(
        unreadable.len(),
        1,
        "the payload was there and unreadable, so the raw form is the only remaining evidence it existed - \
         dropping it makes the event indistinguishable from one never emitted"
    );
    assert_eq!(
        unreadable[0].content["gen_ai.input.messages"].as_str(),
        Some("{"),
        "and what is kept is what the producer wrote"
    );

    // Readable: the container is replaced by what its attributes hold, which is the whole point of the
    // declaration - the conversation must not be reported twice.
    let readable = container(vec![make_kv(
        "gen_ai.input.messages",
        r#"[{"role":"user","content":"q"}]"#,
    )]);
    assert_eq!(readable.len(), 1);
    assert!(
        matches!(&readable[0].source, MessageSource::Event { name, .. }
            if name == "gen_ai.input.messages"),
        "the reading answered, so the message is the *attribute's*, not the container's: {:?}",
        readable[0].source
    );

    // Empty: nothing a rule names, so there is nothing that failed and nothing to preserve.
    assert!(
        container(vec![make_kv("some_other_attr", "value")]).is_empty(),
        "an ordinary empty container stays suppressed - keeping it would emit a message whose content is \
         unrelated attributes"
    );
}

#[test]
fn crewai_empty_agent_snapshot_is_not_a_user_message() {
    let snapshot = r#"{
        "agent":{"entity_type":"agent","role":"Assistant","goal":"Answer questions"},
        "context":"",
        "tools":[]
    }"#;
    let attrs = make_attrs(&[("crew_key", "crew-1"), ("input.value", snapshot)]);
    let mut messages = Vec::new();
    let mut definitions = Vec::new();

    extract_messages_from_attrs(
        &mut messages,
        &mut definitions,
        &attrs,
        "Assistant._execute_core",
        Utc::now(),
        ExtractionMode::PerCarrier,
        is_tool_execution_span(&attrs),
    );

    assert!(
        messages.iter().all(
            |message| !matches!(
                &message.source,
                MessageSource::Attribute { key, .. } if key == "input.value"
            )
        ),
        "CrewAI's empty agent envelope is metadata, not a conversation: {messages:?}"
    );
}

#[test]
fn openai_agents_logfire_function_span_keeps_input_and_output() {
    let attrs = make_attrs(&[
        ("logfire.msg_template", "Function: {name}"),
        ("name", "retrieve_user_preference"),
        ("input", r#"{"preference_type":"favorite_number"}"#),
        ("output", "7"),
        ("gen_ai.system", "openai"),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "the SDK's function span must be classified as a tool execution"
    );

    let mut messages = Vec::new();
    let mut definitions = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut definitions,
        &attrs,
        "Function: retrieve_user_preference",
        Utc::now(),
        ExtractionMode::PerCarrier,
        true,
    );

    assert_eq!(messages.len(), 2, "both sides of the function call are needed");
    assert_eq!(messages[0].content["role"], "assistant");
    assert_eq!(messages[0].content["content"][0]["type"], "tool_use");
    assert_eq!(
        messages[0].content["content"][0]["name"],
        "retrieve_user_preference"
    );
    assert_eq!(messages[1].content["role"], "tool");
    assert_eq!(messages[1].content["content"][0]["type"], "tool_result");
    assert_eq!(
        messages[1].content["content"][0]["name"],
        "retrieve_user_preference"
    );
}

/// Regression: a chat prompt template's rendered messages are messages, not one structured answer.
///
/// The template span's output is the `messages` it rendered. Read generically it became an assistant
/// `json` block that repeated the request inside the conversation it framed.
#[test]
fn a_rendered_chat_prompt_is_read_as_its_messages() {
    let attrs = make_attrs(&[
        ("openinference.span.kind", "PROMPT"),
        ("input.value", "What is Kyoto known for?"),
        (
            "output.value",
            r#"{"messages": [
                {"content": "Be brief.", "type": "system", "name": null, "id": null},
                {"content": "What is Kyoto known for?", "type": "human", "name": null, "id": null}
            ]}"#,
        ),
        ("output.mime_type", "application/json"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    extract_messages_from_context(
        &mut messages,
        &mut tools,
        SpanExtraction {
            name: "ChatPromptTemplate",
            attrs: &attrs,
            scope_name: Some("openinference.instrumentation.langchain"),
            scope_version: Some("0.1.78"),
            is_tool_span: false,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );

    let rendered: Vec<_> = messages
        .iter()
        .filter(|m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "output.value"))
        .map(|m| {
            (
                m.content["role"].as_str().unwrap_or_default(),
                m.content["content"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        rendered,
        [("system", "Be brief."), ("user", "What is Kyoto known for?")]
    );
}
