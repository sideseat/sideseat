use super::*;
use crate::types::{AudioFormat, AudioOutputConfig, StopReason};
use serde_json::json;

#[test]
fn test_build_request_basic() {
    let config = ProviderConfig::new("gpt-4.1").with_max_tokens(512);
    let messages = vec![Message::user("Hello")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["model"], "gpt-4.1");
    assert_eq!(req["max_completion_tokens"], 512);
    assert_eq!(req["messages"][0]["role"], "user");
}

#[test]
fn test_system_injection() {
    let config = ProviderConfig::new("gpt-4.1")
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
    let config = ProviderConfig::new("gpt-4.1")
        .with_tools(vec![Tool::new(
            "search",
            "Search",
            json!({"type": "object"}),
        )])
        .with_tool_choice(ToolChoice::Any);
    let messages = vec![Message::user("Search")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["tool_choice"], "required");
}

#[test]
fn test_logprobs() {
    let config = ProviderConfig::new("gpt-4.1")
        .with_logprobs(true)
        .with_top_logprobs(3);
    let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
    assert_eq!(req["logprobs"], true);
    assert_eq!(req["top_logprobs"], 3);
}

#[test]
fn test_top_logprobs_implies_logprobs() {
    // When only top_logprobs is set, logprobs=true must be injected automatically
    let config = ProviderConfig::new("gpt-4.1").with_top_logprobs(5);
    let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
    assert_eq!(req["logprobs"], true);
    assert_eq!(req["top_logprobs"], 5);
}

#[test]
fn test_store() {
    let config = ProviderConfig::new("gpt-4.1").with_store(false);
    let req = build_request(&[Message::user("Hi")], &config, false).unwrap();
    assert_eq!(req["store"], false);
}

#[test]
fn test_audio_output() {
    let config = ProviderConfig::new("gpt-4o-audio-preview")
        .with_audio_output(AudioOutputConfig::new("alloy").with_format(AudioFormat::Wav));
    let req = build_request(&[Message::user("Say hello")], &config, false).unwrap();
    assert_eq!(req["modalities"], json!(["text", "audio"]));
    assert_eq!(req["audio"]["voice"], "alloy");
    assert_eq!(req["audio"]["format"], "wav");
}

#[test]
fn test_audio_output_pcm16() {
    let config = ProviderConfig::new("gpt-4o-audio-preview")
        .with_audio_output(AudioOutputConfig::new("nova").with_format(AudioFormat::Pcm16));
    let req = build_request(&[Message::user("Say hi")], &config, false).unwrap();
    assert_eq!(req["audio"]["format"], "pcm16");
}

#[test]
fn test_audio_output_no_format() {
    // When no format specified, audio object has only voice — no format key
    let config = ProviderConfig::new("gpt-4o-audio-preview")
        .with_audio_output(AudioOutputConfig::new("shimmer"));
    let req = build_request(&[Message::user("Hello")], &config, false).unwrap();
    assert_eq!(req["modalities"], json!(["text", "audio"]));
    assert_eq!(req["audio"]["voice"], "shimmer");
    assert!(req["audio"]["format"].is_null());
}

#[test]
fn test_structured_output() {
    let mut config = ProviderConfig::new("gpt-4.1").with_max_tokens(256);
    config.extra.insert(
        "output_schema".to_string(),
        json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
    );
    let messages = vec![Message::user("Give me a name")];
    let req = build_request(&messages, &config, false).unwrap();
    assert_eq!(req["response_format"]["type"], "json_schema");
}

#[test]
fn test_parse_response() {
    let json = json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "model": "gpt-4.1",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello there!"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello there!"));
    assert_eq!(resp.usage.input_tokens, 8);
}

#[test]
fn test_parse_tool_call() {
    let json = json!({
        "id": "chatcmpl-456",
        "object": "chat.completion",
        "model": "gpt-4.1",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"location\":\"NYC\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30}
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
    assert!(matches!(resp.stop_reason, StopReason::ToolUse));
}

#[tokio::test]
async fn test_integration_complete() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: OPENAI_API_KEY not set");
            return;
        }
    };
    let provider = OpenAIChatProvider::new(api_key);
    let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hello' in one word.")];
    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.content.is_empty());
}

#[tokio::test]
async fn test_integration_stream() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: OPENAI_API_KEY not set");
            return;
        }
    };
    use crate::provider::collect_stream;
    let provider = OpenAIChatProvider::new(api_key);
    let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hi'.")];
    let stream = provider.stream(messages, config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.content.is_empty());
}
