#[test]
fn test_strands_agents_inference_operation_details_event_complex_messages() {
    // Array is stored as-is at ingestion; expansion happens at query time in SideML pipeline
    let output_messages = r#"[
        {"role":"assistant","content":null,"tool_calls":[{"id":"call_123","function":{"name":"get_weather","arguments":"{\"city\":\"NYC\"}"}}]},
        {"role":"tool","tool_call_id":"call_123","content":"72°F, sunny"},
        {"role":"assistant","content":"The weather in NYC is 72°F and sunny."}
    ]"#;
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("gen_ai.output.messages", output_messages)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    // Array stored as single RawMessage (expansion at query time)
    assert_eq!(msgs.len(), 1);
    let content = &msgs[0].content;

    // Content is the array with 3 messages
    assert!(
        content.is_array(),
        "Should be array (expansion at query time)"
    );
    assert_eq!(content.as_array().map(|a| a.len()), Some(3));

    // First element: assistant with tool calls
    assert_eq!(content[0]["role"].as_str(), Some("assistant"));
    assert!(content[0]["tool_calls"].is_array());

    // Second element: tool response
    assert_eq!(content[1]["role"].as_str(), Some("tool"));
    assert_eq!(content[1]["tool_call_id"].as_str(), Some("call_123"));

    // Third element: final assistant response
    assert_eq!(content[2]["role"].as_str(), Some("assistant"));
    assert_eq!(
        content[2]["content"].as_str(),
        Some("The weather in NYC is 72°F and sunny.")
    );
}

#[test]
fn test_strands_agents_inference_operation_details_event_input() {
    // Array is stored as-is at ingestion; expansion happens at query time in SideML pipeline
    let input_messages = r#"[{"role":"user","content":"What's the weather?"}]"#;
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("gen_ai.input.messages", input_messages)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    // Array stored as single RawMessage (expansion at query time)
    assert_eq!(msgs.len(), 1);
    let msg = &msgs[0];

    // Content is the array (not expanded at ingestion)
    let content = &msg.content;
    assert!(
        content.is_array(),
        "Content should be array (expansion at query time)"
    );
    assert_eq!(content[0]["role"].as_str(), Some("user"));
    assert_eq!(content[0]["content"].as_str(), Some("What's the weather?"));

    // Source should be gen_ai.input.messages
    assert!(
        matches!(msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.input.messages")
    );
}

#[test]
fn test_strands_agents_inference_operation_details_event_no_messages() {
    // Event without messages attribute should return empty vec
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("some_other_attr", "value")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    assert!(msgs.is_empty());
}

#[test]
fn test_strands_agents_inference_operation_details_event_output() {
    // Array is stored as-is at ingestion; expansion happens at query time in SideML pipeline
    let output_messages =
        r#"[{"role":"assistant","content":"The weather in NYC is 72°F and sunny."}]"#;
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("gen_ai.output.messages", output_messages)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    // Array stored as single RawMessage (expansion at query time)
    assert_eq!(msgs.len(), 1);
    let msg = &msgs[0];

    // Content is the array (not expanded at ingestion)
    let content = &msg.content;
    assert!(
        content.is_array(),
        "Content should be array (expansion at query time)"
    );
    assert_eq!(content[0]["role"].as_str(), Some("assistant"));
    assert_eq!(
        content[0]["content"].as_str(),
        Some("The weather in NYC is 72°F and sunny.")
    );

    // Source should be gen_ai.output.messages
    assert!(
        matches!(msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.output.messages")
    );
}

#[test]
fn test_strands_agents_tool_input_event() {
    let content = r#"{"city": "New York City", "days": 3}"#;
    let event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![
            make_kv("role", "tool"),
            make_kv("content", content),
            make_kv("id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
        ],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    assert_eq!(
        msg.content.get("role").and_then(|v| v.as_str()),
        Some("tool")
    );
    assert_eq!(
        msg.content.get("id").and_then(|v| v.as_str()),
        Some("tooluse_ehAKs6dKRFS5DAfnsNn_xQ")
    );

    let content_val = msg.content.get("content").unwrap();
    assert!(content_val.is_object());
    assert_eq!(content_val["city"].as_str(), Some("New York City"));
}

#[test]
fn test_strands_agents_tool_message_with_result() {
    let content = r#"[{"toolResult": {"toolUseId": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ", "status": "success", "content": [{"text": "Weather forecast for New York City for the next 3 days is sunny."}]}}]"#;
    let event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("content", content)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Literal content preserved
    let content_val = msg.content.get("content").unwrap();
    assert!(content_val.is_array());
    let tool_result = &content_val[0]["toolResult"];
    assert_eq!(
        tool_result["toolUseId"].as_str(),
        Some("tooluse_ehAKs6dKRFS5DAfnsNn_xQ")
    );
    assert_eq!(tool_result["status"].as_str(), Some("success"));
    assert_eq!(
        tool_result["content"][0]["text"].as_str(),
        Some("Weather forecast for New York City for the next 3 days is sunny.")
    );
}

#[test]
fn test_strands_agents_user_message_event() {
    let content =
        r#"[{"text": "Provide a 3-day weather forecast for New York City and greet the user."}]"#;
    let event = Event {
        name: "gen_ai.user.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("content", content)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Literal content preserved
    let content_val = msg.content.get("content").unwrap();
    assert!(content_val.is_array());
    assert_eq!(
        content_val[0]["text"].as_str(),
        Some("Provide a 3-day weather forecast for New York City and greet the user.")
    );

    // Event name in source, not content
    assert!(
        matches!(msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.user.message")
    );
}

#[test]
fn test_strands_style_tool_definitions() {
    // Strands uses detailed tool schemas with json_schema
    let definitions_json = r#"[
        {
            "name": "weather_forecast",
            "description": "Get weather forecast for a city",
            "json_schema": {
                "type": "object",
                "properties": {
                    "city": {"type": "string", "description": "City name"},
                    "days": {"type": "integer", "description": "Forecast days", "default": 3}
                },
                "required": ["city"]
            }
        }
    ]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", definitions_json)]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(!tool_definitions.is_empty());
    let def = &tool_definitions[0];
    // Content is directly the tools array
    assert!(def.content.is_array());
    let tools = def.content.as_array().unwrap();
    assert_eq!(tools[0]["name"].as_str(), Some("weather_forecast"));
    assert!(tools[0]["json_schema"].is_object());
    assert_eq!(
        tools[0]["json_schema"]["properties"]["city"]["description"].as_str(),
        Some("City name")
    );
}

#[test]
fn test_strands_tool_result_in_tool_message_event() {
    let content =
        r#"[{"toolResult":{"toolUseId":"tool_001","status":"success","content":[{"text":"4"}]}}]"#;
    let event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![make_kv("content", content)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    assert_eq!(msgs.len(), 1);
    let msg = &msgs[0];
    let content_val = msg.content.get("content").unwrap();
    let tool_result = &content_val[0]["toolResult"];
    assert_eq!(tool_result["toolUseId"].as_str(), Some("tool_001"));
    assert_eq!(tool_result["status"].as_str(), Some("success"));
    assert_eq!(tool_result["content"][0]["text"].as_str(), Some("4"));
}

#[test]
fn test_strands_tool_use_in_choice_event() {
    let message = r#"[{"toolUse":{"toolUseId":"tool_001","name":"calculator","input":{"expression":"2+2"}}}]"#;
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
    assert_eq!(msgs.len(), 1);
    let msg = &msgs[0];
    let message_val = msg.content.get("message").unwrap();
    let tool_use = &message_val[0]["toolUse"];
    assert_eq!(tool_use["name"].as_str(), Some("calculator"));
    assert_eq!(tool_use["input"]["expression"].as_str(), Some("2+2"));
}

#[test]
fn traceloop_decorator_values_are_not_chat_messages() {
    let attrs = make_attrs(&[
        (
            "traceloop.entity.input",
            r#"{"args":["model"],"kwargs":{"location":"Paris"}}"#,
        ),
        (
            "traceloop.entity.output",
            r#"{"first":"answer","second":"another answer"}"#,
        ),
    ]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut tool_definitions,
        &attrs,
        "workflow",
        Utc::now(),
        ExtractionMode::FirstMatch,
        false,
    );

    assert!(messages.is_empty());
    assert!(tool_definitions.is_empty());
}

#[test]
fn test_user_id_from_ai_telemetry_metadata() {
    let attrs = make_attrs(&[("ai.telemetry.metadata.userId", "user-67890")]);

    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(
        span.user_id,
        Some("user-67890".to_string()),
        "Should extract user ID from ai.telemetry.metadata.userId"
    );
}

#[test]
fn test_vercel_ai_legacy_result_object_fallback() {
    // SDK versions < 4.0.0 use ai.result.object for structured output
    let attrs = make_attrs(&[("ai.result.object", r#"{"name":"John","age":30}"#)]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract messages from legacy ai.result.object"
    );
}

#[test]
fn test_vercel_ai_legacy_result_text_fallback() {
    // SDK versions < 4.0.0 use ai.result.text instead of ai.response.text
    let attrs = make_attrs(&[
        (
            "ai.prompt.messages",
            r#"[{"role":"user","content":"Hello"}]"#,
        ),
        ("ai.result.text", "Legacy response"),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract messages from legacy ai.result.text");

    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"));
    assert!(
        response.is_some(),
        "Should create assistant message from ai.result.text"
    );
    assert_eq!(
        response
            .unwrap()
            .content
            .get("content")
            .and_then(|v| v.as_str()),
        Some("Legacy response")
    );
}

#[test]
fn test_vercel_ai_legacy_result_tool_calls_fallback() {
    // SDK versions < 4.0.0 use ai.result.toolCalls
    let attrs = make_attrs(&[(
        "ai.result.toolCalls",
        r#"[{"id":"call_123","name":"get_weather"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract messages from legacy ai.result.toolCalls"
    );
    assert!(
        messages[0].content.get("tool_calls").is_some(),
        "Should have tool_calls from legacy attribute"
    );
}

#[test]
fn test_vercel_ai_prompt_tools_extraction() {
    // ai.prompt.tools contains tool definitions - now extracted by extract_tool_definitions()
    let attrs = make_attrs(&[(
        "ai.prompt.tools",
        r#"[{"type":"function","function":{"name":"get_weather","description":"Get weather","inputSchema":{"type":"object"}}}]"#,
    )]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(
        tool_definitions.len(),
        1,
        "Should extract tool definitions from ai.prompt.tools"
    );
    assert!(tool_definitions[0].content.is_array());
}

#[test]
fn test_vercel_ai_prompt_tools_with_tool_choice() {
    // ai.prompt.tools with ai.prompt.toolChoice - now extracted by extract_tool_definitions()
    let attrs = make_attrs(&[
        (
            "ai.prompt.tools",
            r#"[{"type":"function","function":{"name":"get_weather"}}]"#,
        ),
        ("ai.prompt.toolChoice", r#"{"type":"required"}"#),
    ]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1, "Should extract tool definitions");
    // Tool definitions stored as array (tool_choice not stored in simplified format)
    assert!(tool_definitions[0].content.is_array());
}

#[test]
fn test_vercel_ai_response_uses_content_not_text() {
    // Vercel AI SDK outputs ai.response.text, but SideML expects "content"
    let attrs = make_attrs(&[
        (
            "ai.prompt.messages",
            r#"[{"role":"user","content":"Hello"}]"#,
        ),
        ("ai.response.text", "Hi there!"),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);

    // Find the response message (assistant role)
    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"));
    assert!(response.is_some(), "Should have assistant response message");

    let response = response.unwrap();
    // Should have "content" not "text" for SideML compatibility
    assert_eq!(
        response.content.get("content").and_then(|v| v.as_str()),
        Some("Hi there!"),
        "Response should use 'content' field, not 'text'"
    );
    assert!(
        response.content.get("text").is_none(),
        "Response should not have 'text' field (use 'content' instead)"
    );
}

#[test]
fn test_vercel_ai_system_message_extraction() {
    // Vercel AI SDK includes system message in ai.prompt.messages array
    let attrs = make_attrs(&[(
        "ai.prompt.messages",
        r#"[{"role":"system","content":"You are a helpful assistant"},{"role":"user","content":"Hello"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(
        messages.len(),
        2,
        "Should extract both system and user messages"
    );

    // Verify system message is extracted
    let system_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"));
    assert!(
        system_msg.is_some(),
        "Should extract system message from ai.prompt.messages"
    );

    let system_msg = system_msg.unwrap();
    assert_eq!(
        system_msg.content.get("content").and_then(|v| v.as_str()),
        Some("You are a helpful assistant"),
        "System message should have correct content"
    );
}

#[test]
fn test_vercel_ai_response_with_tool_calls_combined() {
    // When both text and toolCalls present, should combine them properly
    let attrs = make_attrs(&[
        ("ai.response.text", "Let me check the weather."),
        (
            "ai.response.toolCalls",
            r#"[{"id":"call_123","name":"get_weather","arguments":"{\"city\":\"NYC\"}"}]"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let response = &messages[0];

    assert_eq!(
        response.content.get("role").and_then(|v| v.as_str()),
        Some("assistant")
    );
    assert_eq!(
        response.content.get("content").and_then(|v| v.as_str()),
        Some("Let me check the weather.")
    );
    // tool_calls should use snake_case for SideML compatibility
    assert!(
        response.content.get("tool_calls").is_some(),
        "Should have tool_calls field (snake_case)"
    );
}

#[test]
fn test_vercel_ai_simple_prompt_fallback() {
    // Some Vercel AI SDK versions use ai.prompt instead of ai.prompt.messages
    let attrs = make_attrs(&[("ai.prompt", r#"[{"role":"user","content":"Hello"}]"#)]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract from ai.prompt when ai.prompt.messages not present"
    );
}

#[test]
fn test_vercel_ai_tool_call_extraction() {
    // Tool call spans have ai.toolCall.args and ai.toolCall.result
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.args", r#"{"city":"NYC"}"#),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract tool call input from ai.toolCall.args"
    );

    let tool_call = &messages[0];
    assert!(
        tool_call.content.get("name").is_some() || tool_call.content.get("tool_name").is_some(),
        "Tool call should have name"
    );
}

#[test]
fn test_vercel_ai_tool_call_result() {
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.result", r#"{"temperature":"72F"}"#),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract tool result from ai.toolCall.result");
}

#[test]
fn test_vercel_ai_tool_call_with_id() {
    // ai.toolCall should include id when present
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.id", "call_abc123"),
        ("ai.toolCall.args", r#"{"location":"NYC"}"#),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages[0].content["role"], "tool_call");
    assert_eq!(messages[0].content["name"], "get_weather");
    assert_eq!(messages[0].content["tool_call_id"], "call_abc123");
}

#[test]
fn test_vercel_ai_tool_result_with_id() {
    // ai.toolCall.result should include id when present
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.id", "call_abc123"),
        ("ai.toolCall.result", r#"{"temperature":"72F"}"#),
    ]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Find the tool result message (not tool_call)
    let tool_result = messages
        .iter()
        .find(|m| m.content["role"] == "tool")
        .unwrap();
    assert_eq!(tool_result.content["name"], "get_weather");
    assert_eq!(tool_result.content["tool_call_id"], "call_abc123");
}

#[test]
fn test_vercel_response_with_tool_calls() {
    let tool_calls = r#"[{"id":"call_abc","type":"function","function":{"name":"search","arguments":"{\"query\":\"rust\"}"}}]"#;
    let attrs = make_attrs(&[
        ("ai.response.text", "Let me search for that."),
        ("ai.response.toolCalls", tool_calls),
    ]);
    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    assert_eq!(
        response.content.get("content").and_then(|c| c.as_str()),
        Some("Let me search for that.")
    );
    let tc = response
        .content
        .get("tool_calls")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(tc.len(), 1);
    assert_eq!(tc[0]["id"].as_str(), Some("call_abc"));
}

#[test]
fn test_vercel_ai_output_value_fallback() {
    // Root span (ai.generateText) has only output.value, no ai.prompt.messages or ai.response.text
    // This tests the output.value fallback for extracting the final response
    // The span name must start with "ai." to be recognized as a Vercel AI span
    let attrs = make_attrs(&[("output.value", "Perfect! Here's your weather forecast...")]);

    let mut messages = Vec::new();
    let found = try_vercel_ai(
        &mut messages,
        &mut Vec::new(),
        &attrs,
        "ai.generateText",
        Utc::now(),
    );

    assert!(
        found,
        "Should extract assistant message from output.value fallback"
    );

    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"));
    assert!(
        response.is_some(),
        "Should create assistant message from output.value"
    );
    assert_eq!(
        response
            .unwrap()
            .content
            .get("content")
            .and_then(|v| v.as_str()),
        Some("Perfect! Here's your weather forecast...")
    );
}

#[test]
fn test_vercel_tool_call_input_extraction() {
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.id", "call_xyz"),
        ("ai.toolCall.args", r#"{"city":"Boston"}"#),
    ]);
    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let tool_call = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool_call"))
        .unwrap();
    assert_eq!(
        tool_call.content.get("name").and_then(|n| n.as_str()),
        Some("get_weather")
    );
    assert_eq!(
        tool_call
            .content
            .get("tool_call_id")
            .and_then(|id| id.as_str()),
        Some("call_xyz")
    );
    assert_eq!(
        tool_call.content["content"]["city"].as_str(),
        Some("Boston")
    );
}

#[test]
fn test_vercel_tool_call_result_extraction() {
    let attrs = make_attrs(&[
        ("ai.toolCall.name", "get_weather"),
        ("ai.toolCall.id", "call_xyz"),
        ("ai.toolCall.result", r#"{"temp":55,"conditions":"rainy"}"#),
    ]);
    let mut messages = Vec::new();
    let found = try_vercel_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let tool_result = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .unwrap();
    assert_eq!(
        tool_result.content.get("name").and_then(|n| n.as_str()),
        Some("get_weather")
    );
    assert_eq!(
        tool_result
            .content
            .get("tool_call_id")
            .and_then(|id| id.as_str()),
        Some("call_xyz")
    );
    assert_eq!(tool_result.content["content"]["temp"].as_i64(), Some(55));
}

#[test]
fn test_vercel_tool_definitions_extraction() {
    // ai.prompt.tools now extracted by extract_tool_definitions()
    let tools = r#"[{"type":"function","function":{"name":"get_weather","description":"Get weather","parameters":{"type":"object"}}}]"#;
    let attrs = make_attrs(&[("ai.prompt.tools", tools)]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1);
    assert!(tool_definitions[0].content.is_array());
}

#[test]
fn test_vercel_tool_definitions_with_tool_choice() {
    // ai.prompt.tools now extracted by extract_tool_definitions()
    let tools = r#"[{"type":"function","function":{"name":"get_weather"}}]"#;
    let tool_choice = r#"{"type":"function","function":{"name":"get_weather"}}"#;
    let attrs = make_attrs(&[
        ("ai.prompt.tools", tools),
        ("ai.prompt.toolChoice", tool_choice),
    ]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1);
    assert!(tool_definitions[0].content.is_array());
}

// ============================================================================
// TOOL EXECUTION SPAN TESTS
// ============================================================================

#[test]
fn test_tool_execution_span_detection() {
    // Span with gen_ai.operation.name == "execute_tool" is a tool span
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "execute_tool"),
        ("gen_ai.tool.name", "weather_forecast"),
        ("gen_ai.tool.call.id", "tooluse_abc123"),
    ]);
    assert!(is_tool_execution_span(&attrs));

    // Span without execute_tool is not a tool span
    let attrs = make_attrs(&[("gen_ai.operation.name", "chat")]);
    assert!(!is_tool_execution_span(&attrs));

    // OpenInference tool span
    let attrs = make_attrs(&[("openinference.span.kind", "TOOL")]);
    assert!(is_tool_execution_span(&attrs));
}

#[test]
fn test_tool_span_events_are_extracted() {
    // Verify that events from tool execution spans are extracted
    let event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1702400000000000000,
        attributes: vec![
            make_kv("role", "tool"),
            make_kv("content", r#"{"city": "New York City", "days": 3}"#),
            make_kv("id", "tooluse_abc123"),
        ],
        dropped_attributes_count: 0,
    };

    // extract_message_from_event should extract the message regardless of span type
    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    assert_eq!(msgs.len(), 1, "Tool message event should be extracted");
    assert_eq!(msgs[0].content["role"], "tool");
}
