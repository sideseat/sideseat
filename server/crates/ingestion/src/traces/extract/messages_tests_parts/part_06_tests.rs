#[test]
fn test_strands_tool_span_with_input_and_output_events() {
    // Simulates the exact format from user's Strands agents example:
    // execute_tool span with gen_ai.tool.message (input) and gen_ai.choice (output)
    use opentelemetry_proto::tonic::trace::v1::Span;

    let tool_input_event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1767099299293199000,
        attributes: vec![
            make_kv("role", "tool"),
            make_kv("content", r#"{"city": "New York City", "days": 3}"#),
            make_kv("id", "tooluse_dWHzG8gDTCuPz8htTlsz_w"),
        ],
        dropped_attributes_count: 0,
    };

    let tool_output_event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1767099299294087000,
        attributes: vec![
            make_kv(
                "message",
                r#"[{"text": "Weather forecast for New York City for the next 3 days is sunny."}]"#,
            ),
            make_kv("id", "tooluse_dWHzG8gDTCuPz8htTlsz_w"),
        ],
        dropped_attributes_count: 0,
    };

    let span = Span {
        trace_id: vec![0; 16],
        span_id: vec![0; 8],
        parent_span_id: vec![],
        name: "execute_tool weather_forecast".to_string(),
        kind: 1,
        start_time_unix_nano: 1767099299293126000,
        end_time_unix_nano: 1767099299294119000,
        attributes: vec![
            make_kv("gen_ai.operation.name", "execute_tool"),
            make_kv("gen_ai.system", "strands-agents"),
            make_kv("gen_ai.tool.name", "weather_forecast"),
            make_kv("gen_ai.tool.call.id", "tooluse_dWHzG8gDTCuPz8htTlsz_w"),
        ],
        events: vec![tool_input_event, tool_output_event],
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
        ("gen_ai.system", "strands-agents"),
        ("gen_ai.tool.name", "weather_forecast"),
        ("gen_ai.tool.call.id", "tooluse_dWHzG8gDTCuPz8htTlsz_w"),
        (
            "gen_ai.tool.description",
            "Get weather forecast for a city.",
        ),
    ]);

    let (messages, tool_defs, _tool_names) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);

    // Both events should be extracted
    assert_eq!(
        messages.len(),
        2,
        "Both tool input and output events should be extracted from tool span"
    );

    // Verify tool input message (gen_ai.tool.message event)
    // Role is derived at query-time; find by event name
    let tool_input = messages.iter().find(
        |m| matches!(&m.source, MessageSource::Event { name, .. } if name == "gen_ai.tool.message"),
    );
    assert!(
        tool_input.is_some(),
        "Should have gen_ai.tool.message (input TO the tool)"
    );
    let tool_input = tool_input.unwrap();
    // Verify tool_call_id is set (from "id" in event or span attribute)
    assert_eq!(
        tool_input.content["tool_call_id"], "tooluse_dWHzG8gDTCuPz8htTlsz_w",
        "tool input should have tool_call_id"
    );
    // Verify name is enriched from span attributes
    assert_eq!(
        tool_input.content["name"], "weather_forecast",
        "tool input should have name from span attributes"
    );

    // Verify tool output message (gen_ai.choice event in tool span)
    // Role is derived at query-time; find by event name
    let tool_output = messages.iter().find(
        |m| matches!(&m.source, MessageSource::Event { name, .. } if name == "gen_ai.choice"),
    );
    assert!(
        tool_output.is_some(),
        "Should have gen_ai.choice (output FROM the tool)"
    );
    let tool_output = tool_output.unwrap();
    // Verify tool_call_id is set for correlation (used by extract_tool_use_id)
    assert_eq!(
        tool_output.content["tool_call_id"], "tooluse_dWHzG8gDTCuPz8htTlsz_w",
        "tool result should have tool_call_id for correlation"
    );

    // Tool definition should also be extracted from attributes
    assert!(
        !tool_defs.is_empty(),
        "Tool definition should be extracted from span attributes"
    );
}

#[test]
fn test_tool_span_extracts_tool_definitions() {
    // Tool execution spans should still extract tool definitions from attributes
    use opentelemetry_proto::tonic::trace::v1::Span;

    let span = Span {
        trace_id: vec![0; 16],
        span_id: vec![0; 8],
        parent_span_id: vec![],
        name: "execute_tool calculator".to_string(),
        kind: 1,
        start_time_unix_nano: 1702400000000000000,
        end_time_unix_nano: 1702400001000000000,
        attributes: vec![
            make_kv("gen_ai.operation.name", "execute_tool"),
            make_kv("gen_ai.tool.name", "calculator"),
            make_kv("gen_ai.tool.call.id", "call_123"),
            make_kv("gen_ai.tool.description", "Perform calculations"),
            make_kv(
                "gen_ai.tool.json_schema",
                r#"{"type":"object","properties":{"expression":{"type":"string"}}}"#,
            ),
        ],
        events: vec![],
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
        ("gen_ai.tool.name", "calculator"),
        ("gen_ai.tool.call.id", "call_123"),
        ("gen_ai.tool.description", "Perform calculations"),
        (
            "gen_ai.tool.json_schema",
            r#"{"type":"object","properties":{"expression":{"type":"string"}}}"#,
        ),
    ]);

    let (_messages, tool_defs, _tool_names) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);

    assert_eq!(
        tool_defs.len(),
        1,
        "Tool definition should be extracted from tool span attributes"
    );

    let tool_def = &tool_defs[0].content;
    assert!(tool_def.is_array());
    let func = &tool_def[0]["function"];
    assert_eq!(func["name"], "calculator");
    assert_eq!(func["description"], "Perform calculations");
}

/// Test with exact Strands tool span data from user samples.
/// This verifies the full extraction pipeline works with real-world data.
#[test]
fn test_strands_tool_span_exact_user_sample() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    // Exact data from user's span sample:
    // {
    //     "name": "execute_tool weather_forecast",
    //     "attributes": {
    //         "gen_ai.operation.name": "execute_tool",
    //         "gen_ai.system": "strands-agents",
    //         "gen_ai.tool.name": "weather_forecast",
    //         "gen_ai.tool.call.id": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"
    //     },
    //     "events": [
    //         { "name": "gen_ai.tool.message", "attributes": {
    //             "role": "tool",
    //             "content": "{\"city\": \"New York City\", \"days\": 3}",
    //             "id": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"
    //         }},
    //         { "name": "gen_ai.choice", "attributes": {
    //             "message": "[{\"text\": \"Weather forecast for New York City for the next 3 days is sunny.\"}]",
    //             "id": "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"
    //         }}
    //     ]
    // }

    let tool_input_event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1734023444422429000, // 2025-12-12T17:10:44.422429Z
        attributes: vec![
            make_kv("role", "tool"),
            make_kv("content", r#"{"city": "New York City", "days": 3}"#),
            make_kv("id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
        ],
        dropped_attributes_count: 0,
    };

    let tool_output_event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1734023444423705000, // 2025-12-12T17:10:44.423705Z
        attributes: vec![
            make_kv(
                "message",
                r#"[{"text": "Weather forecast for New York City for the next 3 days is sunny."}]"#,
            ),
            make_kv("id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
        ],
        dropped_attributes_count: 0,
    };

    let span = Span {
        trace_id: vec![
            0x84, 0xfa, 0xd8, 0xdd, 0x18, 0x4f, 0x29, 0x92, 0x49, 0x60, 0xd4, 0x66, 0xcc, 0x60,
            0xef, 0xca,
        ],
        span_id: vec![0x9c, 0x64, 0x34, 0xa9, 0x73, 0x9b, 0x2f, 0x81],
        parent_span_id: vec![0x1c, 0x72, 0x8b, 0xa2, 0x35, 0xf2, 0x57, 0x4f],
        name: "execute_tool weather_forecast".to_string(),
        kind: 1, // INTERNAL
        start_time_unix_nano: 1734023444422359000,
        end_time_unix_nano: 1734023444423724000,
        attributes: vec![
            make_kv("gen_ai.operation.name", "execute_tool"),
            make_kv("gen_ai.system", "strands-agents"),
            make_kv("gen_ai.tool.name", "weather_forecast"),
            make_kv("gen_ai.tool.call.id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
            make_kv(
                "gen_ai.tool.description",
                "Get weather forecast for a city.",
            ),
            make_kv(
                "gen_ai.tool.json_schema",
                r#"{"properties": {"city": {"description": "The name of the city", "type": "string"}, "days": {"default": 3, "description": "Number of days for the forecast", "type": "integer"}}, "required": ["city"], "type": "object"}"#,
            ),
        ],
        events: vec![tool_input_event, tool_output_event],
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
        ("gen_ai.system", "strands-agents"),
        ("gen_ai.tool.name", "weather_forecast"),
        ("gen_ai.tool.call.id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
        (
            "gen_ai.tool.description",
            "Get weather forecast for a city.",
        ),
        (
            "gen_ai.tool.json_schema",
            r#"{"properties": {"city": {"description": "The name of the city", "type": "string"}, "days": {"default": 3, "description": "Number of days for the forecast", "type": "integer"}}, "required": ["city"], "type": "object"}"#,
        ),
    ]);

    let (messages, tool_defs, _tool_names) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);

    // Both events should be extracted
    assert_eq!(
        messages.len(),
        2,
        "Should extract both tool input and output from tool span"
    );

    // Find tool input message (gen_ai.tool.message event)
    // Role derived at query-time; find by event name
    let tool_call = messages.iter().find(
        |m| matches!(&m.source, MessageSource::Event { name, .. } if name == "gen_ai.tool.message"),
    );
    assert!(tool_call.is_some(), "Should have tool_call message");
    let tool_call = tool_call.unwrap();

    // Verify tool_call has correct metadata
    assert_eq!(
        tool_call.content["tool_call_id"], "tooluse_ehAKs6dKRFS5DAfnsNn_xQ",
        "tool_call should have tool_call_id"
    );
    assert_eq!(
        tool_call.content["name"], "weather_forecast",
        "tool_call should have name from span attributes"
    );

    // Find tool output message (gen_ai.choice event in tool span)
    // Role derived at query-time; find by event name
    let tool_result = messages.iter().find(
        |m| matches!(&m.source, MessageSource::Event { name, .. } if name == "gen_ai.choice"),
    );
    assert!(tool_result.is_some(), "Should have tool result message");
    let tool_result = tool_result.unwrap();

    // Verify tool result has correct correlation ID
    assert_eq!(
        tool_result.content["tool_call_id"], "tooluse_ehAKs6dKRFS5DAfnsNn_xQ",
        "tool result should have tool_call_id for correlation"
    );

    // Verify tool definitions are extracted from span attributes
    assert_eq!(
        tool_defs.len(),
        1,
        "Tool definition should be extracted from span attributes"
    );
}

/// Test that chat spans with tool_use content are NOT affected by tool span changes.
/// The is_tool_span flag should only affect execute_tool spans.
#[test]
fn test_chat_span_with_tool_use_not_affected_by_tool_span_logic() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    // This is a CHAT span (not tool span) with gen_ai.choice containing toolUse
    let choice_event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1734023444405896000,
        attributes: vec![
            make_kv("finish_reason", "tool_use"),
            make_kv(
                "message",
                r#"[{"toolUse": {"toolUseId": "tooluse_abc", "name": "weather_forecast", "input": {"city": "NYC"}}}]"#,
            ),
        ],
        dropped_attributes_count: 0,
    };

    let span = Span {
        trace_id: vec![0; 16],
        span_id: vec![0; 8],
        parent_span_id: vec![],
        name: "chat".to_string(),
        kind: 1,
        start_time_unix_nano: 1734023443174753000,
        end_time_unix_nano: 1734023444406031000,
        attributes: vec![
            make_kv("gen_ai.operation.name", "chat"), // NOT execute_tool
            make_kv("gen_ai.system", "strands-agents"),
        ],
        events: vec![choice_event],
        links: vec![],
        status: None,
        trace_state: String::new(),
        flags: 0,
        dropped_attributes_count: 0,
        dropped_events_count: 0,
        dropped_links_count: 0,
    };

    let span_attrs = make_attrs(&[
        ("gen_ai.operation.name", "chat"),
        ("gen_ai.system", "strands-agents"),
    ]);

    let (messages, _tool_defs, _tool_names) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);

    assert_eq!(
        messages.len(),
        1,
        "Should extract one message from chat span"
    );

    // Role derivation happens at query-time, not extraction
    // Verify event name is preserved for query-time pipeline to derive role
    let msg = &messages[0];
    assert!(
        matches!(&msg.source, MessageSource::Event { name, .. } if name == "gen_ai.choice"),
        "Event name should be preserved for query-time role derivation"
    );

    // Content should be preserved (role not set at extraction)
    assert!(
        msg.content.get("message").is_some(),
        "Message content should be preserved"
    );
}

/// Test tool span without gen_ai.tool.call.id attribute - should still work
#[test]
fn test_tool_span_without_call_id_attribute() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    let tool_input_event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1734023444422429000,
        attributes: vec![
            make_kv("role", "tool"),
            make_kv("content", r#"{"city": "NYC"}"#),
            // Note: id is in event, not span attributes
            make_kv("id", "call_from_event"),
        ],
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
            // NO gen_ai.tool.call.id attribute
        ],
        events: vec![tool_input_event],
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
    ]);

    let (messages, _tool_defs, _tool_names) =
        extract_messages_for_span(&span, &span_attrs, Utc::now(), ExtractionMode::FirstMatch);

    assert_eq!(messages.len(), 1);
    let msg = &messages[0];

    // Raw role from event attributes is preserved during ingestion
    // Semantic role transformation happens at query-time in SideML
    assert_eq!(
        msg.content.get("role").and_then(|r| r.as_str()),
        Some("tool"),
        "Raw role from event attributes should be preserved"
    );

    // Event name preserved for query-time processing
    assert!(
        matches!(&msg.source, MessageSource::Event { name, .. } if name == "gen_ai.tool.message"),
        "Event name should be preserved"
    );

    // id field from event is mapped to tool_call_id for correlation
    assert_eq!(
        msg.content.get("tool_call_id").and_then(|r| r.as_str()),
        Some("call_from_event"),
        "id field from event should be mapped to tool_call_id"
    );
}

// ============================================================================
// EXTRACT_TOOL_DEFINITIONS TESTS
// ============================================================================

#[test]
fn test_extract_tool_definitions_vercel_ai_prompt_tools() {
    let tools = r#"[{"type":"function","function":{"name":"get_weather","description":"Get weather for a city"}}]"#;
    let attrs = make_attrs(&[("ai.prompt.tools", tools)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1, "Should extract ai.prompt.tools");
    assert!(tool_names.is_empty());

    let content = &tool_definitions[0].content;
    assert!(content.is_array());
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["function"]["name"].as_str(),
        Some("get_weather"),
        "Should preserve tool name"
    );
}

#[test]
fn test_extract_tool_definitions_openinference_llm_tools() {
    let tools = r#"[{"name":"search","description":"Search the web"}]"#;
    let attrs = make_attrs(&[("llm.tools", tools)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1, "Should extract llm.tools");
    assert!(tool_names.is_empty());

    let content = &tool_definitions[0].content;
    assert!(content.is_array());
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"].as_str(), Some("search"));
}

#[test]
fn test_extract_tool_definitions_gen_ai_tool_definitions() {
    let tools = r#"[{"type":"function","function":{"name":"calculator"}}]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", tools)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(
        tool_definitions.len(),
        1,
        "Should extract gen_ai.tool.definitions"
    );
    assert!(tool_names.is_empty());
}

#[test]
fn test_extract_tool_definitions_multiple_sources() {
    // When multiple sources provide tool definitions, all should be extracted
    let vercel_tools = r#"[{"type":"function","function":{"name":"weather"}}]"#;
    let otel_tools = r#"[{"type":"function","function":{"name":"calculator"}}]"#;
    let attrs = make_attrs(&[
        ("ai.prompt.tools", vercel_tools),
        ("gen_ai.tool.definitions", otel_tools),
    ]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(
        tool_definitions.len(),
        2,
        "Should extract from multiple sources"
    );
}

#[test]
fn test_extract_tool_definitions_vercel_stringified_array() {
    // Vercel AI sends tools as an OTLP array where each element is a JSON string.
    // After extract_attributes(), this becomes a JSON array of strings.
    // We need to parse each string to get the actual tool objects.
    let tools = r#"["{\"type\":\"function\",\"name\":\"weather\",\"description\":\"Get weather\"}","{\"type\":\"function\",\"name\":\"search\",\"description\":\"Search the web\"}"]"#;
    let attrs = make_attrs(&[("ai.prompt.tools", tools)]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1, "Should extract ai.prompt.tools");

    let content = &tool_definitions[0].content;
    assert!(content.is_array(), "Content should be an array");

    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 2, "Should have 2 tools");

    // After normalization, each element should be an object (not a string)
    assert!(arr[0].is_object(), "First tool should be parsed as object");
    assert!(arr[1].is_object(), "Second tool should be parsed as object");

    assert_eq!(arr[0]["name"].as_str(), Some("weather"));
    assert_eq!(arr[1]["name"].as_str(), Some("search"));
}

#[test]
fn test_extract_tool_definitions_from_request_data() {
    // Logfire (older versions) embeds tools inside request_data, not as a separate attribute.
    let request_data = r#"{"messages":[{"role":"user","content":"Weather?"}],"model":"gpt-4o","tools":[{"type":"function","function":{"name":"get_weather","description":"Get weather","parameters":{"type":"object","properties":{"location":{"type":"string"}}}}}]}"#;
    let attrs = make_attrs(&[("request_data", request_data)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(
        tool_definitions.len(),
        1,
        "Should extract tools from request_data"
    );
    assert!(tool_names.is_empty());

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_extract_tool_definitions_gen_ai_takes_precedence_over_request_data() {
    // When gen_ai.tool.definitions is present (newer logfire), don't duplicate from request_data.
    let gen_ai_tools = r#"[{"type":"function","function":{"name":"get_weather"}}]"#;
    let request_data = r#"{"messages":[],"model":"gpt-4o","tools":[{"type":"function","function":{"name":"get_weather"}}]}"#;
    let attrs = make_attrs(&[
        ("gen_ai.tool.definitions", gen_ai_tools),
        ("request_data", request_data),
    ]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(
        tool_definitions.len(),
        1,
        "Should only extract from gen_ai.tool.definitions, not duplicate from request_data"
    );
}

#[test]
fn test_extract_tool_definitions_request_data_no_tools() {
    // request_data without tools field should not produce definitions.
    let request_data = r#"{"messages":[{"role":"user","content":"Hi"}],"model":"gpt-4o"}"#;
    let attrs = make_attrs(&[("request_data", request_data)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(tool_definitions.is_empty());
    assert!(tool_names.is_empty());
}

// ============================================================================
// INTEGRATION TESTS - extract_messages_for_span
// ============================================================================

/// Test that extract_messages_for_span correctly handles VercelAISDK generation spans.
/// This test simulates a real VercelAISDK span with ai.prompt.messages attribute.
#[test]
fn test_extract_messages_for_span_vercel_ai_sdk() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    // Create a mock OTLP span with VercelAISDK attributes
    let otlp_span = Span {
        name: "ai.generateText.doGenerate".to_string(),
        attributes: vec![
            make_kv(
                "ai.prompt.messages",
                r#"[{"role":"system","content":"You are a helpful assistant"},{"role":"user","content":"What's the weather?"}]"#,
            ),
            make_kv("ai.response.text", "Let me check the weather for you."),
            make_kv("operation.name", "ai.generateText.doGenerate"),
        ],
        events: vec![], // No events - VercelAISDK uses attributes
        ..Default::default()
    };

    // Extract attributes like the pipeline does
    let span_attrs = crate::otlp::extract_attributes(&otlp_span.attributes);

    // Verify span_attrs contains the ai.prompt.messages attribute
    assert!(
        span_attrs.contains_key("ai.prompt.messages"),
        "span_attrs should contain ai.prompt.messages"
    );

    // Call extract_messages_for_span
    let (messages, _tool_defs, _tool_names) = extract_messages_for_span(
        &otlp_span,
        &span_attrs,
        Utc::now(),
        ExtractionMode::FirstMatch,
    );

    // Verify messages were extracted
    assert!(
        !messages.is_empty(),
        "Should extract messages from VercelAISDK generation span"
    );

    // Should have at least system, user, and assistant response
    assert!(
        messages.len() >= 3,
        "Should have system, user, and assistant messages. Got: {:?}",
        messages
            .iter()
            .map(|m| m.content.get("role").and_then(|r| r.as_str()))
            .collect::<Vec<_>>()
    );

    // Verify system message is extracted
    let system_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"));
    assert!(
        system_msg.is_some(),
        "Should extract system message. Got roles: {:?}",
        messages
            .iter()
            .map(|m| m.content.get("role").and_then(|r| r.as_str()))
            .collect::<Vec<_>>()
    );

    // Verify user message is extracted
    let user_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("user"));
    assert!(
        user_msg.is_some(),
        "Should extract user message. Got roles: {:?}",
        messages
            .iter()
            .map(|m| m.content.get("role").and_then(|r| r.as_str()))
            .collect::<Vec<_>>()
    );

    // Verify messages can be serialized (this is what happens in persist)
    let messages_json = serde_json::to_value(&messages).expect("Should serialize to JSON");
    assert!(
        messages_json.is_array(),
        "Serialized messages should be an array"
    );
    assert_eq!(
        messages_json.as_array().unwrap().len(),
        messages.len(),
        "Serialized array should have same length"
    );

    // Verify messages can be deserialized back (this is what happens in query)
    let json_str = serde_json::to_string(&messages).expect("Should serialize to string");
    let deserialized: Vec<RawMessage> =
        serde_json::from_str(&json_str).expect("Should deserialize from string");
    assert_eq!(
        deserialized.len(),
        messages.len(),
        "Deserialized messages should have same length"
    );
}

/// Test that extract_messages_for_span correctly handles CrewAI spans with embedded messages array.
/// This test simulates a real CrewAI span with output.value containing a messages array.
#[test]
fn test_extract_messages_for_span_crewai() {
    use opentelemetry_proto::tonic::trace::v1::Span;

    // Create a mock OTLP span with CrewAI attributes - simulating actual CrewAI output
    let output_value = r#"{"description": "Provide weather forecast", "raw": "Final answer", "messages": [{"role": "system", "content": "You are Weather Forecaster."}, {"role": "user", "content": "Get forecast for London."}, {"role": "assistant", "content": "I'll get the temperature forecast."}, {"role": "assistant", "content": "Here's the 7-day forecast for London."}]}"#;

    let otlp_span = Span {
        name: "Weather Forecaster._execute_core".to_string(),
        attributes: vec![
            make_kv("crew_key", "test-crew-key"),
            make_kv("crew_id", "test-crew-id"),
            make_kv("task_key", "test-task-key"),
            make_kv("output.value", output_value),
            make_kv("openinference.span.kind", "AGENT"),
        ],
        events: vec![], // CrewAI uses attributes, not events
        ..Default::default()
    };

    // Extract attributes like the pipeline does
    let span_attrs = crate::otlp::extract_attributes(&otlp_span.attributes);

    // Verify CrewAI detection attributes are present
    assert!(span_attrs.contains_key("crew_key"), "Should have crew_key");
    assert!(
        span_attrs.contains_key("output.value"),
        "Should have output.value"
    );

    // Call extract_messages_for_span
    let (messages, _tool_defs, _tool_names) = extract_messages_for_span(
        &otlp_span,
        &span_attrs,
        Utc::now(),
        ExtractionMode::FirstMatch,
    );

    // Four history messages from the messages array, plus the answer from `raw`.
    assert_eq!(
        messages.len(),
        5,
        "Should extract 4 history messages plus the answer in `raw`. Got: {}",
        messages.len()
    );
    assert_eq!(
        messages[4].content["role"].as_str(),
        Some("assistant"),
        "the last message should be the answer CrewAI puts in `raw`"
    );

    // Verify first message is system
    assert_eq!(
        messages[0].content.get("role").and_then(|r| r.as_str()),
        Some("system"),
        "First message should be system"
    );

    // Verify second message is user
    assert_eq!(
        messages[1].content.get("role").and_then(|r| r.as_str()),
        Some("user"),
        "Second message should be user"
    );

    // Verify third and fourth messages are assistant
    assert_eq!(
        messages[2].content.get("role").and_then(|r| r.as_str()),
        Some("assistant"),
        "Third message should be assistant"
    );
    assert_eq!(
        messages[3].content.get("role").and_then(|r| r.as_str()),
        Some("assistant"),
        "Fourth message should be assistant"
    );

    // Verify messages can be serialized (this is what happens in persist)
    let messages_json = serde_json::to_value(&messages).expect("Should serialize to JSON");
    assert!(
        messages_json.is_array(),
        "Serialized messages should be an array"
    );
    assert_eq!(
        messages_json.as_array().unwrap().len(),
        5,
        "Serialized array should have the four history messages plus the answer"
    );
}

// ============================================================================
// REGRESSION TESTS
// ============================================================================

#[test]
fn regression_langgraph_indexed_tool_definitions() {
    // Regression test: LangGraph stores tool definitions as indexed attributes
    // Pattern: llm.tools.N.tool.json_schema
    //
    let mut attrs = HashMap::new();
    attrs.insert(
        "llm.tools.0.tool.json_schema".to_string(),
        r#"{"type": "function", "function": {"name": "temperature_forecast", "description": "Get temperature", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}"#.to_string(),
    );
    attrs.insert(
        "llm.tools.1.tool.json_schema".to_string(),
        r#"{"type": "function", "function": {"name": "precipitation_forecast", "description": "Get precipitation", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}"#.to_string(),
    );

    let timestamp = Utc::now();
    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, timestamp);

    // Should extract tool definitions
    assert_eq!(
        tool_definitions.len(),
        1,
        "Should have one tool definition entry (array of tools)"
    );

    let tools_array = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools_array.len(), 2, "Should have 2 tools");

    // Verify first tool
    let first_tool = &tools_array[0];
    assert_eq!(
        first_tool["function"]["name"], "temperature_forecast",
        "First tool should be temperature_forecast"
    );

    // Verify second tool
    let second_tool = &tools_array[1];
    assert_eq!(
        second_tool["function"]["name"], "precipitation_forecast",
        "Second tool should be precipitation_forecast"
    );

    // Should also extract tool names
    assert_eq!(tool_names.len(), 1, "Should have tool names entry");
    let names_array = tool_names[0].content.as_array().unwrap();
    assert_eq!(names_array.len(), 2, "Should have 2 tool names");
    assert!(
        names_array
            .iter()
            .any(|n| n.as_str() == Some("temperature_forecast")),
        "Should include temperature_forecast"
    );
    assert!(
        names_array
            .iter()
            .any(|n| n.as_str() == Some("precipitation_forecast")),
        "Should include precipitation_forecast"
    );
}
