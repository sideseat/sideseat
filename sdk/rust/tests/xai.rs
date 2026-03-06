//! Integration tests for xAI Grok provider (direct API only).

#[macro_use]
mod common;
use common::*;

const XAI_MODEL: &str = "grok-3-mini";
const XAI_VISION_MODEL: &str = "grok-4-0709";
const XAI_EMBED_MODEL: &str = "grok-embed";

// ---------------------------------------------------------------------------
// Basic chat
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_complete() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(XAI_MODEL);

    let resp = provider.complete(vec![user_msg("Say hello in one word.")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
    assert!(resp.id.is_some());
    assert!(resp.model.is_some());
}

#[tokio::test]
async fn test_xai_stream() {
    let (server, provider) = mock_xai();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_EVENTS);
    let config = default_config(XAI_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_xai_system_prompt() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let mut config = default_config(XAI_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = provider.complete(vec![user_msg("Hello!")], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_xai_multi_turn() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(XAI_MODEL);

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_xai_tools() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_TOOL_JSON);
    let mut config = default_config(XAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'mango'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_xai_structured_output() {
    use sideseat::types::ResponseFormat;
    let json_body = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"{\"name\":\"Bob\",\"age\":30}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let (server, provider) = mock_xai();
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(json_body);
    });
    let mut config = default_config(XAI_MODEL);
    config.response_format = Some(ResponseFormat::Json);

    let resp = provider
        .complete(vec![user_msg("Return a JSON object with fields 'name' and 'age'.")], config)
        .await
        .unwrap();

    let text = resp.text();
    assert!(!text.is_empty());
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    assert!(parsed.is_ok(), "expected valid JSON, got: {text}");
}

// ---------------------------------------------------------------------------
// Reasoning model
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_reasoning() {
    use sideseat::types::ReasoningEffort;
    // canned response has "221" in text
    let reasoning_resp = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"17 * 13 = 221"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"reasoning_tokens":20}}"#;
    let (server, provider) = mock_xai();
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(reasoning_resp);
    });
    let mut config = default_config(XAI_MODEL);
    config.reasoning_effort = Some(ReasoningEffort::Low);
    config.max_tokens = Some(512);

    let resp = provider
        .complete(vec![user_msg("What is 17 * 13? Think step by step.")], config)
        .await
        .unwrap();

    let text = resp.text();
    assert!(!text.is_empty());
    assert!(text.contains("221"), "expected correct answer 221, got: {text}");
}

#[tokio::test]
async fn test_xai_reasoning_stream() {
    use sideseat::types::ReasoningEffort;
    let stream_events = concat!(
        "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"17 * 13 = 221\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
        "data: [DONE]\n\n",
    );
    let (server, provider) = mock_xai();
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "text/event-stream").body(stream_events);
    });
    let mut config = default_config(XAI_MODEL);
    config.reasoning_effort = Some(ReasoningEffort::Low);
    config.max_tokens = Some(512);

    let stream = provider.stream(vec![user_msg("What is 17 * 13?")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.text().contains("221"));
}

// ---------------------------------------------------------------------------
// Vision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_vision() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let mut config = default_config(XAI_VISION_MODEL);
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

// ---------------------------------------------------------------------------
// Embeddings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_embed() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/embeddings", XAI_EMBED_JSON);

    let request = EmbeddingRequest::new(XAI_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty());
}

// ---------------------------------------------------------------------------
// Logprobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_logprobs() {
    let (server, provider) = mock_xai();
    mock_json(&server, POST, "/chat/completions", OPENAI_LOGPROBS_JSON);
    let config = default_config(XAI_MODEL).with_logprobs(true).with_top_logprobs(2);

    let resp = provider.complete(vec![user_msg("Say hello in one word.")], config).await.unwrap();

    let lp = resp.logprobs.as_ref().expect("expected logprobs in response");
    assert!(!lp.is_empty());
    assert!(!lp[0].top_logprobs.is_empty());
}

// ---------------------------------------------------------------------------
// List models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_list_models() {
    let (server, provider) = mock_xai();
    mock_json(&server, GET, "/models", XAI_LIST_MODELS_JSON);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("grok")));
}
