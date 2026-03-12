//! Integration tests for Google Gemini Interactions API provider.

#[macro_use]
mod common;
use common::*;

use sideseat::providers::GeminiInteractionsProvider;

const INTERACTIONS_MODEL: &str = "gemini-2.5-flash-lite";

#[tokio::test]
async fn test_interactions_complete() {
    let (server, provider) = mock_gemini_interactions();
    mock_json(&server, POST, "/interactions", INTERACTIONS_COMPLETE_JSON);
    let config = default_config(INTERACTIONS_MODEL);

    let resp = provider
        .complete(vec![user_msg("Say hello in one word.")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
    assert!(resp.id.is_some(), "expected interaction id in response");
}

#[tokio::test]
async fn test_interactions_stream() {
    let (server, provider) = mock_gemini_interactions();
    mock_sse(&server, POST, "/interactions", INTERACTIONS_STREAM_EVENTS);
    let config = default_config(INTERACTIONS_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
    assert!(
        resp.id.is_some(),
        "expected interaction id in streaming response"
    );
}

#[tokio::test]
async fn test_interactions_system_prompt() {
    let (server, provider) = mock_gemini_interactions();
    mock_json(&server, POST, "/interactions", INTERACTIONS_COMPLETE_JSON);
    let mut config = default_config(INTERACTIONS_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = provider
        .complete(vec![user_msg("Hello!")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_interactions_tools() {
    let (server, provider) = mock_gemini_interactions();
    mock_json(&server, POST, "/interactions", INTERACTIONS_TOOL_JSON);
    let mut config = default_config(INTERACTIONS_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'mango'")], config)
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_interactions_stateful_conversation() {
    let server = MockServer::start();
    let config = default_config(INTERACTIONS_MODEL);

    // Register Turn 2 mock first (FIFO: specific match checked before fallback)
    // Turn 2 body contains "previous_interaction_id" → return "purple" response
    let turn2_resp = r#"{"id":"interaction-test-2","status":"completed","outputs":[{"type":"text","text":"your favourite colour is purple"}],"usage":{"total_input_tokens":10,"total_output_tokens":5},"model":"models/test"}"#;
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/interactions")
            .body_includes("previous_interaction_id");
        then.status(200)
            .header("content-type", "application/json")
            .body(turn2_resp);
    });
    // Turn 1 fallback: no body restriction → return initial response
    mock_json(&server, POST, "/interactions", INTERACTIONS_COMPLETE_JSON);

    let provider1 = GeminiInteractionsProvider::new("test-key").with_api_base(server.base_url());
    let resp1 = provider1
        .complete(
            vec![user_msg("My favourite colour is purple.")],
            config.clone(),
        )
        .await
        .unwrap();

    assert!(!resp1.text().is_empty());
    let interaction_id = resp1.id.clone().expect("expected interaction id");

    // Turn 2: second interaction references the previous ID
    let provider2 = GeminiInteractionsProvider::new("test-key")
        .with_api_base(server.base_url())
        .with_previous_interaction_id(&interaction_id);

    let resp2 = provider2
        .complete(vec![user_msg("What is my favourite colour?")], config)
        .await
        .unwrap();

    let text = resp2.text().to_lowercase();
    assert!(
        text.contains("purple"),
        "expected model to recall colour, got: {text}"
    );
}

#[tokio::test]
async fn test_interactions_list_models() {
    let (server, provider) = mock_gemini_interactions();
    server.mock(|when, then| {
        when.method(GET).path_includes("/models");
        then.status(200)
            .header("content-type", "application/json")
            .body(GEMINI_LIST_MODELS_JSON);
    });

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("gemini")));
}
