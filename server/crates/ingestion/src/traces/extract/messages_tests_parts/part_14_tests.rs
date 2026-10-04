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
            scope_version: Some("0.1.78"),
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
    assert_eq!(messages[1].content["content"].as_str(), Some("Plan a trip."));
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
            scope_version: Some("4.5.4"),
            is_tool_span: false,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );
    assert!(messages.is_empty(), "{messages:?}");
}
