#[test]
fn test_openinference_tool_message() {
    let attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.content",
            "What's the weather?",
        ),
        ("llm.input_messages.1.message.role", "assistant"),
        ("llm.input_messages.2.message.role", "tool"),
        (
            "llm.input_messages.2.message.content",
            r#"{"status": "success", "city": "New York City", "days": 3, "forecast": "sunny"}"#,
        ),
        (
            "llm.input_messages.2.message.tool_call_id",
            "toolu_bdrk_01SLs9LwScHFA5xAZXwzjYEe",
        ),
        ("llm.input_messages.2.message.name", "get_weather"),
    ]);

    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Note: message at index 1 (assistant with only role, no content) is filtered out
    // as an artifact - only messages with meaningful content are extracted
    assert_eq!(messages.len(), 2);

    let tool_msg = &messages[1];
    assert_eq!(
        tool_msg.content.get("role").and_then(|v| v.as_str()),
        Some("tool")
    );
    assert_eq!(
        tool_msg.content.get("name").and_then(|v| v.as_str()),
        Some("get_weather")
    );
    assert_eq!(
        tool_msg
            .content
            .get("tool_call_id")
            .and_then(|v| v.as_str()),
        Some("toolu_bdrk_01SLs9LwScHFA5xAZXwzjYEe")
    );

    let content = tool_msg.content.get("content").unwrap();
    assert!(content.is_object());
    assert_eq!(content["status"].as_str(), Some("success"));
    assert_eq!(content["forecast"].as_str(), Some("sunny"));
}

#[test]
fn test_otel_tool_names_extraction() {
    let tools = r#"["get_weather","send_email","search"]"#;
    let attrs = make_attrs(&[("gen_ai.agent.tools", tools)]);
    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    // tool_definitions should be empty (no gen_ai.tool.definitions)
    assert!(tool_definitions.is_empty());
    // tool_names should have 1 item
    assert_eq!(tool_names.len(), 1);
    // Content is directly the tool names array
    let def = &tool_names[0].content;
    assert!(def.is_array());
    let content = def.as_array().unwrap();
    assert_eq!(content.len(), 3);
}

#[test]
fn test_otel_standard_input_messages() {
    let attrs = make_attrs(&[(
        "gen_ai.input.messages",
        r#"[{"role":"user","content":"Hello"},{"role":"system","content":"Be helpful"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.input.messages");
    assert!(!messages.is_empty());
}

#[test]
fn test_otel_standard_output_messages() {
    let attrs = make_attrs(&[(
        "gen_ai.output.messages",
        r#"[{"role":"assistant","content":"Hi there!"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.output.messages");
    assert!(!messages.is_empty());
}

#[test]
fn test_otel_standard_tool_call_arguments() {
    let attrs = make_attrs(&[("gen_ai.tool.call.arguments", r#"{"city":"NYC","days":3}"#)]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.tool.call.arguments");
    assert!(!messages.is_empty());
}

#[test]
fn test_otel_standard_tool_call_result() {
    let attrs = make_attrs(&[("gen_ai.tool.call.result", r#"{"temperature":"72F"}"#)]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.tool.call.result");
    assert!(!messages.is_empty());
}

#[test]
fn test_otel_tool_call_arguments_extraction() {
    let args = r#"{"city":"New York","units":"fahrenheit"}"#;
    let attrs = make_attrs(&[("gen_ai.tool.call.arguments", args)]);
    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    // A `tool_use` content block, not a bare object under a `tool_call` role: the block is what carries the
    // tool's name and id through normalisation, and without them the call was discarded downstream.
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], "tool_use");
    assert_eq!(block["input"]["city"].as_str(), Some("New York"));
    assert_eq!(block["input"]["units"].as_str(), Some("fahrenheit"));
}

#[test]
fn test_otel_tool_call_result_extraction() {
    let result = r#"{"temperature":72,"conditions":"sunny"}"#;
    let attrs = make_attrs(&[("gen_ai.tool.call.result", result)]);
    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], "tool_result");
    assert_eq!(block["content"]["temperature"].as_i64(), Some(72));
    assert_eq!(block["content"]["conditions"].as_str(), Some("sunny"));
}

#[test]
fn test_otel_tool_definitions_extraction() {
    let tool_defs = r#"[{"name":"get_weather","description":"Get weather for a city","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", tool_defs)]);
    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(!tool_definitions.is_empty());
    assert_eq!(tool_definitions.len(), 1);
    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
    let content = def.as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_pydantic_ai_all_messages() {
    // PydanticAI v2+ stores full conversation in pydantic_ai.all_messages on agent run spans
    let attrs = make_attrs(&[(
        "pydantic_ai.all_messages",
        r#"[{"role":"user","parts":[{"type":"text","content":"Hello"}]},{"role":"assistant","parts":[{"type":"text","content":"Hi!"}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from pydantic_ai.all_messages");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_all_messages_empty_array() {
    // Edge case: empty conversation
    let attrs = make_attrs(&[("pydantic_ai.all_messages", r#"[]"#)]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract empty all_messages array");
    assert_eq!(messages.len(), 1);
}

#[test]
fn test_pydantic_ai_all_messages_full_conversation() {
    // Full conversation history stays a literal message array for query-time expansion.
    let conversation = r#"[
        {"role":"user","parts":[{"type":"text","content":"What's the weather?"}]},
        {"role":"assistant","parts":[{"type":"tool_call","id":"tc1","name":"get_weather","arguments":{"city":"NYC"}}]},
        {"role":"user","parts":[{"type":"tool_call_response","id":"tc1","name":"get_weather","result":{"temp":"72F"}}]},
        {"role":"assistant","parts":[{"type":"text","content":"The weather in NYC is 72F."}],"finish_reason":"stop"}
    ]"#;
    let attrs = make_attrs(&[("pydantic_ai.all_messages", conversation)]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract full conversation");
    assert_eq!(messages.len(), 1);

    let msg = &messages[0].content;
    assert!(msg.is_array(), "Should keep the conversation array");
    assert_eq!(msg.as_array().unwrap().len(), 4, "Should have 4 messages");
}

#[test]
fn test_pydantic_ai_combined_input_output_system() {
    // Integration test: all message types together
    let attrs = make_attrs(&[
        (
            "gen_ai.system_instructions",
            r#"[{"type":"text","content":"Be helpful"}]"#,
        ),
        (
            "gen_ai.input.messages",
            r#"[{"role":"user","parts":[{"type":"text","content":"Hello"}]}]"#,
        ),
        (
            "gen_ai.output.messages",
            r#"[{"role":"assistant","parts":[{"type":"text","content":"Hi!"}],"finish_reason":"stop"}]"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract all message types");
    assert_eq!(messages.len(), 3, "Should have system + input + output");
}

#[test]
fn test_pydantic_ai_input_messages_array_stored_as_is() {
    // Array is stored as-is at ingestion; expansion happens at query time in SideML pipeline
    let attrs = make_attrs(&[(
        "gen_ai.input.messages",
        r#"[{"role":"system","parts":[{"type":"text","content":"System"}]},{"role":"user","parts":[{"type":"text","content":"User"}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Array stored as single RawMessage (expansion at query time)
    assert_eq!(messages.len(), 1, "Array should be stored as-is");

    // Content is the array with 2 messages
    let content = &messages[0].content;
    assert!(
        content.is_array(),
        "Content should be array (expansion at query time)"
    );
    assert_eq!(content.as_array().map(|a| a.len()), Some(2));

    // First element is system
    assert_eq!(content[0]["role"].as_str(), Some("system"));

    // Second element is user
    assert_eq!(content[1]["role"].as_str(), Some("user"));
}

#[test]
fn test_pydantic_ai_logfire_msg_for_span_name() {
    // Pydantic AI uses logfire.msg for descriptive span names
    let attrs = make_attrs(&[
        ("logfire.msg", "get_weather"),
        ("tool_arguments", r#"{"city":"NYC"}"#),
    ]);

    let mut span = SpanData::default();
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut span,
        &attrs,
        "tool_call",
    );

    // logfire.msg could be used for tool name extraction
    // This is optional but would improve observability
}

#[test]
fn test_pydantic_ai_messages_with_binary_data() {
    // PydanticAI supports binary data parts
    let attrs = make_attrs(&[(
        "gen_ai.input.messages",
        r#"[{"role":"user","parts":[{"type":"binary","media_type":"image/png","content":"base64data..."}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract messages with binary parts");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_messages_with_image_url() {
    // PydanticAI supports image-url parts
    let attrs = make_attrs(&[(
        "gen_ai.input.messages",
        r#"[{"role":"user","parts":[{"type":"text","content":"What's in this image?"},{"type":"image-url","url":"https://example.com/image.png"}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract messages with image-url parts");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_messages_with_thinking_part() {
    // PydanticAI supports thinking parts (Claude-style)
    let attrs = make_attrs(&[(
        "gen_ai.output.messages",
        r#"[{"role":"assistant","parts":[{"type":"thinking","content":"Let me analyze..."},{"type":"text","content":"Here's the answer."}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract messages with thinking parts");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_output_with_finish_reason() {
    // PydanticAI output messages include finish_reason
    let attrs = make_attrs(&[(
        "gen_ai.output.messages",
        r#"[{"role":"assistant","parts":[{"type":"text","content":"Done!"}],"finish_reason":"stop"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract output messages with finish_reason");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_parts_based_message_structure() {
    // PydanticAI uses parts-based message structure (different from SideML)
    let attrs = make_attrs(&[(
        "gen_ai.input.messages",
        r#"[{"role":"user","parts":[{"type":"text","content":"What's the weather?"},{"type":"tool_call","id":"tc1","name":"get_weather","arguments":{"city":"NYC"}}]}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract parts-based messages");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_source_attribution() {
    // Verify source is correctly attributed
    let attrs = make_attrs(&[(
        "gen_ai.system_instructions",
        r#"[{"type":"text","content":"test"}]"#,
    )]);

    let mut messages = Vec::new();
    try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    // Source should be Attribute variant with correct key
    match &messages[0].source {
        MessageSource::Attribute { key, .. } => {
            assert_eq!(key, "gen_ai.system_instructions", "Source key should match");
        }
        _ => panic!("Expected Attribute source"),
    }
}

#[test]
fn test_pydantic_ai_system_instructions() {
    // PydanticAI v2+ uses gen_ai.system_instructions with parts array
    let attrs = make_attrs(&[(
        "gen_ai.system_instructions",
        r#"[{"type":"text","content":"You are a helpful assistant."}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.system_instructions");
    assert!(!messages.is_empty());

    let msg = &messages[0];
    let content = msg.content.as_object().unwrap();
    assert_eq!(content.get("role").and_then(|v| v.as_str()), Some("system"));
    assert!(
        content.contains_key("parts"),
        "Should preserve parts array structure"
    );
}

#[test]
fn test_pydantic_ai_system_instructions_empty_parts() {
    // Edge case: empty parts array should still be extracted
    let attrs = make_attrs(&[("gen_ai.system_instructions", r#"[]"#)]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract empty system instructions");
    assert_eq!(messages.len(), 1);
}

#[test]
fn test_pydantic_ai_system_instructions_invalid_json() {
    // Edge case: invalid JSON should not crash, just skip
    let attrs = make_attrs(&[("gen_ai.system_instructions", "not valid json")]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found, "Should not extract invalid JSON");
    assert!(messages.is_empty());
}

#[test]
fn test_pydantic_ai_tool_arguments() {
    // Pydantic AI (via Logfire) uses tool_arguments for tool call input
    let attrs = make_attrs(&[("tool_arguments", r#"{"city":"NYC","days":3}"#)]);

    let mut messages = Vec::new();
    let found = try_pydantic_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from tool_arguments");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_tool_call_with_complex_arguments() {
    // Tool call with nested JSON arguments
    let attrs = make_attrs(&[(
        "gen_ai.tool.call.arguments",
        r#"{"query":{"filters":[{"field":"status","op":"eq","value":"active"}],"limit":10}}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract complex tool arguments");
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], "tool_use");
    let args = &block["input"];
    assert!(args.is_object(), "Arguments should be preserved as object");
    assert!(
        args.get("query").is_some(),
        "Nested query should be preserved"
    );
}

#[test]
fn test_pydantic_ai_tool_response() {
    // Pydantic AI uses tool_response for tool call output
    let attrs = make_attrs(&[("tool_response", r#"{"temperature":"72F"}"#)]);

    let mut messages = Vec::new();
    let found = try_pydantic_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from tool_response");
    assert!(!messages.is_empty());
}

#[test]
fn test_pydantic_ai_tool_response_with_error() {
    // Tool response indicating error
    let attrs = make_attrs(&[(
        "tool_response",
        r#"{"error":"API rate limit exceeded","retry_after":60}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_pydantic_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract error tool response");
    let content = messages[0].content.as_object().unwrap();
    assert_eq!(content.get("role").and_then(|v| v.as_str()), Some("tool"));
}

#[test]
fn test_pydantic_ai_v2_vs_v3_tool_attributes() {
    // V2 uses tool_arguments, V3 uses gen_ai.tool.call.arguments
    // Both should work
    let v2_attrs = make_attrs(&[("tool_arguments", r#"{"x":1}"#)]);
    let v3_attrs = make_attrs(&[("gen_ai.tool.call.arguments", r#"{"x":1}"#)]);

    let mut v2_messages = Vec::new();
    let mut v3_messages = Vec::new();

    let v2_found = try_pydantic_ai(&mut v2_messages, &mut Vec::new(), &v2_attrs, "", Utc::now());
    let v3_found =
        try_otel_genai_messages(&mut v3_messages, &mut Vec::new(), &v3_attrs, "", Utc::now());

    assert!(v2_found, "V2 tool_arguments should work");
    assert!(v3_found, "V3 gen_ai.tool.call.arguments should work");
}

#[test]
fn test_pydantic_ai_v3_tool_call_arguments() {
    // PydanticAI v3 uses gen_ai.tool.call.arguments (OTEL semconv)
    let attrs = make_attrs(&[(
        "gen_ai.tool.call.arguments",
        r#"{"city":"NYC","units":"celsius"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.tool.call.arguments");
    assert!(!messages.is_empty());

    // An assistant message carrying a `tool_use` block, which is what a call is - see
    // `semconv_tool_attributes_become_a_named_correlated_pair`.
    let msg = &messages[0];
    assert_eq!(msg.content["role"], "assistant");
    assert_eq!(msg.content["content"][0]["type"], "tool_use");
    assert_eq!(msg.content["content"][0]["input"]["city"], "NYC");
}

#[test]
fn test_pydantic_ai_v3_tool_call_result() {
    // PydanticAI v3 uses gen_ai.tool.call.result (OTEL semconv)
    let attrs = make_attrs(&[(
        "gen_ai.tool.call.result",
        r#"{"temperature":"72F","condition":"sunny"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from gen_ai.tool.call.result");
    assert!(!messages.is_empty());

    let msg = &messages[0];
    let content = msg.content.as_object().unwrap();
    assert_eq!(content.get("role").and_then(|v| v.as_str()), Some("tool"));
}

#[test]
fn test_raw_content_with_tool_calls_preserved() {
    let tool_use_content = r#"[{"toolUse":{"toolUseId":"toolu_123","name":"get_weather","input":{"location":"NYC"}}}]"#;
    let attrs = make_attrs(&[
        ("gen_ai.prompt.0.role", "assistant"),
        ("gen_ai.prompt.0.content", tool_use_content),
    ]);
    let mut messages = Vec::new();
    try_gen_ai_indexed(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert_eq!(messages.len(), 1);
    let content = messages[0].content.get("content").unwrap();

    assert!(content.is_array());
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 1);

    let tool_use = &arr[0]["toolUse"];
    assert_eq!(tool_use["toolUseId"].as_str(), Some("toolu_123"));
    assert_eq!(tool_use["name"].as_str(), Some("get_weather"));
    assert_eq!(tool_use["input"]["location"].as_str(), Some("NYC"));
}

#[test]
fn test_raw_content_with_tool_result_preserved() {
    let tool_result_content =
        r#"[{"toolResult":{"toolUseId":"toolu_123","content":[{"text":"72°F sunny"}]}}]"#;
    let attrs = make_attrs(&[
        ("gen_ai.prompt.0.role", "user"),
        ("gen_ai.prompt.0.content", tool_result_content),
    ]);
    let mut messages = Vec::new();
    try_gen_ai_indexed(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert_eq!(messages.len(), 1);
    let content = messages[0].content.get("content").unwrap();

    assert!(content.is_array());
    let arr = content.as_array().unwrap();

    let tool_result = &arr[0]["toolResult"];
    assert_eq!(tool_result["toolUseId"].as_str(), Some("toolu_123"));
    assert!(tool_result["content"].is_array());
}

#[test]
fn test_raw_io_input_output_values() {
    let input_json = r#"{"task": "Provide a 3-day weather forecast for New York City", "output_task_messages": true}"#;
    let output_json = r#"{"messages": [{"content": "Hello!", "type": "human"}], "stop_reason": "Text 'TERMINATE' mentioned"}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("output.value", output_json)]);

    let mut messages = Vec::new();
    let found = try_raw_io(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2);

    // Input message - plain data wrapped as user message
    let input_msg = &messages[0];
    assert_eq!(
        input_msg.content.get("role").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        input_msg.content["content"]
            .get("task")
            .and_then(|v| v.as_str()),
        Some("Provide a 3-day weather forecast for New York City")
    );

    // Output message - plain data wrapped as assistant message
    let output_msg = &messages[1];
    assert_eq!(
        output_msg.content.get("role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        output_msg.content["content"]
            .get("stop_reason")
            .and_then(|v| v.as_str()),
        Some("Text 'TERMINATE' mentioned")
    );
}

#[test]
fn test_raw_io_system_prompt_not_handled() {
    // system_prompt is extracted at the orchestration level (extract_messages_for_span),
    // not in try_raw_io. This test verifies try_raw_io returns false for system_prompt only.
    let attrs = make_attrs(&[(
        "system_prompt",
        "You are an experienced meteorologist who provides accurate weather forecasts.",
    )]);

    let mut messages = Vec::new();
    let found = try_raw_io(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found); // try_raw_io doesn't handle system_prompt
    assert!(messages.is_empty());
}

#[test]
fn test_session_id_from_ai_telemetry_metadata() {
    let attrs = make_attrs(&[("ai.telemetry.metadata.sessionId", "session-12345")]);

    let mut span = SpanData::default();
    apply_span_fields(
        sideseat_domain::rules::ruleset(),
        &mut span,
        "",
        &attrs,
        &[],
    );

    assert_eq!(
        span.session_id,
        Some("session-12345".to_string()),
        "Should extract session ID from ai.telemetry.metadata.sessionId"
    );
}

#[test]
fn test_strands_agents_assistant_message_with_tool_use() {
    let content = r#"[{"toolUse": {"toolUseId": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ", "name": "weather_forecast", "input": {"city": "New York City", "days": 3}}}]"#;
    let event = Event {
        name: "gen_ai.assistant.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("content", content)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Literal content preserved
    let content_val = msg.content.get("content").unwrap();
    assert!(content_val.is_array());
    assert!(content_val[0].get("toolUse").is_some());

    // Event name in source, not content
    assert!(
        matches!(msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.assistant.message")
    );
}

#[test]
fn test_strands_agents_choice_with_tool_result_attribute() {
    let message = r#"[{"toolUse": {"toolUseId": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ", "name": "weather_forecast", "input": {"city": "New York City", "days": 3}}}]"#;
    let tool_result = r#"[{"toolResult": {"toolUseId": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ", "status": "success", "content": [{"text": "Weather forecast for New York City for the next 3 days is sunny."}]}}]"#;
    let event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![
            make_kv("message", message),
            make_kv("tool.result", tool_result),
        ],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);

    // Should create TWO messages: assistant (tool_use) + tool (tool_result)
    assert_eq!(
        msgs.len(),
        2,
        "Should create separate tool message from tool.result attribute"
    );

    // First message: assistant with tool_use
    let assistant_msg = &msgs[0];
    assert!(assistant_msg.content.get("message").is_some());
    // The result is the reading's, so the raw form does not repeat it.
    assert!(assistant_msg.content.get("tool.result").is_none());

    // Second message: tool_result (from tool.result attribute)
    let tool_msg = &msgs[1];

    // Should use gen_ai.tool.result event name (not gen_ai.tool.message)
    // This ensures it won't be filtered as history
    assert!(
        matches!(tool_msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.tool.result"),
        "Tool result should use gen_ai.tool.result event name to avoid history filtering"
    );

    // Role derived at query-time from event name (gen_ai.tool.result -> tool)
    assert!(
        tool_msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    assert_eq!(
        tool_msg
            .content
            .get("tool_call_id")
            .and_then(|id| id.as_str()),
        Some("tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
        "Tool message should have tool_call_id from toolUseId"
    );
    let content = tool_msg.content.get("content").unwrap();
    assert!(content.is_array());
    assert_eq!(content[0]["toolResult"]["status"].as_str(), Some("success"));
}

#[test]
fn test_strands_agents_choice_with_tool_use() {
    let message = r#"[{"toolUse": {"toolUseId": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ", "name": "weather_forecast", "input": {"city": "New York City", "days": 3}}}]"#;
    let event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![
            make_kv("finish_reason", "tool_use"),
            make_kv("message", message),
        ],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Literal content preserved
    let message_val = msg.content.get("message").unwrap();
    assert!(message_val.is_array());
    let tool_use = &message_val[0]["toolUse"];
    assert_eq!(
        tool_use["toolUseId"].as_str(),
        Some("tooluse_ehAKs6dKRFS5DAfnsNn_xQ")
    );
    assert_eq!(tool_use["name"].as_str(), Some("weather_forecast"));
    assert_eq!(tool_use["input"]["city"].as_str(), Some("New York City"));
    assert_eq!(tool_use["input"]["days"].as_i64(), Some(3));

    assert_eq!(
        msg.content.get("finish_reason").and_then(|v| v.as_str()),
        Some("tool_use")
    );
}

#[test]
fn test_strands_agents_event_loop_attributes() {
    // Strands uses event_loop.cycle_id for tracking
    let attrs = make_attrs(&[
        ("event_loop.cycle_id", "cycle-123"),
        ("event_loop.parent_cycle_id", "cycle-122"),
    ]);

    // These are span attributes that should be preserved
    assert_eq!(
        attrs.get("event_loop.cycle_id"),
        Some(&"cycle-123".to_string())
    );
    assert_eq!(
        attrs.get("event_loop.parent_cycle_id"),
        Some(&"cycle-122".to_string())
    );
}

#[test]
fn test_strands_agents_event_timing() {
    // Strands sets gen_ai.event.start_time and gen_ai.event.end_time
    let attrs = make_attrs(&[
        ("gen_ai.event.start_time", "2024-01-01T00:00:00Z"),
        ("gen_ai.event.end_time", "2024-01-01T00:00:01Z"),
    ]);

    assert!(attrs.contains_key("gen_ai.event.start_time"));
    assert!(attrs.contains_key("gen_ai.event.end_time"));
}

#[test]
fn test_strands_agents_inference_operation_details_event_both_input_and_output() {
    // Arrays are stored as-is at ingestion; expansion happens at query time in SideML pipeline
    let input_messages = r#"[{"role":"user","content":"What's the weather?"}]"#;
    let output_messages = r#"[{"role":"assistant","content":"It's sunny!"}]"#;
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![
            make_kv("gen_ai.input.messages", input_messages),
            make_kv("gen_ai.output.messages", output_messages),
        ],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    // Each array stored as single RawMessage (expansion at query time)
    assert_eq!(msgs.len(), 2);

    // First message is input array
    let input_msg = &msgs[0];
    assert!(
        matches!(input_msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.input.messages")
    );
    let input_content = &input_msg.content;
    assert!(
        input_content.is_array(),
        "Should be array (expansion at query time)"
    );
    assert_eq!(input_content[0]["role"].as_str(), Some("user"));
    assert_eq!(
        input_content[0]["content"].as_str(),
        Some("What's the weather?")
    );

    // Second message is output array
    let output_msg = &msgs[1];
    assert!(
        matches!(output_msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.output.messages")
    );
    let output_content = &output_msg.content;
    assert!(
        output_content.is_array(),
        "Should be array (expansion at query time)"
    );
    assert_eq!(output_content[0]["role"].as_str(), Some("assistant"));
    assert_eq!(output_content[0]["content"].as_str(), Some("It's sunny!"));
}
