//! Integration tests for xAI Grok provider (direct API only).
//!
//! xAI is not available on AWS Bedrock; only direct API access is supported.
//!
//! ```bash
//! XAI_API_KEY=... cargo test -p sideseat --test xai -- --nocapture
//! ```

#[macro_use]
mod common;
use common::*;

macro_rules! xai_api_key_env {
    () => {{
        match std::env::var("XAI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: XAI_API_KEY not set");
                return;
            }
        }
    }};
}

// Latest cost-efficient model — grok-3-mini has reasoning and is cheapest
const XAI_MODEL: &str = "grok-3-mini";
// Latest flagship with vision support
const XAI_VISION_MODEL: &str = "grok-4-0709";
// Embedding model
const XAI_EMBED_MODEL: &str = "grok-embed";

// ---------------------------------------------------------------------------
// Basic chat
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_complete() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let config = default_config(XAI_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
    assert!(resp.id.is_some(), "expected response id");
    assert!(resp.model.is_some(), "expected model in response");
}

#[tokio::test]
async fn test_xai_stream() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let config = default_config(XAI_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty streaming response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
}

#[tokio::test]
async fn test_xai_system_prompt() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let mut config = default_config(XAI_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello!")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
}

#[tokio::test]
async fn test_xai_multi_turn() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let config = default_config(XAI_MODEL);

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    let resp = retry(|| provider.complete(messages.clone(), config.clone()))
        .await
        .unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("alice"), "expected model to recall name, got: {text}");
}

#[tokio::test]
async fn test_xai_tools() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let mut config = default_config(XAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = retry(|| {
        provider.complete(vec![user_msg("Please echo the word 'mango'")], config.clone())
    })
    .await
    .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_xai_structured_output() {
    use sideseat::types::ResponseFormat;

    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let mut config = default_config(XAI_MODEL);
    config.response_format = Some(ResponseFormat::Json);

    let resp = retry(|| {
        provider.complete(
            vec![user_msg(
                "Return a JSON object with fields 'name' (string) and 'age' (number). \
                 Name: Bob, Age: 30.",
            )],
            config.clone(),
        )
    })
    .await
    .unwrap();

    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response");
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    assert!(parsed.is_ok(), "expected valid JSON, got: {text}");
}

// ---------------------------------------------------------------------------
// Reasoning model (grok-3-mini with reasoning_effort)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_reasoning() {
    use sideseat::types::ReasoningEffort;

    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let mut config = default_config(XAI_MODEL);
    config.reasoning_effort = Some(ReasoningEffort::Low);
    config.max_tokens = Some(512);

    let resp = retry(|| {
        provider.complete(
            vec![user_msg("What is 17 * 13? Think step by step.")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response");
    assert!(text.contains("221"), "expected correct answer 221, got: {text}");
    // grok-3-mini with reasoning_effort returns thinking blocks
    let has_thinking = resp.content.iter().any(|b| matches!(b, ContentBlock::Thinking(_)));
    println!("Reasoning tokens: {}, has_thinking: {has_thinking}", resp.usage.reasoning_tokens);
}

#[tokio::test]
async fn test_xai_reasoning_stream() {
    use sideseat::types::ReasoningEffort;

    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let mut config = default_config(XAI_MODEL);
    config.reasoning_effort = Some(ReasoningEffort::Low);
    config.max_tokens = Some(512);

    let stream = provider.stream(
        vec![user_msg("What is 17 * 13? Think step by step.")],
        config,
    );
    let resp = collect_stream(stream).await.unwrap();

    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response");
    assert!(text.contains("221"), "expected correct answer 221, got: {text}");
}

// ---------------------------------------------------------------------------
// Vision (grok-4-0709)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_vision() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
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

    match retry(|| provider.complete(messages.clone(), config.clone())).await {
        Ok(r) => assert!(!r.text().is_empty(), "expected non-empty response"),
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("not found")
                || msg.contains("access")
                || msg.contains("not available")
                || msg.contains("unavailable")
                || msg.contains("capacity")
            {
                println!("Skipping vision test: {e}");
            } else {
                panic!("unexpected error: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Embeddings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_embed() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);

    let request = EmbeddingRequest::new(XAI_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    match provider.embed(request).await {
        Ok(resp) => {
            assert_eq!(resp.embeddings.len(), 2, "expected 2 embeddings");
            assert!(!resp.embeddings[0].is_empty(), "expected non-empty embedding");
            assert!(resp.embeddings[0].len() > 100, "expected embedding dimension > 100");
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("not found") || msg.contains("model") || msg.contains("access") {
                println!("Skipping embed test (model unavailable): {e}");
            } else {
                panic!("unexpected error: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Logprobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_logprobs() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);
    let config = default_config(XAI_MODEL).with_logprobs(true).with_top_logprobs(2);

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await
        .unwrap();

    let lp = resp.logprobs.as_ref().expect("expected logprobs in response");
    assert!(!lp.is_empty(), "expected non-empty logprobs");
    assert!(!lp[0].top_logprobs.is_empty(), "expected top_logprobs entries");
}

// ---------------------------------------------------------------------------
// List models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_xai_list_models() {
    let api_key = xai_api_key_env!();
    let provider = XAIProvider::new(api_key);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "expected at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("grok")),
        "expected a grok model, got: {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}
