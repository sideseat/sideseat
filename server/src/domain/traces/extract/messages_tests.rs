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

    // Literal content preserved (no metadata wrapper)
    let msg = &messages[0];
    assert_eq!(
        msg.content.get("raw").and_then(|v| v.as_str()),
        Some("Hello! Welcome! Here's the forecast...")
    );
    assert_eq!(
        msg.content.get("agent").and_then(|v| v.as_str()),
        Some("Weather Forecaster")
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
    // Should extract 3 individual messages from the messages array
    assert_eq!(messages.len(), 3);

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());
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
    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());
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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool_call"));
    let content = msg.get("content").unwrap();
    assert_eq!(content["city"].as_str(), Some("New York"));
    assert_eq!(content["units"].as_str(), Some("fahrenheit"));
}

#[test]
fn test_otel_tool_call_result_extraction() {
    let result = r#"{"temperature":72,"conditions":"sunny"}"#;
    let attrs = make_attrs(&[("gen_ai.tool.call.result", result)]);
    let mut messages = Vec::new();
    let found = try_otel_genai_messages(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1);
    let msg = &messages[0].content;
    assert_eq!(msg.get("role").and_then(|r| r.as_str()), Some("tool"));
    let content = msg.get("content").unwrap();
    assert_eq!(content["temperature"].as_i64(), Some(72));
    assert_eq!(content["conditions"].as_str(), Some("sunny"));
}

#[test]
fn test_otel_tool_definitions_extraction() {
    let tool_defs = r#"[{"name":"get_weather","description":"Get weather for a city","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]"#;
    let attrs = make_attrs(&[("gen_ai.tool.definitions", tool_defs)]);
    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let content = messages[0].content.as_object().unwrap();
    let args = content.get("content").unwrap();
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

    let msg = &messages[0];
    let content = msg.content.as_object().unwrap();
    assert_eq!(
        content.get("role").and_then(|v| v.as_str()),
        Some("tool_call")
    );
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);

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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let (tool_definitions, _tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let msgs = extract_message_from_event(&event, false);
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

    let msgs = extract_message_from_event(&event, false);
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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let msgs = extract_message_from_event(&event, false);
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
        extract_messages_for_span(&span, &span_attrs, Utc::now());

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
        extract_messages_for_span(&span, &span_attrs, Utc::now());

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
        extract_messages_for_span(&span, &span_attrs, Utc::now());

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
        extract_messages_for_span(&span, &span_attrs, Utc::now());

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
        extract_messages_for_span(&span, &span_attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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

    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, Utc::now());

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
    let (messages, _tool_defs, _tool_names) =
        extract_messages_for_span(&otlp_span, &span_attrs, Utc::now());

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
    let (messages, _tool_defs, _tool_names) =
        extract_messages_for_span(&otlp_span, &span_attrs, Utc::now());

    // Should extract 4 individual messages from the messages array
    assert_eq!(
        messages.len(),
        4,
        "Should extract 4 individual messages from CrewAI output.value.messages array. Got: {}",
        messages.len()
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
        4,
        "Serialized array should have 4 messages"
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
    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, timestamp);

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
    let (tool_definitions, tool_names) = extract_tool_definitions(&attrs, timestamp);

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

    assert!(tool_defs.is_empty());
}

#[test]
fn test_openai_agents_response_no_tools_field() {
    // Response without tools field (e.g., non-agent response) → no tool_definitions
    let response_json = r#"{"id":"resp_3","output":[{"type":"message","content":[{"type":"text","text":"hello"}]}]}"#;
    let attrs = make_attrs(&[("response", response_json)]);
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (tool_defs, _) = extract_tool_definitions(&attrs, Utc::now());

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
        let (tool_definitions, _) = extract_tool_definitions(&attrs, Utc::now());

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
    let (messages, _tool_defs, _tool_names) =
        extract_messages_for_span(&otlp_span, &span_attrs, Utc::now());

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
