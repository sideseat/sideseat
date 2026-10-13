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
    // One deliberate departure from the retired chain: the list is read before the flattened `content`,
    // because a producer that writes both keeps reasoning only in the list.
    assert_eq!(
        chain,
        vec![
            "contents",
            "content",
            "message",
            "parts",
            "text",
            "object",
            "arguments"
        ],
        "the declared order is the retired chain's, with the list first"
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
    // The empty object is the one deliberate divergence from the retired list: `{}` is a value a producer
    // wrote - structured output of nothing - and the retired exclusion left `output.value = "{}"` unwrapped,
    // so the view showed an empty roleless message instead of the empty data.
    assert!(is_plain_data_value(&serde_json::json!({})));
    assert!(!is_plain_data_value_legacy(&serde_json::json!({})));
    let shapes = [
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
        is_tool_execution_span(sideseat_domain::rules::ruleset(), &attrs),
    );

    assert!(
        messages.iter().all(|message| !matches!(
            &message.source,
            MessageSource::Attribute { key, .. } if key == "input.value"
        )),
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
        is_tool_execution_span(sideseat_domain::rules::ruleset(), &attrs),
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

    assert_eq!(
        messages.len(),
        2,
        "both sides of the function call are needed"
    );
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

/// A user message a JVM framework printed with `toString` is decoded only on that framework's spans, only in a user
/// entry, and only where it is exactly an unnamed `UserMessage` with no attributes holding some part that is not
/// text, printing back byte for byte; any other text - cut short, named, carrying attributes, text alone (which
/// the framework exports as the text), prose that reads as some other printed object, the same text in a system
/// or assistant entry, or on another producer's span - stays the content it was.
#[test]
fn a_printed_user_message_is_decoded_only_when_it_is_one_whole() {
    let read_as = |scope: &'static str, role: &str, content: &str| {
        let attrs = make_attrs(&[
            ("openinference.span.kind", "LLM"),
            ("llm.input_messages.0.message.role", role),
            ("llm.input_messages.0.message.content", content),
        ]);
        let span = SpanExtraction {
            rules: sideseat_domain::rules::ruleset(),
            name: "LLM",
            attrs: &attrs,
            scope_name: Some(scope),
            is_tool_span: false,
        };
        let mut messages = Vec::new();
        let mut unused = Vec::new();
        try_declared_rules_for_span(
            &mut messages,
            &mut unused,
            span,
            Utc::now(),
            &mut std::collections::HashSet::new(),
        );
        assert_eq!(messages.len(), 1, "one entry");
        messages[0].content["content"].clone()
    };
    let read_from = |scope: &'static str, content: &str| read_as(scope, "user", content);
    let read = |content: &str| read_from("com.arize.langchain4j", content);
    let whole = r#"UserMessage { name = null, contents = [TextContent { text = "Describe it." }, ImageContent { image = Image { url = null, base64Data = "AAAA", mimeType = "image/png", revisedPrompt = null } detailLevel = LOW }, PdfFileContent { pdfFile = PdfFile { url = null, base64Data = "JVBE", mimeType = "application/pdf" } }], attributes = {} }"#;
    let decoded = read(whole);
    assert_eq!(decoded["__java_class"], "UserMessage", "{decoded}");
    let classes: Vec<&str> = decoded["contents"]
        .as_array()
        .expect("its contents")
        .iter()
        .filter_map(|block| block["__java_class"].as_str())
        .collect();
    assert_eq!(classes, ["TextContent", "ImageContent", "PdfFileContent"]);
    let image = r#"ImageContent { image = Image { url = null, base64Data = "AAAA", mimeType = "image/png", revisedPrompt = null } detailLevel = LOW }"#;
    for kept in [
        format!(r#"UserMessage {{ name = null, contents = [{image}, TextContent {{ text = "This is the c..."#),
        format!(r#"UserMessage {{ name = "ada", contents = [{image}], attributes = {{}} }}"#),
        format!(r#"UserMessage {{ name = null, contents = [{image}], attributes = {{k=v}} }}"#),
        r#"UserMessage { name = null, contents = [TextContent { text = "hi" }], attributes = {} }"#.to_string(),
        r#"UserMessage { name = null, contents = [TextContent { text = "a" }, TextContent { text = "b" }], attributes = {} }"#.to_string(),
        r#"TextContent { text = "Prose that reads as a printed object." }"#.to_string(),
    ] {
        assert_eq!(read(&kept), serde_json::json!(kept), "stays the text it was");
    }
    // The decoded form is the user's: the same print in any other entry is what that entry says.
    let control =
        format!(r#"UserMessage {{ name = null, contents = [{image}], attributes = {{}} }}"#);
    assert_eq!(
        read(&control)["__java_class"],
        "UserMessage",
        "the control decodes"
    );
    for role in ["system", "assistant"] {
        assert_eq!(
            read_as("com.arize.langchain4j", role, &control),
            serde_json::json!(control),
            "{role}"
        );
    }
    // The same text from any other producer is what that producer sent, never a printed object.
    for scope in [
        "openinference.instrumentation.openai",
        "openinference.instrumentation.llama_index",
    ] {
        assert_eq!(read_from(scope, whole), serde_json::json!(whole), "{scope}");
    }
}

/// An agent's step result a Python framework printed with `repr` is read for the provider's response it holds -
/// the reasoning its model call's own span dropped - only on that framework's output-parsing span, and only where
/// the event reprints byte for byte as the step result holding one; anything else reads nothing here.
#[test]
fn a_printed_agent_output_is_read_for_the_response_it_holds() {
    let read_as = |name: &'static str, scope: &'static str, ev: &str| {
        let payload = serde_json::json!({"ctx": "<workflows.context.context.Context object at 0x1>", "ev": ev});
        let attrs = make_attrs(&[
            ("openinference.span.kind", "CHAIN"),
            ("input.value", &payload.to_string()),
        ]);
        let span = SpanExtraction {
            rules: sideseat_domain::rules::ruleset(),
            name,
            attrs: &attrs,
            scope_name: Some(scope),
            is_tool_span: false,
        };
        let mut messages = Vec::new();
        let mut unused = Vec::new();
        try_declared_rules_for_span(
            &mut messages,
            &mut unused,
            span,
            Utc::now(),
            &mut std::collections::HashSet::new(),
        );
        serde_json::to_string(&messages.iter().map(|m| &m.content).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let (name, scope) = (
        "BaseWorkflowAgent.parse_agent_output",
        "openinference.instrumentation.llama_index",
    );
    let output = "AgentOutput(response=None, current_agent_name='assistant', raw={'output': {'message': {'role': 'assistant', 'content': [{'reasoningContent': {'reasoningText': {'text': 'Weigh the crossings.', 'signature': 'sig-1'}}}, {'text': 'Seventeen minutes.'}]}}})";
    let shown = read_as(name, scope, output);
    assert!(
        shown.contains("reasoningContent\":{\"reasoningText")
            && shown.contains("Seventeen minutes."),
        "{shown}"
    );
    // Each negative holds the same reasoning, so only the guard it breaks keeps it unread.
    let cut = &output[..output.len() - 8];
    let other = output.replacen("AgentOutput(", "ToolCallResult(", 1);
    for (why, name, scope, ev) in [
        ("cut short", name, scope, cut),
        ("another event", name, scope, other.as_str()),
        (
            "another span",
            "BaseWorkflowAgent.run_agent_step",
            scope,
            output,
        ),
        (
            "another producer",
            name,
            "openinference.instrumentation.openai",
            output,
        ),
    ] {
        let shown = read_as(name, scope, ev);
        assert!(
            !shown.contains("\"type\":\"thinking\"")
                && !shown.contains("reasoningContent\":{\"reasoningText"),
            "{why}: {shown}"
        );
    }
}
