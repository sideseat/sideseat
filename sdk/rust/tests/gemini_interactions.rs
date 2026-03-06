//! Integration tests for Google Gemini Interactions API provider.
//!
//! The Interactions API is the next-generation Gemini interface with server-side
//! conversation history. Not available on Vertex AI — only direct API key auth.
//!
//! ```bash
//! GEMINI_API_KEY=AIza... cargo test -p sideseat --test gemini_interactions -- --nocapture
//! ```

#[macro_use]
mod common;
use common::*;

use sideseat::providers::GeminiInteractionsProvider;

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

// Use gemini-2.5-flash-lite for free-tier quota availability.
// gemini-3-flash-preview has lower free-tier limits and exhausts quickly.
const INTERACTIONS_MODEL: &str = "gemini-2.5-flash-lite";

// ---------------------------------------------------------------------------
// Basic chat
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interactions_complete() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiInteractionsProvider::new(api_key);
    let config = default_config(INTERACTIONS_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say hello in one word.")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0, "expected input tokens > 0");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
    assert!(resp.id.is_some(), "expected interaction id in response");
}

#[tokio::test]
async fn test_interactions_stream() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiInteractionsProvider::new(api_key);
    let config = default_config(INTERACTIONS_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty streaming response");
    assert!(resp.usage.output_tokens > 0, "expected output tokens > 0");
    assert!(resp.id.is_some(), "expected interaction id in streaming response");
}

#[tokio::test]
async fn test_interactions_system_prompt() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiInteractionsProvider::new(api_key);
    let mut config = default_config(INTERACTIONS_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello!")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
}

// Note: multi-turn via client-side history is not supported by the Interactions API.
// The API manages conversation history server-side via previous_interaction_id.
// See test_interactions_stateful_conversation for the correct multi-turn test.

#[tokio::test]
async fn test_interactions_tools() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiInteractionsProvider::new(api_key);
    let mut config = default_config(INTERACTIONS_MODEL);
    config.tools = vec![echo_tool()];

    let resp = retry(|| {
        provider.complete(vec![user_msg("Please echo the word 'mango'")], config.clone())
    })
    .await
    .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

// ---------------------------------------------------------------------------
// Stateful conversation via previous_interaction_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interactions_stateful_conversation() {
    let api_key = gemini_api_key_env!();
    let config = default_config(INTERACTIONS_MODEL);

    // Turn 1: send a message and capture the interaction ID
    let provider1 = GeminiInteractionsProvider::new(api_key.clone());
    let resp1 = retry(|| provider1.complete(vec![user_msg("My favourite colour is purple.")], config.clone()))
        .await
        .unwrap();

    assert!(!resp1.text().is_empty());
    let interaction_id = resp1.id.clone().expect("expected interaction id for stateful test");

    // Turn 2: continue the conversation using the previous interaction ID
    let provider2 = GeminiInteractionsProvider::new(api_key)
        .with_previous_interaction_id(&interaction_id);
    let resp2 = retry(|| provider2.complete(vec![user_msg("What is my favourite colour?")], config.clone()))
        .await
        .unwrap();

    let text = resp2.text().to_lowercase();
    assert!(
        text.contains("purple"),
        "expected model to recall colour from previous interaction, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// List models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interactions_list_models() {
    let api_key = gemini_api_key_env!();
    let provider = GeminiInteractionsProvider::new(api_key);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "expected at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("gemini")),
        "expected a gemini model, got: {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}
