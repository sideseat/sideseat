//! Integration tests for Cohere provider (direct API + Cohere via Bedrock).
//!
//! ```bash
//! COHERE_API_KEY=... cargo test -p sideseat -- --nocapture cohere
//!
//! # Cohere via Bedrock (API key / bearer token):
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture cohere_bedrock
//! ```

#[macro_use]
mod common;
use common::*;

macro_rules! cohere_api_key_env {
    () => {{
        match std::env::var("COHERE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: COHERE_API_KEY not set");
                return;
            }
        }
    }};
}

const COHERE_MODEL: &str = "command-r-08-2024";
const COHERE_EMBED_MODEL: &str = "embed-english-v3.0";

// ---------------------------------------------------------------------------
// Cohere direct API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cohere_complete() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let config = default_config(COHERE_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
}

#[tokio::test]
async fn test_cohere_stream() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let config = default_config(COHERE_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty streaming response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
}

#[tokio::test]
async fn test_cohere_system_prompt() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let mut config = default_config(COHERE_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello!")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
}

#[tokio::test]
async fn test_cohere_multi_turn() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let config = default_config(COHERE_MODEL);

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
async fn test_cohere_tools() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let mut config = default_config(COHERE_MODEL);
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
async fn test_cohere_embed() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);

    let request = EmbeddingRequest::new(COHERE_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 2, "expected 2 embeddings");
    assert!(!resp.embeddings[0].is_empty(), "expected non-empty embedding");
    assert!(resp.embeddings[0].len() > 100, "expected embedding dimension > 100");
}

#[tokio::test]
async fn test_cohere_count_tokens() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);
    let config = default_config(COHERE_MODEL);

    let count = provider
        .count_tokens(vec![user_msg("Hello, world!")], config)
        .await
        .unwrap();

    assert!(count.input_tokens > 0, "expected token count > 0");
}

#[tokio::test]
async fn test_cohere_list_models() {
    let api_key = cohere_api_key_env!();
    let provider = CohereProvider::new(api_key);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "expected at least one model");
    assert!(models.iter().any(|m| m.id.contains("command")), "expected a command model");
}

// ---------------------------------------------------------------------------
// Cohere via AWS Bedrock (API key / bearer token)
// ---------------------------------------------------------------------------

fn bedrock_cohere_command_r(_region: &str) -> String {
    // Cohere Command R does not have a cross-region inference profile — no prefix needed.
    "cohere.command-r-v1:0".to_string()
}

fn bedrock_cohere_embed_english(_region: &str) -> String {
    "cohere.embed-english-v3".to_string()
}

#[tokio::test]
async fn test_cohere_bedrock_complete() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_cohere_command_r(&region));

    match retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone())).await {
        Ok(r) => {
            assert!(!r.text().is_empty(), "expected non-empty text response");
            // Cohere v1 on Bedrock invoke_model does not return token counts in the response body.
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_cohere_bedrock_stream() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_cohere_command_r(&region));

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    match collect_stream(stream).await {
        Ok(r) => {
            assert!(!r.text().is_empty(), "expected non-empty streaming response");
            assert!(r.usage.input_tokens > 0, "expected input tokens > 0");
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}

#[tokio::test]
async fn test_cohere_bedrock_system_prompt() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let mut config = default_config(&bedrock_cohere_command_r(&region));
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
async fn test_cohere_bedrock_multi_turn() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let config = default_config(&bedrock_cohere_command_r(&region));

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
async fn test_cohere_bedrock_tools() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let mut config = default_config(&bedrock_cohere_command_r(&region));
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

#[tokio::test]
async fn test_cohere_bedrock_embed() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = CohereProvider::with_api_key_bedrock(api_key, region.clone());
    let model = bedrock_cohere_embed_english(&region);

    let request = EmbeddingRequest::new(&model, vec!["Hello, world!", "Rust is great"]);
    match provider.embed(request).await {
        Ok(r) => {
            assert_eq!(r.embeddings.len(), 2, "expected 2 embeddings");
            assert!(!r.embeddings[0].is_empty(), "expected non-empty embedding vector");
            assert!(r.embeddings[0].len() > 100, "expected embedding dimension > 100");
        }
        Err(e) if bedrock_model_not_available(&e) => {
            println!("Skipping: model not available in {region}: {e}");
        }
        Err(e) => panic!("unexpected error: {e}"),
    }
}
