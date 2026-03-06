//! Integration tests for Google Gemini provider.

#[macro_use]
mod common;
use common::*;

const GEMINI_MODEL: &str = "gemini-2.5-flash-lite";
const GEMINI_EMBED_MODEL: &str = "gemini-embedding-001";

#[tokio::test]
async fn test_gemini_complete() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":generateContent", GEMINI_COMPLETE_JSON);
    let config = default_config(GEMINI_MODEL);

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_gemini_stream() {
    let (server, provider) = mock_gemini();
    mock_sse(&server, POST, ":streamGenerateContent", GEMINI_STREAM_EVENTS);
    let config = default_config(GEMINI_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_gemini_system_prompt() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":generateContent", GEMINI_COMPLETE_JSON);
    let mut config = default_config(GEMINI_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = provider.complete(vec![user_msg("Hello!")], config).await.unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_gemini_multi_turn() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":generateContent", GEMINI_COMPLETE_JSON);
    let config = default_config(GEMINI_MODEL);

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_gemini_tools() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":generateContent", GEMINI_TOOL_JSON);
    let mut config = default_config(GEMINI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'papaya'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_gemini_structured_output() {
    use sideseat::types::ResponseFormat;
    let structured_json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"{\"animal\":\"cat\",\"sound\":\"meow\"}"}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
    let (server, provider) = mock_gemini();
    server.mock(|when, then| {
        when.method(POST).path_includes(":generateContent");
        then.status(200).header("content-type", "application/json").body(structured_json);
    });
    let mut config = default_config(GEMINI_MODEL);
    config.max_tokens = Some(2048);
    config.response_format = Some(ResponseFormat::JsonSchema {
        name: "animal_sound".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "animal": {"type": "string"},
                "sound": {"type": "string"}
            },
            "required": ["animal", "sound"]
        }),
        strict: false,
    });

    let resp = provider.complete(vec![user_msg("Name an animal and the sound it makes.")], config).await.unwrap();

    let text = resp.text();
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("response should be valid JSON");
    assert!(parsed["animal"].is_string());
    assert!(parsed["sound"].is_string());
}

#[tokio::test]
async fn test_gemini_count_tokens() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":countTokens", GEMINI_COUNT_TOKENS_JSON);
    let config = default_config(GEMINI_MODEL);

    let count = provider.count_tokens(vec![user_msg("Hello, how are you?")], config).await.unwrap();
    assert!(count.input_tokens > 0);
}

#[tokio::test]
async fn test_gemini_embed() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":embedContent", GEMINI_EMBED_JSON);

    let req = EmbeddingRequest::new(GEMINI_EMBED_MODEL, vec!["Hello world"]);
    let resp = provider.embed(req).await.unwrap();
    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty());
}

#[tokio::test]
async fn test_gemini_vision() {
    let (server, provider) = mock_gemini();
    mock_json(&server, POST, ":generateContent", GEMINI_COMPLETE_JSON);
    let mut config = default_config(GEMINI_MODEL);
    config.max_tokens = Some(128);

    let image = ContentBlock::Image(ImageContent {
        source: MediaSource::Base64(sideseat::types::Base64Data {
            media_type: "image/png".to_string(),
            data: TINY_PNG_B64.to_string(),
        }),
        format: None,
        detail: None,
    });

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::text("What color is this image?".to_string()), image],
        name: None,
        cache_control: None,
    }];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_gemini_list_models() {
    let (server, provider) = mock_gemini();
    // Register models mock specifically (not for generateContent paths)
    server.mock(|when, then| {
        when.method(GET).path_includes("/models");
        then.status(200).header("content-type", "application/json").body(GEMINI_LIST_MODELS_JSON);
    });

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("gemini")));
}
