use std::collections::HashMap;

use chrono::Utc;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::span::Event;
use serde_json::json;

use super::*;

// Cross-module imports for integration tests
use crate::traces::extract::attributes::{SpanData, apply_span_fields};

fn make_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn make_kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    }
}

#[test]
fn test_autogen_assistant_message() {
    let message_json = r#"{"content":"I can help with that!","source":"assistant_agent","thought":"Let me think...","type":"AssistantMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["name"].as_str(),
        Some("assistant_agent")
    );
    // Thought is prepended as thinking content block
    let content = messages[0].content["content"]
        .as_array()
        .expect("content should be array");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"].as_str(), Some("thinking"));
    assert_eq!(content[0]["text"].as_str(), Some("Let me think..."));
    assert_eq!(content[1].as_str(), Some("I can help with that!"));
}

#[test]
fn test_autogen_function_execution_result_message() {
    let message_json = r#"{"content":[{"content":"72 degrees","name":"get_weather","call_id":"call_123","is_error":false}],"type":"FunctionExecutionResultMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(messages[0].content["name"].as_str(), Some("get_weather"));
    assert_eq!(messages[0].content["content"].as_str(), Some("72 degrees"));
    assert_eq!(
        messages[0].content["tool_call_id"].as_str(),
        Some("call_123")
    );
}

#[test]
fn test_autogen_function_execution_result_with_call_id() {
    let msg = r#"{"type":"FunctionExecutionResultMessage","content":[{"name":"get_weather","content":"72F sunny","call_id":"call_abc"}]}"#;
    let attrs = make_attrs(&[("message", msg)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let result = &messages[0].content;
    assert_eq!(result.get("role").and_then(|r| r.as_str()), Some("tool"));
    assert_eq!(
        result.get("name").and_then(|n| n.as_str()),
        Some("get_weather")
    );
    assert_eq!(
        result.get("tool_call_id").and_then(|id| id.as_str()),
        Some("call_abc")
    );
}

#[test]
fn test_autogen_llm_call_event() {
    let llm_call_json = r#"{
        "type": "LLMCall",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "What is the weather?"}
        ],
        "response": {
            "content": "I'll check the weather for you.",
            "tool_calls": [{"id": "call_1", "function": {"name": "get_weather", "arguments": "{}"}}]
        },
        "prompt_tokens": 50,
        "completion_tokens": 20,
        "tools": [{"name": "get_weather", "description": "Get weather info"}]
    }"#;
    let attrs = make_attrs(&[("body", llm_call_json)]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_autogen(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    // Should have: 2 input messages + 1 response = 3 (tool definitions go to separate vector)
    assert_eq!(messages.len(), 3);
    assert_eq!(tool_definitions.len(), 1);

    // Check input messages
    assert_eq!(messages[0].content["role"].as_str(), Some("system"));
    assert_eq!(messages[1].content["role"].as_str(), Some("user"));

    // Check response
    assert_eq!(messages[2].content["role"].as_str(), Some("assistant"));
    assert!(messages[2].content["tool_calls"].is_array());

    // Check tool definitions (now in separate vector)
    // Content is directly the tools array
    assert!(tool_definitions[0].content.is_array());
}

#[test]
fn test_autogen_llm_stream_end_event() {
    let stream_end_json = r#"{
        "type": "LLMStreamEnd",
        "response": {
            "choices": [{
                "message": {
                    "content": "Here is the weather forecast.",
                    "tool_calls": null
                }
            }]
        },
        "prompt_tokens": 100,
        "completion_tokens": 50
    }"#;
    let attrs = make_attrs(&[("log.body", stream_end_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Here is the weather forecast.")
    );
}

#[test]
fn test_autogen_message_attribute_with_messages_array() {
    // AutoGen outer message format with messages array (OpenInference TextMessage)
    let message_json = r#"{"messages":[{"id":"d07ee6fd-1bef-4d63-bdae-4c710f6dcdd2","source":"user","models_usage":null,"metadata":{},"created_at":"2025-12-12T17:11:24.103905Z","content":"Provide a 3-day weather forecast for New York City and greet the user. Say TERMINATE when done.","type":"TextMessage"}],"output_task_messages":true}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    // TextMessage normalized to SideML format with role based on source
    let msg = &messages[0];
    assert_eq!(
        msg.content["content"].as_str(),
        Some(
            "Provide a 3-day weather forecast for New York City and greet the user. Say TERMINATE when done."
        )
    );
    // source:"user" -> role:"user" in normalized output
    assert_eq!(msg.content["role"].as_str(), Some("user"));
}

#[test]
fn test_autogen_no_message_skipped() {
    let attrs = make_attrs(&[("message", "No Message")]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found);
    assert!(messages.is_empty());
}

#[test]
fn test_autogen_system_message() {
    let message_json = r#"{"content":"You are a helpful assistant.","type":"SystemMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("system"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("You are a helpful assistant.")
    );
}

#[test]
fn test_autogen_tool_call_event() {
    let tool_call_json = r#"{
        "type": "ToolCall",
        "tool_name": "get_weather",
        "arguments": {"location": "NYC"},
        "result": "72 degrees and sunny"
    }"#;
    let attrs = make_attrs(&[("body", tool_call_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(messages[0].content["name"].as_str(), Some("get_weather"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("72 degrees and sunny")
    );
    assert_eq!(
        messages[0].content["tool_call"]["name"].as_str(),
        Some("get_weather")
    );
    assert_eq!(
        messages[0].content["tool_call"]["arguments"]["location"].as_str(),
        Some("NYC")
    );
}

#[test]
fn test_autogen_tool_call_event_with_result() {
    let tool_call = r#"{"type":"ToolCall","tool_name":"get_weather","arguments":{"city":"NYC"},"result":"72F sunny"}"#;
    let attrs = make_attrs(&[("body", tool_call)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool"));
    assert_eq!(
        msg.get("name").and_then(|n| n.as_str()),
        Some("get_weather")
    );
    assert_eq!(
        msg.get("content").and_then(|c| c.as_str()),
        Some("72F sunny")
    );
    let tool_call = msg.get("tool_call").unwrap();
    assert_eq!(tool_call["name"].as_str(), Some("get_weather"));
    assert_eq!(tool_call["arguments"]["city"].as_str(), Some("NYC"));
}

#[test]
fn test_autogen_tool_definitions_in_llm_call() {
    let llm_call = r#"{"type":"LLMCall","messages":[{"role":"user","content":"What is the weather?"}],"tools":[{"name":"get_weather","description":"Get weather","parameters":{}}]}"#;
    let attrs = make_attrs(&[("body", llm_call)]);
    let mut messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let found = try_autogen(&mut messages, &mut tool_definitions, &attrs, "", Utc::now());

    assert!(found);
    // Should have user message in messages, tool definitions in separate vector
    assert_eq!(messages.len(), 1);
    assert_eq!(tool_definitions.len(), 1);
    let tool_def = &tool_definitions[0];
    // Content is directly the tools array
    assert!(tool_def.content.is_array());
    assert_eq!(tool_def.content.as_array().unwrap().len(), 1);
}

#[test]
fn test_autogen_user_message() {
    let message_json = r#"{"content":"Hello, world!","source":"user_agent","type":"UserMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Hello, world!")
    );
    assert_eq!(messages[0].content["name"].as_str(), Some("user_agent"));
}

#[test]
fn test_autogen_openinference_text_message_user() {
    // OpenInference AutoGen TextMessage with source:"user"
    let message_json =
        r#"{"id":"abc123","source":"user","content":"What is the weather?","type":"TextMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("user"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("What is the weather?")
    );
}

#[test]
fn test_autogen_openinference_text_message_assistant() {
    // OpenInference AutoGen TextMessage with source other than "user" -> assistant
    let message_json = r#"{"id":"xyz789","source":"weather_assistant","content":"Here is the forecast.","type":"TextMessage"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Here is the forecast.")
    );
    assert_eq!(
        messages[0].content["name"].as_str(),
        Some("weather_assistant")
    );
}

#[test]
fn test_autogen_openinference_tool_call_request_event() {
    // OpenInference AutoGen ToolCallRequestEvent
    let message_json = r#"{"message":{"id":"event123","source":"assistant","content":[{"id":"tool_call_1","name":"get_weather","arguments":"{\"city\": \"NYC\"}"}],"type":"ToolCallRequestEvent"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg["role"].as_str(), Some("assistant"));
    // Tool calls should be normalized
    let tool_calls = msg["tool_calls"]
        .as_array()
        .expect("tool_calls should be array");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["id"].as_str(), Some("tool_call_1"));
    assert_eq!(
        tool_calls[0]["function"]["name"].as_str(),
        Some("get_weather")
    );
}

#[test]
fn test_autogen_openinference_nested_message() {
    // OpenInference AutoGen nested message format: {"message": {...}}
    let message_json = r#"{"message":{"id":"nested123","source":"weather_agent","content":"Nested content","type":"TextMessage"}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("Nested content")
    );
    assert_eq!(messages[0].content["name"].as_str(), Some("weather_agent"));
}

#[test]
fn test_crewai_output_value() {
    let output_json = r#"{"raw": "Hello! Welcome! Here's the forecast...", "pydantic": null, "agent": "Weather Forecaster"}"#;
    // CrewAI detection requires a CrewAI-specific attribute
    let attrs = make_attrs(&[("output.value", output_json), ("crew_key", "test-key")]);
    let mut messages = Vec::new();
    let found = try_crewai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    // The answer as an assistant message, not the payload object. Emitting the object put a JSON
    // blob where the conversation view expects a reply, and the reply is what `raw` holds.
    let msg = &messages[0];
    assert_eq!(msg.content["role"].as_str(), Some("assistant"));
    assert_eq!(
        msg.content["content"].as_str(),
        Some("Hello! Welcome! Here's the forecast...")
    );
}

#[test]
fn test_crewai_tasks_preserved() {
    let tasks_json = r#"[{"key": "79861a87be85893981e6e32de034a9c9", "id": "44462efe-cbec-4bc2-b892-1c1f09bca38a", "async_execution?": false, "human_input?": false, "agent_role": "Weather Forecaster", "agent_key": "2704760cd615ff308a4a362d20321071", "tools_names": ["get_weather"]}]"#;
    let attrs = make_attrs(&[("crew_tasks", tasks_json)]);
    let mut messages = Vec::new();
    let found = try_crewai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);

    // Literal content preserved (tasks array directly, no metadata wrapper)
    let msg = &messages[0];
    assert!(msg.content.is_array());
    assert_eq!(
        msg.content[0]["agent_role"].as_str(),
        Some("Weather Forecaster")
    );
    assert!(msg.content[0]["tools_names"].is_array());
}

#[test]
fn test_crewai_output_with_messages_array() {
    // CrewAI output.value with embedded messages array should extract individual messages
    let output_json = r#"{"description": "Test task", "raw": "Result", "messages": [{"role": "system", "content": "System message"}, {"role": "user", "content": "User message"}, {"role": "assistant", "content": "Assistant message"}]}"#;
    let attrs = make_attrs(&[("output.value", output_json), ("crew_key", "test-key")]);
    let mut messages = Vec::new();
    let found = try_crewai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    // Three history messages plus the answer in `raw`. The answer is emitted alongside the
    // history, not instead of it: CrewAI puts the conversation in `messages` and what the model
    // produced in `raw`, and treating `raw` as a fallback lost the answer of every run that
    // carried history.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[3].content["role"].as_str(), Some("assistant"));
    assert_eq!(messages[3].content["content"].as_str(), Some("Result"));

    // Verify each message has role and content
    assert_eq!(messages[0].content["role"].as_str(), Some("system"));
    assert_eq!(
        messages[0].content["content"].as_str(),
        Some("System message")
    );
    assert_eq!(messages[1].content["role"].as_str(), Some("user"));
    assert_eq!(messages[2].content["role"].as_str(), Some("assistant"));
}

#[test]
fn test_crewai_output_with_tool_calls_without_content() {
    // CrewAI/OpenAI-style assistant tool call message can omit explicit content.
    // Preserve these messages so tool calls are not dropped.
    let output_json = r#"{"messages":[{"role":"assistant","tool_calls":[{"id":"call_1","function":{"name":"temperature_forecast","arguments":"{\"city\":\"New York\"}"}}]}]}"#;
    let attrs = make_attrs(&[("output.value", output_json), ("crew_key", "test-key")]);
    let mut messages = Vec::new();
    let found = try_crewai(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"].as_str(), Some("assistant"));
    assert!(messages[0].content["tool_calls"].is_array());
}

#[test]
fn test_crewai_tool_definitions_from_input_value_tools_array() {
    let input_json = r#"{"tools":["name='temperature_forecast' description=\"Tool Name: temperature_forecast\nTool Arguments: {'city': {'description': 'City name', 'type': 'str'}, 'days': {'description': None, 'type': 'int'}}\nTool Description: Get temperature forecast\""]}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("crew_key", "test")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("temperature_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get temperature forecast")
    );
    assert_eq!(
        func["parameters"]["properties"]["city"]["type"].as_str(),
        Some("string")
    );
    assert_eq!(
        func["parameters"]["properties"]["city"]["description"].as_str(),
        Some("City name")
    );
    assert_eq!(
        func["parameters"]["properties"]["days"]["type"].as_str(),
        Some("integer")
    );
}

#[test]
fn test_crewai_tool_definitions_from_input_value_structured_tool() {
    let input_json = r#"{"tool":"CrewStructuredTool(name='precipitation_forecast', description='Tool Name: precipitation_forecast\nTool Arguments: {'city': {'description': None, 'type': 'str'}}\nTool Description: Get precipitation forecast')"}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("crew_key", "test")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("precipitation_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get precipitation forecast")
    );
    assert_eq!(
        func["parameters"]["properties"]["city"]["type"].as_str(),
        Some("string")
    );
}

#[test]
fn test_crewai_tool_definitions_from_input_value_escaped_newlines() {
    // Some CrewAI payloads carry literal "\n" in repr strings (double-escaped in JSON).
    // Ensure Tool Name parsing doesn't swallow the whole description blob.
    let input_json = r#"{"tools":["name='temperature_forecast' description=\"Tool Name: temperature_forecast\\nTool Arguments: {'city': {'description': None, 'type': 'str'}}\\nTool Description: Get temperature forecast\""]}"#;
    let attrs = make_attrs(&[("input.value", input_json), ("crew_key", "test")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("temperature_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get temperature forecast")
    );
}

#[test]
fn test_crewai_tool_definitions_from_input_value_object_tools() {
    let input_json = r#"{
        "tools": [
            {
                "name": "temperature_forecast",
                "description": "Get temperature forecast",
                "parameters": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "precipitation_forecast",
                    "description": "Get precipitation forecast",
                    "parameters": {
                        "type": "object",
                        "properties": { "days": { "type": "integer" } }
                    }
                }
            }
        ]
    }"#;
    let attrs = make_attrs(&[("input.value", input_json), ("crew_key", "test")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 2);

    let temp = tools
        .iter()
        .find(|t| t["function"]["name"] == json!("temperature_forecast"))
        .unwrap();
    assert_eq!(
        temp["function"]["description"].as_str(),
        Some("Get temperature forecast")
    );
    assert_eq!(
        temp["function"]["parameters"]["properties"]["city"]["type"].as_str(),
        Some("string")
    );

    let precip = tools
        .iter()
        .find(|t| t["function"]["name"] == json!("precipitation_forecast"))
        .unwrap();
    assert_eq!(
        precip["function"]["description"].as_str(),
        Some("Get precipitation forecast")
    );
    assert_eq!(
        precip["function"]["parameters"]["properties"]["days"]["type"].as_str(),
        Some("integer")
    );
}

#[test]
fn test_crewai_tool_definitions_prefers_rich_over_name_only() {
    let input_json = r#"{
        "tools": [
            "temperature_forecast",
            {
                "name": "temperature_forecast",
                "description": "Tool Name: temperature_forecast\nTool Arguments: {'city': {'description': 'City name', 'type': 'str'}}\nTool Description: Get temperature forecast"
            }
        ]
    }"#;
    let attrs = make_attrs(&[("input.value", input_json), ("crew_key", "test")]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("temperature_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get temperature forecast")
    );
    assert_eq!(
        func["parameters"]["properties"]["city"]["description"].as_str(),
        Some("City name")
    );
}

#[test]
fn test_crewai_tool_definitions_from_agents_tools_object() {
    let crew_agents_json = r#"[
        {
            "tools": [
                {
                    "name": "temperature_forecast",
                    "description": "Get temperature forecast",
                    "parameters": {
                        "type": "object",
                        "properties": { "city": { "type": "string" } }
                    }
                }
            ]
        }
    ]"#;
    let attrs = make_attrs(&[("crew_agents", crew_agents_json)]);

    let (tool_definitions, _tool_names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_definitions.len(), 1);

    let tools = tool_definitions[0].content.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let func = &tools[0]["function"];
    assert_eq!(func["name"].as_str(), Some("temperature_forecast"));
    assert_eq!(
        func["description"].as_str(),
        Some("Get temperature forecast")
    );
}

#[test]
fn test_event_explicit_role_not_overwritten() {
    // If event already has role attribute, don't override it
    let event = Event {
        name: "gen_ai.user.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", "Hello!"), make_kv("role", "custom_role")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Explicit role should be preserved
    assert_eq!(
        msg.content.get("role").and_then(|v| v.as_str()),
        Some("custom_role"),
        "Explicit role attribute should not be overwritten"
    );
}

#[test]
fn test_event_raw_json_content_preserved() {
    let json_content = r#"{"text":"Hello","metadata":{"key":"value"}}"#;
    let event = Event {
        name: "gen_ai.user.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", json_content)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    let content = msg.content.get("content").unwrap();
    assert!(content.is_object());
    assert_eq!(content["text"].as_str(), Some("Hello"));
    assert_eq!(content["metadata"]["key"].as_str(), Some("value"));
}

// Role derivation tests - verify raw extraction stores event name for query-time derivation
// Actual role derivation is tested in sideml/tests.rs (query-time processing)

#[test]
fn test_event_extracts_raw_without_role_assistant_message() {
    // Event without explicit role attribute should extract raw content
    // Role derived at query-time from event name stored in source
    let event = Event {
        name: "gen_ai.assistant.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", "Hi there!")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Role NOT set at extraction - derived at query-time
    assert!(
        msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    // Event name stored in source for query-time role derivation
    assert!(matches!(
        &msg.source,
        MessageSource::Event { name, .. } if name == "gen_ai.assistant.message"
    ));
}

#[test]
fn test_event_extracts_raw_without_role_choice() {
    let event = Event {
        name: "gen_ai.choice".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![
            make_kv("message", "Response content"),
            make_kv("finish_reason", "stop"),
        ],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    // Role NOT set at extraction - derived at query-time
    assert!(
        msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    assert!(matches!(
        &msg.source,
        MessageSource::Event { name, .. } if name == "gen_ai.choice"
    ));
}

#[test]
fn test_event_extracts_raw_without_role_system_message() {
    let event = Event {
        name: "gen_ai.system.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", "You are helpful.")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    assert!(
        msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    assert!(matches!(
        &msg.source,
        MessageSource::Event { name, .. } if name == "gen_ai.system.message"
    ));
}

#[test]
fn test_event_extracts_raw_without_role_tool_message() {
    let event = Event {
        name: "gen_ai.tool.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", r#"{"result": "72F"}"#)],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    assert!(
        msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    assert!(matches!(
        &msg.source,
        MessageSource::Event { name, .. } if name == "gen_ai.tool.message"
    ));
}

#[test]
fn test_event_extracts_raw_without_role_user_message() {
    let event = Event {
        name: "gen_ai.user.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", "Hello!")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    let msg = &msgs[0];

    assert!(
        msg.content.get("role").is_none(),
        "Role should not be set at extraction time"
    );
    assert!(matches!(
        &msg.source,
        MessageSource::Event { name, .. } if name == "gen_ai.user.message"
    ));
}

#[test]
fn test_extract_gen_ai_indexed_messages() {
    let attrs = make_attrs(&[
        ("gen_ai.prompt.0.content", "Hello!"),
        ("gen_ai.prompt.0.role", "user"),
        ("gen_ai.completion.0.content", "Hi there!"),
        ("gen_ai.completion.0.role", "assistant"),
    ]);
    let mut messages = Vec::new();
    let found = try_gen_ai_indexed(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    assert!(found);
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].content.get("role").and_then(|r| r.as_str()),
        Some("user")
    );
    assert_eq!(
        messages[1].content.get("role").and_then(|r| r.as_str()),
        Some("assistant")
    );
}

#[test]
fn test_extract_logfire_events_json() {
    let events_json = r#"[
        {"event.name":"gen_ai.system.message","content":"You are helpful.","role":"system"},
        {"event.name":"gen_ai.user.message","content":"Hello!","role":"user"},
        {"event.name":"gen_ai.assistant.message","content":"Hi there!","role":"assistant"}
    ]"#;
    let attrs = make_attrs(&[("events", events_json)]);
    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    assert!(found);
    assert_eq!(messages.len(), 3);
}

#[test]
fn test_extract_message_from_event() {
    let event = Event {
        name: "gen_ai.user.message".to_string(),
        time_unix_nano: 1704067200000000000,
        attributes: vec![make_kv("content", "Hello!")],
        dropped_attributes_count: 0,
    };

    let msgs = extract_message_from_event(&event, "", &HashMap::new(), false);
    assert_eq!(msgs.len(), 1);
    let msg = &msgs[0];

    // Raw format preserves literal attributes only (no metadata)
    assert_eq!(msg.content.get("content"), Some(&json!("Hello!")));
    // Event name is in source, not content
    assert!(
        matches!(msg.source, MessageSource::Event { ref name, .. } if name == "gen_ai.user.message")
    );
}

#[test]
fn test_extract_openinference_messages() {
    let attrs = make_attrs(&[
        ("llm.input_messages.0.message.role", "user"),
        ("llm.input_messages.0.message.content", "What is 2+2?"),
        ("llm.output_messages.0.message.role", "assistant"),
        ("llm.output_messages.0.message.content", "4"),
    ]);
    let mut messages = Vec::new();
    let found = try_openinference(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());
    assert!(found);
    assert_eq!(messages.len(), 2);
}
