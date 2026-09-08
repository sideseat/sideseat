//! Tests for message extraction

use std::collections::HashMap;

use chrono::Utc;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::span::Event;
use serde_json::json;

use super::*;

// Cross-module imports for integration tests
use crate::domain::traces::extract::attributes::{SpanData, extract_genai, extract_semantic};

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
    extract_semantic(&mut span, &attrs);

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
    extract_semantic(&mut span, &attrs);

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
    extract_genai(&mut span, &attrs, "test_span");

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
    extract_genai(&mut span, &attrs, "test_span");

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
    // Full conversation history - wrapped as context message for proper normalization
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

    // Wrapped as context message with conversation_history type
    let msg = &messages[0].content;
    assert!(msg.is_object(), "Should be wrapped as message object");
    assert_eq!(msg["role"].as_str(), Some("context"));
    assert_eq!(msg["type"].as_str(), Some("conversation_history"));

    // The conversation array is in the content field
    let content = &msg["content"];
    assert!(
        content.is_array(),
        "Content should be the conversation array"
    );
    assert_eq!(
        content.as_array().unwrap().len(),
        4,
        "Should have 4 messages"
    );
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
    extract_genai(&mut span, &attrs, "tool_call");

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
    extract_semantic(&mut span, &attrs);

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
    assert!(assistant_msg.content.get("tool.result").is_some());

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
fn test_traceloop_entity_input() {
    let attrs = make_attrs(&[(
        "traceloop.entity.input",
        r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_traceloop(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from traceloop.entity.input");
    assert!(!messages.is_empty());
}

#[test]
fn test_traceloop_entity_output() {
    let attrs = make_attrs(&[("traceloop.entity.output", r#"{"response":"Hi there!"}"#)]);

    let mut messages = Vec::new();
    let found = try_traceloop(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from traceloop.entity.output");
    assert!(!messages.is_empty());
}

#[test]
fn test_user_id_from_ai_telemetry_metadata() {
    let attrs = make_attrs(&[("ai.telemetry.metadata.userId", "user-67890")]);

    let mut span = SpanData::default();
    extract_semantic(&mut span, &attrs);

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
    let span_attrs = crate::utils::otlp::extract_attributes(&otlp_span.attributes);

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
    let span_attrs = crate::utils::otlp::extract_attributes(&otlp_span.attributes);

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

#[test]
fn test_autogen_tool_call_summary_nested_is_skipped() {
    // ToolCallSummaryMessage in nested {"message": {...}} format (autogen process spans).
    // Must be skipped — it concatenates tool results as Python repr() noise.
    let message_json = r#"{"message":{"id":"test-id","source":"weather_assistant","models_usage":null,"metadata":{},"created_at":"2026-01-01T00:00:00Z","content":"{'status': 'success', 'content': [{'json': {'city': 'NYC'}}]}","type":"ToolCallSummaryMessage","tool_calls":[],"results":[]}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "try_autogen should claim the span");
    assert_eq!(
        messages.len(),
        0,
        "ToolCallSummaryMessage should produce no messages"
    );
}

#[test]
fn test_autogen_tool_call_summary_direct_is_skipped() {
    // ToolCallSummaryMessage in direct format (no nesting)
    let message_json =
        r#"{"type":"ToolCallSummaryMessage","source":"agent","content":"tool result text"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "try_autogen should claim the span");
    assert_eq!(
        messages.len(),
        0,
        "ToolCallSummaryMessage should produce no messages"
    );
}

#[test]
fn test_autogen_tool_execution_event_full_pipeline() {
    // Simulate the full extractor pipeline with real AutoGen process span attributes
    let message_json = r#"{"message":{"id":"4c7c875e","source":"memory_code_assistant","models_usage":null,"metadata":{},"created_at":"2026-02-12T01:18:52.894771Z","content":[{"content":"49\n","name":"execute_python_code","call_id":"toolu_01PtmR8iwDwcKnnJ7L12Lg1j","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[
        ("message", message_json),
        ("messaging.destination", "RoundRobinGroupChatManager_xyz"),
        ("messaging.operation", "process"),
        ("recipient_agent_class", "RoundRobinGroupChatManager"),
        ("recipient_agent_type", "RoundRobinGroupChatManager_xyz"),
        ("sender_agent_class", "ChatAgentContainer"),
        ("sender_agent_type", "memory_code_assistant_xyz"),
    ]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut tool_defs,
        &attrs,
        "autogen process RoundRobinGroupChatManager_xyz",
        Utc::now(),
        ExtractionMode::FirstMatch,
        false,
    );

    assert_eq!(
        messages.len(),
        1,
        "ToolCallExecutionEvent should extract 1 tool_result message via full pipeline"
    );
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
}

#[test]
fn test_autogen_tool_execution_event_via_extract_messages_for_span() {
    // Full end-to-end test using extract_messages_for_span (the actual ingestion entry point)
    use opentelemetry_proto::tonic::trace::v1::Span;

    let message_json = r#"{"message":{"id":"4c7c875e","source":"memory_code_assistant","models_usage":null,"metadata":{},"created_at":"2026-02-12T01:18:52.894771Z","content":[{"content":"49\n","name":"execute_python_code","call_id":"toolu_01PtmR8iwDwcKnnJ7L12Lg1j","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;

    let otlp_span = Span {
        name: "autogen process RoundRobinGroupChatManager_xyz".to_string(),
        attributes: vec![
            make_kv("message", message_json),
            make_kv("messaging.destination", "RoundRobinGroupChatManager_xyz"),
            make_kv("messaging.operation", "process"),
            make_kv("recipient_agent_class", "RoundRobinGroupChatManager"),
            make_kv("recipient_agent_type", "RoundRobinGroupChatManager_xyz"),
            make_kv("sender_agent_class", "ChatAgentContainer"),
            make_kv("sender_agent_type", "memory_code_assistant_xyz"),
        ],
        events: vec![],
        ..Default::default()
    };

    let span_attrs = crate::utils::otlp::extract_attributes(&otlp_span.attributes);
    let (messages, _tool_defs, _tool_names) = extract_messages_for_span(
        &otlp_span,
        &span_attrs,
        Utc::now(),
        ExtractionMode::FirstMatch,
    );

    assert_eq!(
        messages.len(),
        1,
        "extract_messages_for_span should extract 1 message from ToolCallExecutionEvent. Got: {:?}",
        messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
}

// ============================================================================
// Logfire request_data / response_data extraction
// ============================================================================

#[test]
fn test_logfire_chat_completions_request_response() {
    let attrs = make_attrs(&[
        (
            "request_data",
            r#"{"messages":[{"role":"user","content":"Hello"}],"model":"gpt-4o"}"#,
        ),
        (
            "response_data",
            r#"{"message":{"role":"assistant","content":"Hi!"},"usage":{"prompt_tokens":5}}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from request_data/response_data");
    assert_eq!(messages.len(), 2, "Should have request + response messages");

    // request_data stored as-is (with messages wrapper)
    assert!(
        matches!(&messages[0].source, MessageSource::Attribute { key, .. } if key == "request_data")
    );
    assert!(messages[0].content.get("messages").is_some());

    // response_data stored as-is (with message wrapper)
    assert!(
        matches!(&messages[1].source, MessageSource::Attribute { key, .. } if key == "response_data")
    );
    assert!(messages[1].content.get("message").is_some());
}

#[test]
fn test_logfire_responses_api_skipped() {
    // Responses API: request_data has no messages array
    let attrs = make_attrs(&[("request_data", r#"{"model":"gpt-4o","stream":true}"#)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        !found,
        "Should not extract from Responses API request_data (no messages)"
    );
    assert!(messages.is_empty());
}

#[test]
fn test_logfire_events_take_precedence() {
    // When events attribute is present, request_data/response_data should be skipped
    let events_json = r#"[{"event.name":"gen_ai.user.message","content":"Hello!"}]"#;
    let attrs = make_attrs(&[
        ("events", events_json),
        (
            "request_data",
            r#"{"messages":[{"role":"user","content":"Hello!"}]}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(
        messages.len(),
        1,
        "Should only have event-based message, not request_data"
    );
    assert!(
        matches!(&messages[0].source, MessageSource::Event { name, .. } if name == "gen_ai.user.message"),
        "Message should come from events, not request_data"
    );
}

#[test]
fn test_logfire_request_only() {
    // Only request_data, no response_data
    let attrs = make_attrs(&[(
        "request_data",
        r#"{"messages":[{"role":"user","content":"Hello"}],"model":"gpt-4o"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1, "Should have only request_data message");
}

#[test]
fn test_logfire_empty_messages_skipped() {
    // request_data with empty messages array should be skipped
    let attrs = make_attrs(&[("request_data", r#"{"messages":[],"model":"gpt-4o"}"#)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found, "Should not extract when messages array is empty");
    assert!(messages.is_empty());
}

#[test]
fn test_logfire_streaming_response() {
    // Streaming response_data with combined_chunk_content
    let attrs = make_attrs(&[(
        "response_data",
        r#"{"combined_chunk_content":"Hello from streaming!","chunk_count":5}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract streaming response_data");
    assert_eq!(messages.len(), 1);
    assert!(
        matches!(&messages[0].source, MessageSource::Attribute { key, .. } if key == "response_data")
    );
    assert!(messages[0].content.get("combined_chunk_content").is_some());
}

// ============================================================================
// CLAUDE CODE CLI (CLAUDE AGENT SDK)
// ============================================================================

#[test]
fn test_claude_code_strips_bracket_tags() {
    assert_eq!(
        strip_bracket_tag("[USER PROMPT]\nhello there"),
        "hello there"
    );
    assert_eq!(
        strip_bracket_tag("[TOOL INPUT: Glob]\n{\"pattern\":\"*.py\"}"),
        "{\"pattern\":\"*.py\"}"
    );
    // No marker, and a bracket that is not a marker, both pass through.
    assert_eq!(strip_bracket_tag("plain text"), "plain text");
    assert_eq!(strip_bracket_tag("[not a marker"), "[not a marker");
}

#[test]
fn test_claude_code_llm_request_messages() {
    // Verbatim shape captured from a real detailed-beta-tracing span.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        ("user_system_prompt", "Think the problem through."),
        (
            "new_context",
            "[USER PROMPT]\nHow many Python files are here?",
        ),
        ("response.model_output", "There are 2 Python files."),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));

    let roles: Vec<_> = messages
        .iter()
        .map(|m| m.content["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant"]);
    assert_eq!(
        messages[1].content["content"],
        json!("How many Python files are here?")
    );
    assert_eq!(
        messages[2].content["content"],
        json!("There are 2 Python files.")
    );
}

#[test]
fn test_claude_code_tool_call_becomes_tool_use_block() {
    let attrs = make_attrs(&[
        ("span.type", "tool"),
        ("tool_name", "Glob"),
        (
            "tool_input",
            "[TOOL INPUT: Glob]\n{\"pattern\":\"**/*.py\"}",
        ),
        ("tool_use_id", "toolu_abc123"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.tool",
        Utc::now()
    ));

    assert_eq!(messages.len(), 1);
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], json!("tool_use"));
    assert_eq!(block["name"], json!("Glob"));
    assert_eq!(block["id"], json!("toolu_abc123"));
    assert_eq!(block["input"]["pattern"], json!("**/*.py"));
}

#[test]
fn test_claude_code_requires_span_type_guard() {
    // tool_name and span.type are both generic names; only the claude_code. span-name
    // prefix gates this extractor, so other frameworks are never hijacked.
    let attrs = make_attrs(&[
        ("span.type", "tool"),
        ("tool_name", "Glob"),
        ("response.model_output", "hi"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "some.other.span",
        Utc::now()
    ));
    assert!(messages.is_empty());
}

#[test]
fn test_claude_code_ignores_redacted_and_empty_values() {
    // With no content gates set the CLI still emits span.type, but nothing usable.
    let attrs = make_attrs(&[("span.type", "llm_request"), ("new_context", "   ")]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));
    assert!(messages.is_empty());
}

#[test]
fn test_claude_code_tool_output_event_is_a_message_event() {
    assert!(is_message_event("tool.output"));
}

#[test]
fn test_claude_code_new_context_tag_drives_role() {
    let now = Utc::now();

    // Id-tagged TOOL RESULT becomes a tool_result block linked by tool_use_id.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        (
            "new_context",
            "[TOOL RESULT: toolu_bdrk_01GJ]\nconfig.py\ninventory.py",
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        now
    ));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"], json!("tool"));
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], json!("tool_result"));
    assert_eq!(block["tool_use_id"], json!("toolu_bdrk_01GJ"));
    assert_eq!(block["content"], json!("config.py\ninventory.py"));

    // Name-tagged TOOL RESULT is the structured duplicate and is skipped.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        ("new_context", "[TOOL RESULT: Glob]\n{\"numFiles\":2}"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "s",
        now
    ));
    assert!(messages.is_empty());

    // Both user tags stay user.
    for tag in ["USER PROMPT", "USER"] {
        let attrs = make_attrs(&[
            ("span.type", "llm_request"),
            ("new_context", &format!("[{tag}]\nhello")),
        ]);
        let mut messages = Vec::new();
        let mut tools = Vec::new();
        assert!(try_claude_code(
            &mut messages,
            &mut tools,
            &attrs,
            "claude_code.llm_request",
            now
        ));
        assert_eq!(messages[0].content["role"], json!("user"), "tag {tag}");
        assert_eq!(messages[0].content["content"], json!("hello"));
    }
}

#[test]
fn test_claude_code_splits_parallel_tool_results() {
    // Verbatim shape from a subagents run: two parallel tool calls put both results
    // in one new_context. Each must keep its own tool_use_id.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        (
            "new_context",
            "[TOOL RESULT: toolu_first]\norders.ts\nshipping.ts\n\n---\n\n\
             [TOOL RESULT: toolu_second]\nNo files found",
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));

    assert_eq!(messages.len(), 2);
    let ids: Vec<_> = messages
        .iter()
        .map(|m| m.content["content"][0]["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["toolu_first", "toolu_second"]);
    assert_eq!(
        messages[0].content["content"][0]["content"],
        json!("orders.ts\nshipping.ts")
    );
    assert_eq!(
        messages[1].content["content"][0]["content"],
        json!("No files found")
    );
    // No leaked marker from the second section.
    for m in &messages {
        let body = m.content["content"][0]["content"].as_str().unwrap();
        assert!(!body.contains("[TOOL RESULT"), "leaked marker in {body:?}");
    }
}

#[test]
fn test_claude_code_unknown_tool_id_prefix_degrades_instead_of_dropping() {
    // If Anthropic ever changes the tool-use id prefix, a TOOL RESULT section must
    // still reach the feed (unlinked) rather than being silently discarded — silent
    // data loss is a far worse failure than a missing tool_use_id.
    let attrs = make_attrs(&[(
        "new_context",
        "[TOOL RESULT: xyz_9f2b]\nconfig.py\ninventory.py",
    )]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"], json!("tool"));
    assert_eq!(
        messages[0].content["content"][0]["content"],
        json!("config.py\ninventory.py")
    );
}

/// The current conventions' tool attributes become a call and a result, with the name and the id.
///
/// `gen_ai.tool.call.arguments` / `.result` are how the present OTel GenAI conventions report a tool call -
/// the Vercel AI SDK's current integration and Microsoft Agent Framework both use them - and they appear on
/// a *tool* span, where attribute extraction used to be skipped wholesale. Read but stripped of the name and
/// id they sit beside, they became a nameless call the pipeline discarded, which is worse than not reading
/// them: every layer looked healthy and the tool call was gone.
#[test]
fn semconv_tool_attributes_become_a_named_correlated_pair() {
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "execute_tool"),
        ("gen_ai.tool.name", "calculator"),
        ("gen_ai.tool.call.id", "call_1"),
        ("gen_ai.tool.call.arguments", r#"{"expression":"2+2"}"#),
        ("gen_ai.tool.call.result", r#"{"value":4}"#),
    ]);

    let mut messages = Vec::new();
    assert!(try_otel_genai_messages(
        &mut messages,
        &mut Vec::new(),
        &attrs,
        "execute_tool calculator",
        Utc::now()
    ));
    assert_eq!(messages.len(), 2, "a call and its result");

    let call = &messages[0].content["content"][0];
    assert_eq!(call["type"], "tool_use");
    assert_eq!(
        call["name"], "calculator",
        "the tool's name is beside the arguments; use it"
    );
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["input"]["expression"], "2+2");

    let result = &messages[1].content["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(
        result["tool_use_id"], "call_1",
        "the result names the call, so the pair survives dedup as a pair"
    );
    assert_eq!(result["content"]["value"], 4);
}

// ============================================================================
// MESSAGE-RULE EQUIVALENCE: the declared rules against the extractors they replaced
// ============================================================================

use crate::domain::rules::message_rules::compile;
use crate::domain::rules::{MessageContext, ruleset, schema};

fn rule_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The rules produce **semantically** what the functions they replaced produced.
///
/// Semantic, not byte: the comparison canonicalises object keys (see `canonical` below), because for an
/// indexed family the baseline had no member order to reproduce - it walked a randomised `HashMap`. So this
/// oracle does not police serialised member order, and the goldens are what catch a change to that: they
/// record the content string, and they are what failed when a projection language re-ordered a provider's
/// payload.
///
/// Compared as *serialised observations*, not counts: a rule that emitted the right number of messages
/// with the wrong carrier tag, the wrong role envelope or an unparsed payload would pass a count check
/// and corrupt every downstream view, since the carrier tag is what claiming, ordering and dedup all key
/// on.
#[test]
fn the_rules_reproduce_the_extractors_they_replaced() {
    let time = chrono::Utc::now();
    // Every carrier the migrated rules read, in the shapes that distinguish the parse modes: valid JSON,
    // an object, a bare string that is not JSON, and a numeric scalar.
    // A span *name* per case, because a rule may be gated on it - and a gated rule compared under a
    // name it cannot match proves nothing at all.
    let cases: Vec<(&str, HashMap<String, String>)> = vec![
        // The richer copy joined on by position: the flattened form redacts a url, and the serialised copy
        // of the same conversation on the same span keeps it. The list opens with a member whose `id` is a
        // *string*, so the witness - "some member states a class path, which this dialect writes as an
        // array" - has to be asked of every match. Asked of the first only, the overlay would not fire and
        // the redacted url would stand, which is what the retired code's `.any()` avoided.
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.contents.0.message_content.type",
                    "text",
                ),
                (
                    "llm.input_messages.0.message.contents.0.message_content.text",
                    "look",
                ),
                ("llm.input_messages.1.message.role", "user"),
                (
                    "llm.input_messages.1.message.contents.0.message_content.type",
                    "image",
                ),
                (
                    "llm.input_messages.1.message.contents.0.message_content.image.image.url",
                    "__REDACTED__",
                ),
                (
                    "input.value",
                    r#"{"messages": [[{"id": "a string, not a class path", "content": "first"}, {"id": ["langchain", "schema", "messages", "HumanMessage"], "content": [{"type": "text", "text": "look"}, {"type": "image_url", "image_url": {"url": "https://real/img.png"}}]}]]}"#,
                ),
            ]),
        ),
        // The OpenInference dialect: indexed message families, retrieval result sets, and the two
        // single-attribute carriers.
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.content",
                    "what is the weather",
                ),
            ]),
        ),
        // A role with nothing to show is not a turn: an index exists as soon as any key mentions it.
        (
            "span",
            rule_attrs(&[("llm.input_messages.0.message.role", "user")]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "user"),
                (
                    "llm.input_messages.0.message.contents.0.message_content.type",
                    "text",
                ),
                (
                    "llm.input_messages.0.message.contents.0.message_content.text",
                    "see this",
                ),
                ("llm.input_messages.1.message.role", "assistant"),
                (
                    "llm.input_messages.1.message.tool_calls.0.tool_call.id",
                    "c1",
                ),
                (
                    "llm.input_messages.1.message.tool_calls.0.tool_call.function.name",
                    "search",
                ),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "tool"),
                ("llm.input_messages.0.message.tool_call_id", "c1"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("llm.input_messages.0.message.role", "assistant"),
                ("llm.input_messages.0.message.function_call.name", "legacy"),
            ]),
        ),
        // An entry-level member beside the nested message, and a JSON-valued one.
        (
            "span",
            rule_attrs(&[
                ("llm.output_messages.0.message.role", "assistant"),
                ("llm.output_messages.0.message.content", "sunny"),
                ("llm.output_messages.0.finish_reason", "stop"),
                ("llm.output_messages.0.message.extra", r#"{"a":1}"#),
            ]),
        ),
        // Retrieval: one message holding every document, not one each.
        (
            "span",
            rule_attrs(&[
                ("retrieval.documents.0.document.id", "d1"),
                ("retrieval.documents.0.document.content", "first"),
                ("retrieval.documents.0.document.score", "0.9"),
                ("retrieval.documents.0.document.metadata", r#"{"src":"kb"}"#),
                ("retrieval.documents.1.document.content", "second"),
                ("retrieval.documents.1.document.score", "not a number"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("reranker.input_documents.0.document.content", "a"),
                ("reranker.output_documents.0.document.content", "a"),
                ("reranker.query", "which is best"),
            ]),
        ),
        ("span", rule_attrs(&[("embedding.text", "vectorise me")])),
        // The AutoGen dialect, whose messages are typed objects: one case per type, the four
        // selection points it writes them at, and its logging channel.
        // The typed table, one case each.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "SystemMessage", "content": "be brief"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "UserMessage", "content": "hi", "source": "user"}"#,
            )]),
        ),
        // Reasoning beside the reply becomes a thinking block ahead of it.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok", "source": "planner", "thought": "thinking it over"}"#,
            )]),
        ),
        // A null thought is not a thought.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok", "source": "planner", "thought": null}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "AssistantMessage", "content": "ok"}"#,
            )]),
        ),
        // `source` names the speaker, so anything but the user is the assistant.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "TextMessage", "content": "hello", "source": "user"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "TextMessage", "content": "hello", "source": "planner"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "TextMessage", "content": "hello"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "MultiModalMessage", "content": [{"type": "text", "text": "see"}], "source": "critic"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "StopMessage", "content": "done", "source": "user"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "HandoffMessage", "content": "over to you", "source": "planner"}"#,
            )]),
        ),
        // A non-string speaker names nobody.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "HandoffMessage", "content": "over to you", "source": {"not": "a string"}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ThoughtEvent", "content": "pondering"}"#,
            )]),
        ),
        // An empty thought is not one.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "ThoughtEvent", "content": ""}"#)]),
        ),
        // Arguments arrive as serialised JSON as often as an object.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "source": "planner", "content": [{"id": "c1", "name": "search", "arguments": "{\"q\":\"x\"}"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "content": [{"id": "c1", "name": "search", "arguments": {"q": "x"}}]}"#,
            )]),
        ),
        // A call with no id pairs with nothing, so the reading finds none.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallRequestEvent", "content": [{"name": "nameless"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallExecutionEvent", "content": [{"call_id": "c1", "name": "search", "content": "found"}]}"#,
            )]),
        ),
        // The batch's call id is a fallback for a result that carries none.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "FunctionExecutionResultMessage", "call_id": "c9", "content": [{"name": "search", "content": "found"}, {"content": "second"}]}"#,
            )]),
        ),
        // A result with nothing in it at all.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallExecutionEvent", "content": [{}]}"#,
            )]),
        ),
        // Claimed and read for nothing: the results are already reported individually.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "ToolCallSummaryMessage", "content": "repr noise"}"#,
            )]),
        ),
        // A type the table does not know still carries a message.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"type": "SomethingNew", "content": "unknown but usable"}"#,
            )]),
        ),
        // ...and one that does not carries none.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"type": "SomethingNew", "source": "x"}"#)]),
        ),
        // A list, including shapes the table refuses that this point still reads.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"messages": [{"type": "TextMessage", "content": "a", "source": "user"}, {"content": ""}, {"content": "loose"}], "output_task_messages": true}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"message": {"type": "ToolCallRequestEvent", "content": [{"id": "c2", "name": "t", "arguments": {}}]}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"message": {"content": "loose"}}"#)]),
        ),
        // A response carries its reply *and* the turns that produced it.
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"response": {"chat_message": {"type": "TextMessage", "content": "final", "source": "planner"}, "inner_messages": [{"type": "ThoughtEvent", "content": "first"}]}}"#,
            )]),
        ),
        // Untyped, recognised by shape.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": "hi", "source": "planner"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": "hi", "source": "user"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"role": "user", "content": "already a message"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"content": [{"id": "c3", "name": "t", "arguments": {}}], "source": "planner"}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "message",
                r#"{"content": [{"call_id": "c4", "content": "r"}]}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": [{"name": "t", "content": "r"}]}"#)]),
        ),
        // Untyped and empty: nothing to read.
        (
            "autogen process",
            rule_attrs(&[("message", r#"{"content": ""}"#)]),
        ),
        // The two sentinels the carrier uses for absence.
        (
            "autogen process",
            rule_attrs(&[("message", r#"No Message"#)]),
        ),
        ("autogen process", rule_attrs(&[("message", r#"{}"#)])),
        ("autogen process", rule_attrs(&[("message", r#"not json"#)])),
        // An aggregate span's input: framework internals, claimed and read for nothing.
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"cancellation_token":"tok"}"#)]),
        ),
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"output_task_messages":true}"#)]),
        ),
        // ...and one that is not, which this dialect does not claim.
        (
            "autogen process",
            rule_attrs(&[("input.value", r#"{"other":1}"#)]),
        ),
        // The logging channel: one carrier holds the conversation, the reply and the tools.
        (
            "autogen process",
            rule_attrs(&[(
                "body",
                r#"{"type": "LLMCall", "messages": [{"role": "user", "content": "q"}, {"content": ""}], "response": {"content": "a", "tool_calls": [{"id": "c5", "type": "function", "function": {"name": "t", "arguments": {}}}]}, "tools": [{"name": "t"}]}"#,
            )]),
        ),
        // The reply in an OpenAI-shaped choice, with an empty call list that is not one.
        (
            "autogen process",
            rule_attrs(&[(
                "log.body",
                r#"{"type": "LLMStreamEnd", "messages": [{"role": "user", "content": "q"}], "response": {"choices": [{"message": {"content": "a", "tool_calls": []}}]}}"#,
            )]),
        ),
        (
            "autogen process",
            rule_attrs(&[(
                "autogen.event",
                r#"{"type": "ToolCall", "tool_name": "search", "arguments": {"q": "x"}, "result": "found"}"#,
            )]),
        ),
        // A tool execution with nothing recorded but its type.
        (
            "autogen process",
            rule_attrs(&[("autogen.event", r#"{"type": "ToolCall"}"#)]),
        ),
        // An event of another type: the gate is the type, so nothing is read.
        (
            "autogen process",
            rule_attrs(&[(
                "body",
                r#"{"type": "Unrelated", "messages": [{"role": "user", "content": "q"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("traceloop.entity.input", r#"{"a":1}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("traceloop.entity.output", r#"[1,2]"#)]),
        ),
        (
            "span",
            rule_attrs(&[
                ("traceloop.entity.input", r#"{"a":1}"#),
                ("traceloop.entity.output", r#""done""#),
            ]),
        ),
        // Not JSON: `Json` mode must skip it, which is what the legacy `extract_json` did.
        (
            "span",
            rule_attrs(&[("traceloop.entity.input", "not json at all")]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.spanInputs", r#"{"messages":[]}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.spanOutputs", r#"{"choices":[]}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("mlflow.chat.tools", r#"[{"name":"t"}]"#)]),
        ),
        ("span", rule_attrs(&[("mlflow.spanInputs", "unparseable")])),
        // `JsonOrString` mode: the string must survive rather than be dropped.
        (
            "span",
            rule_attrs(&[("tool_arguments", r#"{"city":"NYC"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("tool_arguments", "plain text arguments")]),
        ),
        ("span", rule_attrs(&[("tool_response", r#"{"v":"sunny"}"#)])),
        ("span", rule_attrs(&[("tool_response", "just a string")])),
        ("span", rule_attrs(&[("tool_response", "42")])),
        // All of them at once, which is also the ordering check.
        (
            "span",
            rule_attrs(&[
                ("traceloop.entity.input", r#"{"a":1}"#),
                ("mlflow.spanInputs", r#"{"b":2}"#),
                ("mlflow.chat.tools", r#"[{"name":"t"}]"#),
                ("tool_arguments", r#"{"c":3}"#),
                ("tool_response", "text"),
            ]),
        ),
        // Nothing at all: both must report `false` and emit nothing.
        ("span", rule_attrs(&[("unrelated.key", "x")])),
        // LangSmith: gated, so the same keys without a marker must yield nothing at all.
        (
            "span",
            rule_attrs(&[(
                "gen_ai.prompt",
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"user","content":"hi"}]}"#,
                ),
            ]),
        ),
        // Two turns, and a member of the request that is not a turn: it must not become one.
        (
            "span",
            rule_attrs(&[
                ("langsmith.trace.name", "t"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"system","content":"s"},{"role":"user","content":"u"},
                    {"role":"noContent"}],"temperature":0.5}"#,
                ),
            ]),
        ),
        // A single message written directly, rather than in an array.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.prompt", r#"{"role":"user","content":"direct"}"#),
            ]),
        ),
        // The completion's choices array, with the finish reason beside each message.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},
                    "finish_reason":"stop"}]}"#,
                ),
            ]),
        ),
        // Two choices, one without a finish reason: the lift must be per element.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},"finish_reason":"stop"},
                    {"message":{"role":"assistant","content":"b"}}]}"#,
                ),
            ]),
        ),
        // A response carrying content and no role - which `any_of` admits and `all_of` would drop.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.completion", r#"{"content":"no role here"}"#),
            ]),
        ),
        // Both sides at once, plus a prefix-only marker.
        (
            "span",
            rule_attrs(&[
                ("langsmith.anything", "x"),
                (
                    "gen_ai.prompt",
                    r#"{"messages":[{"role":"user","content":"q"}]}"#,
                ),
                (
                    "gen_ai.completion",
                    r#"{"choices":[{"message":{"role":"assistant","content":"a"},
                    "finish_reason":"length"}]}"#,
                ),
            ]),
        ),
        // Unparseable on both sides: skipped, not stored as prose.
        (
            "span",
            rule_attrs(&[
                ("langsmith.span.kind", "llm"),
                ("gen_ai.prompt", "not json"),
                ("gen_ai.completion", "also not json"),
            ]),
        ),
        // Indexed families. Two turns, in the flattened encoding.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "system"),
                ("gen_ai.prompt.0.content", "be brief"),
                ("gen_ai.prompt.1.role", "user"),
                ("gen_ai.prompt.1.content", "hello"),
            ]),
        ),
        // An index mentioned by a key that is not content: it must not become a turn.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content", "q"),
                ("gen_ai.prompt.1.role", "assistant"),
            ]),
        ),
        // Nested content, which the convention also writes: presence must be satisfied by it.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content.0.text", "nested"),
            ]),
        ),
        // Both families at once, and out of key order - entries must come back by index.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.completion.1.content", "second"),
                ("gen_ai.completion.0.content", "first"),
                ("gen_ai.prompt.0.role", "user"),
                ("gen_ai.prompt.0.content", "q"),
            ]),
        ),
        // A member whose value opens as JSON, and one that merely contains a brace.
        (
            "span",
            rule_attrs(&[
                (
                    "gen_ai.completion.0.content",
                    r#"[{"type":"text","text":"a"}]"#,
                ),
                ("gen_ai.completion.0.finish_reason", "stop"),
                ("gen_ai.completion.0.note", "not {json} really"),
            ]),
        ),
        // Opens as JSON and does not parse: the raw text must survive.
        (
            "span",
            rule_attrs(&[("gen_ai.completion.0.content", "{ truncated")]),
        ),
        // A double-digit index, so the parse is not one character wide.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.prompt.10.role", "user"),
                ("gen_ai.prompt.10.content", "tenth"),
            ]),
        ),
        // A family with no index at all.
        ("span", rule_attrs(&[("gen_ai.prompt.content", "no index")])),
        // LiveKit: prose, and the empty-value skip.
        ("span", rule_attrs(&[("lk.instructions", "be brief")])),
        ("span", rule_attrs(&[("lk.instructions", "")])),
        // Either key for the user's turn, and the tag must be the one found.
        ("span", rule_attrs(&[("lk.user_input", "hello")])),
        ("span", rule_attrs(&[("lk.input_text", "hello")])),
        (
            "span",
            rule_attrs(&[("lk.user_input", "first"), ("lk.input_text", "second")]),
        ),
        ("span", rule_attrs(&[("lk.input_text", "")])),
        // A literal member naming what kind of context a payload is.
        (
            "span",
            rule_attrs(&[("lk.chat_ctx", r#"[{"role":"user","content":"x"}]"#)]),
        ),
        // Definitions rather than a message.
        (
            "span",
            rule_attrs(&[("lk.function_tools", r#"[{"name":"weather"}]"#)]),
        ),
        // A call written across three attributes.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.arguments", r#"{"city":"NYC"}"#),
                ("lk.function_tool.name", "weather"),
                ("lk.function_tool.id", "call_1"),
            ]),
        ),
        // The same, with no name or id: the attachments must simply be absent.
        (
            "span",
            rule_attrs(&[("lk.function_tool.arguments", "raw args")]),
        ),
        // The error flag, present and true.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.output", r#"{"v":1}"#),
                ("lk.function_tool.name", "weather"),
                ("lk.function_tool.is_error", "true"),
            ]),
        ),
        // Present and false: the literal must not attach.
        (
            "span",
            rule_attrs(&[
                ("lk.function_tool.output", "failed"),
                ("lk.function_tool.is_error", "false"),
            ]),
        ),
        // A reply with tool calls of the same response attached.
        (
            "span",
            rule_attrs(&[
                ("lk.response.text", "here you go"),
                ("lk.response.function_calls", r#"[{"name":"t"}]"#),
            ]),
        ),
        // A reply with no calls.
        ("span", rule_attrs(&[("lk.response.text", "just text")])),
        // Calls with no text: one message, under `tool_calls` rather than `content`.
        (
            "span",
            rule_attrs(&[("lk.response.function_calls", r#"[{"name":"t"}]"#)]),
        ),
        // Empty text beside calls - the either/or is on *presence*, matching the legacy reading.
        (
            "span",
            rule_attrs(&[
                ("lk.response.text", ""),
                ("lk.response.function_calls", r#"[{"name":"t"}]"#),
            ]),
        ),
        // The conventions themselves: request and response kept whole.
        (
            "span",
            rule_attrs(&[("gen_ai.input.messages", r#"[{"role":"user","parts":[]}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gen_ai.output.messages",
                r#"[{"role":"assistant","parts":[]}]"#,
            )]),
        ),
        // A structured instruction, which goes under `parts` rather than `content`.
        (
            "span",
            rule_attrs(&[(
                "gen_ai.system_instructions",
                r#"[{"type":"text","content":"be brief"}]"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("gen_ai.system_instructions", "not json")]),
        ),
        // A whole conversation on an agent-run span.
        (
            "span",
            rule_attrs(&[("pydantic_ai.all_messages", r#"[{"role":"user"}]"#)]),
        ),
        // A tool call as a block, with the name and id beside it.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.arguments", r#"{"city":"NYC"}"#),
                ("gen_ai.tool.name", "weather"),
                ("gen_ai.tool.call.id", "call_1"),
            ]),
        ),
        // No name attribute: the empty default must be present, not the member omitted.
        (
            "span",
            rule_attrs(&[("gen_ai.tool.call.arguments", r#"{"city":"NYC"}"#)]),
        ),
        // The result, where an absent name is *omitted* rather than defaulted.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.result", r#"{"v":"sunny"}"#),
                ("gen_ai.tool.call.id", "call_1"),
            ]),
        ),
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.result", "plain result"),
                ("gen_ai.tool.name", "weather"),
            ]),
        ),
        // Both halves of the pair on one span.
        (
            "span",
            rule_attrs(&[
                ("gen_ai.tool.call.arguments", r#"{"a":1}"#),
                ("gen_ai.tool.call.result", r#"{"b":2}"#),
                ("gen_ai.tool.name", "t"),
                ("gen_ai.tool.call.id", "id1"),
            ]),
        ),
        // Vercel: the prompt under each spelling, both tagged canonically.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[("ai.prompt", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        // Non-object elements in the array must be skipped, not emitted as turns.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"[{"role":"user"},"stray",42]"#)]),
        ),
        // Not an array at all.
        (
            "span",
            rule_attrs(&[("ai.prompt.messages", r#"{"role":"user"}"#)]),
        ),
        // A tool call across three attributes.
        (
            "span",
            rule_attrs(&[
                ("ai.toolCall.args", r#"{"city":"NYC"}"#),
                ("ai.toolCall.name", "weather"),
                ("ai.toolCall.id", "c1"),
            ]),
        ),
        // The composed response: current spellings.
        (
            "span",
            rule_attrs(&[
                ("ai.response.text", "the answer"),
                ("ai.response.toolCalls", r#"[{"name":"t"}]"#),
                ("ai.response.finishReason", "stop"),
            ]),
        ),
        // Legacy spellings, which must read identically.
        (
            "span",
            rule_attrs(&[
                ("ai.result.text", "legacy answer"),
                ("ai.result.toolCalls", r#"[{"name":"t"}]"#),
                ("ai.result.object", r#"{"k":1}"#),
            ]),
        ),
        // A structured object under the current name, plus a swept member.
        (
            "span",
            rule_attrs(&[
                ("ai.response.object", r#"{"k":1}"#),
                ("ai.response.id", "resp-1"),
                ("ai.response.model", "m"),
            ]),
        ),
        // The gated fallback: `output.value` is read only on evidence this is such a span.
        (
            "span",
            rule_attrs(&[("output.value", "generic"), ("ai.prompt.messages", "[]")]),
        ),
        (
            "span",
            rule_attrs(&[("output.value", "generic"), ("ai.toolCall.name", "t")]),
        ),
        // No evidence at all: the fallback must not claim the generic carrier.
        ("span", rule_attrs(&[("output.value", "generic")])),
        // Nothing of the family: no response message at all, not one holding only a role.
        (
            "span",
            rule_attrs(&[("unrelated", "x"), ("ai.somethingElse", "y")]),
        ),
        // Claude Code, under a span name its rules can match - the gate is the point of the name column.
        (
            "claude_code.interaction",
            rule_attrs(&[("user_system_prompt", "   ")]),
        ),
        (
            "claude_code.interaction",
            rule_attrs(&[("user_system_prompt", "be terse")]),
        ),
        (
            "claude_code.llm_request",
            rule_attrs(&[("response.model_output", "the reply")]),
        ),
        // The same attributes under an unrelated span name: the gate must withhold them.
        ("span", rule_attrs(&[("user_system_prompt", "be terse")])),
        // One tagged section: the user turn.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[USER PROMPT]\nwhat is 2+2")]),
        ),
        // An untagged body is still the user's turn, not discarded.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "bare text with no tag")]),
        ),
        // A bracket with no newline is not a tag - it must not swallow the first line.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[not a tag] still text")]),
        ),
        // A tool result, id-tagged: the id is what pairs it with its call.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: toolu_abc]\nthe file contents")]),
        ),
        // The name-tagged structured duplicate: dropped, narrowly.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: Read]\n{\"path\":\"a\"}")]),
        ),
        // Name-tagged but *not* JSON: an unrecognised section must still reach the feed.
        (
            "claude_code.interaction",
            rule_attrs(&[("new_context", "[TOOL RESULT: Read]\nplain text result")]),
        ),
        // Several sections in one attribute, one per parallel call, each keeping its own id.
        (
            "claude_code.interaction",
            rule_attrs(&[(
                "new_context",
                "[TOOL RESULT: toolu_1]\nfirst\n\n---\n\n[TOOL RESULT: toolu_2]\nsecond",
            )]),
        ),
        // A section whose body is empty is skipped.
        (
            "claude_code.interaction",
            rule_attrs(&[(
                "new_context",
                "[USER PROMPT]\n\n\n---\n\n[USER PROMPT]\nreal",
            )]),
        ),
        // A tool call: the name is the read carrier, the input stripped of its tag then parsed.
        (
            "claude_code.tool",
            rule_attrs(&[
                ("tool_name", "Read"),
                ("tool_input", "[TOOL INPUT: Read]\n{\"path\":\"/tmp/a\"}"),
                ("tool_use_id", "toolu_9"),
            ]),
        ),
        // Unparseable input falls back to the empty form, because the block's shape requires it.
        (
            "claude_code.tool",
            rule_attrs(&[("tool_name", "Read"), ("tool_input", "not json")]),
        ),
        // No input attribute at all.
        ("claude_code.tool", rule_attrs(&[("tool_name", "Bash")])),
        // A blank name is absence: no call.
        (
            "claude_code.tool",
            rule_attrs(&[("tool_name", "  "), ("tool_use_id", "toolu_1")]),
        ),
        // CrewAI: gated, so the same payload without a marker yields nothing.
        (
            "span",
            rule_attrs(&[("output.value", r#"{"raw":"the answer"}"#)]),
        ),
        // The answer alone.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("output.value", r#"{"raw":"the answer"}"#),
            ]),
        ),
        // History *and* the answer - the case that reading them as alternatives lost.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q"},
                        {"role":"assistant","content":"partial"}],"raw":"the answer"}"#,
                ),
            ]),
        ),
        // A task's own turns, two levels down.
        (
            "span",
            rule_attrs(&[
                ("task_key", "t"),
                (
                    "output.value",
                    r#"{"tasks_output":[{"messages":[{"role":"user","content":"a"}]},
                        {"messages":[{"role":"assistant","content":"b"}]}]}"#,
                ),
            ]),
        ),
        // A member of a task that is not a turn must not become one.
        (
            "span",
            rule_attrs(&[
                ("crew_id", "c"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q"},{"role":"noContent"},
                        {"content":"noRole"},{"role":"a","tool_calls":[]}]}"#,
                ),
            ]),
        ),
        // A blank answer is not an answer, and with no turns either the payload is kept whole.
        (
            "span",
            rule_attrs(&[("crew_key", "k"), ("output.value", r#"{"raw":"   "}"#)]),
        ),
        // No documented shape at all: kept whole rather than leaving the span empty.
        (
            "span",
            rule_attrs(&[("crew_key", "k"), ("output.value", r#"{"other":1}"#)]),
        ),
        // The task definitions.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("crew_tasks", r#"[{"name":"research"}]"#),
            ]),
        ),
        // Unparseable output: skipped, and the task list still read.
        (
            "span",
            rule_attrs(&[
                ("crew_key", "k"),
                ("crew_tasks", r#"[{"name":"n"}]"#),
                ("output.value", "not json"),
            ]),
        ),
        // Logfire: recognised events, each tagged with its own name.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"event.name":"gen_ai.user.message","content":"q"},
                    {"event.name":"gen_ai.choice","content":"a"}]"#,
            )]),
        ),
        // Unnamed events carrying multimodal blocks, grouped into runs by side.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"data":{"type":"input_image","url":"u"}},
                    {"data":{"type":"output_text","text":"b"}}]"#,
            )]),
        ),
        // Mixed: recognised events must all precede the grouped blocks.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"event.name":"gen_ai.choice","content":"answer"},
                    {"data":{"type":"output_text","text":"b"}}]"#,
            )]),
        ),
        // A run returning to a previous side is a new run, not a merge.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"input_text","text":"a"}},
                    {"data":{"type":"output_text","text":"b"}},
                    {"data":{"type":"input_text","text":"c"}}]"#,
            )]),
        ),
        // A block whose type matches neither side is skipped.
        (
            "span",
            rule_attrs(&[(
                "events",
                r#"[{"data":{"type":"other"}},{"data":{"type":"input_text","text":"a"}}]"#,
            )]),
        ),
        // The prompt and the whole-conversation carriers.
        (
            "span",
            rule_attrs(&[("prompt", r#"[{"role":"user","content":"q"}]"#)]),
        ),
        (
            "span",
            rule_attrs(&[(
                "all_messages_events",
                r#"[{"role":"assistant","content":"a"}]"#,
            )]),
        ),
        // The response, in both documented shapes.
        (
            "span",
            rule_attrs(&[(
                "response_data",
                r#"{"message":{"role":"assistant","content":"a"}}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("response_data", r#"{"combined_chunk_content":"streamed"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("response_data", r#"{"combined_chunk_content":""}"#)]),
        ),
        // The request payload: a fallback, read only when nothing else carried the conversation.
        (
            "span",
            rule_attrs(&[(
                "request_data",
                r#"{"messages":[{"role":"user","content":"q"}]}"#,
            )]),
        ),
        // With the events present, the request payload must not be read as well.
        (
            "span",
            rule_attrs(&[
                (
                    "events",
                    r#"[{"event.name":"gen_ai.user.message","content":"q"}]"#,
                ),
                (
                    "request_data",
                    r#"{"messages":[{"role":"user","content":"q"}]}"#,
                ),
            ]),
        ),
        // An empty request payload is not a conversation.
        (
            "span",
            rule_attrs(&[("request_data", r#"{"messages":[]}"#)]),
        ),
        // ADK: the request, with both spellings of its members.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"systemInstruction":{"parts":[{"text":"be brief"}]},
                    "contents":[{"role":"user","parts":[{"text":"q"}]},
                                {"role":"model","parts":[{"text":"a"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"systemInstruction":"a bare string"}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"config":{"system_instruction":"the framework's own spelling"}}"#,
            )]),
        ),
        // An empty request is not a request.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.llm_request", "{}")]),
        ),
        // Tools: wrapped declarations under each spelling, and a bare group.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"function_declarations":[{"name":"a"},{"name":"b"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"functionDeclarations":[{"name":"c"}]}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"config":{"tools":[{"name":"bare"}]}}"#,
            )]),
        ),
        // A wrapped group beside a bare one: the per-element decision.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"tools":[{"function_declarations":[{"name":"a"}]},{"name":"bare"}]}"#,
            )]),
        ),
        // The response: the provider's role alias and the finish reason beside the content.
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_response",
                r#"{"content":{"role":"model","parts":[{"text":"a"}]},"finish_reason":"STOP"}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_response",
                r#"{"content":{"role":"user","parts":[]}}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.llm_response", "{}")]),
        ),
        // The arguments fallback: read only when the request supplied nothing.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.tool_call_args", r#"{"city":"NYC"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[
                (
                    "gcp.vertex.agent.llm_request",
                    r#"{"contents":[{"role":"user","parts":[]}]}"#,
                ),
                ("gcp.vertex.agent.tool_call_args", r#"{"city":"NYC"}"#),
            ]),
        ),
        // A tool's result, and the conversation handed to an agent.
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.tool_response", r#"{"v":"sunny"}"#)]),
        ),
        (
            "span",
            rule_attrs(&[("gcp.vertex.agent.data", r#"[{"role":"user"}]"#)]),
        ),
        ("span", rule_attrs(&[("gcp.vertex.agent.data", "{}")])),
        ("span", rule_attrs(&[("gcp.vertex.agent.data", "[]")])),
        // LangGraph: a state object, gated.
        (
            "span",
            rule_attrs(&[(
                "output.value",
                r#"{"messages":[{"type":"human","content":"q"}]}"#,
            )]),
        ),
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "agent"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"human","content":"q"},
                    {"type":"ai","content":"a"}]}"#,
                ),
            ]),
        ),
        // The answer beside the conversation - the shape that once lost every reply.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "agent"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"human","content":"q"}],"raw":"the answer"}"#,
                ),
            ]),
        ),
        // Serialised form: the discriminator and content under the constructor's kwargs.
        (
            "span",
            rule_attrs(&[
                ("langgraph.thread_id", "t"),
                (
                    "output.value",
                    r#"{"messages":[{"lc":{"type":"ai"},"kwargs":{"content":"a",
                    "tool_calls":[{"name":"t"}]}}]}"#,
                ),
            ]),
        ),
        // An empty tool-call list must not attach.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"ai","content":"a","tool_calls":[]}]}"#,
                ),
            ]),
        ),
        // A tool reply, with its call id and name.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"type":"tool","content":"r",
                    "tool_call_id":"c1","name":"temp"}]}"#,
                ),
            ]),
        ),
        // Nested state, which the walk exists for.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"state":{"messages":[{"type":"human","content":"q"}]}}"#,
                ),
            ]),
        ),
        // Already canonical: kept exactly as it arrived.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                (
                    "output.value",
                    r#"{"messages":[{"role":"user","content":"q","extra":1}]}"#,
                ),
            ]),
        ),
        // A single message on its own carrier.
        (
            "span",
            rule_attrs(&[
                ("langgraph.node", "n"),
                ("message", r#"{"type":"system","content":"be brief"}"#),
            ]),
        ),
        // The answer must be read even when the request side was found - the asymmetry that
        // closed a real loss.
        (
            "span",
            rule_attrs(&[
                (
                    "events",
                    r#"[{"event.name":"gen_ai.user.message","content":"q"}]"#,
                ),
                (
                    "response_data",
                    r#"{"message":{"role":"assistant","content":"a"}}"#,
                ),
            ]),
        ),
    ];

    // The tool definitions CrewAI's metadata carriers yield, **frozen as outputs** rather than exempted.
    // The retired reader's code cannot be frozen here: its grammar moved into the sealed `tool_repr`
    // module, so copying it would compare that module against itself. Its *outputs* can be, and are the
    // thing that matters - taken from the pre-`9b013f86` implementation for exactly these shapes.
    let frozen_crew_tools = |attrs: &HashMap<String, String>| -> Vec<RawToolDefinition> {
        // **Literal captured outputs** for the exact shapes the cases carry - not a reimplementation. My
        // first attempt re-derived the name collection and was wrong twice over: it read `tools_names` *or*
        // `tools` where the retired reader read both, and it dropped the rich-definition quality selection
        // entirely - so it could have agreed with the rules for the wrong reason. A literal is either right
        // or visibly wrong.
        //
        // The retired reader's *code* cannot be frozen here: its grammar moved into the sealed `tool_repr`
        // module, so a copy would compare that module against itself.
        const CAPTURED: &[(&str, &str, &str)] = &[
            (
                "crew_tasks",
                r#"[{"name":"research"}]"#,
                r#"[{"type":"function","function":{"name":"research"}}]"#,
            ),
            (
                "crew_tasks",
                r#"[{"name":"n"}]"#,
                r#"[{"type":"function","function":{"name":"n"}}]"#,
            ),
        ];
        // Every `crew_*` payload among the cases must have a literal. Matching on the exact string means an
        // edited case would silently stop applying, and the comparison would then pass with *neither* side
        // producing anything - a green oracle that checks nothing, which is worse than a named exemption.
        // The tool carriers only - `crew_key` / `crew_id` / `task_key` are markers that gate the rules,
        // not payloads they read.
        for (carrier, raw) in attrs
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "crew_tasks" | "crew_agents"))
        {
            assert!(
                CAPTURED.iter().any(|(c, r, _)| c == carrier && r == raw),
                "`{carrier}` carries a payload with no captured output: {raw}\nAdd it to CAPTURED, or the \
                 oracle compares nothing on this carrier."
            );
        }
        CAPTURED
            .iter()
            .filter(|(carrier, raw, _)| attrs.get(*carrier).map(String::as_str) == Some(*raw))
            .map(|(carrier, _, expected)| {
                RawToolDefinition::from_attr(
                    carrier,
                    time,
                    serde_json::from_str(expected).expect("the captured output parses"),
                )
            })
            .collect()
    };

    let mut disagreements = Vec::new();
    for (span_name, case) in &cases {
        let mut legacy_msgs: Vec<RawMessage> = Vec::new();
        let mut legacy_tools: Vec<RawToolDefinition> = Vec::new();
        let mut legacy_found = false;
        // The caller's tool-span gate, replicated: `extract_per_carrier` skips every extractor but the
        // conventions on a tool execution span, so the legacy side must be compared under that same rule.
        // Without it the oracle compares at two different levels - the rules apply the gate internally
        // (it is a declared rule property now) while these functions expected their caller to.
        let is_tool_span = is_tool_execution_span(case);
        // In rank order, which is the order the `EXTRACTORS` list had them - and **claiming per
        // extractor**, exactly as `extract_per_carrier` does. Without the claiming this side reports
        // duplicates production never produced: two dialects do read `message`, and the earlier extractor
        // owned it. The oracle was blind to that until consolidating the extractors made the two rules run
        // in one call, where nothing discarded the second.
        let mut legacy_claimed: HashSet<String> = HashSet::new();
        for f in [
            try_otel_genai_messages,
            try_gen_ai_indexed,
            try_openinference,
            try_vercel_ai,
            try_logfire_events,
            try_google_adk,
            try_langgraph,
            try_mlflow,
            try_traceloop,
            try_pydantic_ai,
            try_langsmith,
            try_livekit,
            try_claude_code,
            try_crewai,
            try_autogen,
        ] {
            if is_tool_span {
                continue;
            }
            let mut produced = Vec::new();
            let tools_before = legacy_tools.len();
            if !f(&mut produced, &mut legacy_tools, case, span_name, time) {
                continue;
            }
            // A reviewed delta, and the only one: an extractor whose whole contribution was a *tool
            // definition* reported success, and the caller reads that as "the message payload was handled"
            // and stops asking. A span stating a dialect's tool list has said nothing about its
            // conversation, so it does not count here either. An extractor that produced nothing at all
            // still counts - that is the claim case, whose entire purpose is to stop the generic reader.
            let tools_only = produced.is_empty() && legacy_tools.len() > tools_before;
            legacy_found |= !tools_only;
            // Recorded after the whole batch, never per message: one extractor legitimately emits several
            // observations for one carrier, and claiming as it goes would keep only the first.
            let mut newly_claimed = Vec::new();
            for message in produced {
                let carrier = carrier_of(&message.source);
                if legacy_claimed.contains(&carrier) {
                    continue;
                }
                newly_claimed.push(carrier);
                legacy_msgs.push(message);
            }
            legacy_claimed.extend(newly_claimed);
        }
        if is_tool_span {
            // Only the conventions read a tool span, and they still do - as declared rules.
            legacy_found |=
                try_otel_genai_messages(&mut legacy_msgs, &mut legacy_tools, case, "span", time);
        }
        // The convention's single-tool triple, frozen from the retired always-on path. It is *appended*,
        // which is where it ran: after every other definition had been collected. Not a reviewed delta any
        // more - there is a counterpart now, so the comparison is real.
        legacy_tools.extend(frozen_crew_tools(case));
        if let Some(triple) = legacy_single_tool_definition(case, time) {
            legacy_tools.push(triple);
        }

        let mut rule_msgs: Vec<RawMessage> = Vec::new();
        let mut rule_tools: Vec<RawToolDefinition> = Vec::new();
        let rule_found = try_declared_rules(
            &mut rule_msgs,
            &mut rule_tools,
            case,
            span_name,
            time,
            &mut std::collections::HashSet::new(),
        );
        // The metadata axis, which production reads on every span through `extract_tool_definitions`. The
        // retired extractors pushed tool definitions into the same vector, so both axes are collected here
        // or a declaration that moved to the always-on path would look like a loss.
        for emission in crate::domain::rules::ruleset().messages.tool_definitions(
            &crate::domain::rules::MessageContext::for_span("", case, is_tool_span),
        ) {
            if emission.target == crate::domain::rules::schema::EmitTarget::ToolDefinitions {
                rule_tools.push(RawToolDefinition::from_attr(
                    emission.carrier.name(),
                    time,
                    emission.value,
                ));
            }
        }

        // Compared as sets of serialised observations: the `EXTRACTORS` order decided which *extractor*
        // claimed a carrier, never the order observations sit in the vector - `extract_per_carrier`
        // claims by carrier name, and the pipeline sorts by provenance afterwards.
        // Compared with object keys sorted, deliberately.
        //
        // Member *order* cannot be compared against this baseline, because for an indexed family the
        // baseline had none: the code being replaced walked the attribute `HashMap`, whose order is
        // randomised per process, straight into a map that preserves insertion order. So the legacy bytes
        // are noise for those carriers, and the rules sort instead. Order does not affect the normalised
        // content hash either - the feed sorts keys before hashing - only the persisted bytes and the
        // reconstruction cache digest. What it *does* affect is reproducibility, which is checked on its
        // own (`a_swept_payload_is_ordered_deterministically`) rather than against a nondeterministic
        // oracle.
        fn canonical(value: &JsonValue) -> JsonValue {
            match value {
                JsonValue::Object(map) => {
                    let mut sorted: Vec<(&String, &JsonValue)> = map.iter().collect();
                    sorted.sort_by_key(|(k, _)| k.as_str());
                    JsonValue::Object(
                        sorted
                            .into_iter()
                            .map(|(k, v)| (k.clone(), canonical(v)))
                            .collect(),
                    )
                }
                JsonValue::Array(items) => JsonValue::Array(items.iter().map(canonical).collect()),
                other => other.clone(),
            }
        }
        // A reviewed delta is a tool *definition* on one of these carriers - decided on the typed
        // observation, before rendering, so it cannot match a message nor a tool whose *content* merely
        // mentions the string. See the comment below the constant for why each is here.
        // Tool *definitions* only. `ai.toolCall.args` / `.result` are deliberately absent: those rules emit
        // messages, and their message exemption below is the justified one - exempting their tool
        // definitions as well would mask a future bogus metadata emission on carriers that should never
        // produce one.
        const REVIEWED_DELTA_CARRIERS: &[&str] = &[];

        let is_reviewed_delta_tool = |t: &RawToolDefinition| -> bool {
            matches!(&t.source, ToolDefinitionSource::Attribute { key, .. }
                if REVIEWED_DELTA_CARRIERS.contains(&key.as_str()))
        };
        // Vercel's tool-call attributes now also reach a tool span as *messages* - the call and its result.
        // The retired code read them only when nothing else had produced a message, and this harness
        // applies the tool-span exclusion, so the legacy side has nothing here. Keyed on the typed message
        // source, so only these two carriers are exempt and any other moved message is still compared.
        const REVIEWED_DELTA_MESSAGE_CARRIERS: &[&str] =
            &["ai.toolCall.args", "ai.toolCall.result"];
        let is_reviewed_delta_msg = |m: &RawMessage| -> bool {
            matches!(&m.source, MessageSource::Attribute { key, .. }
                if REVIEWED_DELTA_MESSAGE_CARRIERS.contains(&key.as_str()))
        };
        // Messages are compared as a *set* and tool definitions **in order**, which is not a stylistic
        // split. Message order is decided later, by the reconstruction pipeline, so the order they sit in
        // this vector says nothing; a tool definition's order is semantic here, because merging keeps the
        // *first* of two equal-quality definitions sharing a name - so the position decides which
        // description and schema win. Sorting them together hid exactly that regression, and only a
        // dedicated test caught it.
        let render = |msgs: &[RawMessage], tools: &[RawToolDefinition]| -> Vec<String> {
            let mut messages: Vec<String> = msgs
                .iter()
                .filter(|m| !is_reviewed_delta_msg(m))
                .map(|m| format!("msg {:?} {}", m.source, canonical(&m.content)))
                .collect();
            messages.sort();
            messages.extend(
                tools
                    .iter()
                    .filter(|t| !is_reviewed_delta_tool(t))
                    .map(|t| format!("tool {:?} {}", t.source, canonical(&t.content))),
            );
            messages
        };
        // Why each carrier is a reviewed delta:
        //
        // - Vercel's tool-call attributes now reach a tool span. The code being replaced read them only
        //   when nothing else had produced a message, so any recognised event dropped the call and its
        //   result; and because this harness applies the caller's tool-span exclusion, the legacy side
        //   produces nothing for these carriers. Pinned by
        //   `an_event_does_not_suppress_a_tool_span_s_own_attributes`.
        //
        // - CrewAI's `crew_tasks` / `crew_agents` **tool definitions**. Their reference is the sealed
        //   `tool_repr` grammar, validated by the dedicated `test_crewai_tool_definitions_*` tests when it
        //   moved - never by this message oracle, whose retired side had no CrewAI tool extraction at all.
        //
        // Both are dropped from the tool vector *before* rendering, keyed on the typed source carrier, so a
        // message that moved is still compared and a tool on another carrier is untouched.
        let legacy = render(&legacy_msgs, &legacy_tools);
        let rules = render(&rule_msgs, &rule_tools);
        // The `found` flags can differ only by a reviewed delta: on a tool span the rules recognise
        // Vercel's tool-call carriers and the harness-excluded legacy side does not. Compared only when the
        // rendered observations agree, so `found` is not a second channel that can hide a real change.
        let found_differs_by_delta = legacy == rules
            && rule_found
            && !legacy_found
            && rule_msgs.iter().any(is_reviewed_delta_msg);
        if (legacy != rules || legacy_found != rule_found) && !found_differs_by_delta {
            disagreements.push(format!(
                "  span `{span_name}` {case:?}\n    table: found={legacy_found} {legacy:?}\n    \
                 rules: found={rule_found} {rules:?}"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "message extraction changed for {} case(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

/// What has moved, and what has not - counted, so the boundary cannot quietly stop moving.
#[test]
fn declared_message_rules_cover_what_they_claim() {
    let plan = &ruleset().messages;
    assert_eq!(
        plan.rule_count(),
        71,
        "the assets declare {} message rules. `EXTRACTORS` holds **one** entry where it held sixteen, and \
         that entry is the generic declared-rules evaluator: even the last-resort carriers are declared \
         now, with `stage: fallback`, so the fallback extractor is gone too. A dialect moves whole or not \
         at all, so there are no part-migrated carriers to count.",
        plan.rule_count()
    );
    for rule in plan.rules() {
        assert!(
            rule.doc.is_some(),
            "message rule `{}` has no doc: the reason a carrier is read the way it is belongs beside \
             the declaration",
            rule.rule_id
        );
    }
    // The carriers really are in the assets, so a clean engine is not a vacuous one.
    let all: String = schema::embedded_sources()
        .values()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();
    for carrier in [
        "traceloop.entity.input",
        "mlflow.spanInputs",
        "mlflow.chat.tools",
        "tool_arguments",
        "tool_response",
    ] {
        assert!(
            all.contains(carrier),
            "carrier `{carrier}` is declared in no asset, so nothing declares it at all"
        );
    }
}

#[test]
fn two_rules_reading_one_carrier_are_refused() {
    // Not a precedence question: the ingestion claims a carrier once, so the second rule could never
    // emit anything and would look live while doing nothing.
    let contested = br#"{
      "id": "t", "doc": "d",
      "messages": [
        {"id": "a", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message",
         "legacy_rank": 1},
        {"id": "b", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message",
         "legacy_rank": 2}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), contested.to_vec())]);
    assert!(matches!(
        compile(&sources),
        Err(crate::domain::rules::message_rules::MessageCompileError::ContestedCarrier { .. })
    ));
}

#[test]
fn the_two_parse_modes_differ_where_it_matters() {
    // `json` skips what it cannot parse; `json_or_string` keeps it. An extractor using the wrong one
    // either drops a plain-text payload or stores a fragment of JSON as prose.
    let both = br#"{
      "id": "t", "doc": "d",
      "messages": [
        {"id": "strict", "doc": "d", "read": {"attribute": "strict"}, "parse": "json",
         "emit": "message", "legacy_rank": 1},
        {"id": "lenient", "doc": "d", "read": {"attribute": "lenient"}, "parse": "json_or_string",
         "emit": "message", "legacy_rank": 2}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), both.to_vec())]);
    let plan = compile(&sources).expect("compiles");
    let span_attrs = rule_attrs(&[("strict", "not json"), ("lenient", "not json")]);
    let emissions = plan.run(&MessageContext::for_span("s", &span_attrs, false));
    let ids: Vec<&str> = emissions.iter().map(|e| e.rule_id).collect();
    assert_eq!(
        ids,
        vec!["lenient"],
        "the strict rule must skip an unparseable value and the lenient one must keep it"
    );
    assert_eq!(emissions[0].value, serde_json::json!("not json"));
}

/// A recognised event must not suppress a tool span's own tool-call attributes.
///
/// The shape the removed orchestration block lost. It read the Vercel tool attributes only when nothing
/// else had produced a message, so any recognised event dropped the call *and* its result - and the
/// equivalence oracle could not see it, because that oracle applies the caller's tool-span exclusion and so
/// compared both implementations after the suppression. No captured fixture carries this shape either.
#[test]
fn an_event_does_not_suppress_a_tool_span_s_own_attributes() {
    let attrs = rule_attrs(&[
        ("ai.toolCall.name", "weather"),
        ("ai.toolCall.id", "call_1"),
        ("ai.toolCall.args", r#"{"city":"NYC"}"#),
        ("ai.toolCall.result", r#"{"temp":21}"#),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "name plus id makes this a tool execution span, which is what gated the lost block"
    );

    let mut messages: Vec<RawMessage> = Vec::new();
    let mut tools: Vec<RawToolDefinition> = Vec::new();
    // A message already present, standing for one a recognised event produced.
    messages.push(RawMessage::from_attr(
        "gen_ai.choice",
        Utc::now(),
        serde_json::json!({"role": "assistant", "content": "thinking"}),
    ));
    let found = try_declared_rules(
        &mut messages,
        &mut tools,
        &attrs,
        "ai.toolCall",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );

    assert!(found);
    let carriers: Vec<String> = messages
        .iter()
        .map(|m| match &m.source {
            MessageSource::Attribute { key, .. } => key.clone(),
            MessageSource::Event { name, .. } => name.clone(),
        })
        .collect();
    assert!(
        carriers.iter().any(|c| c == "ai.toolCall.args"),
        "the call survives an earlier message: {carriers:?}"
    );
    assert!(
        carriers.iter().any(|c| c == "ai.toolCall.result"),
        "and so does its result: {carriers:?}"
    );
}

/// A swept payload is ordered deterministically, whatever the attribute map's own order.
///
/// The property the oracle cannot check, because the baseline it compares against did not have it. An
/// unsorted sweep writes different bytes for the same span on different runs - the map's iteration order is
/// randomised per process and the payload is persisted with its insertion order - which changes the
/// reconstruction cache digest for rows nobody touched.
#[test]
fn a_swept_payload_is_ordered_deterministically() {
    // Enough members that a randomised walk would almost certainly differ between two orders.
    let attrs = rule_attrs(&[
        ("ai.response.text", "answer"),
        ("ai.response.id", "r1"),
        ("ai.response.model", "m"),
        ("ai.response.providerMetadata", "{}"),
        ("ai.response.timestamp", "t"),
        ("ai.response.msgId", "x"),
        ("ai.response.finishReason", "stop"),
    ]);
    let rendered: Vec<String> = (0..8)
        .map(|_| {
            let mut messages: Vec<RawMessage> = Vec::new();
            let mut tools: Vec<RawToolDefinition> = Vec::new();
            try_declared_rules(
                &mut messages,
                &mut tools,
                &attrs,
                "ai.generateText",
                Utc::now(),
                &mut std::collections::HashSet::new(),
            );
            messages
                .iter()
                .map(|m| m.content.to_string())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    assert!(
        rendered.windows(2).all(|w| w[0] == w[1]),
        "the same span produced different bytes across runs: {rendered:?}"
    );
    // And the members really are in sorted order, which is what makes it stable across *processes* - equal
    // runs inside one process would also pass if the map order happened to be fixed.
    let payload = &rendered[0];
    let finish = payload.find("finishReason").expect("swept member present");
    let model = payload.find("\"model\"").expect("swept member present");
    assert!(
        finish < model,
        "swept members are inserted in sorted order: {payload}"
    );
}

/// The carrier-ownership check catches conflicts a single projection of it missed.
///
/// It used to compare "the carrier a rule reads" and so missed four real shapes: a `tag_as` emitting a name
/// the rule never read, an indexed family against an exact key it generates, a `compose` consuming a carrier
/// another rule emits, and a sweep overlapping an exact source. Each of those is a rule that silently never
/// emits, or two rules writing the same carrier - and either way a reader cannot tell.
#[test]
fn carrier_ownership_conflicts_are_refused() {
    let cases: &[(&str, &str)] = &[
        (
            "tag_as collides with another rule's carrier",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"y"},"tag_as":"x","parse":"json",
                 "emit":"message","legacy_rank":2}]}"#,
        ),
        (
            "an indexed family covers an exact key it generates",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"emit":"message","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"f.0"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "a compose consumes a carrier another rule emits",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"r.text"},"parse":"json","emit":"message",
                 "legacy_rank":1},
                {"id":"b","doc":"d","compose":{"tag":"r","members":[
                    {"as":"content","from_any_of":["r.text"]}]},"emit":"message","legacy_rank":2}]}"#,
        ),
        (
            "a sweep overlaps an exact source of another rule",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"p.one"},"parse":"json","emit":"message",
                 "legacy_rank":1},
                {"id":"b","doc":"d","compose":{"tag":"q","members":[
                    {"sweep_prefix":"p."}]},"emit":"message","legacy_rank":2}]}"#,
        ),
    ];
    for (what, asset) in cases {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            matches!(
                compile(&sources),
                Err(
                    crate::domain::rules::message_rules::MessageCompileError::ContestedCarrier { .. }
                )
            ),
            "should have been refused: {what}"
        );
    }
}

/// A construct the engine cannot execute, or a field it would ignore, is refused rather than accepted.
#[test]
fn inexpressible_rules_are_refused() {
    let cases: &[(&str, &str)] = &[
        (
            "`read.event` is accepted by the schema and never executed",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"event":"e"},"parse":"json","emit":"message",
                 "legacy_rank":1}]}"#,
        ),
        (
            "`compose` with `wrap`, which would be ignored",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","compose":{"tag":"q","members":[{"as":"c","from_any_of":["k"]}]},
                 "wrap":{"role":"user"},"emit":"message","legacy_rank":1}]}"#,
        ),
        (
            "an indexed family with `wrap`, which would be ignored",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"wrap":{"role":"user"},
                 "emit":"message","legacy_rank":1}]}"#,
        ),
        (
            "`entry_member` without an indexed family",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"k","entry_member":"m"},"parse":"json",
                 "emit":"message","legacy_rank":1}]}"#,
        ),
        (
            "a default section route before another route, which can never match",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"k"},"parse":"text","emit":"message",
                 "legacy_rank":1,
                 "sections":{"split_on":"|","routes":[
                    {"role":"user"},{"tag_prefix":"T:","role":"tool"}]}}]}"#,
        ),
        (
            "a compose member that is both a sweep and a named source",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","compose":{"tag":"q","members":[
                    {"as":"c","from_any_of":["k"],"sweep_prefix":"p."}]},"emit":"message",
                 "legacy_rank":1}]}"#,
        ),
        (
            "a gate on a resource dimension a message rule is never given",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                 "legacy_rank":1,"when":{"service_name":["svc"]}}]}"#,
        ),
        // A branch leaf's own copy of a field only the entry points read. Four spellings, because the
        // parent's no-dead-fields rule had no mirror here and each of these compiled into silence.
        (
            "a branch leaf declaring a stage",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "stage":"fallback"}]}}]}"#,
        ),
        (
            "a branch leaf declaring an event",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "when_event":["some.event"]}]}}]}"#,
        ),
        (
            "a branch leaf claiming to replace an event's raw form",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "replaces_raw_event":true}]}}]}"#,
        ),
        (
            "a branch leaf declaring a rank, which orders nothing - the branch order is positional",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "legacy_rank":2}]}}]}"#,
        ),
        // Presence, not value: each of these writes out the field's own default, which is still a statement
        // the engine reads from somewhere else. A check comparing against the default accepted all three.
        (
            "a branch leaf declaring the default stage explicitly",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "stage":"dialect"}]}}]}"#,
        ),
        (
            "a branch leaf declaring an empty event list",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "when_event":[]}]}}]}"#,
        ),
        (
            "a branch leaf declaring that it does not replace an event's raw form",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "replaces_raw_event":false}]}}]}"#,
        ),
        // The parent's own dead fields. `reads_tool_spans` is the observable one: the permission is read
        // from the leaves, so a parent granting it made the whole branch skipped on a tool span.
        (
            "a branch parent granting tool-span permission its leaves do not have",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,"reads_tool_spans":true,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent declaring a parse mode for a carrier it does not read",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,"parse":"json",
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent declaring a carrier tag its leaves override",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,"tag_as":"q",
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent requiring a non-empty value it never reads",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,"require_non_empty":true,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent requiring members of an entry it never assembles",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,
                 "require_members":{"all_of":[{"name":"role"}]},
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
    ];
    for (what, asset) in cases {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_err(),
            "should have been refused: {what}"
        );
    }
}

/// A `require` that holds of every payload is not a condition, and a tag collision is not a read collision.
///
/// Four shapes, each one a rule that compiles and cannot work:
///
/// - `exists: true` beside `exists: false` on one path holds of every value there is, so a rule whose only
///   reading carries it claims its carrier always - while `rule_is_wholly_conditional` read the non-empty
///   `require` as evidence that it sometimes yields, and a second rule on that carrier was permanently dead.
///   The pair is a tautology on **any** path, unlike the value complements, because `exists` is the predicate
///   that decides presence.
/// - Two rules tagging one carrier from *different* attributes both survive: runtime ownership is over the
///   carrier a rule read, so nothing separates them, and the tag is what carrier semantics, identity and
///   ordering key on. Shared physical ownership is what excuses such a pair - the accepted half below - and a
///   conditional read is not.
/// - A `compose` always emits `compose.tag`, so a `tag_as` beside it was checked for collisions under a name
///   the rule never emits.
#[test]
fn a_tautological_requirement_is_not_a_condition() {
    let refused = [
        (
            "an `exists` complement, which holds of every payload",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[
                    {"path":"$.v","exists":true},{"path":"$.v","exists":false}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "two rules tagging one carrier from different attributes",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"shared","when":{"attr_exists":["marker"]},"legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"y"},"parse":"json","emit":"message",
                 "tag_as":"shared","legacy_rank":2}]}"#,
        ),
        (
            "a compose declaring a tag it does not emit",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","tag_as":"declared","legacy_rank":1,
                 "compose":{"tag":"actual","members":[{"as":"content","from_any_of":["k"]}]}}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_err(),
            "should have been refused: {what}"
        );
    }

    // Accepted, and each for a reason the refusals above depend on. A tag collision between two rules reading
    // *one* carrier is resolved by ownership - which is what lets one dialect claim `input.value` while
    // another reads it - and a genuine `exists` condition on one side is a condition.
    let accepted = [
        (
            "two rules tagging one carrier and reading the same attribute",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"shared","when":{"attr_exists":["marker"]},"legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"shared","legacy_rank":2}]}"#,
        ),
        (
            "a single `exists` requirement, which is a real condition",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[{"path":"$.v","exists":true}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "an indexed family requiring members, beside a rule reading one of its keys",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"emit":"message","legacy_rank":1,
                 "require_members":{"all_of":[{"name":"content"}]}},
                {"id":"b","doc":"d","read":{"attribute":"f.0.role"},"parse":"json","emit":"message",
                 "tag_as":"b.own.tag","legacy_rank":2}]}"#,
        ),
        (
            "a rule reading a later spelling of a carrier another rule reads first",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute_any_of":["first","second"]},"parse":"json",
                 "emit":"message","tag_as":"a.own.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"second"},"parse":"json","emit":"message",
                 "tag_as":"b.own.tag","legacy_rank":2}]}"#,
        ),
    ];
    for (what, asset) in accepted {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_ok(),
            "should have been accepted: {what} - {:?}",
            compile(&sources).err()
        );
    }
}

/// A condition separates two rules only when it is a *different* condition, and a shared tag needs a proof.
///
/// Four shapes, each a rule that compiled and could never emit:
///
/// - two rules gated on the **same** thing, both reading one carrier: whenever the gate holds the earlier
///   owns the carrier, and otherwise neither runs. A boolean "is conditional" called that pair safe;
/// - a tag collision excused by *any* static overlap between the rules' reads. A rule reading
///   `attribute_any_of: ["first", "second"]` owns `first` when both are present, so a rule reading `second`
///   under the same tag is not resolved by ownership at all;
/// - a `one_of`/`none_of` complement on a *member* path, which is a tautology for the same reason the
///   `exists` pair is: a sole `none_of` holds of an absent value, so between them every value and its
///   absence are covered;
/// - the same written `$['v']`, which rendered differently and escaped a same-path check.
#[test]
fn a_condition_separates_two_rules_only_when_it_differs() {
    let refused = [
        (
            "two rules gated on the same thing, reading one carrier",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker"]},"tag_as":"a.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "a shared tag where ownership does not resolve the pair",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute_any_of":["first","second"]},"parse":"json",
                 "emit":"message","tag_as":"shared","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"second"},"parse":"json","emit":"message",
                 "tag_as":"shared","legacy_rank":2}]}"#,
        ),
        (
            "a `one_of`/`none_of` complement on a member path",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[
                    {"path":"$.v","one_of":["a"]},{"path":"$.v","none_of":["a"]}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "the same complement with the path written in bracket form",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[
                    {"path":"$.v","exists":true},{"path":"$['v']","exists":false}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "`elements` beside a `tag_as` it never emits",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"declared","legacy_rank":1,
                 "elements":{"passes":[{"tag_from":"$.name"}]}}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_err(),
            "should have been refused: {what}"
        );
    }

    let accepted = [
        (
            "two rules gated on different things",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker.a"]},"tag_as":"a.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker.b"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "a shared tag where both rules necessarily own one carrier",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"shared","when":{"attr_exists":["marker"]},"legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"shared","legacy_rank":2}]}"#,
        ),
        (
            "a `none_of` that forbids a value nothing else requires",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[
                    {"path":"$.v","one_of":["a"]},{"path":"$.v","none_of":["a","b"]}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
    ];
    for (what, asset) in accepted {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_ok(),
            "should have been accepted: {what} - {:?}",
            compile(&sources).err()
        );
    }
}

/// A gate is a **disjunction**, so one gate can hold wherever another does without being the same gate.
///
/// Equality was the wrong relation. `attr_exists: ["a", "b"]` holds everywhere `attr_exists: ["a"]` does, so
/// the wider-gated rule suppresses the narrower one whenever it comes first - and their declared forms differ,
/// which is all an equality test could see. The relation is directional and rank-aware: the question is
/// whether the *earlier* rule leaves anything for the later one.
///
/// It also has to be two facets rather than one label. A rule can be gated **and** payload-narrowed, and
/// folding them together made "same gate, mutually exclusive payloads" - a working pair - look dead.
#[test]
fn a_wider_gate_suppresses_a_narrower_one() {
    let refused = [
        (
            "a superset gate at the earlier rank",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["a","b"]},"tag_as":"a.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["a"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "a shorter span-name prefix, which covers every longer one",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"span_name":["chat"]},"tag_as":"a.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"span_name":["chat.completions"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "a root `starts_with` beside its own negation, which holds of every value",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
                 "alternatives":[{"require":{"any":[
                    {"starts_with":"a"},{"lacks_prefix":"a"}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "legacy_rank":2}]}"#,
        ),
        (
            "`elements` beside an explicitly false aggregate, which it also ignores",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "aggregate_into_array":false,"legacy_rank":1,
                 "elements":{"passes":[{"tag_from":"$.name"}]}}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_err(),
            "should have been refused: {what}"
        );
    }

    let accepted = [
        (
            "the subset gate at the earlier rank, which leaves spans for the wider one",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["a"]},"tag_as":"a.tag","legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["a","b"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "the same gate, where the earlier rule may read nothing on a span it runs on",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker"]},"tag_as":"a.tag","legacy_rank":1,
                 "alternatives":[{"require":{"any":[{"path":"$.kind","one_of":["first"]}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "when":{"attr_exists":["marker"]},"tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "two composes sharing a tag, which every emission owns",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","legacy_rank":1,"when":{"attr_exists":["m.a"]},
                 "compose":{"tag":"shared","members":[{"as":"content","from_any_of":["k1"]}]}},
                {"id":"b","doc":"d","legacy_rank":2,"when":{"attr_exists":["m.b"]},
                 "compose":{"tag":"shared","members":[{"as":"content","from_any_of":["k2"]}]}}]}"#,
        ),
        (
            "a reading narrowed only by `require_parent`, which narrows as `require` does",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"a.tag","legacy_rank":1,
                 "alternatives":[{"require_parent":{"any":[{"path":"$.kind","one_of":["k"]}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"b.tag","legacy_rank":2}]}"#,
        ),
        (
            "a single-spelling `attribute_any_of`, which is not a choice",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute_any_of":["only"]},"parse":"json","emit":"message",
                 "tag_as":"shared","when":{"attr_exists":["m"]},"legacy_rank":1},
                {"id":"b","doc":"d","read":{"attribute":"only"},"parse":"json","emit":"message",
                 "tag_as":"shared","legacy_rank":2}]}"#,
        ),
    ];
    for (what, asset) in accepted {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_ok(),
            "should have been accepted: {what} - {:?}",
            compile(&sources).err()
        );
    }
}

/// A conditional claim is conditional about **one carrier**, not about every carrier its rule reads.
///
/// Two shapes, each a rule that would be permanently dead while compilation called the pair conditional:
///
/// - a witnessed overlay made its whole rule conditional, so a second rule reading one of the *family's* own
///   keys was accepted - and on a span carrying the family and no side payload, the family rule reads and
///   owns that key on every span, so the second could never emit;
/// - "every alternative carries a `require`" ignored `also` and `fallback`, which `all_readings` also emits
///   through - so a rule with one required alternative and an unconditional fallback claimed its carrier
///   always and still counted as conditional.
///
/// The accepted halves are what keep this from being an over-refusal: the same overlay against a rule reading
/// the *side payload* is a genuine pair, and so is a rule whose every reading is required.
#[test]
fn conditionality_is_a_property_of_the_carrier_not_of_the_rule() {
    // Rule A reads family `f` always and `side` only where the witness holds; rule B reads `f.0.content`.
    let family_key_conflict = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","legacy_rank":1,
         "read":{"indexed_family":"f","overlay":{
            "from":"side","parse":"json","select_any_of":["$"],
            "witness":{"any":[{"path":"$[*].id","kind":"array"}]},
            "when_member_prefix":"contents.","content_any_of":["$.content"],
            "as_member":"content"}},
         "emit":"message"},
        {"id":"b","doc":"d","read":{"attribute":"f.0.content"},"parse":"json","emit":"message",
         "tag_as":"b.own.tag","legacy_rank":2}]}"#;
    // The same overlay, against a rule reading the payload the overlay joins against. Genuinely conditional:
    // A consumes `side` only where the witness holds, and yields it elsewhere.
    let side_payload_pair = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","legacy_rank":1,
         "read":{"indexed_family":"f","overlay":{
            "from":"side","parse":"json","select_any_of":["$"],
            "witness":{"any":[{"path":"$[*].id","kind":"array"}]},
            "when_member_prefix":"contents.","content_any_of":["$.content"],
            "as_member":"content"}},
         "emit":"message"},
        {"id":"b","doc":"d","read":{"attribute":"side"},"parse":"json","emit":"message",
         "legacy_rank":2}]}"#;
    // Rule A reads `x` through one required alternative *and* an unconditional fallback, so it claims `x` on
    // every span; rule B reads `x` too.
    let unconditional_fallback = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
         "alternatives":[{"require":{"any":[{"path":"$.marker","exists":true}]},
                          "wrap":{"role":"user","content_from_any_of":["$.content"]}}],
         "fallback":[{"wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
        {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
         "legacy_rank":2}]}"#;
    // The same rule with no unconditional path: every reading is required, so it yields on a payload none
    // recognises and the pair is genuine.
    let all_readings_required = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1,
         "alternatives":[{"require":{"any":[{"path":"$.marker","exists":true}]},
                          "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
        {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
         "legacy_rank":2}]}"#;

    for (what, asset) in [
        (
            "a family key another rule reads, beside a witnessed overlay",
            family_key_conflict,
        ),
        (
            "an unconditional fallback beside a required alternative",
            unconditional_fallback,
        ),
    ] {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_err(),
            "the second rule is permanently dead and should have been refused: {what}"
        );
    }
    for (what, asset) in [
        (
            "the overlay's own payload, which it reads only where witnessed",
            side_payload_pair,
        ),
        (
            "a rule whose every reading is required",
            all_readings_required,
        ),
    ] {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&sources).is_ok(),
            "this claim really is conditional and must be permitted: {what} - {:?}",
            compile(&sources).err()
        );
    }
}

/// An event rule's gate asks about the **span**, and its `read` draws from the **event**.
///
/// Both dimensions were unavailable at that entry point: the span name was passed as `""`, so a `span_name`
/// gate compiled and could only ever fail, and the event's own attribute map stood in for the span's, so an
/// `attr_exists` gate asked about the wrong map. Neither was an error - the rule simply never fired, which is
/// the shape this whole engine exists to make impossible.
///
/// One dimension per rule, because gate signals are ORed: a rule naming two would be satisfied by either and
/// could not tell which one the engine actually consulted.
#[test]
fn an_event_rules_gate_asks_about_its_span_not_about_the_event() {
    let asset = br#"{"id":"t","doc":"d",
      "message_events":[{"name":"some.event","doc":"a probe event"}],
      "messages":[
        {"id":"by_name","doc":"d","when_event":["some.event"],
         "when":{"span_name":["chat "]},
         "read":{"attribute":"payload"},"parse":"json","emit":"message","legacy_rank":1},
        {"id":"by_attr","doc":"d","when_event":["some.event"],
         "when":{"attr_exists":["framework.marker"]},
         "read":{"attribute":"other"},"parse":"json","emit":"message","legacy_rank":2}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), asset.to_vec())]);
    let plan = compile(&sources).expect("an event rule may be gated on the span that carries it");

    let event_attrs = rule_attrs(&[
        ("payload", r#"{"role":"user","content":"q"}"#),
        ("other", r#"{"role":"user","content":"r"}"#),
    ]);
    let marked = rule_attrs(&[("framework.marker", "yes")]);
    let bare = rule_attrs(&[]);
    let fired = |span_name: &str, span_attrs: &HashMap<String, String>| -> Vec<String> {
        plan.from_event("some.event", &event_attrs, span_name, span_attrs, false)
            .0
            .iter()
            .map(|e| e.rule_id.to_string())
            .collect()
    };

    assert_eq!(
        fired("chat model", &bare),
        vec!["by_name"],
        "the span name is the real one, so the name-gated rule reads its event"
    );
    assert_eq!(
        fired("tool execution", &marked),
        vec!["by_attr"],
        "the attribute gate asks about the span's map, which carries the marker"
    );
    assert!(
        fired("tool execution", &bare).is_empty(),
        "neither gate holds of this span"
    );
    // And the gate is not satisfiable from the event's own map, which used to stand in for the span's.
    let self_marked = rule_attrs(&[
        ("payload", r#"{"role":"user","content":"q"}"#),
        ("other", r#"{"role":"user","content":"r"}"#),
        ("framework.marker", "yes"),
    ]);
    assert!(
        plan.from_event("some.event", &self_marked, "tool execution", &bare, false)
            .0
            .is_empty(),
        "an event carrying the marker is not a span carrying it"
    );
}

/// The declared evidence answers "is this a tool running" exactly as the retired list of branches did.
///
/// Written as the table it replaced, so a dialect whose signal is deleted from an asset fails here rather
/// than silently letting rules read a tool span as a model's turn.
#[test]
fn the_declared_span_facts_reproduce_the_legacy_tool_span_test() {
    /// A span's attributes, and whether the branches this replaced called it a tool execution.
    type ToolSpanCase = (&'static str, Vec<(&'static str, &'static str)>, bool);
    // Each case, and what the branches it replaced answered.
    let cases: Vec<ToolSpanCase> = vec![
        ("nothing at all", vec![], false),
        (
            "the operation the span reports",
            vec![("gen_ai.operation.name", "execute_tool")],
            true,
        ),
        (
            "a different operation",
            vec![("gen_ai.operation.name", "chat")],
            false,
        ),
        (
            "a span kind, in the capitals the convention writes",
            vec![("openinference.span.kind", "TOOL")],
            true,
        ),
        (
            "the same, lowercase",
            vec![("openinference.span.kind", "tool")],
            true,
        ),
        (
            "another kind",
            vec![("openinference.span.kind", "LLM")],
            false,
        ),
        // The conjunction, both ways round: a name alone sits on a model span that mentions a tool.
        (
            "a tool name alone",
            vec![("gen_ai.tool.name", "search")],
            false,
        ),
        (
            "a call id alone",
            vec![("gen_ai.tool.call.id", "c1")],
            false,
        ),
        (
            "a tool name and a call id",
            vec![
                ("gen_ai.tool.name", "search"),
                ("gen_ai.tool.call.id", "c1"),
            ],
            true,
        ),
        (
            "a tool response, which only the span that ran it records",
            vec![("gcp.vertex.agent.tool_response", "{}")],
            true,
        ),
        (
            "one dialect's spelling of the pair",
            vec![("ai.toolCall.name", "search"), ("ai.toolCall.id", "c1")],
            true,
        ),
        ("half of it", vec![("ai.toolCall.name", "search")], false),
    ];
    for (what, attrs, expected) in cases {
        let attrs = make_attrs(&attrs);
        assert_eq!(
            is_tool_execution_span(&attrs),
            expected,
            "{what}: the declared evidence disagrees with the branches it replaced"
        );
    }
}

/// Two dialects that both read one carrier must not both emit from it.
///
/// The ranks exist to decide which is tried first, and the ownership check permits a collision only when a
/// condition separates them - so the *first* rule to read a carrier owns it, as one extractor claiming a
/// carrier used to mean. Without that, consolidating extractors into one entry turned "the earlier one
/// won" into "both emit", which a reader sees as the same turn twice.
#[test]
fn one_carrier_is_read_by_one_rule() {
    let attrs = make_attrs(&[
        ("langgraph.node", "agent"),
        ("message", r#"{"type":"HumanMessage","content":"hi"}"#),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    try_declared_rules(
        &mut messages,
        &mut tools,
        &attrs,
        "span",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );
    let from_message: Vec<&RawMessage> = messages
        .iter()
        .filter(|m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "message"))
        .collect();
    assert_eq!(
        from_message.len(),
        1,
        "the `message` carrier produced {} observations: {:?}",
        from_message.len(),
        from_message
            .iter()
            .map(|m| m.content.to_string())
            .collect::<Vec<_>>()
    );
}

/// A declared tool definition is read on every span, including one that is a tool running.
///
/// `reads_tool_spans` asks whether a rule may read a tool span **as a conversation** - its messages are
/// that tool's input and result, not a model's turn. That is not a question about tool *definitions*, and
/// the path reading them has always run on every span. Routing them through the message evaluator applied
/// the message gate, so a framework's tool list vanished on any span that also carried a tool-execution
/// signal.
#[test]
fn declared_tool_definitions_survive_a_tool_execution_span() {
    let agents = r#"[{"role":"Weather Expert","tools_names":["get_weather"]}]"#;
    let attrs = make_attrs(&[
        ("crew_agents", agents),
        ("crew_key", "k"),
        // Any declared tool-execution signal.
        ("gen_ai.operation.name", "execute_tool"),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "the case has to be a tool span for this to mean anything"
    );
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        tool_defs.len(),
        1,
        "a tool span lost its declared tool definitions"
    );
}

/// A carrier whose whole content is a tool list is not also a conversation.
///
/// The repr grammar runs on the metadata axis, outside claiming, which is right - a tool definition is not
/// a message. But it still *reads* a carrier, and when that carrier holds nothing but the tool list, the
/// generic fallback would go on to present the same Python `repr` as a user turn. So a repr rule that
/// recognised the carrier claims it on the message axis too, saying "mine, and no conversation".
#[test]
fn a_carrier_holding_only_a_tool_list_is_not_read_as_a_conversation() {
    let repr = r#"{"tools": ["CrewStructuredTool(name='search', description='Tool Arguments: {\"q\": {\"type\": \"str\"}}')"]}"#;
    let attrs = make_attrs(&[("crew_key", "k"), ("input.value", repr)]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_defs.len(), 1, "the tool list should be read");

    let mut messages = Vec::new();
    let mut defs = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut defs,
        &attrs,
        "span",
        Utc::now(),
        ExtractionMode::PerCarrier,
        is_tool_execution_span(&attrs),
    );
    let from_input: Vec<&RawMessage> = messages
        .iter()
        .filter(
            |m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "input.value"),
        )
        .collect();
    assert!(
        from_input.is_empty(),
        "the tool list was also presented as a conversation: {:?}",
        from_input
            .iter()
            .map(|m| m.content.to_string())
            .collect::<Vec<_>>()
    );
}

/// A message rule that also declares tools on the same carrier yields both.
///
/// AutoGen's logging channel carries the conversation, the reply and the tools offered, in one carrier via
/// `also`. The whole rule is on the message axis (its target is `Message`), so it runs through `run` - and
/// its tool-definition reading has to survive that path, or a framework that co-locates tools and
/// conversation loses its tools. This is the shape `is_metadata_rule` must not mishandle by routing the
/// whole rule one way.
#[test]
fn a_message_rule_may_also_emit_tool_definitions() {
    let body = r#"{"type": "LLMCall", "messages": [{"role": "user", "content": "q"}], "response": {"content": "a"}, "tools": [{"name": "search"}]}"#;
    let attrs = make_attrs(&[("body", body)]);
    // The conversation is read on the message axis, the tools on the metadata axis - two entry points,
    // routed by each emission's own target, from the one rule.
    let mut messages = Vec::new();
    let mut unused = Vec::new();
    try_declared_rules(
        &mut messages,
        &mut unused,
        &attrs,
        "span",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );
    assert!(!messages.is_empty(), "the conversation and reply were lost");
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        tools.len(),
        1,
        "the tool list co-located with the conversation was lost: {tools:?}"
    );
}

/// The tools-only claim must not swallow a conversation that sits beside the tools.
///
/// Codex's case: `{tools: [...], messages: [...]}`. The tool parser reads the list on the metadata axis,
/// but the carrier also holds a real conversation, so claiming it as "mine and empty" loses the turn.
/// Excluding only `context` did not establish "holds only tools" - `messages` is a conversation too.
#[test]
fn a_tools_list_beside_a_conversation_does_not_claim_the_carrier() {
    let mixed = r#"{"tools": ["search"], "messages": [{"role": "user", "content": "hello"}]}"#;
    let attrs = make_attrs(&[("crew_key", "k"), ("input.value", mixed)]);
    let mut messages = Vec::new();
    let mut unused = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut unused,
        &attrs,
        "span",
        Utc::now(),
        ExtractionMode::PerCarrier,
        is_tool_execution_span(&attrs),
    );
    let from_input: Vec<&RawMessage> = messages
        .iter()
        .filter(
            |m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "input.value"),
        )
        .collect();
    assert!(
        !from_input.is_empty(),
        "the conversation beside the tools was claimed away"
    );
}

/// A present-but-empty declaration wrapper has declared no tools, and is not itself a tool.
///
/// The retired code chose the wrapper member by *presence* and extended by its elements, so an empty
/// `function_declarations: []` contributed nothing. Choosing "the first path that yielded something"
/// instead skips the empty member and falls through to emitting the wrapper object as a tool - a tool
/// called nothing, with the wrapper's own shape.
#[test]
fn an_empty_declaration_wrapper_yields_no_tool() {
    let request = r#"{"tools":[{"function_declarations":[]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert!(
        tools.is_empty(),
        "an empty declaration wrapper produced a tool: {tools:?}"
    );
}

/// Both spellings present: the first *declared* one wins, whichever is non-empty.
#[test]
fn the_first_declared_wrapper_spelling_wins() {
    let request = r#"{"tools":[{"function_declarations":[{"name":"snake"}],"functionDeclarations":[{"name":"camel"}]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    let names: Vec<String> = tools
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    assert_eq!(names, vec!["snake".to_string()]);
}

/// An indexed entry that wraps one serialised payload *is* that payload.
///
/// The family's members arrive as `<prefix>.<n>.tool.json_schema`, so stripping the entry prefix leaves a
/// member literally named `tool.json_schema` - a dot in the key, not a nesting - which is why the projection
/// is bracket-quoted. Both rules over the family are pinned here: the schema list and the names projected
/// out of it, which are different observations.
#[test]
fn an_indexed_entry_projects_its_leaf_payload() {
    let attrs = make_attrs(&[
        (
            "llm.tools.0.tool.json_schema",
            r#"{"type":"function","function":{"name":"search","parameters":{"type":"object"}}}"#,
        ),
        (
            "llm.tools.1.tool.json_schema",
            r#"{"type":"function","function":{"name":"calculator"}}"#,
        ),
    ]);
    let (defs, names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(defs.len(), 1, "the family is one observation: {defs:?}");
    let schemas = defs[0].content.as_array().expect("an array of schemas");
    assert_eq!(schemas.len(), 2);
    assert_eq!(
        schemas[0]["function"]["name"].as_str(),
        Some("search"),
        "the projection returned the wrapper rather than the schema: {schemas:?}"
    );
    assert_eq!(names.len(), 1, "the names are their own observation");
    let listed: Vec<&str> = names[0]
        .content
        .as_array()
        .expect("an array of names")
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert_eq!(listed, vec!["search", "calculator"]);
}

/// The single-tool triple yields to each of its two precedences independently.
///
/// Its gate is one `unless` naming two dimensions - this dialect's own list carriers, and the convention's
/// single-tool attribute. A `DetectMatch` holds when **any** dimension does, which is what makes one gate
/// express two independent precedences; were it all-of, neither alone would suppress the triple and the same
/// tools would be reported twice. The retired code expressed both as a global "has anything produced tools
/// yet" flag, which any unrelated dialect could satisfy first.
#[test]
fn the_single_tool_triple_yields_to_each_precedence_alone() {
    let triple: &[(&str, &str)] = &[
        ("tool.name", "secondary"),
        ("tool.description", "a tool"),
        ("tool.parameters", r#"{"type":"object"}"#),
    ];
    let names_of = |attrs: &HashMap<String, String>| -> Vec<String> {
        extract_tool_definitions("", attrs, Utc::now())
            .0
            .iter()
            .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
            .filter_map(|t| {
                t["function"]["name"]
                    .as_str()
                    .or_else(|| t["name"].as_str())
                    .map(str::to_string)
            })
            .collect()
    };

    // Alone, the triple is read.
    let mut only = triple.to_vec();
    assert!(names_of(&make_attrs(&only)).contains(&"secondary".to_string()));

    // The dialect's own list carrier suppresses it - same tools, described once.
    only.push(("llm.tools", r#"[{"name":"listed"}]"#));
    assert!(!names_of(&make_attrs(&only)).contains(&"secondary".to_string()));

    // And so does the convention's own single-tool attribute, on its own.
    let mut with_convention = triple.to_vec();
    with_convention.push(("gen_ai.tool.name", "primary"));
    let seen = names_of(&make_attrs(&with_convention));
    assert!(seen.contains(&"primary".to_string()));
    assert!(!seen.contains(&"secondary".to_string()));
}

/// A malformed list carrier does not suppress a perfectly good single-tool triple.
///
/// The precedence is "the triple is read if the lists produced nothing" - which is what the retired code's
/// flag tested. Expressed as an `attr_exists` gate it became "the triple is read if no list carrier is
/// *present*", so an unparseable `llm.tools` took the tools away entirely rather than yielding to the
/// triple. A branch set tests production, which is the condition that was always meant.
#[test]
fn a_malformed_list_carrier_yields_to_the_single_tool_triple() {
    let attrs = make_attrs(&[
        ("llm.tools", "not json at all"),
        ("tool.name", "secondary"),
        ("tool.description", "a tool"),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let names: Vec<String> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec!["secondary".to_string()],
        "a malformed list carrier suppressed the triple: {defs:?}"
    );
}

/// A malformed indexed schema is dropped, not reported as a tool.
///
/// An indexed member is sniffed, so an unparseable schema arrives as the string it is. Projected without a
/// declared parse mode that string became a "tool definition" - junk where the retired code, which required
/// `serde_json::from_str` to succeed, reported nothing.
#[test]
fn a_malformed_indexed_schema_is_not_a_tool() {
    let attrs = make_attrs(&[
        ("llm.tools.0.tool.json_schema", "{not json"),
        (
            "llm.tools.1.tool.json_schema",
            r#"{"type":"function","function":{"name":"good"}}"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let schemas: Vec<JsonValue> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        schemas.len(),
        1,
        "the malformed schema was reported as a tool: {schemas:?}"
    );
    assert_eq!(schemas[0]["function"]["name"].as_str(), Some("good"));
}

/// A present wrapper that is not a list has not declared its contents.
///
/// A wrapper member *is* a list of declarations. Present but scalar, it says nothing - and the retired code,
/// which required the member to be an array, fell back to the enclosing tool group. Emitting the scalar
/// itself would report a "tool" that is a string.
#[test]
fn a_non_array_declaration_wrapper_falls_back_to_the_group() {
    let request = r#"{"tools":[{"name":"bare","function_declarations":"not a list"}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let tools: Vec<JsonValue> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(tools.len(), 1, "expected the enclosing group: {tools:?}");
    assert_eq!(tools[0]["name"].as_str(), Some("bare"));
}

/// A tool with no name is not a tool definition.
///
/// `composed` emits as soon as *any* member resolved, so a span carrying only `tool.description` produced a
/// canonical definition with no `name` - unusable, and the retired branch required the name to be there.
#[test]
fn a_nameless_single_tool_is_not_emitted() {
    let attrs = make_attrs(&[("tool.description", "a tool with no name")]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert!(
        defs.is_empty(),
        "a nameless tool definition was emitted: {defs:?}"
    );
}

/// The declared fallback stage, in the retired extractor's shape.
///
/// `try_raw_io` is gone: its four carriers are declared with `stage: fallback`, and the stage is evaluated by
/// the plan. These call sites assert the same behaviour through the declaration.
fn try_raw_io(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let produced: Vec<RawMessage> = crate::domain::rules::ruleset()
        .messages
        .fallback(
            &crate::domain::rules::MessageContext::for_span(
                span_name,
                attrs,
                is_tool_execution_span(attrs),
            ),
            &std::collections::HashSet::new(),
        )
        .into_iter()
        .map(|e| RawMessage::from_attr(e.carrier.name(), timestamp, e.value))
        .collect();
    let found = !produced.is_empty();
    messages.extend(produced);
    found
}

/// The convention's inference-details container is read on a tool span too.
///
/// The retired extractor answered this event *before* any tool-span check, so declaring the rules without
/// `reads_tool_spans` narrowed them. The bundled tool result is the opposite case and correctly stays
/// gated: it was guarded by `!is_tool_span` in the code it replaced.
#[test]
fn the_inference_details_container_is_read_on_a_tool_span() {
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1_702_400_000_000_000_000,
        attributes: vec![make_kv(
            "gen_ai.input.messages",
            r#"[{"role":"user","content":"hi"}]"#,
        )],
        dropped_attributes_count: 0,
    };
    let on_tool_span = extract_message_from_event(&event, "", &HashMap::new(), true);
    // The *carrier*, not the count: without the rule the raw event is emitted instead, which is also one
    // message and would let this pass for the wrong reason.
    let carriers: Vec<String> = on_tool_span
        .iter()
        .map(|m| match &m.source {
            MessageSource::Event { name, .. } => name.clone(),
            MessageSource::Attribute { key, .. } => key.clone(),
        })
        .collect();
    assert_eq!(
        carriers,
        vec!["gen_ai.input.messages".to_string()],
        "the container was narrowed on a tool span, or emitted as its own raw form"
    );
}

/// The fallback stage does not re-read a carrier the dialect stage already read.
///
/// The two stages meet on one path: a generation span whose *answer* is unaccounted for reads the fallback
/// after a dialect produced something. With independent claim sets, a dialect's reading of `output.value`
/// and the fallback's reading of it both survive - the answer twice. That is the shape the compiler permits
/// (a dialect `Claim` on the generic carrier) and no asset currently produces, so it is tested at the
/// mechanism rather than through a fixture.
///
/// Asserted on the **inheritance** itself, because the names carry their kind as an `attr:` / `event:`
/// prefix - the caller's own spelling. Taking them as bare attribute names was a defect that made this do
/// nothing at all, and a span-level test could not see it.
#[test]
fn the_fallback_inherits_what_the_dialect_stage_read() {
    let attrs = make_attrs(&[(
        "output.value",
        r#"{"role":"assistant","content":"the answer"}"#,
    )]);
    let ctx = crate::domain::rules::MessageContext::for_span("call_llm", &attrs, false);
    let plan = &ruleset().messages;

    let read_afresh = plan.fallback(&ctx, &std::collections::HashSet::new());
    assert!(
        read_afresh
            .iter()
            .any(|e| e.carrier.name() == "output.value"),
        "the fallback should read the generic carrier when nothing has"
    );

    let inherited = plan.fallback(
        &ctx,
        &std::collections::HashSet::from([crate::domain::rules::message_rules::OwnedCarrier {
            is_event: false,
            name: "output.value".to_string(),
        }]),
    );
    assert!(
        !inherited.iter().any(|e| e.carrier.name() == "output.value"),
        "the fallback re-read a carrier the dialect stage had already read: {inherited:?}"
    );
}

/// The retired single-tool triple, frozen as the equivalence oracle's reference.
///
/// Not history: the declared rule must reproduce it, including the identifier test that excludes a
/// synthetic aggregate reported under a parenthesised name, and the position - it ran *after* every other
/// definition had been collected, and merging keeps the first of two equal-quality definitions with one
/// name, so the order decides which description and schema win.
///
/// Copied from `extract_tool_definitions` as of **`08f69b86`**, the commit that declared it. A hand-copy is
/// as trustworthy as any other reviewed data and no less so than a golden - but it cannot be re-derived, so
/// the commit is named here for anyone who needs to check it against the original.
fn legacy_single_tool_definition(
    attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
) -> Option<RawToolDefinition> {
    let tool_name = attrs.get("gen_ai.tool.name")?;
    if !tool_name.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let mut func = json!({ "name": tool_name });
    if let Some(desc) = attrs.get("gen_ai.tool.description") {
        func["description"] = json!(desc);
    }
    if let Some(schema) = attrs
        .get("gen_ai.tool.json_schema")
        .and_then(|s| serde_json::from_str::<JsonValue>(s).ok())
    {
        func["parameters"] = schema;
    }
    Some(RawToolDefinition::from_attr(
        "gen_ai.tool.name",
        timestamp,
        json!([{"type": "function", "function": func}]),
    ))
}

/// The declared triple reproduces the retired one, and comes last.
#[test]
fn the_declared_single_tool_triple_reproduces_the_retired_one() {
    let cases: Vec<Vec<(&str, &str)>> = vec![
        vec![("gen_ai.tool.name", "weather")],
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.description", "looks it up"),
        ],
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.description", "looks it up"),
            ("gen_ai.tool.json_schema", r#"{"type":"object"}"#),
        ],
        // The synthetic aggregate one dialect reports: not a tool anyone can call.
        vec![("gen_ai.tool.name", "(merged tools)")],
        vec![("gen_ai.tool.name", "_private")],
        vec![("gen_ai.tool.name", "9lives")],
        // A malformed schema is not a schema.
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.json_schema", "{not json"),
        ],
    ];
    for case in cases {
        let attrs = make_attrs(&case);
        let expected = legacy_single_tool_definition(&attrs, Utc::now());
        let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
        let declared: Vec<&RawToolDefinition> = defs
            .iter()
            .filter(
                |d| matches!(&d.source, ToolDefinitionSource::Attribute { key, .. } if key == "gen_ai.tool.name"),
            )
            .collect();
        match expected {
            None => assert!(
                declared.is_empty(),
                "the declared rule emitted a definition the retired one refused: {case:?} -> {declared:?}"
            ),
            Some(want) => {
                assert_eq!(declared.len(), 1, "{case:?}");
                assert_eq!(declared[0].content, want.content, "{case:?}");
            }
        }
    }
}

/// The triple comes after every other tool definition, which is what decides the merge.
#[test]
fn the_single_tool_triple_is_read_last() {
    let attrs = make_attrs(&[
        ("llm.tools", r#"[{"name":"listed"}]"#),
        ("gen_ai.tool.name", "triple"),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let order: Vec<String> = defs
        .iter()
        .map(|d| match &d.source {
            ToolDefinitionSource::Attribute { key, .. } => key.clone(),
        })
        .collect();
    assert_eq!(
        order,
        vec!["llm.tools".to_string(), "gen_ai.tool.name".to_string()],
        "the triple must come last - merging keeps the first of two equal-quality definitions with one name"
    );

    // The consequence, asserted rather than described: with the *same* name in both, the position decides
    // which description survives. Reversed, the triple's would win and the list's would be dropped.
    let same_name = make_attrs(&[
        (
            "llm.tools",
            r#"[{"type":"function","function":{"name":"shared","description":"from the list"}}]"#,
        ),
        ("gen_ai.tool.name", "shared"),
        ("gen_ai.tool.description", "from the triple"),
    ]);
    let (defs, _) = extract_tool_definitions("", &same_name, Utc::now());
    let merged = crate::domain::sideml::tools::normalize_tools(&JsonValue::Array(
        defs.iter()
            .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
            .collect(),
    ));
    let descriptions: Vec<&str> = merged
        .as_array()
        .expect("an array of tools")
        .iter()
        .filter_map(|t| t["function"]["description"].as_str())
        .collect();
    // The order survives normalisation, which is what the position guarantees at this level: the merge that
    // keeps the first of two equal-quality definitions with one name happens later, in the feed, and it sees
    // the list's definition first. Reversed here, the triple's description would be the one it kept.
    assert_eq!(
        descriptions,
        vec!["from the list", "from the triple"],
        "the order the merge will see is wrong: {merged:?}"
    );
}

/// CrewAI's metadata carriers, over the matrix the oracle's two cases do not reach.
///
/// The oracle compares only the shapes its cases carry, and those are two minimal `crew_tasks` payloads.
/// This covers what the retired reader actually did and what a re-derivation gets wrong: **both** carriers,
/// **both** member names *together* (it read `tools_names` and `tools`, not one or the other), a rich
/// definition beside a bare name for the same tool (quality selection, which decides which survives), and a
/// `repr` string (the grammar, reached through the declared vocabulary).
#[test]
fn the_crew_metadata_carriers_yield_what_the_retired_reader_did() {
    let names_of = |defs: &[RawToolDefinition], carrier: &str| -> Vec<String> {
        defs.iter()
            .filter(
                |d| matches!(&d.source, ToolDefinitionSource::Attribute { key, .. } if key == carrier),
            )
            .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
            .filter_map(|t| {
                t["function"]["name"]
                    .as_str()
                    .or_else(|| t["name"].as_str())
                    .map(str::to_string)
            })
            .collect()
    };

    // Both carriers are read, each as its own observation.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        ("crew_agents", r#"[{"tools_names":["from_agents"]}]"#),
        ("crew_tasks", r#"[{"tools_names":["from_tasks"]}]"#),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        names_of(&defs, "crew_agents"),
        vec!["from_agents".to_string()]
    );
    assert_eq!(
        names_of(&defs, "crew_tasks"),
        vec!["from_tasks".to_string()]
    );

    // Both member names in one entry, together - not one or the other.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools_names":["named"],"tools":[{"name":"listed"}]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let mut both = names_of(&defs, "crew_agents");
    both.sort();
    assert_eq!(
        both,
        vec!["listed".to_string(), "named".to_string()],
        "both member names are read, which a re-derivation reading one *or* the other misses"
    );

    // A rich definition beside a bare name for one tool: the richer one survives.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools_names":["shared"],"tools":[{"name":"shared","description":"the rich one"}]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let described: Vec<String> = defs
        .iter()
        .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["function"]["description"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        described,
        vec!["the rich one".to_string()],
        "quality selection keeps the richer definition of one name: {defs:?}"
    );
    assert_eq!(names_of(&defs, "crew_agents"), vec!["shared".to_string()]);

    // A `repr` string, reached through the declared vocabulary.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools":["CrewStructuredTool(name='search', description='Tool Arguments: {\"q\": {\"type\": \"str\"}}')"]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(names_of(&defs, "crew_agents"), vec!["search".to_string()]);
}
