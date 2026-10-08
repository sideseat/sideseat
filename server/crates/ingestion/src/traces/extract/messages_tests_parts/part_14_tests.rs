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
