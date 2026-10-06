#[test]
fn test_langsmith_completion_with_choices() {
    let completion_json = r#"{
        "id": "chatcmpl-123",
        "model": "gpt-4",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Hello! How can I help you?"
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 8}
    }"#;
    let attrs = make_attrs(&[
        ("gen_ai.completion", completion_json),
        ("langsmith.span.kind", "llm"),
    ]);
    let mut messages = Vec::new();
    let found = try_langsmith(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Hello! How can I help you?")
    );
    assert_eq!(messages[0].content["finish_reason"].as_str(), Some("stop"));
}

#[test]
fn test_langsmith_not_detected_without_attrs() {
    let prompt_json = r#"{"messages": [{"role": "user", "content": "Test"}]}"#;
    let attrs = make_attrs(&[("gen_ai.prompt", prompt_json)]);
    let mut messages = Vec::new();
    let found = try_langsmith(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    // Should NOT match because no langsmith.* attributes
    assert!(!found);
}

#[test]
fn test_langsmith_prompt_and_completion_together() {
    let prompt_json = r#"{"messages": [{"role": "user", "content": "What is 2+2?"}]}"#;
    let completion_json = r#"{"choices": [{"message": {"role": "assistant", "content": "4"}, "finish_reason": "stop"}]}"#;
    let attrs = make_attrs(&[
        ("gen_ai.prompt", prompt_json),
        ("gen_ai.completion", completion_json),
        ("langsmith.trace.session_id", "session-123"),
    ]);
    let mut messages = Vec::new();
    let found = try_langsmith(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(messages[1].content["role"].as_str(), Some("assistant"));
}

#[test]
fn test_langsmith_prompt_with_messages() {
    let prompt_json = r#"{
        "model": "gpt-4",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "Hello!"}
        ]
    }"#;
    let attrs = make_attrs(&[
        ("gen_ai.prompt", prompt_json),
        ("langsmith.span.kind", "llm"),
    ]);
    let mut messages = Vec::new();
    let found = try_langsmith(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content["role"].as_str(), Some("system"));
    assert_eq!(messages[1].content["role"].as_str(), Some("user"));
}

#[test]
fn test_langsmith_tool_call_response() {
    let completion_json = r#"{
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"location\": \"NYC\"}"}}
                ]
            },
            "finish_reason": "tool_calls"
        }]
    }"#;
    let attrs = make_attrs(&[
        ("gen_ai.completion", completion_json),
        ("langsmith.span.kind", "llm"),
    ]);
    let mut messages = Vec::new();
    let found = try_langsmith(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert!(messages[0].content["tool_calls"].is_array());
}

#[test]
fn test_livekit_function_tool_output() {
    let attrs = make_attrs(&[(
        "lk.function_tool.output",
        r#"{"temperature":"72F","condition":"sunny"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from lk.function_tool.output");
    assert!(!messages.is_empty());
}

#[test]
fn test_livekit_input_text() {
    let attrs = make_attrs(&[("lk.input_text", "What's the weather in NYC?")]);

    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from lk.input_text");
    assert!(!messages.is_empty());
}

#[test]
fn test_livekit_response_only_function_calls() {
    // Response with only function calls (no text)
    let func_calls = r#"[{"name":"send_message","arguments":"{}"}]"#;
    let attrs = make_attrs(&[("lk.response.function_calls", func_calls)]);
    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    assert!(response.content.get("tool_calls").is_some());
}

#[test]
fn test_livekit_response_text() {
    let attrs = make_attrs(&[("lk.response.text", "The weather is sunny, 72F.")]);

    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from lk.response.text");
    assert!(!messages.is_empty());
}

#[test]
fn test_livekit_response_with_function_calls() {
    let func_calls = r#"[{"name":"get_weather","arguments":"{\"city\":\"Portland\"}"}]"#;
    let attrs = make_attrs(&[
        ("lk.response.text", "Let me check the weather."),
        ("lk.response.function_calls", func_calls),
    ]);
    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let response = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    assert_eq!(
        response.content.get("content").and_then(|c| c.as_str()),
        Some("Let me check the weather.")
    );
    let tool_calls = response
        .content
        .get("tool_calls")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_livekit_tool_call_input_extraction() {
    let attrs = make_attrs(&[
        ("lk.function_tool.name", "get_weather"),
        ("lk.function_tool.id", "tool_123"),
        ("lk.function_tool.arguments", r#"{"city":"Seattle"}"#),
    ]);
    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

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
        Some("tool_123")
    );
    assert_eq!(
        tool_call.content["content"]["city"].as_str(),
        Some("Seattle")
    );
}

#[test]
fn test_livekit_tool_definitions_extraction() {
    let tools = r#"[{"name":"get_weather","description":"Get current weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]"#;
    let attrs = make_attrs(&[("lk.function_tools", tools)]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_livekit(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    assert!(messages.is_empty()); // Tool definitions go to separate vector
    assert_eq!(tool_definitions.len(), 1);
    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
    let content = def.as_array().unwrap();
    assert_eq!(content[0]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_livekit_tool_output_extraction() {
    let attrs = make_attrs(&[
        ("lk.function_tool.name", "get_weather"),
        ("lk.function_tool.id", "tool_123"),
        ("lk.function_tool.output", r#"{"temp":58,"rain":true}"#),
    ]);
    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let tool_result = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .unwrap();
    assert_eq!(
        tool_result.content.get("name").and_then(|n| n.as_str()),
        Some("get_weather")
    );
    assert_eq!(tool_result.content["content"]["temp"].as_i64(), Some(58));
}

#[test]
fn test_livekit_tool_output_with_error() {
    let attrs = make_attrs(&[
        ("lk.function_tool.name", "get_weather"),
        ("lk.function_tool.output", "City not found"),
        ("lk.function_tool.is_error", "true"),
    ]);
    let mut messages = Vec::new();
    let found = try_livekit(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    let tool_result = messages
        .iter()
        .find(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .unwrap();
    assert_eq!(
        tool_result
            .content
            .get("is_error")
            .and_then(|e| e.as_bool()),
        Some(true)
    );
    assert_eq!(
        tool_result.content.get("content").and_then(|c| c.as_str()),
        Some("City not found")
    );
}

#[test]
fn test_logfire_all_messages_events_attribute() {
    // Logfire uses "all_messages_events" for output
    let attrs = make_attrs(&[(
        "all_messages_events",
        r#"[{"role":"assistant","content":"Response"}]"#,
    )]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from 'all_messages_events' attribute");
}

#[test]
fn test_logfire_events_preserve_event_name_for_query_time_role_derivation() {
    // Logfire events are stored with event name for query-time role derivation
    let events_json = r#"[
        {"event.name":"gen_ai.user.message","content":"Hello!"},
        {"event.name":"gen_ai.assistant.message","content":"Hi there!"}
    ]"#;
    let attrs = make_attrs(&[("events", events_json)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 2);

    // First message: event name preserved for query-time role derivation
    let user_msg = &messages[0];
    assert!(
        matches!(&user_msg.source, MessageSource::Event { name, .. } if name == "gen_ai.user.message"),
        "Event name should be preserved in MessageSource::Event for query-time role derivation"
    );
    // Role NOT set at extraction time - derived at query-time by sideml pipeline
    assert!(
        user_msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );

    // Second message: same pattern
    let assistant_msg = &messages[1];
    assert!(
        matches!(&assistant_msg.source, MessageSource::Event { name, .. } if name == "gen_ai.assistant.message"),
        "Event name should be preserved for query-time role derivation"
    );
    assert!(
        assistant_msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
}

#[test]
fn test_logfire_prompt_attribute() {
    // Logfire uses "prompt" attribute for input
    let attrs = make_attrs(&[("prompt", r#"[{"role":"user","content":"Hello"}]"#)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from 'prompt' attribute");
}

#[test]
fn test_logfire_tool_arguments_extraction() {
    let args = r#"{"query":"what is rust","max_results":10}"#;
    let attrs = make_attrs(&[("tool_arguments", args)]);
    let mut messages = Vec::new();
    let found = try_pydantic_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool_call"));
    assert_eq!(msg["content"]["query"].as_str(), Some("what is rust"));
}

#[test]
fn test_logfire_tool_definitions_via_otel() {
    // Logfire/PydanticAI can use OTEL gen_ai.tool.definitions
    let defs = r#"[{"name":"web_search","description":"Search the web"}]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", defs)]);
    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());

    assert!(!tool_definitions.is_empty());
    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
    assert_eq!(
        def.as_array().unwrap()[0]["name"].as_str(),
        Some("web_search")
    );
}

#[test]
fn test_logfire_tool_response_extraction() {
    let response = r#"["Result 1","Result 2","Result 3"]"#;
    let attrs = make_attrs(&[("tool_response", response)]);
    let mut messages = Vec::new();
    let found = try_pydantic_ai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool"));
    let content = msg.get("content").unwrap().as_array().unwrap();
    assert_eq!(content.len(), 3);
}

#[test]
fn test_mlflow_chat_tools_extraction() {
    let attrs = make_attrs(&[(
        "mlflow.chat.tools",
        r#"[{"name":"get_weather","description":"Get weather info"}]"#,
    )]);

    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_mlflow(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found, "Should extract from mlflow.chat.tools");
    assert!(messages.is_empty()); // Tool definitions go to separate vector
    assert_eq!(tool_definitions.len(), 1);

    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
}

#[test]
fn test_mlflow_session_id_extraction() {
    let attrs = make_attrs(&[
        ("mlflow.trace.session", "mlflow-session-123"),
        ("mlflow.spanInputs", "{}"),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(span.session_id, Some("mlflow-session-123".to_string()));
}

#[test]
fn test_mlflow_span_inputs() {
    let attrs = make_attrs(&[(
        "mlflow.spanInputs",
        r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_mlflow(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from mlflow.spanInputs");
    assert!(!messages.is_empty());
}

#[test]
fn test_mlflow_span_inputs_with_tools() {
    // MLflow spanInputs may contain messages with tool_calls
    let inputs = r#"{"messages":[{"role":"assistant","content":"","tool_calls":[{"id":"call_1","function":{"name":"search"}}]}]}"#;
    let attrs = make_attrs(&[("mlflow.spanInputs", inputs)]);
    let mut messages = Vec::new();
    let found = try_mlflow(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    // Raw content is preserved
    let msg = &messages[0].content;
    assert!(msg["messages"][0]["tool_calls"].is_array());
}

#[test]
fn test_mlflow_span_outputs() {
    let attrs = make_attrs(&[(
        "mlflow.spanOutputs",
        r#"{"response":"Hi there!","model":"gpt-4"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_mlflow(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from mlflow.spanOutputs");
    assert!(!messages.is_empty());
}

#[test]
fn test_mlflow_tool_definitions_extraction() {
    let tools = r#"[{"type":"function","function":{"name":"get_stock_price","description":"Get stock price","parameters":{"type":"object"}}}]"#;
    let attrs = make_attrs(&[("mlflow.chat.tools", tools)]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_mlflow(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    assert!(messages.is_empty()); // Tool definitions go to separate vector
    assert_eq!(tool_definitions.len(), 1);
    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
    let content = def.as_array().unwrap();
    assert_eq!(
        content[0]["function"]["name"].as_str(),
        Some("get_stock_price")
    );
}

#[test]
fn test_mlflow_user_id_extraction() {
    let attrs = make_attrs(&[
        ("mlflow.trace.user", "mlflow-user-456"),
        ("mlflow.spanInputs", "{}"),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(span.user_id, Some("mlflow-user-456".to_string()));
}

#[test]
fn test_openai_style_tool_definitions() {
    // OpenAI/Anthropic style function definitions
    let definitions_json = r#"[
        {
            "type": "function",
            "function": {
                "name": "get_current_weather",
                "description": "Get the current weather in a given location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "The city and state, e.g. San Francisco, CA"
                        },
                        "unit": {
                            "type": "string",
                            "enum": ["celsius", "fahrenheit"]
                        }
                    },
                    "required": ["location"]
                }
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
    assert_eq!(tools[0]["type"].as_str(), Some("function"));
    assert_eq!(
        tools[0]["function"]["name"].as_str(),
        Some("get_current_weather")
    );
}

#[test]
fn test_openinference_embedding_text() {
    let attrs = make_attrs(&[
        ("embedding.text", "This is the text to embed."),
        ("embedding.model_name", "text-embedding-ada-002"),
    ]);

    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    let msg = &messages[0];
    assert_eq!(
        msg.content.get("role").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        msg.content.get("content").and_then(|v| v.as_str()),
        Some("This is the text to embed.")
    );
    assert_eq!(
        msg.content.get("_source").and_then(|v| v.as_str()),
        Some("embedding.text")
    );
}

#[test]
fn test_openinference_invocation_parameters() {
    let params_json = r#"{"temperature":0.7,"max_tokens":1000,"top_p":0.9}"#;
    let attrs = make_attrs(&[
        ("llm.invocation_parameters", params_json),
        ("llm.model_name", "gpt-4"),
    ]);

    let mut span = SpanData::default();
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut span,
        &attrs,
        "test_span",
    );

    assert_eq!(span.gen_ai_temperature, Some(0.7));
    assert_eq!(span.gen_ai_max_tokens, Some(1000));
    assert_eq!(span.gen_ai_top_p, Some(0.9));
}

#[test]
fn test_openinference_invocation_parameters_does_not_override() {
    let params_json = r#"{"temperature":0.7,"max_tokens":1000}"#;
    let attrs = make_attrs(&[
        ("llm.invocation_parameters", params_json),
        ("gen_ai.request.temperature", "0.5"),
    ]);

    let mut span = SpanData::default();
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut span,
        &attrs,
        "test_span",
    );

    // Explicit attribute should take precedence
    assert_eq!(span.gen_ai_temperature, Some(0.5));
    // Fallback should still work for missing values
    assert_eq!(span.gen_ai_max_tokens, Some(1000));
}

#[test]
fn test_openinference_llm_messages_with_tool_calls() {
    let tools_json = r#"[{"name": "get_weather", "description": "Get weather forecast", "input_schema": {"properties": {"city": {"type": "string"}}}}]"#;
    let attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.content",
            "Provide a 3-day weather forecast for New York City and greet the user.",
        ),
        ("llm.output_messages.0.message.role", "assistant"),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.id",
            "toolu_bdrk_01SLs9LwScHFA5xAZXwzjYEe",
        ),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.function.name",
            "get_weather",
        ),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.function.arguments",
            r#"{"city": "New York City", "days": 3}"#,
        ),
        ("llm.tools", tools_json),
    ]);

    // Messages extracted by try_openinference
    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    assert!(found);
    assert_eq!(messages.len(), 2); // 2 messages: user input, assistant output

    // Tool definitions extracted separately by extract_tool_definitions()
    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    // Input message - literal content only, no metadata
    let input_msg = &messages[0];
    assert!(input_msg.content.get("_is_output").is_none()); // No metadata
    assert_eq!(
        input_msg.content.get("role").and_then(|v| v.as_str()),
        Some("user")
    );
    assert_eq!(
        input_msg.content.get("content").and_then(|v| v.as_str()),
        Some("Provide a 3-day weather forecast for New York City and greet the user.")
    );
    assert!(input_msg.content.get("_tools").is_none()); // No metadata

    // Output message - literal content only, no metadata
    let output_msg = &messages[1];
    assert!(output_msg.content.get("_is_output").is_none()); // No metadata
    assert_eq!(
        output_msg.content.get("role").and_then(|v| v.as_str()),
        Some("assistant")
    );

    // Raw flattened keys preserved (sideml handles unflattening)
    assert_eq!(
        output_msg
            .content
            .get("tool_calls.0.tool_call.id")
            .and_then(|v| v.as_str()),
        Some("toolu_bdrk_01SLs9LwScHFA5xAZXwzjYEe")
    );
    assert_eq!(
        output_msg
            .content
            .get("tool_calls.0.tool_call.function.name")
            .and_then(|v| v.as_str()),
        Some("get_weather")
    );
}

#[test]
fn test_openinference_llm_tools_extraction() {
    // llm.tools is now extracted by extract_tool_definitions()
    let tools_json = r#"[{"name":"get_weather","description":"Get weather forecast","input_schema":{"type":"object"}}]"#;
    let attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        (
            "llm.input_messages.0.message.content",
            "What's the weather?",
        ),
        ("llm.tools", tools_json),
    ]);

    // Messages still extracted by try_openinference
    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    assert!(found);
    assert_eq!(messages.len(), 1); // Only user message

    // Tool definitions extracted separately
    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools_def = &tool_definitions[0];
    assert!(tools_def.content.is_array());
    let tools = tools_def.content.as_array().unwrap();
    assert_eq!(tools[0]["name"].as_str(), Some("get_weather"));
}

#[test]
fn test_openinference_reranker_documents() {
    let attrs = make_attrs(&[
        ("reranker.query", "What is the capital of France?"),
        ("reranker.input_documents.0.document.id", "doc-1"),
        (
            "reranker.input_documents.0.document.content",
            "Paris is the capital.",
        ),
        ("reranker.output_documents.0.document.id", "doc-1"),
        (
            "reranker.output_documents.0.document.content",
            "Paris is the capital.",
        ),
        ("reranker.output_documents.0.document.score", "0.98"),
    ]);

    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Should have: reranker.query (user), input_documents, output_documents
    assert_eq!(messages.len(), 3);

    // Check query message
    let query_msg = messages
        .iter()
        .find(|m| m.content.get("_source").and_then(|v| v.as_str()) == Some("reranker.query"));
    assert!(query_msg.is_some());
    assert_eq!(
        query_msg
            .unwrap()
            .content
            .get("content")
            .and_then(|v| v.as_str()),
        Some("What is the capital of France?")
    );

    // Check output documents have score
    let output_docs = messages.iter().find(|m| {
        m.content.get("_source").and_then(|v| v.as_str()) == Some("reranker.output_documents")
    });
    assert!(output_docs.is_some());
    let docs = output_docs
        .unwrap()
        .content
        .get("content")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(docs[0]["score"].as_f64(), Some(0.98));
}

#[test]
fn test_openinference_retrieval_documents() {
    let attrs = make_attrs(&[
        ("retrieval.documents.0.document.id", "doc-1"),
        (
            "retrieval.documents.0.document.content",
            "Paris is the capital of France.",
        ),
        ("retrieval.documents.0.document.score", "0.95"),
        ("retrieval.documents.1.document.id", "doc-2"),
        (
            "retrieval.documents.1.document.content",
            "The Eiffel Tower is in Paris.",
        ),
        ("retrieval.documents.1.document.score", "0.87"),
    ]);

    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    let docs_msg = &messages[0];
    assert_eq!(
        docs_msg.content.get("role").and_then(|v| v.as_str()),
        Some("documents")
    );

    let docs = docs_msg.content.get("content").unwrap().as_array().unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["id"].as_str(), Some("doc-1"));
    assert_eq!(
        docs[0]["content"].as_str(),
        Some("Paris is the capital of France.")
    );
    assert_eq!(docs[0]["score"].as_f64(), Some(0.95));
}

#[test]
fn test_openinference_tool_call_in_message() {
    // OpenInference embeds tool calls in llm.output_messages with tool_calls field
    let attrs = make_attrs(&[
        ("llm.output_messages.0.message.role", "assistant"),
        ("llm.output_messages.0.message.content", ""),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.id",
            "call_123",
        ),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.function.name",
            "get_weather",
        ),
        (
            "llm.output_messages.0.message.tool_calls.0.tool_call.function.arguments",
            r#"{"city":"NYC"}"#,
        ),
    ]);
    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert!(!messages.is_empty());
}

#[test]
fn test_openinference_tool_definitions_extraction() {
    // llm.tools is now extracted by extract_tool_definitions()
    let tools = r#"[{"name":"search","description":"Search the web"},{"name":"calculator","description":"Do math"}]"#;
    let attrs = make_attrs(&[("llm.tools", tools)]);

    let (tool_definitions, _) = extract_tool_definitions("", &attrs, Utc::now());

    assert_eq!(tool_definitions.len(), 1);
    // Content is directly the tools array
    let def = &tool_definitions[0].content;
    assert!(def.is_array());
    let content = def.as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["name"].as_str(), Some("search"));
    assert_eq!(content[1]["name"].as_str(), Some("calculator"));
}
