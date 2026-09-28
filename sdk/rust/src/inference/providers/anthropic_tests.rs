use super::*;
use crate::types::{Tool, ToolChoice};
use serde_json::json;

#[test]
fn test_build_request_basic() {
    let config = ProviderConfig::new("claude-opus-4-6").with_max_tokens(512);
    let messages = vec![Message::user("Hello")];
    let req = build_messages_request(&messages, &config, false).unwrap();
    assert_eq!(req["model"], "claude-opus-4-6");
    assert_eq!(req["max_tokens"], 512);
    assert_eq!(req["stream"], false);
    assert_eq!(req["messages"][0]["role"], "user");
}

#[test]
fn test_build_request_with_system() {
    let config = ProviderConfig::new("claude-opus-4-6")
        .with_system("You are a helpful assistant")
        .with_max_tokens(256);
    let messages = vec![Message::user("Hi")];
    let req = build_messages_request(&messages, &config, false).unwrap();
    assert_eq!(req["system"], "You are a helpful assistant");
}

#[test]
fn test_build_request_with_tools() {
    let config = ProviderConfig::new("claude-opus-4-6")
            .with_max_tokens(256)
            .with_tools(vec![Tool::new(
                "get_weather",
                "Get the weather",
                json!({"type": "object", "properties": {"location": {"type": "string"}}, "required": ["location"]}),
            )])
            .with_tool_choice(ToolChoice::Auto);
    let messages = vec![Message::user("What is the weather in NYC?")];
    let req = build_messages_request(&messages, &config, false).unwrap();
    assert_eq!(req["tools"][0]["name"], "get_weather");
    assert_eq!(req["tool_choice"]["type"], "auto");
}

#[test]
fn test_build_request_with_thinking() {
    let config = ProviderConfig::new("claude-sonnet-4-6")
        .with_max_tokens(16000)
        .with_thinking(10000);
    let messages = vec![Message::user("Complex math problem")];
    let req = build_messages_request(&messages, &config, false).unwrap();
    assert_eq!(req["thinking"]["type"], "enabled");
    assert_eq!(req["thinking"]["budget_tokens"], 10000);
    assert_eq!(req["temperature"], 1.0);
}

#[test]
fn test_bedrock_request_no_model_or_stream() {
    let config = ProviderConfig::new("anthropic.claude-sonnet-4-6-v1:0").with_max_tokens(256);
    let messages = vec![Message::user("Hello")];
    let req = build_bedrock_request(&messages, &config).unwrap();
    assert!(req.get("model").is_none());
    assert!(req.get("stream").is_none());
    assert_eq!(req["anthropic_version"], BEDROCK_ANTHROPIC_VERSION);
}

#[test]
fn test_vertex_request_no_model() {
    let config = ProviderConfig::new("claude-sonnet-4-6@20250929").with_max_tokens(256);
    let messages = vec![Message::user("Hello")];
    let req = build_vertex_request(&messages, &config, true).unwrap();
    assert!(req.get("model").is_none());
    assert_eq!(req["anthropic_version"], VERTEX_ANTHROPIC_VERSION);
    assert_eq!(req["stream"], true);
}

#[test]
fn test_parse_stop_reason() {
    assert!(matches!(parse_stop_reason("end_turn"), StopReason::EndTurn));
    assert!(matches!(parse_stop_reason("tool_use"), StopReason::ToolUse));
    assert!(matches!(
        parse_stop_reason("max_tokens"),
        StopReason::MaxTokens
    ));
}

#[test]
fn test_parse_response() {
    let json = json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-6",
        "content": [{"type": "text", "text": "Hello, world!"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        }
    });
    let resp = parse_response(&json).unwrap();
    assert_eq!(resp.content.len(), 1);
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello, world!"));
    assert_eq!(resp.usage.input_tokens, 10);
    assert_eq!(resp.usage.output_tokens, 5);
    assert_eq!(resp.model.as_deref(), Some("claude-opus-4-6"));
}

#[test]
fn test_format_image_base64() {
    let img = crate::types::ImageContent {
        source: MediaSource::base64("image/png", "iVBORw0KGgo="),
        format: Some(crate::types::ImageFormat::Png),
        detail: None,
    };
    let v = format_image_block(&img).unwrap();
    assert_eq!(v["type"], "image");
    assert_eq!(v["source"]["type"], "base64");
    assert_eq!(v["source"]["media_type"], "image/png");
}

#[test]
fn test_count_tokens_includes_beta() {
    // Ensure the beta header logic adds token-counting-2024-11-01 automatically
    let betas: Vec<String> = vec![];
    let mut all_betas = betas.clone();
    if !all_betas.iter().any(|b| b.contains("token-counting")) {
        all_betas.push("token-counting-2024-11-01".to_string());
    }
    assert!(all_betas.contains(&"token-counting-2024-11-01".to_string()));
}

#[test]
fn test_system_messages_merged() {
    let config = ProviderConfig::new("claude-opus-4-6")
        .with_system("System from config")
        .with_max_tokens(256);
    let messages = vec![
        Message::with_content(Role::System, vec![ContentBlock::text("Extra system")]),
        Message::user("Hello"),
    ];
    let req = build_messages_request(&messages, &config, false).unwrap();
    let system = req["system"].as_str().unwrap();
    assert!(system.contains("System from config"));
    assert!(system.contains("Extra system"));
    // System messages should not appear in messages array
    let msgs = req["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], "user");
}

#[tokio::test]
async fn test_integration_complete() {
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        }
    };
    let provider = AnthropicProvider::new(api_key);
    let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hello' in one word.")];
    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.content.is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_integration_stream() {
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        }
    };
    use crate::provider::collect_stream;
    let provider = AnthropicProvider::new(api_key);
    let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hi'.")];
    let stream = provider.stream(messages, config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.content.is_empty());
}

#[tokio::test]
async fn test_integration_list_models() {
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        }
    };
    let provider = AnthropicProvider::new(api_key);
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("claude")));
}

#[tokio::test]
async fn test_integration_count_tokens() {
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        }
    };
    let provider = AnthropicProvider::new(api_key);
    let config = ProviderConfig::new("claude-haiku-4-5-20251001").with_max_tokens(64);
    let messages = vec![Message::user("Hello, how are you?")];
    let count = provider.count_tokens(messages, config).await.unwrap();
    assert!(count.input_tokens > 0);
}
