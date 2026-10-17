/// Regression: `messages_to_dict` writes the type beside a `data` member holding the message.
///
/// Graph state and the runnables inside a graph report their messages in that form; with only the type
/// readable, a runnable's input became opaque `unknown` blocks repeating the request beside the readable
/// copy on its model span.
#[test]
fn a_langchain_message_dict_reads_its_data_member() {
    let attrs = make_attrs(&[
        ("openinference.span.kind", "CHAIN"),
        ("metadata", r#"{"langgraph_node": "plan"}"#),
        (
            "input.value",
            r#"[
                {"type": "system", "data": {"content": "Be brief.", "type": "system", "id": null}},
                {"type": "human", "data": {"content": "Plan a trip.", "type": "human", "id": "m1"}},
                {"type": "ai", "data": {"content": "", "type": "ai",
                    "tool_calls": [{"id": "call_1", "name": "plan", "args": {"city": "Vienna"}}]}},
                {"type": "tool", "data": {"content": "done", "type": "tool", "tool_call_id": "call_1", "name": "plan"}}
            ]"#,
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    extract_messages_from_context(
        &mut messages,
        &mut tools,
        SpanExtraction {
            rules: sideseat_domain::rules::ruleset(),
            name: "RunnableSequence",
            attrs: &attrs,
            scope_name: Some("openinference.instrumentation.langchain"),
            is_tool_span: false,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );

    let roles: Vec<_> = messages
        .iter()
        .map(|m| m.content["role"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);
    assert_eq!(messages[0].content["content"].as_str(), Some("Be brief."));
    assert_eq!(
        messages[1].content["content"].as_str(),
        Some("Plan a trip.")
    );
    assert!(messages[2].content["tool_calls"].is_array());
    assert_eq!(messages[3].content["tool_call_id"].as_str(), Some("call_1"));
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
            rules: sideseat_domain::rules::ruleset(),
            name: "ChatPromptTemplate",
            attrs: &attrs,
            scope_name: Some("openinference.instrumentation.langchain"),
            is_tool_span: false,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );

    let rendered: Vec<_> = messages
        .iter()
        .filter(
            |m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "output.value"),
        )
        .map(|m| {
            (
                m.content["role"].as_str().unwrap_or_default(),
                m.content["content"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        rendered,
        [
            ("system", "Be brief."),
            ("user", "What is Kyoto known for?")
        ]
    );
}

/// Regression: LlamaIndex's private request-preparation helper is claimed for every LLM class.
///
/// The claim named only `OpenAI._prepare_chat_with_tools`, so on Bedrock the helper's serialised
/// request reached the generic fallback and every conversation opened with two JSON blocks.
#[test]
fn a_llamaindex_request_preparation_span_contributes_no_messages() {
    let attrs = make_attrs(&[
        ("llm.model_name", "global.anthropic.claude-sonnet-5-5"),
        ("input.mime_type", "application/json"),
        ("output.mime_type", "application/json"),
        (
            "input.value",
            r#"{"tools": [], "user_msg": null, "chat_history": ["ChatMessage(role=<MessageRole.USER: 'user'>)"]}"#,
        ),
        (
            "output.value",
            r#"{"messages": ["ChatMessage(role=<MessageRole.USER: 'user'>)"]}"#,
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    extract_messages_from_context(
        &mut messages,
        &mut tools,
        SpanExtraction {
            rules: sideseat_domain::rules::ruleset(),
            name: "BedrockConverse._prepare_chat_with_tools",
            attrs: &attrs,
            scope_name: Some("openinference.instrumentation.llama_index"),
            is_tool_span: false,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );
    assert!(messages.is_empty(), "{messages:?}");
}

/// An OpenAI Agents Responses span records its request twice: the input items whole in `raw_input`, and a
/// derived copy in `events` that keeps an image only as its data URL's text. The items are the request
/// wherever they parse; a `raw_input` an attribute limit cut short leaves the events the conversation, so the
/// request is never shown as nothing.
#[test]
fn an_openai_agents_request_is_read_from_its_items_unless_they_were_cut_short() {
    let items = r#"[{"role": "user", "content": [
        {"type": "input_text", "text": "Describe the image."},
        {"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo="}
    ]}]"#;
    let events = r#"[
        {"event.name": "gen_ai.system.message", "content": "Be brief.", "role": "system"},
        {"event.name": "gen_ai.user.message", "content": "Describe the image.", "role": "user"},
        {"event.name": "gen_ai.user.message", "content": "data:image/png;base64,iVBORw0KGgo=", "role": "user"}
    ]"#;
    let response = r#"{"instructions": "Be brief.", "output": [
        {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "A pixel."}]}
    ]}"#;
    let read = |raw_input: &str| {
        let attrs = make_attrs(&[
            ("raw_input", raw_input),
            ("events", events),
            ("response", response),
            ("gen_ai.system", "openai"),
        ]);
        let mut messages = Vec::new();
        let mut tools = Vec::new();
        extract_messages_from_context(
            &mut messages,
            &mut tools,
            SpanExtraction {
                rules: sideseat_domain::rules::ruleset(),
                name: "Responses API with {gen_ai.request.model!r}",
                attrs: &attrs,
                scope_name: Some("logfire.openai_agents"),
                is_tool_span: false,
            },
            Utc::now(),
            ExtractionMode::PerCarrier,
        );
        messages
    };
    let image_part = |m: &serde_json::Value| {
        m["content"].as_array().is_some_and(|parts| {
            parts
                .iter()
                .any(|p| p["type"].as_str() == Some("input_image"))
        })
    };
    let data_url_text = |m: &serde_json::Value| {
        m["content"]
            .as_str()
            .is_some_and(|t| t.starts_with("data:"))
    };

    let whole = read(items);
    assert!(whole.iter().any(|m| image_part(&m.content)), "{whole:?}");
    assert!(
        !whole.iter().any(|m| data_url_text(&m.content)),
        "{whole:?}"
    );
    assert!(whole.iter().any(|m| m.content["role"] == "system"));

    let cut = read(&items[..60]);
    assert!(!cut.iter().any(|m| image_part(&m.content)), "{cut:?}");
    assert!(cut.iter().any(|m| data_url_text(&m.content)), "{cut:?}");
    assert!(
        cut.iter()
            .any(|m| m.content["content"].as_str() == Some("Describe the image."))
    );
}

/// A tool message that states its call id under a declared member keeps it, and the span keeps its own: the span's
/// `gen_ai.tool.call.id` is written onto a tool message only where the message states no id of its own, so where
/// the two differ each statement stays where the producer made it.
#[test]
fn a_tool_message_s_stated_call_id_is_not_overwritten_by_the_span_s() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    let event = |name: &str, attributes: Vec<_>| Event {
        name: name.to_string(),
        time_unix_nano: 1734023444422429000,
        attributes,
        dropped_attributes_count: 0,
    };
    let span = Span {
        trace_id: vec![0; 16],
        span_id: vec![0; 8],
        parent_span_id: vec![],
        name: "execute_tool my_tool".to_string(),
        kind: 1,
        start_time_unix_nano: 1734023444422359000,
        end_time_unix_nano: 1734023444423724000,
        attributes: vec![
            make_kv("gen_ai.operation.name", "execute_tool"),
            make_kv("gen_ai.tool.name", "my_tool"),
            make_kv("gen_ai.tool.call.id", "call_from_span"),
        ],
        events: vec![
            event(
                "gen_ai.tool.message",
                vec![
                    make_kv("content", r#"{"city": "NYC"}"#),
                    make_kv("tool_use_id", "call_stated"),
                ],
            ),
            event(
                "gen_ai.choice",
                vec![
                    make_kv("content", "Sunny"),
                    make_kv("tool_use_id", "call_stated"),
                ],
            ),
        ],
        links: vec![],
        status: None,
        trace_state: String::new(),
        flags: 0,
        dropped_attributes_count: 0,
        dropped_events_count: 0,
        dropped_links_count: 0,
    };
    let span_attrs = make_attrs(&[
        ("gen_ai.operation.name", "execute_tool"),
        ("gen_ai.tool.name", "my_tool"),
        ("gen_ai.tool.call.id", "call_from_span"),
    ]);
    let (messages, _, _) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);
    assert_eq!(messages.len(), 2, "{messages:?}");
    // The span's own statement stays on the span.
    let mut span_data = crate::traces::extract::attributes::SpanData::default();
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut span_data,
        &span_attrs,
        "execute_tool my_tool",
    );
    assert_eq!(
        span_data.gen_ai_tool_call_id.as_deref(),
        Some("call_from_span")
    );
    for message in &messages {
        assert_ne!(
            message.content.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_from_span"),
            "the span's id overwrote the message's own: {message:?}"
        );
        assert_eq!(
            sideseat_domain::sideml::tools::extract_tool_use_id(&message.content, "tool")
                .as_deref(),
            Some("call_stated"),
            "{message:?}"
        );
    }
}

/// An indexed completion that states no role is the response's: the family's `entry_defaults` make it the
/// assistant's, and a role the entry does state stands.
#[test]
fn a_role_less_indexed_completion_is_the_assistant_s() {
    let attrs = make_attrs(&[
        ("gen_ai.completion.0.content", "2+2 is 4."),
        ("gen_ai.completion.1.content", "A tool's word."),
        ("gen_ai.completion.1.role", "tool"),
    ]);
    let mut messages = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut Vec::new(),
        &attrs,
        "chat",
        Utc::now(),
        ExtractionMode::PerCarrier,
        false,
    );
    let roles: Vec<Option<&str>> = messages
        .iter()
        .map(|m| m.content.get("role").and_then(|r| r.as_str()))
        .collect();
    assert_eq!(roles, [Some("assistant"), Some("tool")]);

    // A role-less entry carrying a result's call id is a tool's result, and on a tool's span an entry is the tool's:
    // neither takes the default, so the evidence that makes them results still decides.
    let result = make_attrs(&[
        ("gen_ai.completion.0.content", "Sunny"),
        ("gen_ai.completion.0.tool_call_id", "c1"),
    ]);
    let roles_of = |attrs: &HashMap<String, String>, is_tool_span: bool| {
        let mut messages = Vec::new();
        extract_messages_from_attrs(
            &mut messages,
            &mut Vec::new(),
            attrs,
            "chat",
            Utc::now(),
            ExtractionMode::PerCarrier,
            is_tool_span,
        );
        messages
            .iter()
            .map(|m| m.content.get("role").cloned())
            .collect::<Vec<_>>()
    };
    assert_eq!(roles_of(&result, false), [None]);
    let tool_span = make_attrs(&[("gen_ai.completion.0.content", "Sunny")]);
    assert_eq!(roles_of(&tool_span, true), [None]);
}

/// The deprecated convention's events hold their request and response under `gen_ai.prompt` and `gen_ai.completion`,
/// as a JSON list of messages or a bare text: each is read from its member, a list as the messages it holds and a
/// bare text as the event's own turn.
#[test]
fn the_deprecated_content_events_are_read_from_their_members() {
    let event = |name: &str, member: &str, value: &str| Event {
        name: name.to_string(),
        time_unix_nano: 1_704_067_200_000_000_000,
        attributes: vec![make_kv(member, value)],
        dropped_attributes_count: 0,
    };
    let read = |name: &str, member: &str, value: &str| {
        extract_message_from_event(&event(name, member, value), "", &HashMap::new(), false)
            .into_iter()
            .map(|m| m.content)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        read(
            "gen_ai.content.completion",
            "gen_ai.completion",
            r#"[{"role": "assistant", "content": "3+3 is 6."}]"#
        ),
        [json!([{"role": "assistant", "content": "3+3 is 6."}])]
    );
    assert_eq!(
        read("gen_ai.content.completion", "gen_ai.completion", "Six."),
        [json!({"role": "assistant", "content": "Six."})]
    );
    assert_eq!(
        read(
            "gen_ai.content.prompt",
            "gen_ai.prompt",
            r#"[{"role": "system", "content": "Be brief."}, {"role": "user", "content": "What is 3+3?"}]"#
        ),
        [
            json!([{"role": "system", "content": "Be brief."}, {"role": "user", "content": "What is 3+3?"}])
        ]
    );
    assert_eq!(
        read("gen_ai.content.prompt", "gen_ai.prompt", "What is 3+3?"),
        [json!({"role": "user", "content": "What is 3+3?"})]
    );
    // A conversation is reported under the member's name, whose carrier replays across traces; a bare prompt under
    // the event's own, which does not.
    let source_of = |value: &str| {
        extract_message_from_event(
            &event("gen_ai.content.prompt", "gen_ai.prompt", value),
            "",
            &HashMap::new(),
            false,
        )
        .into_iter()
        .map(|m| match m.source {
            MessageSource::Event { name, .. } => name,
            MessageSource::Attribute { key, .. } => key,
        })
        .collect::<Vec<_>>()
    };
    assert_eq!(
        source_of(r#"[{"role": "user", "content": "Q"}]"#),
        ["gen_ai.prompt"]
    );
    assert_eq!(source_of("Q"), ["gen_ai.content.prompt"]);
    // Chosen by what the text parses to, not by its first character: JSON after whitespace is still a conversation.
    assert_eq!(
        source_of(" \n\t[{\"role\": \"user\", \"content\": \"Q\"}]"),
        ["gen_ai.prompt"]
    );
    // An object stating no role is no shape the convention writes: not read as a guessed envelope, whatever it holds.
    for shape in [
        r#"{"content": "Six."}"#,
        r#"{"content": "Sunny", "tool_call_id": "c1"}"#,
        r#"{"content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}"#,
    ] {
        assert!(
            read("gen_ai.content.completion", "gen_ai.completion", shape)
                .iter()
                .all(|content| content.get("gen_ai.completion").is_some()),
            "{shape} was read as a message"
        );
    }
    // A bare text is kept as the text it is, never read as the JSON scalar it happens to spell.
    for text in ["null", "4.20", "true"] {
        assert_eq!(
            read("gen_ai.content.completion", "gen_ai.completion", text),
            [json!({"role": "assistant", "content": text})],
            "{text:?}"
        );
    }
}
