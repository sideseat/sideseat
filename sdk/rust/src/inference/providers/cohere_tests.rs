use super::*;
use serde_json::json;

#[test]
fn test_build_request_basic() {
    let config = ProviderConfig::new("command-r-plus").with_max_tokens(512);
    let messages = vec![Message::user("Hello")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["model"], "command-r-plus");
    assert_eq!(req["max_tokens"], 512);
    assert_eq!(req["stream"], false);
    assert_eq!(req["messages"][0]["role"], "user");
    assert_eq!(req["messages"][0]["content"], "Hello");
}

#[test]
fn test_system_injection() {
    let config = ProviderConfig::new("command-r-plus")
        .with_system("Be helpful")
        .with_max_tokens(256);
    let messages = vec![Message::user("Hi")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["messages"][0]["role"], "system");
    assert_eq!(req["messages"][0]["content"], "Be helpful");
    assert_eq!(req["messages"][1]["role"], "user");
}

#[test]
fn test_tool_choice_required() {
    use crate::types::Tool;
    let config = ProviderConfig::new("command-r-plus")
        .with_tools(vec![Tool::new(
            "search",
            "Search the web",
            json!({"type": "object"}),
        )])
        .with_tool_choice(ToolChoice::Any);
    let messages = vec![Message::user("Search something")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["tool_choice"], "required");
    assert_eq!(req["tools"][0]["function"]["name"], "search");
}

#[test]
fn test_parse_response_text() {
    let json = json!({
        "id": "chat-123",
        "finish_reason": "COMPLETE",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello there!"}]
        },
        "usage": {
            "billed_units": {"input_tokens": 10, "output_tokens": 5},
            "tokens": {"input_tokens": 10, "output_tokens": 5}
        }
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello there!"));
    assert_eq!(resp.usage.input_tokens, 10);
    assert_eq!(resp.usage.output_tokens, 5);
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

#[test]
fn test_parse_response_tool_call() {
    let json = json!({
        "id": "chat-456",
        "finish_reason": "TOOL_CALL",
        "message": {
            "role": "assistant",
            "content": [],
            "tool_calls": [{
                "id": "tc_abc",
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Paris\"}"
                }
            }]
        },
        "usage": {
            "billed_units": {"input_tokens": 20, "output_tokens": 10},
            "tokens": {"input_tokens": 20, "output_tokens": 10}
        }
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
    assert!(matches!(resp.stop_reason, StopReason::ToolUse));
}

#[test]
fn test_parse_finish_reasons() {
    assert!(matches!(
        parse_finish_reason("COMPLETE"),
        StopReason::EndTurn
    ));
    assert!(matches!(
        parse_finish_reason("MAX_TOKENS"),
        StopReason::MaxTokens
    ));
    assert!(matches!(
        parse_finish_reason("TOOL_CALL"),
        StopReason::ToolUse
    ));
    assert!(matches!(
        parse_finish_reason("STOP_SEQUENCE"),
        StopReason::StopSequence(_)
    ));
    assert!(matches!(parse_finish_reason("ERROR"), StopReason::Other(s) if s == "error"));
}

#[test]
fn test_with_api_base() {
    let provider = CohereProvider::new("key").with_api_base("https://proxy.example.com/v2");
    if let CohereBackend::Direct {
        base_url, api_base, ..
    } = &provider.backend
    {
        assert_eq!(base_url, "https://proxy.example.com/v2/chat");
        assert_eq!(api_base, "https://proxy.example.com/v2");
    } else {
        panic!("expected Direct backend");
    }
}

#[test]
fn test_tool_result_formatting() {
    use crate::types::{ContentBlock, ToolResultBlock};
    let messages = vec![
        Message::user("Call the tool"),
        Message::with_content(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "tc_1".into(),
                name: "search".into(),
                input: json!({"query": "rust"}),
            })],
        ),
        Message::with_content(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "tc_1".into(),
                content: vec![ContentBlock::text("Results here")],
                is_error: false,
            })],
        ),
    ];
    let formatted = format_messages(&messages, None).unwrap();
    let arr = formatted.as_array().unwrap();
    let tool_msg = arr.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool_msg["tool_call_id"], "tc_1");
    assert_eq!(tool_msg["content"], "Results here");
}

#[test]
fn test_build_bedrock_request_basic() {
    let config = ProviderConfig::new("cohere.command-r-v1:0").with_max_tokens(256);
    let messages = vec![Message::user("Hello")];
    let req = build_bedrock_request(&messages, &config).unwrap();
    assert_eq!(req["message"], "Hello");
    assert_eq!(req["max_tokens"], 256);
    assert!(!req.as_object().unwrap().contains_key("model"));
    assert!(!req.as_object().unwrap().contains_key("stream"));
}

#[test]
fn test_build_bedrock_request_system() {
    let config = ProviderConfig::new("cohere.command-r-v1:0")
        .with_system("You are helpful")
        .with_max_tokens(256);
    let messages = vec![Message::user("Hi")];
    let req = build_bedrock_request(&messages, &config).unwrap();
    assert_eq!(req["preamble"], "You are helpful");
    assert_eq!(req["message"], "Hi");
}

#[test]
fn test_build_bedrock_request_chat_history() {
    let config = ProviderConfig::new("cohere.command-r-v1:0").with_max_tokens(256);
    let messages = vec![
        Message::user("What is 2+2?"),
        Message::assistant("4"),
        Message::user("And 3+3?"),
    ];
    let req = build_bedrock_request(&messages, &config).unwrap();
    assert_eq!(req["message"], "And 3+3?");
    let history = req["chat_history"].as_array().unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["role"], "USER");
    assert_eq!(history[0]["message"], "What is 2+2?");
    assert_eq!(history[1]["role"], "CHATBOT");
    assert_eq!(history[1]["message"], "4");
}

#[test]
fn test_format_bedrock_tools() {
    let tools = vec![Tool::new(
        "get_weather",
        "Get the weather",
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The city"
                }
            },
            "required": ["location"]
        }),
    )];
    let formatted = format_bedrock_tools(&tools);
    let tool = &formatted[0];
    assert_eq!(tool["name"], "get_weather");
    let param = &tool["parameter_definitions"]["location"];
    assert_eq!(param["type"], "str");
    assert_eq!(param["required"], true);
    assert_eq!(param["description"], "The city");
}

#[test]
fn test_parse_bedrock_response_text() {
    let json = json!({
        "id": "b1",
        "text": "Hello!",
        "finish_reason": "COMPLETE",
        "usage": {
            "billed_units": { "input_tokens": 5, "output_tokens": 3 }
        }
    });
    let resp = parse_bedrock_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello!"));
    assert_eq!(resp.usage.input_tokens, 5);
    assert_eq!(resp.usage.output_tokens, 3);
}

#[test]
fn test_parse_bedrock_response_tool_call() {
    let json = json!({
        "id": "b2",
        "text": "",
        "finish_reason": "TOOL_CALL",
        "tool_calls": [{
            "id": "tc_1",
            "name": "get_weather",
            "parameters": { "location": "Paris" }
        }],
        "usage": {
            "billed_units": { "input_tokens": 15, "output_tokens": 8 }
        }
    });
    let resp = parse_bedrock_response(&json).unwrap();
    assert!(
        matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather" && tu.input["location"] == "Paris")
    );
    assert!(matches!(resp.stop_reason, StopReason::ToolUse));
}

#[tokio::test]
async fn test_integration_complete() {
    let api_key = match std::env::var("COHERE_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: COHERE_API_KEY not set");
            return;
        }
    };
    let provider = CohereProvider::new(api_key);
    let config = ProviderConfig::new("command-r-08-2024").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hello' in one word.")];
    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.content.is_empty());
}

#[tokio::test]
async fn test_integration_stream() {
    let api_key = match std::env::var("COHERE_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: COHERE_API_KEY not set");
            return;
        }
    };
    use crate::provider::collect_stream;
    let provider = CohereProvider::new(api_key);
    let config = ProviderConfig::new("command-r-08-2024").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hi'.")];
    let stream = provider.stream(messages, config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.content.is_empty());
}
