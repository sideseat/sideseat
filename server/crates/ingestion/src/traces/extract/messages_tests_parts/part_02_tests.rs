
#[test]
fn test_gen_ai_tool_names_and_definitions_together() {
    // Both agent tools and definitions present - should extract to separate vectors
    let tools_json = r#"["get_weather", "search"]"#;
    let definitions_json = r#"[{"name": "get_weather", "description": "Weather API"}]"#;
    let attrs = make_attrs(&[
        ("gen_ai.agent.tools", tools_json),
        ("gen_ai.tool.definitions", definitions_json),
    ]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    // tool_definitions should have 1 item (from gen_ai.tool.definitions)
    assert_eq!(tool_definitions.len(), 1);
    let defs_def = &tool_definitions[0];
    // Content is directly the definitions array
    assert!(defs_def.content.is_array());
    assert_eq!(defs_def.content.as_array().unwrap().len(), 1);

    // tool_names should have 1 item (from gen_ai.agent.tools)
    assert_eq!(tool_names.len(), 1);
    let tools_def = &tool_names[0];
    // Content is directly the tool names array
    assert!(tools_def.content.is_array());
    assert_eq!(tools_def.content.as_array().unwrap().len(), 2);
}

#[test]
fn test_gen_ai_tool_names_extraction() {
    // gen_ai.agent.tools - list of tools available to the agent
    let tools_json = r#"["get_weather", "search", "calculator"]"#;
    let attrs = make_attrs(&[("gen_ai.agent.tools", tools_json)]);

    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    // tool_definitions should be empty (no gen_ai.tool.definitions)
    assert!(tool_definitions.is_empty());

    // tool_names should have 1 item
    assert_eq!(tool_names.len(), 1);

    let def = &tool_names[0];
    // Content is directly the tool names array
    assert!(def.content.is_array());
    let tools = def.content.as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0].as_str(), Some("get_weather"));
    assert_eq!(tools[1].as_str(), Some("search"));
    assert_eq!(tools[2].as_str(), Some("calculator"));

    // Verify source attribution
    match &def.source {
        ToolDefinitionSource::Attribute { key, .. } => {
            assert_eq!(key, "gen_ai.agent.tools");
        }
    }
}

#[test]
fn test_gen_ai_tool_names_with_conversation_messages() {
    // Tool definitions alongside regular conversation messages
    // Tool extraction is done separately from conversation messages
    let tools_json = r#"["calculator"]"#;
    let input_json = r#"[{"role": "user", "content": "What is 2+2?"}]"#;
    let output_json = r#"[{"role": "assistant", "content": "4"}]"#;
    let attrs = make_attrs(&[
        ("gen_ai.agent.tools", tools_json),
        ("gen_ai.input.messages", input_json),
        ("gen_ai.output.messages", output_json),
    ]);

    let mut messages = Vec::new();
    // Extract conversation messages
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    // Extract tool definitions and tool names separately
    let (tool_definitions, tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(found);
    // Messages should have: input, output (tool definitions are separate now)
    assert_eq!(messages.len(), 2);
    // tool_definitions should be empty (no gen_ai.tool.definitions)
    assert!(tool_definitions.is_empty());
    // tool_names should have 1 entry (from gen_ai.agent.tools)
    assert_eq!(tool_names.len(), 1);

    // Verify we have all message types
    let has_input = messages
        .iter()
        .any(|m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "gen_ai.input.messages"));
    let has_output = messages
        .iter()
        .any(|m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "gen_ai.output.messages"));
    // tool_names content is directly the array
    let has_tools = tool_names.iter().any(|d| d.content.is_array());

    assert!(has_input, "Should have input messages");
    assert!(has_output, "Should have output messages");
    assert!(has_tools, "Should have tool names in tool_names");
}

#[test]
fn test_crewai_tool_definitions_extracted_from_agents_metadata() {
    // CrewAI tool metadata must be extracted even when span also has conversation messages.
    // This ensures tools appear in Thread > Tools without relying on fallback attr extraction.
    let agents_json = r#"[{"role":"Weather Expert","tools_names":["get_weather","get_forecast"]},{"role":"Data Analyst","tools":["analyze_data"]}]"#;
    let attrs = make_attrs(&[
        ("crew_key", "crew-test"),
        ("crew_agents", agents_json),
        (
            "gen_ai.output.messages",
            r#"[{"role":"assistant","content":"Here is the forecast"}]"#,
        ),
    ]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1);
    let tools = tool_definitions[0].content.as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert_eq!(names, vec!["get_weather", "get_forecast", "analyze_data"]);
}

#[test]
fn test_crewai_tool_definitions_extracted_from_tasks_metadata() {
    // CrewAI tasks can carry tool lists; support both tools_names and tools formats.
    let tasks_json = r#"[{"key":"t1","tools_names":["temperature_forecast","precipitation_forecast"]},{"key":"t2","tools":[{"name":"wind_forecast"},"precipitation_forecast",{"function":{"name":"humidity_forecast"}}]}]"#;
    let attrs = make_attrs(&[("crew_tasks", tasks_json), ("crew_id", "crew-123")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1);
    let tools = tool_definitions[0].content.as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "temperature_forecast",
            "precipitation_forecast",
            "wind_forecast",
            "humidity_forecast"
        ]
    );
}

#[test]
fn test_gen_ai_indexed_nested_content() {
    // Some SDKs use nested content arrays like gen_ai.prompt.0.content.0.text
    let attrs = make_attrs(&[
        ("gen_ai.prompt.0.role", "user"),
        ("gen_ai.prompt.0.content.0.type", "text"),
        ("gen_ai.prompt.0.content.0.text", "Hello!"),
        ("gen_ai.prompt.0.content.1.type", "image_url"),
        (
            "gen_ai.prompt.0.content.1.image_url.url",
            "https://example.com/image.png",
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_gen_ai_indexed(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    // Should find message even with nested content structure
    assert!(
        found,
        "Should extract message with nested content (gen_ai.prompt.0.content.0.text)"
    );
    assert!(
        !messages.is_empty(),
        "Should have extracted at least one message"
    );
}

#[test]
fn test_gen_ai_tool_definitions_extraction() {
    // gen_ai.tool.definitions - full tool schemas
    let definitions_json = r#"[
        {
            "name": "get_weather",
            "description": "Get weather for a location",
            "parameters": {
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City name"}
                },
                "required": ["location"]
            }
        },
        {
            "name": "search",
            "description": "Search the web",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            }
        }
    ]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", definitions_json)]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(!tool_definitions.is_empty());
    assert_eq!(tool_definitions.len(), 1);

    let def = &tool_definitions[0];
    // Content is directly the definitions array
    assert!(def.content.is_array());
    let definitions = def.content.as_array().unwrap();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0]["name"].as_str(), Some("get_weather"));
    assert_eq!(
        definitions[0]["description"].as_str(),
        Some("Get weather for a location")
    );
    assert_eq!(definitions[1]["name"].as_str(), Some("search"));

    // Verify source attribution
    match &def.source {
        ToolDefinitionSource::Attribute { key, .. } => {
            assert_eq!(key, "gen_ai.tool.definitions");
        }
    }
}

#[test]
fn test_gen_ai_tool_definitions_invalid_json() {
    // Invalid JSON is skipped (can't extract tool name, causes issues downstream)
    let invalid_json = "not valid json";
    let attrs = make_attrs(&[("gen_ai.tool.definitions", invalid_json)]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    // Invalid JSON should be skipped
    assert!(tool_definitions.is_empty());
}

#[test]
fn test_gen_ai_tool_individual_attributes() {
    // Tool definition from individual gen_ai.tool.* attributes
    let json_schema = r#"{
        "properties": {
            "city": {"description": "The name of the city", "type": "string"},
            "days": {"default": 3, "description": "Number of days", "type": "integer"}
        },
        "required": ["city"],
        "type": "object"
    }"#;
    let attrs = make_attrs(&[
        ("gen_ai.tool.name", "weather_forecast"),
        (
            "gen_ai.tool.description",
            "Get weather forecast for a city.",
        ),
        ("gen_ai.tool.json_schema", json_schema),
    ]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(!tool_definitions.is_empty());
    assert_eq!(tool_definitions.len(), 1);

    let def = &tool_definitions[0];
    // Content is directly an array with one tool definition
    assert!(def.content.is_array());
    let content = def.content.as_array().unwrap();
    assert_eq!(content.len(), 1);

    let tool_def = &content[0];
    assert_eq!(tool_def["type"].as_str(), Some("function"));

    let func = &tool_def["function"];
    assert_eq!(func["name"].as_str(), Some("weather_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get weather forecast for a city.")
    );

    // Verify parameters were parsed
    let params = &func["parameters"];
    assert!(params.is_object());
    assert_eq!(params["type"].as_str(), Some("object"));
    assert!(params["properties"]["city"].is_object());
}

#[test]
fn test_google_adk_data_attribute() {
    // gcp.vertex.agent.data contains conversation history sent to agent
    let data_json = r#"[
        {"role": "user", "parts": [{"text": "What's the weather?"}]},
        {"role": "model", "parts": [{"text": "The weather is sunny."}]}
    ]"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.data", data_json)]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    let msg = &messages[0];
    assert_eq!(
        msg.content.get("role").and_then(|r| r.as_str()),
        Some("data")
    );
    assert_eq!(
        msg.content.get("type").and_then(|t| t.as_str()),
        Some("conversation_history")
    );

    let content = msg.content.get("content").unwrap();
    assert!(content.is_array());
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[test]
fn test_google_adk_data_empty_ignored() {
    // Empty data should not create a message
    let attrs = make_attrs(&[("gcp.vertex.agent.data", "[]")]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found);
    assert!(messages.is_empty());
}

#[test]
fn test_google_adk_empty_llm_request_fallback_to_tool_args() {
    // When llm_request is "{}", should fall back to tool_call_args
    let attrs = make_attrs(&[
        ("gcp.vertex.agent.llm_request", "{}"),
        (
            "gcp.vertex.agent.tool_call_args",
            r#"{"city":"New York","days":3}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract from tool_call_args when llm_request is empty object"
    );

    // Should have the tool call args as input
    let has_tool_input = messages.iter().any(|m| {
        m.content.get("city").is_some()
            || m.content
                .get("content")
                .and_then(|c| c.get("city"))
                .is_some()
    });
    assert!(has_tool_input, "Should extract tool_call_args content");
}

#[test]
fn test_google_adk_empty_llm_response_fallback_to_tool_response() {
    // When llm_response is "{}", should fall back to tool_response
    let attrs = make_attrs(&[
        ("gcp.vertex.agent.llm_response", "{}"),
        (
            "gcp.vertex.agent.tool_response",
            r#"{"temperature":"72F","condition":"sunny"}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        found,
        "Should extract from tool_response when llm_response is empty object"
    );
}

#[test]
fn test_google_adk_extract_tools_from_config() {
    // ADK stores tool definitions in config.tools
    let request_json = r#"{
        "model": "gemini-pro",
        "config": {
            "system_instruction": "You are a helpful assistant",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather for a location",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"}
                        }
                    }
                }
            ]
        },
        "contents": [{"role": "user", "parts": [{"text": "Hello"}]}]
    }"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request_json)]);

    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_google_adk(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    // Should have: system instruction and user message (tools go to tool_definitions)
    assert!(messages.len() >= 2);
    assert_eq!(tool_definitions.len(), 1);

    // Check for tool definitions in the tool_definitions vector
    let tools_def = &tool_definitions[0];
    // Content is directly the tools array
    assert!(tools_def.content.is_array());
    let tools = tools_def.content.as_array().unwrap();
    assert_eq!(tools[0]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_google_adk_full_request_response_flow() {
    // Test a complete ADK LLM call with request and response
    let request_json = r#"{
        "model": "gemini-2.5-flash",
        "config": {
            "system_instruction": "You are a weather assistant",
            "top_p": 0.95,
            "max_output_tokens": 1024
        },
        "contents": [
            {"role": "user", "parts": [{"text": "What's the weather in NYC?"}]}
        ]
    }"#;
    let response_json = r#"{
        "content": {
            "role": "model",
            "parts": [{"text": "The weather in NYC is 72°F and sunny."}]
        },
        "finish_reason": "STOP",
        "usage_metadata": {
            "prompt_token_count": 20,
            "candidates_token_count": 15,
            "total_token_count": 35
        }
    }"#;
    let attrs = make_attrs(&[
        ("gcp.vertex.agent.llm_request", request_json),
        ("gcp.vertex.agent.llm_response", response_json),
    ]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Should have: system, user, and assistant messages
    let has_system = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"));
    let has_user = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("user"));
    let has_assistant = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"));

    assert!(has_system, "Should have system message");
    assert!(has_user, "Should have user message");
    assert!(has_assistant, "Should have assistant message");

    // Check finish_reason in response
    let assistant_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    assert_eq!(
        assistant_msg
            .content
            .get("finish_reason")
            .and_then(|f| f.as_str()),
        Some("stop")
    );
}

#[test]
fn test_vertex_ai_native_system_instruction() {
    // Vertex AI native format uses camelCase systemInstruction with parts
    let request_json = r#"{
        "systemInstruction": {
            "parts": [{"text": "You are a helpful AI assistant"}]
        },
        "contents": [
            {"role": "user", "parts": [{"text": "Hello"}]}
        ]
    }"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request_json)]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);

    // Should have system instruction and user message
    let has_system = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"));
    let has_user = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("user"));

    assert!(
        has_system,
        "Should extract systemInstruction as system message"
    );
    assert!(has_user, "Should have user message");

    // Verify system message has parts content
    let system_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"))
        .unwrap();
    assert!(
        system_msg.content.get("content").is_some(),
        "System message should have content"
    );
}

#[test]
fn test_vertex_ai_native_tools_top_level() {
    // Vertex AI native format has tools at top level, not in config
    let request_json = r#"{
        "tools": [{
            "functionDeclarations": [{
                "name": "get_weather",
                "description": "Get weather for a location"
            }]
        }],
        "contents": [
            {"role": "user", "parts": [{"text": "What's the weather?"}]}
        ]
    }"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request_json)]);

    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_google_adk(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    assert!(
        !tool_definitions.is_empty(),
        "Should extract top-level tools"
    );
}

#[test]
fn test_google_adk_tool_call_and_response() {
    // Test ADK tool execution span
    let tool_args = r#"{"location": "New York City"}"#;
    let tool_response = r#"{"temperature": "72F", "condition": "sunny"}"#;
    let attrs = make_attrs(&[
        ("gcp.vertex.agent.llm_request", "{}"),
        ("gcp.vertex.agent.llm_response", "{}"),
        ("gcp.vertex.agent.tool_call_args", tool_args),
        ("gcp.vertex.agent.tool_response", tool_response),
    ]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);

    // Should have tool_call and tool messages
    let has_tool_call = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool_call"));
    let has_tool_response = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool"));

    assert!(has_tool_call, "Should have tool_call message");
    assert!(has_tool_response, "Should have tool response message");
}

#[test]
fn test_google_adk_tool_call_args_extraction() {
    let args = r#"{"city":"San Francisco"}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.tool_call_args", args)]);
    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool_call"));
    assert_eq!(msg["content"]["city"].as_str(), Some("San Francisco"));
}

#[test]
fn test_google_adk_tool_definitions_extraction() {
    let request = r#"{"model":"gemini-pro","config":{"tools":[{"function_declarations":[{"name":"get_weather","description":"Get weather","parameters":{"type":"object"}}]}]},"contents":[]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_google_adk(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    assert!(messages.is_empty());
    assert_eq!(tool_definitions.len(), 1);
    let tool_def = &tool_definitions[0];
    let tools = tool_def.content.as_array().unwrap();
    // Must be unwrapped: individual tool objects, not {"function_declarations": [...]} wrappers
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"].as_str(), Some("get_weather"));
    assert_eq!(tools[0]["description"].as_str(), Some("Get weather"));
    assert!(
        tools[0].get("function_declarations").is_none(),
        "function_declarations wrapper must be unwrapped"
    );
}

#[test]
fn test_google_adk_tool_response_extraction() {
    let response = r#"{"temperature":65,"conditions":"foggy"}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.tool_response", response)]);
    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool"));
    assert_eq!(msg["content"]["temperature"].as_i64(), Some(65));
}

#[test]
fn test_google_adk_valid_llm_request_not_skipped() {
    // When llm_request has actual content, should use it (not fall back)
    let request_json =
        r#"{"model":"gemini-pro","contents":[{"role":"user","parts":[{"text":"Hello"}]}]}"#;
    let attrs = make_attrs(&[
        ("gcp.vertex.agent.llm_request", request_json),
        (
            "gcp.vertex.agent.tool_call_args",
            r#"{"should":"not be used"}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_google_adk(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Should use llm_request, not tool_call_args
    let has_user_msg = messages
        .iter()
        .any(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("user"));
    assert!(has_user_msg, "Should use valid llm_request content");
}

#[test]
fn test_langchain_tool_calls_in_ai_message() {
    let output = r#"[{"type":"AIMessage","content":"","tool_calls":[{"name":"search","args":{"query":"rust"},"id":"call_abc"}]}]"#;
    // LangGraph detection requires langgraph.* attributes
    let attrs = make_attrs(&[("output.value", output), ("langgraph.node", "agent")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let ai_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    let tool_calls = ai_msg
        .content
        .get("tool_calls")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["name"].as_str(), Some("search"));
}

#[test]
fn test_langchain_tool_message() {
    let output =
        r#"[{"type":"ToolMessage","content":"Search results here","tool_call_id":"call_abc"}]"#;
    // LangGraph detection requires langgraph.* attributes
    let attrs = make_attrs(&[("output.value", output), ("langgraph.node", "tools")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let tool_msg = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .unwrap();
    assert_eq!(
        tool_msg
            .content
            .get("tool_call_id")
            .and_then(|id| id.as_str()),
        Some("call_abc")
    );
    assert_eq!(
        tool_msg.content.get("content").and_then(|c| c.as_str()),
        Some("Search results here")
    );
}

#[test]
fn test_langgraph_ai_message() {
    let output_json =
        r#"{"messages": [{"type": "ai", "content": "I'm doing great!", "tool_calls": []}]}"#;
    let attrs = make_attrs(&[
        ("output.value", output_json),
        ("langgraph.checkpoint_ns", "test|node:123"),
    ]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("I'm doing great!")
    );
}

#[test]
fn test_langgraph_ai_message_with_tool_calls() {
    let output_json = r#"{
        "messages": [{
            "type": "ai",
            "content": "Let me check the weather.",
            "tool_calls": [
                {"id": "call_1", "name": "get_weather", "args": {"location": "NYC"}}
            ]
        }]
    }"#;
    let attrs = make_attrs(&[("output.value", output_json), ("langgraph.node", "agent")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert!(messages[0].content["tool_calls"].is_array());
}

#[test]
fn test_langgraph_human_message() {
    let input_json = r#"{"messages": [{"type": "human", "content": "Hello, how are you?"}]}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("langgraph.node", "agent")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Hello, how are you?")
    );
}

#[test]
fn test_langgraph_metadata_detection() {
    let input_json = r#"{"messages": [{"type": "human", "content": "Test"}]}"#;
    let metadata = r#"{"langgraph_node": "agent", "langgraph_checkpoint_ns": "root"}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("metadata", metadata)]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
}

#[test]
fn test_langgraph_multiple_messages() {
    let output_json = r#"{
        "messages": [
            {"type": "human", "content": "Hello"},
            {"type": "ai", "content": "Hi there!"},
            {"type": "human", "content": "What's up?"}
        ]
    }"#;
    let attrs = make_attrs(&[("output.value", output_json), ("langgraph.node", "chatbot")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(messages[1].content["role"].as_str(), Some("assistant"));
    assert_eq!(messages[2].content["role"].as_str(), Some("user"));
}

#[test]
fn test_langgraph_not_detected_without_attrs() {
    let input_json = r#"{"messages": [{"type": "human", "content": "Test"}]}"#;
    let attrs = make_attrs(&[("input.value", input_json)]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    // Should NOT match because no langgraph.* attributes
    assert!(!found);
}

#[test]
fn test_langgraph_serialized_langchain_message() {
    // LangChain serialized format with kwargs
    let input_json = r#"{
        "messages": [{
            "lc": {"type": "constructor"},
            "type": "HumanMessage",
            "kwargs": {
                "content": "What is the weather?"
            }
        }]
    }"#;
    let attrs = make_attrs(&[("input.value", input_json), ("langgraph.node", "agent")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("What is the weather?")
    );
}

#[test]
fn test_langgraph_system_message() {
    let input_json = r#"{"type": "system", "content": "You are a helpful assistant."}"#;
    let attrs = make_attrs(&[("message", input_json), ("langgraph.node", "setup")]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("system"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("You are a helpful assistant.")
    );
}

#[test]
fn test_langgraph_tool_message() {
    let output_json = r#"{"messages": [{"type": "tool", "content": "72 degrees", "tool_call_id": "call_123", "name": "get_weather"}]}"#;
    let attrs = make_attrs(&[
        ("output.value", output_json),
        ("langgraph.thread_id", "thread-456"),
    ]);
    let mut messages = Vec::new();
    let found = try_langgraph(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(messages[0].content["content"].as_str(), Some("72 degrees"));
    assert_eq!(
        messages[0].content["tool_call_id"].as_str(),
        Some("call_123")
    );
    assert_eq!(messages[0].content["name"].as_str(), Some("get_weather"));
}
