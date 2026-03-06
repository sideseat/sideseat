//! Integration tests for Mistral provider (direct API + Mistral models via Bedrock).
//!
//! ```bash
//! MISTRAL_API_KEY=... cargo test -p sideseat -- --nocapture mistral
//!
//! # Mistral via Bedrock (API key / bearer token):
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture mistral_bedrock
//! ```

#[macro_use]
mod common;
use common::*;

macro_rules! mistral_api_key_env {
    () => {{
        match std::env::var("MISTRAL_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: MISTRAL_API_KEY not set");
                return;
            }
        }
    }};
}

const MISTRAL_MODEL: &str = "mistral-small-latest";
const MISTRAL_EMBED_MODEL: &str = "mistral-embed";

// Mistral Large 2407 is the most capable Mistral model on Bedrock.
// Mistral Bedrock models do not use cross-region inference profiles.
fn bedrock_mistral_large(region: &str) -> String {
    let _ = region;
    "mistral.mistral-large-2407-v1:0".to_string()
}

fn bedrock_mistral_small(region: &str) -> String {
    let _ = region;
    "mistral.mistral-small-2402-v1:0".to_string()
}

// ---------------------------------------------------------------------------
// Mistral direct API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mistral_complete() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let config = default_config(MISTRAL_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
    assert!(resp.id.is_some(), "expected response id");
}

#[tokio::test]
async fn test_mistral_stream() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let config = default_config(MISTRAL_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty streaming response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
}

#[tokio::test]
async fn test_mistral_system_prompt() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let mut config = default_config(MISTRAL_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello!")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
}

#[tokio::test]
async fn test_mistral_multi_turn() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let config = default_config(MISTRAL_MODEL);

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
async fn test_mistral_tools() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let mut config = default_config(MISTRAL_MODEL);
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
async fn test_mistral_structured_output() {
    use sideseat::types::ResponseFormat;

    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);
    let mut config = default_config(MISTRAL_MODEL);
    config.response_format = Some(ResponseFormat::Json);

    let resp = retry(|| {
        provider.complete(
            vec![user_msg("Return a JSON object with fields 'name' and 'age'. Name: Bob, Age: 30.")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response");
    // Verify it's valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    assert!(parsed.is_ok(), "expected valid JSON, got: {text}");
}

#[tokio::test]
async fn test_mistral_embed() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);

    let request = EmbeddingRequest::new(MISTRAL_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 2, "expected 2 embeddings");
    assert!(!resp.embeddings[0].is_empty(), "expected non-empty embedding");
    assert!(resp.embeddings[0].len() > 100, "expected embedding dimension > 100");
}

#[tokio::test]
async fn test_mistral_list_models() {
    let api_key = mistral_api_key_env!();
    let provider = MistralProvider::new(api_key);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "expected at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("mistral")),
        "expected a mistral model"
    );
}

// ---------------------------------------------------------------------------
// Mistral via AWS Bedrock (API key / bearer token)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mistral_bedrock_complete() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = MistralProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_mistral_small(&region));

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await;

    match resp {
        Ok(r) => {
            assert!(!r.text().is_empty(), "expected non-empty text response");
            // Bedrock Mistral does not return usage in its response body.
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_mistral_bedrock_stream() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = MistralProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_mistral_small(&region));

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    match collect_stream(stream).await {
        Ok(r) => {
            assert!(!r.text().is_empty(), "expected non-empty streaming response");
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_mistral_bedrock_system_prompt() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = MistralProvider::with_api_key_bedrock(api_key, region.clone());
    let mut config = default_config(&bedrock_mistral_small(&region));
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    match retry(|| provider.complete(vec![user_msg("Hello!")], config.clone())).await {
        Ok(r) => assert!(!r.text().is_empty(), "expected non-empty response"),
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_mistral_bedrock_multi_turn() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = MistralProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_mistral_small(&region));

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    match retry(|| provider.complete(messages.clone(), config.clone())).await {
        Ok(r) => {
            let text = r.text().to_lowercase();
            assert!(text.contains("alice"), "expected model to recall name, got: {text}");
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_mistral_bedrock_tools() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = MistralProvider::with_api_key_bedrock(api_key, region.clone());
    let mut config = default_config(&bedrock_mistral_large(&region));
    config.tools = vec![echo_tool()];

    match retry(|| {
        provider.complete(
            vec![user_msg("Please echo the word 'mango'")],
            config.clone(),
        )
    })
    .await
    {
        Ok(r) => {
            let has_tool = r.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
            assert!(has_tool, "expected tool_use block, got: {:?}", r.content);
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}
