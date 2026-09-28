use super::*;
use crate::types::StopReason;
use serde_json::json;

#[test]
fn test_build_request_random_seed() {
    let messages = vec![Message::user("Hello")];
    let mut config = ProviderConfig::new("mistral-large-latest").with_max_tokens(100);
    config.seed = Some(42);
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["random_seed"], json!(42));
    assert!(req.get("seed").is_none(), "must use random_seed, not seed");
}

#[test]
fn test_build_request_no_top_k() {
    let messages = vec![Message::user("Hello")];
    let mut config = ProviderConfig::new("mistral-large-latest");
    config.top_k = Some(50);
    let req = build_request(&messages, &config, false).unwrap();
    assert!(req.get("top_k").is_none(), "Mistral does not support top_k");
}

#[test]
fn test_build_request_response_format_json() {
    let messages = vec![Message::user("Hello")];
    let mut config = ProviderConfig::new("mistral-large-latest");
    config.response_format = Some(ResponseFormat::Json);
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["response_format"]["type"], "json_object");
}

#[test]
fn test_build_request_response_format_json_schema() {
    let messages = vec![Message::user("Hello")];
    let mut config = ProviderConfig::new("mistral-large-latest");
    config.response_format = Some(ResponseFormat::JsonSchema {
        name: "Answer".to_string(),
        schema: json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}),
        strict: true,
    });
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["response_format"]["type"], "json_schema");
    assert_eq!(req["response_format"]["json_schema"]["name"], "Answer");
    assert_eq!(req["response_format"]["json_schema"]["strict"], true);
}

#[test]
fn test_build_request_safe_prompt_via_extra() {
    let messages = vec![Message::user("Hello")];
    let mut config = ProviderConfig::new("mistral-large-latest");
    config.extra.insert("safe_prompt".to_string(), json!(true));
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["safe_prompt"], json!(true));
}

#[test]
fn test_format_messages_system() {
    let messages = vec![Message::user("Hi")];
    let result = format_messages(&messages, Some("You are helpful.")).unwrap();
    let arr = result.as_array().unwrap();
    assert_eq!(arr[0]["role"], "system");
    assert_eq!(arr[0]["content"], "You are helpful.");
    assert_eq!(arr[1]["role"], "user");
}

#[test]
fn test_format_messages_tool_result() {
    use crate::types::{ContentBlock, ToolResultBlock};
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: "call_1".to_string(),
            content: vec![ContentBlock::text("42 degrees")],
            is_error: false,
        })],
        name: None,
        cache_control: None,
    }];
    let result = format_messages(&messages, None).unwrap();
    let arr = result.as_array().unwrap();
    assert_eq!(arr[0]["role"], "tool");
    assert_eq!(arr[0]["tool_call_id"], "call_1");
    assert_eq!(arr[0]["content"], "42 degrees");
}

#[test]
fn test_parse_response_text() {
    let json = json!({
        "id": "msg-123",
        "model": "mistral-large-latest",
        "choices": [{
            "message": {"role": "assistant", "content": "Hello!"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t.text == "Hello!"));
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.usage.output_tokens, 3);
    assert_eq!(resp.id.as_deref(), Some("msg-123"));
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

#[test]
fn test_parse_response_tool_call() {
    let json = json!({
        "id": "msg-456",
        "model": "mistral-large-latest",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "D681PevKs",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 15, "completion_tokens": 8, "total_tokens": 23}
    });
    let resp = parse_response(&json).unwrap();
    assert!(
        matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather" && tu.id == "D681PevKs")
    );
    assert!(matches!(resp.stop_reason, StopReason::ToolUse));
}

#[test]
fn test_format_tool_choice() {
    assert_eq!(format_tool_choice(&ToolChoice::Auto), json!("auto"));
    assert_eq!(format_tool_choice(&ToolChoice::None), json!("none"));
    assert_eq!(format_tool_choice(&ToolChoice::Any), json!("any"));
    assert_eq!(
        format_tool_choice(&ToolChoice::Tool {
            name: "my_fn".into()
        }),
        json!({"type": "function", "function": {"name": "my_fn"}})
    );
}
