#[test]
fn regression_langgraph_indexed_tool_definitions_sparse() {
    // Regression test: LangGraph with sparse tool indices (not starting at 0)
    // This can happen if some tools are filtered or conditionally added
    let mut attrs = HashMap::new();
    // Note: Starting at index 1, not 0
    attrs.insert(
        "llm.tools.1.tool.json_schema".to_string(),
        r#"{"type": "function", "function": {"name": "search", "description": "Search the web"}}"#
            .to_string(),
    );
    attrs.insert(
        "llm.tools.3.tool.json_schema".to_string(),
        r#"{"type": "function", "function": {"name": "calculate", "description": "Do math"}}"#
            .to_string(),
    );

    let timestamp = Utc::now();
    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, timestamp);

    // Should extract both tools despite sparse indices
    assert!(!tool_definitions.is_empty(), "Should have tool definitions");
    let tools_array = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(
        tools_array.len(),
        2,
        "Should have 2 tools even with sparse indices"
    );

    // Verify both tools extracted
    let tool_names_extracted: Vec<&str> = tools_array
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(tool_names_extracted.contains(&"search"));
    assert!(tool_names_extracted.contains(&"calculate"));

    // Tool names should also be extracted
    assert!(!tool_names.is_empty());
}

#[test]
fn regression_openinference_messages_with_tool_calls() {
    // Regression test: OpenInference format with tool calls in output messages
    // Pattern: llm.output_messages.N.message.*
    let mut attrs = HashMap::new();
    attrs.insert(
        "llm.output_messages.0.message.role".to_string(),
        "assistant".to_string(),
    );
    attrs.insert(
        "llm.output_messages.0.message.tool_calls.0.tool_call.id".to_string(),
        "call_abc123".to_string(),
    );
    attrs.insert(
        "llm.output_messages.0.message.tool_calls.0.tool_call.function.name".to_string(),
        "get_weather".to_string(),
    );
    attrs.insert(
        "llm.output_messages.0.message.tool_calls.0.tool_call.function.arguments".to_string(),
        r#"{"city": "NYC"}"#.to_string(),
    );

    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let timestamp = Utc::now();

    let found = try_openinference(&mut messages, &mut tool_definitions, &attrs, "", timestamp);

    assert!(found, "Should find OpenInference messages");
    assert_eq!(messages.len(), 1, "Should extract one message");

    // Verify the message has tool call attributes
    let msg_content = &messages[0].content;
    assert!(
        msg_content.get("tool_calls").is_some()
            || msg_content
                .as_object()
                .map(|o| o.keys().any(|k| k.starts_with("tool_calls")))
                .unwrap_or(false),
        "Message should have tool_calls: {:?}",
        msg_content
    );
}

// ============================================================================
// Regression tests for tool definition & tool result fixes
// ============================================================================

#[test]
fn test_google_adk_function_declarations_unwrapped_snake_case() {
    // ADK sends tools as [{"function_declarations": [{"name": ...}, ...]}]
    // Must be flattened to individual tool objects
    let request = r#"{"model":"gemini-pro","config":{"tools":[{"function_declarations":[{"name":"search","description":"Search the web","parameters":{"type":"object","properties":{"q":{"type":"string"}}}},{"name":"calculator","description":"Do math"}]}]},"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    try_google_adk(&mut messages, &mut tool_defs, &attrs, "", Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(
        tools.len(),
        2,
        "Both tools must be flattened from the group"
    );
    assert_eq!(tools[0]["name"].as_str(), Some("search"));
    assert_eq!(tools[1]["name"].as_str(), Some("calculator"));
    // Verify wrapper is removed
    for tool in tools {
        assert!(tool.get("function_declarations").is_none());
    }
}

#[test]
fn test_google_adk_function_declarations_unwrapped_camel_case() {
    // Vertex AI native uses camelCase: functionDeclarations
    let request = r#"{"model":"gemini-pro","tools":[{"functionDeclarations":[{"name":"get_weather","description":"Weather lookup"}]}],"contents":[{"role":"user","parts":[{"text":"weather?"}]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    try_google_adk(&mut messages, &mut tool_defs, &attrs, "", Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"].as_str(), Some("get_weather"));
    assert!(tools[0].get("functionDeclarations").is_none());
}

#[test]
fn test_google_adk_tools_without_declarations_wrapper() {
    // Tools without function_declarations wrapper (direct tool objects) pass through
    let request = r#"{"model":"gemini-pro","config":{"tools":[{"name":"direct_tool","description":"Direct"}]},"contents":[]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    try_google_adk(&mut messages, &mut tool_defs, &attrs, "", Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"].as_str(), Some("direct_tool"));
}

#[test]
fn test_autogen_tool_call_execution_event_unpacked() {
    // ToolCallExecutionEvent has content array: [{content, name, call_id, is_error}]
    let message_json = r#"{"message":{"id":"evt1","source":"tool_agent","content":[{"content":"72F and sunny","name":"get_weather","call_id":"call_abc123","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg["role"].as_str(), Some("tool"));
    assert_eq!(msg["content"].as_str(), Some("72F and sunny"));
    assert_eq!(msg["name"].as_str(), Some("get_weather"));
    assert_eq!(msg["tool_call_id"].as_str(), Some("call_abc123"));
}

#[test]
fn test_autogen_tool_call_execution_event_structured_content() {
    // ToolCallExecutionEvent with structured (non-string) inner content
    let message_json = r#"{"message":{"id":"evt2","source":"tool_agent","content":[{"content":{"temperature":72,"unit":"F"},"name":"get_temp","call_id":"call_xyz","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg["role"].as_str(), Some("tool"));
    assert_eq!(msg["content"]["temperature"].as_i64(), Some(72));
    assert_eq!(msg["name"].as_str(), Some("get_temp"));
}

#[test]
fn test_autogen_tool_call_execution_event_empty_content_array() {
    // Empty content array → normalize_autogen_message returns vec![] →
    // fallback preserves raw nested message (has "content" key)
    let message_json = r#"{"message":{"id":"evt3","source":"tool_agent","content":[],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Raw message preserved as fallback (content key exists even though empty)
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].content["type"].as_str(),
        Some("ToolCallExecutionEvent")
    );
}

#[test]
fn test_autogen_tool_call_execution_event_fallback_call_id() {
    // call_id at top level when not in content item
    let message_json = r#"{"message":{"id":"evt4","source":"tool_agent","call_id":"call_top","content":[{"content":"result","name":"func1"}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].content["tool_call_id"].as_str(),
        Some("call_top"),
        "Should fall back to top-level call_id"
    );
}

#[test]
fn test_autogen_infer_tool_call_request_no_type() {
    // Message without type field, content is array of [{id, name, arguments}]
    let message_json = r#"{"source":"assistant","content":[{"id":"call_1","name":"get_weather","arguments":"{\"city\":\"NYC\"}"}]}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg["role"].as_str(), Some("assistant"));
    let tool_calls = msg["tool_calls"]
        .as_array()
        .expect("tool_calls should be array");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["id"].as_str(), Some("call_1"));
    assert_eq!(
        tool_calls[0]["function"]["name"].as_str(),
        Some("get_weather")
    );
    assert_eq!(
        tool_calls[0]["function"]["arguments"]["city"].as_str(),
        Some("NYC")
    );
}

#[test]
fn test_autogen_infer_tool_call_execution_no_type() {
    // Message without type field, content is array of [{content, name, call_id}]
    let message_json =
        r#"{"content":[{"content":"72 degrees","name":"get_weather","call_id":"call_1"}]}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(messages[0].content["name"].as_str(), Some("get_weather"));
    assert_eq!(messages[0].content["content"].as_str(), Some("72 degrees"));
    assert_eq!(messages[0].content["tool_call_id"].as_str(), Some("call_1"));
}

#[test]
fn test_autogen_infer_text_no_type() {
    // Message without type field, string content + source
    let message_json = r#"{"source":"weather_agent","content":"The weather is sunny."}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("The weather is sunny.")
    );
    assert_eq!(messages[0].content["name"].as_str(), Some("weather_agent"));
}

#[test]
fn test_autogen_infer_empty_skipped() {
    // Message without type field, empty string content → skipped
    let message_json = r#"{"source":"agent","content":""}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    // The message is empty so normalize returns vec![], but the raw message
    // has "content" key so it falls through to the raw preservation path
    assert!(found);
}

#[test]
fn test_autogen_thought_event() {
    let message_json = r#"{"content":"Let me reason about this...","source":"reasoning_agent","type":"ThoughtEvent"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    let content = messages[0].content["content"]
        .as_array()
        .expect("content should be array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"].as_str(), Some("thinking"));
    assert_eq!(
        content[0]["text"].as_str(),
        Some("Let me reason about this...")
    );
}

#[test]
fn test_autogen_handoff_message() {
    let message_json = r#"{"content":"Handing off to travel agent","source":"triage_agent","target":"travel_agent","type":"HandoffMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Handing off to travel agent")
    );
    assert_eq!(messages[0].content["name"].as_str(), Some("triage_agent"));
}

#[test]
fn test_autogen_stop_message() {
    let message_json = r#"{"content":"TERMINATE","source":"user","type":"StopMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(messages[0].content["content"].as_str(), Some("TERMINATE"));
}

#[test]
fn test_autogen_multimodal_message() {
    let message_json = r#"{"content":["Here is the image:",{"type":"image","url":"https://example.com/img.png"}],"source":"user","type":"MultiModalMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    let content = messages[0].content["content"]
        .as_array()
        .expect("content should be array");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0].as_str(), Some("Here is the image:"));
}

#[test]
fn test_autogen_oi_span_claimed() {
    // OpenInference AutoGen span with cancellation_token → claimed but no messages extracted
    let input_val = r#"{"cancellation_token":"<autogen_core._cancellation_token.CancellationToken object at 0x1234>","output_task_messages":true}"#;
    let attrs = make_attrs(&[("input.value", input_val)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "AutoGen OI span should be claimed");
    assert!(
        messages.is_empty(),
        "No messages should be extracted from OI aggregation span"
    );
}

#[test]
fn test_autogen_response_wrapper() {
    // Response wrapper format: {"response": {"chat_message": {...}, "inner_messages": [...]}}
    let message_json = r#"{"response":{"chat_message":{"content":"Here is the result","source":"agent","type":"TextMessage"},"inner_messages":[{"content":[{"content":"72F","name":"get_weather","call_id":"call_1"}],"type":"ToolCallExecutionEvent"}]}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2);
    // chat_message normalized
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Here is the result")
    );
    // inner_messages tool result
    assert_eq!(messages[1].content["role"].as_str(), Some("tool"));
    assert_eq!(messages[1].content["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_autogen_empty_json_skipped() {
    let attrs = make_attrs(&[("message", "{}")]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found);
    assert!(messages.is_empty());
}

#[test]
fn test_autogen_tool_execution_multiple_results() {
    // ToolCallExecutionEvent with 2 items → should produce 2 messages
    let message_json = r#"{"content":[{"content":"72 degrees","name":"get_temperature","call_id":"call_1"},{"content":"30% chance of rain","name":"get_precipitation","call_id":"call_2"}],"type":"ToolCallExecutionEvent"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2, "Both tool results should be extracted");
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[0].content["name"].as_str(),
        Some("get_temperature")
    );
    assert_eq!(messages[0].content["content"].as_str(), Some("72 degrees"));
    assert_eq!(messages[0].content["tool_call_id"].as_str(), Some("call_1"));
    assert_eq!(messages[1].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[1].content["name"].as_str(),
        Some("get_precipitation")
    );
    assert_eq!(
        messages[1].content["content"].as_str(),
        Some("30% chance of rain")
    );
    assert_eq!(messages[1].content["tool_call_id"].as_str(), Some("call_2"));
}

#[test]
fn test_autogen_function_execution_multiple_results() {
    // FunctionExecutionResultMessage with 2 items → should produce 2 messages
    let message_json = r#"{"content":[{"content":"72 degrees","name":"get_temperature","call_id":"call_a"},{"content":"Light rain expected","name":"get_precipitation","call_id":"call_b"}],"type":"FunctionExecutionResultMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(
        messages.len(),
        2,
        "Both function results should be extracted"
    );
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[0].content["name"].as_str(),
        Some("get_temperature")
    );
    assert_eq!(messages[0].content["tool_call_id"].as_str(), Some("call_a"));
    assert_eq!(messages[1].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[1].content["name"].as_str(),
        Some("get_precipitation")
    );
    assert_eq!(messages[1].content["tool_call_id"].as_str(), Some("call_b"));
}

#[test]
fn test_crewai_tool_names_from_crew_agents() {
    // Tool definitions are now extracted by extract_tool_definitions(), not try_crewai().
    let agents_json = r#"[{"role":"Weather Expert","tools_names":["get_weather","get_forecast"]},{"role":"Data Analyst","tools_names":["analyze_data"]}]"#;
    let attrs = make_attrs(&[("crew_agents", agents_json), ("crew_key", "test-key")]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["function"]["name"].as_str(), Some("get_weather"));
    assert_eq!(tools[1]["function"]["name"].as_str(), Some("get_forecast"));
    assert_eq!(tools[2]["function"]["name"].as_str(), Some("analyze_data"));
}

#[test]
fn test_crewai_tool_names_deduplicated() {
    // Two agents share the same tool → should appear once.
    // Tool definitions are now extracted by extract_tool_definitions(), not try_crewai().
    let agents_json = r#"[{"role":"Agent A","tools_names":["shared_tool","unique_a"]},{"role":"Agent B","tools_names":["shared_tool","unique_b"]}]"#;
    let attrs = make_attrs(&[("crew_agents", agents_json), ("crew_id", "test")]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 3, "shared_tool must appear only once");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert_eq!(names, vec!["shared_tool", "unique_a", "unique_b"]);
}

#[test]
fn test_openai_agents_tool_definitions_from_response() {
    // OpenAI Agents SDK stores tool schemas in the response attribute
    let response_json = r#"{"id":"resp_1","output":[],"tools":[{"type":"function","name":"get_weather","description":"Get weather","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]},"strict":true}]}"#;
    let attrs = make_attrs(&[("response", response_json)]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"].as_str(), Some("get_weather"));
    assert!(tools[0].get("parameters").is_some());
}

#[test]
fn test_openai_agents_response_empty_tools_skipped() {
    // Empty tools array in response → no tool_definitions produced
    let response_json = r#"{"id":"resp_2","output":[],"tools":[]}"#;
    let attrs = make_attrs(&[("response", response_json)]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(tool_defs.is_empty());
}

#[test]
fn test_openai_agents_response_no_tools_field() {
    // Response without tools field (e.g., non-agent response) → no tool_definitions
    let response_json = r#"{"id":"resp_3","output":[{"type":"message","content":[{"type":"text","text":"hello"}]}]}"#;
    let attrs = make_attrs(&[("response", response_json)]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(tool_defs.is_empty());
}

#[test]
fn test_openinference_tool_attributes_extraction() {
    // OpenInference tool.* attributes (single tool per span)
    let params = r#"{"type":"object","properties":{"query":{"type":"string"}}}"#;
    let attrs = make_attrs(&[
        ("tool.name", "search_web"),
        ("tool.description", "Search the web for information"),
        ("tool.parameters", params),
    ]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let tools = tool_defs[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"].as_str(), Some("function"));
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("search_web"));
    assert_eq!(
        func["description"].as_str(),
        Some("Search the web for information")
    );
    assert!(func.get("parameters").is_some());
    assert_eq!(func["parameters"]["type"].as_str(), Some("object"));
}

#[test]
fn test_openinference_tool_name_only() {
    // tool.name without description or parameters
    let attrs = make_attrs(&[("tool.name", "simple_tool")]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_defs.len(), 1);
    let func = &tool_defs[0].content[0]["function"];
    assert_eq!(func["name"].as_str(), Some("simple_tool"));
    assert!(func.get("description").is_none());
    assert!(func.get("parameters").is_none());
}

#[test]
fn test_openinference_tool_skipped_when_genai_tool_exists() {
    // When gen_ai.tool.name exists, tool.name should not also produce a definition
    let attrs = make_attrs(&[
        ("gen_ai.tool.name", "primary_tool"),
        ("tool.name", "secondary_tool"),
    ]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());

    // gen_ai.tool.name takes priority; tool.name is skipped
    let all_names: Vec<&str> = tool_defs
        .iter()
        .flat_map(|td| {
            td.content
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
        })
        .collect();
    assert!(all_names.contains(&"primary_tool"));
    assert!(
        !all_names.contains(&"secondary_tool"),
        "OI tool.name should be skipped when gen_ai.tool.name exists"
    );
}

#[test]
fn test_tool_definition_skips_synthetic_names() {
    // Framework-internal names like "(merged tools)" from Google ADK should be filtered.
    // Valid tool names start with an alphanumeric character or underscore.
    let cases = vec![
        ("(merged tools)", true),       // ADK synthetic span
        ("<internal>", true),           // Hypothetical framework marker
        ("[placeholder]", true),        // Hypothetical framework marker
        ("get_weather", false),         // Valid ASCII tool name
        ("_private", false),            // Valid underscore-prefixed name
        ("获取天气", false),            // Valid Chinese tool name
        ("получить_погоду", false),     // Valid Russian tool name
        ("wetterbericht_holen", false), // Valid German tool name
    ];

    for (name, should_be_empty) in cases {
        let attrs = make_attrs(&[("gen_ai.tool.name", name)]);
        let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

        if should_be_empty {
            assert!(
                tool_definitions.is_empty(),
                "Synthetic name '{}' should produce no tool definition",
                name
            );
        } else {
            assert_eq!(
                tool_definitions.len(),
                1,
                "Valid name '{}' should produce a tool definition",
                name
            );
            let content = tool_definitions[0].content.as_array().unwrap();
            assert_eq!(
                content[0]["function"]["name"].as_str(),
                Some(name),
                "Tool name should be preserved"
            );
        }
    }
}

// === Plain Data Wrapping Tests ===

#[test]
fn test_try_raw_io_wraps_plain_data_output() {
    let attrs = make_attrs(&[("output.value", r#"{"name":"Jane Doe","age":28}"#)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    let ts = Utc::now();

    try_raw_io(&mut messages, &mut tool_defs, &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    let raw = &messages[0].content;
    assert_eq!(raw["role"], "assistant");
    assert_eq!(raw["content"]["name"], "Jane Doe");
    assert_eq!(raw["content"]["age"], 28);
}

#[test]
fn test_try_raw_io_wraps_plain_data_input() {
    let attrs = make_attrs(&[("input.value", r#"{"query":"test","limit":10}"#)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    let ts = Utc::now();

    try_raw_io(&mut messages, &mut tool_defs, &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    let raw = &messages[0].content;
    assert_eq!(raw["role"], "user");
    assert_eq!(raw["content"]["query"], "test");
}

#[test]
fn test_try_raw_io_doesnt_wrap_message_shaped_output() {
    let attrs = make_attrs(&[("output.value", r#"{"role":"assistant","content":"Hello!"}"#)]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    let ts = Utc::now();

    try_raw_io(&mut messages, &mut tool_defs, &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    let raw = &messages[0].content;
    assert_eq!(raw["role"], "assistant");
    assert_eq!(raw["content"], "Hello!");
}

// ========== OI multimodal enrichment tests ==========

#[test]
fn test_oi_multimodal_enrichment_from_input_value() {
    // OI dotted attrs: 2 content blocks (text + image with __REDACTED__ URL)
    // input.value: 3 blocks (text + image with real URL + file/PDF)
    let mut attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.contents.0.message_content.type",
            "text",
        ),
        (
            "llm.input_messages.0.message.contents.0.message_content.text",
            "Analyze this",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.type",
            "image",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.image.image.url",
            "__REDACTED__",
        ),
    ]);
    // input.value with LangChain-serialized messages
    let input_value = json!([{
        "id": ["langchain", "schema", "messages", "HumanMessage"],
        "lc": 1,
        "type": "constructor",
        "kwargs": {
            "content": [
                {"type": "text", "text": "Analyze this"},
                {"type": "image_url", "image_url": {"url": "#!B64!#::f78eehash"}},
                {"type": "file", "mime_type": "application/pdf", "data": "#!B64!#::f65fhash", "name": "task-document"}
            ]
        }
    }]);
    attrs.insert(
        "input.value".to_string(),
        serde_json::to_string(&input_value).unwrap(),
    );

    let mut messages = Vec::new();
    let ts = Utc::now();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", ts);

    assert!(found);
    assert_eq!(messages.len(), 1);

    // Content should be replaced with the richer input.value content
    let content = &messages[0].content;
    let arr = content["content"]
        .as_array()
        .expect("content should be array");
    assert_eq!(arr.len(), 3, "Should have all 3 blocks from input.value");

    // Verify blocks have real data, not __REDACTED__
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[1]["type"], "image_url");
    assert!(
        arr[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .contains("#!B64!#::")
    );
    assert_eq!(arr[2]["type"], "file");
    assert_eq!(arr[2]["mime_type"], "application/pdf");
}

#[test]
fn test_oi_enrichment_skips_text_only() {
    // Text-only OI message: no contents.* keys, should not be enriched
    let mut attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        ("llm.input_messages.0.message.content", "Hello world"),
    ]);
    let input_value = json!([{
        "id": ["langchain", "schema", "messages", "HumanMessage"],
        "lc": 1,
        "kwargs": {"content": "Hello world"}
    }]);
    attrs.insert(
        "input.value".to_string(),
        serde_json::to_string(&input_value).unwrap(),
    );

    let mut messages = Vec::new();
    let ts = Utc::now();
    try_openinference(&mut messages, &mut Vec::new(), &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    // Content should remain as-is (string, not array)
    assert_eq!(messages[0].content["content"], "Hello world");
}

#[test]
fn test_oi_enrichment_same_block_count_replaces() {
    // Same block count (2 vs 2) but OI has __REDACTED__ URL.
    // input.value has real URL — should still replace.
    let mut attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.contents.0.message_content.type",
            "text",
        ),
        (
            "llm.input_messages.0.message.contents.0.message_content.text",
            "Describe this",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.type",
            "image",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.image.image.url",
            "__REDACTED__",
        ),
    ]);
    let input_value = json!([{
        "id": ["langchain", "schema", "messages", "HumanMessage"],
        "lc": 1,
        "kwargs": {
            "content": [
                {"type": "text", "text": "Describe this"},
                {"type": "image_url", "image_url": {"url": "#!B64!#::realhash"}}
            ]
        }
    }]);
    attrs.insert(
        "input.value".to_string(),
        serde_json::to_string(&input_value).unwrap(),
    );

    let mut messages = Vec::new();
    let ts = Utc::now();
    try_openinference(&mut messages, &mut Vec::new(), &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    let content = &messages[0].content;
    let arr = content["content"]
        .as_array()
        .expect("content should be array");
    assert_eq!(arr.len(), 2);
    // Image should have real URL, not __REDACTED__
    assert!(
        arr[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .contains("#!B64!#::")
    );
}

#[test]
fn test_oi_enrichment_no_input_value() {
    // No input.value attribute: enrichment should be a no-op
    let attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.contents.0.message_content.type",
            "text",
        ),
        (
            "llm.input_messages.0.message.contents.0.message_content.text",
            "Hello",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.type",
            "image",
        ),
        (
            "llm.input_messages.0.message.contents.1.message_content.image.image.url",
            "__REDACTED__",
        ),
    ]);

    let mut messages = Vec::new();
    let ts = Utc::now();
    try_openinference(&mut messages, &mut Vec::new(), &attrs, "", ts);

    assert_eq!(messages.len(), 1);
    // Should still have the original dotted-key content (no enrichment)
    let content = &messages[0].content;
    assert!(
        content
            .as_object()
            .unwrap()
            .keys()
            .any(|k| k.starts_with("contents."))
    );
}

#[test]
fn test_autogen_tool_execution_event_nested_format() {
    // ToolCallExecutionEvent in nested {"message": {...}} format — must produce tool_result
    let message_json = r#"{"message":{"id":"test-id","source":"agent","models_usage":null,"metadata":{},"created_at":"2026-01-01T00:00:00Z","content":[{"content":"49\n","name":"execute_python_code","call_id":"call_123","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "try_autogen should claim the span");
    assert_eq!(messages.len(), 1, "Should produce 1 tool_result message");
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[0].content["name"].as_str(),
        Some("execute_python_code")
    );
}
