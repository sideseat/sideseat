//! Integration tests for Google Gemini provider.
//!
//! ```bash
//! GEMINI_API_KEY=AIza... cargo test -p sideseat --test gemini -- --nocapture
//! ```

#[macro_use]
mod common;
use common::*;

macro_rules! gemini_api_key_env {
    () => {{
        match std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY")) {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: GEMINI_API_KEY / GOOGLE_API_KEY not set");
                return;
            }
        }
    }};
}

// Latest Gemini model with free-tier quota
const GEMINI_MODEL: &str = "gemini-3-flash-preview";
const GEMINI_EMBED_MODEL: &str = "gemini-embedding-001";

#[tokio::test]
async fn test_gemini_complete() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config(GEMINI_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_gemini_stream() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config(GEMINI_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_gemini_system_prompt() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let mut config = default_config(GEMINI_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello!")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_gemini_multi_turn() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config(GEMINI_MODEL);

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
async fn test_gemini_tools() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let mut config = default_config(GEMINI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = retry(|| {
        provider.complete(vec![user_msg("Please echo the word 'papaya'")], config.clone())
    })
    .await
    .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_gemini_structured_output() {
    use sideseat::types::ResponseFormat;

    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let mut config = default_config(GEMINI_MODEL);
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

    let resp = retry(|| {
        provider.complete(vec![user_msg("Name an animal and the sound it makes.")], config.clone())
    })
    .await
    .unwrap();

    let text = resp.text();
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("response should be valid JSON");
    assert!(parsed["animal"].is_string());
    assert!(parsed["sound"].is_string());
}

#[tokio::test]
async fn test_gemini_count_tokens() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config(GEMINI_MODEL);
    let count = provider
        .count_tokens(vec![user_msg("Hello, how are you?")], config)
        .await
        .unwrap();
    assert!(count.input_tokens > 0, "should report non-zero token count");
}

#[tokio::test]
async fn test_gemini_embed() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let req = EmbeddingRequest::new(GEMINI_EMBED_MODEL, vec!["Hello world"]);
    let resp = provider.embed(req).await.unwrap();
    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty(), "embedding vector should not be empty");
}

#[tokio::test]
async fn test_gemini_vision() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
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

    let resp = retry(|| provider.complete(messages.clone(), config.clone()))
        .await
        .unwrap();
    assert!(!resp.text().is_empty(), "expected non-empty response");
}

#[tokio::test]
async fn test_gemini_list_models() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("gemini")),
        "expected a gemini model"
    );
}
