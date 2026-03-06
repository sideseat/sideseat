//! Integration tests for OpenAI providers (Chat Completions + Responses API).
//!
//! ```bash
//! OPENAI_API_KEY=sk-... cargo test -p sideseat -- --nocapture openai
//! ```

#[macro_use]
mod common;
use common::*;

// ---------------------------------------------------------------------------
// OpenAI Chat Completions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_complete() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let config = default_config("gpt-4o-mini");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.content.is_empty());
    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response, got empty");
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_openai_stream() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let config = default_config("gpt-4o-mini");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    let text = resp.text();
    assert!(!text.is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_openai_tools() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let mut config = default_config("gpt-4o-mini");
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'mango'")], config)
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_openai_structured_output() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let mut config = default_config("gpt-4o-mini");
    config.extra.insert(
        "output_schema".to_string(),
        serde_json::json!({
            "name": "color_response",
            "schema": {
                "type": "object",
                "properties": {
                    "color": {"type": "string"},
                    "hex": {"type": "string"}
                },
                "required": ["color", "hex"],
                "additionalProperties": false
            }
        }),
    );

    let resp = provider
        .complete(
            vec![user_msg(
                "What color is the sky? Respond with color name and hex code.",
            )],
            config,
        )
        .await
        .unwrap();

    let text = resp.text();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("response should be valid JSON");
    assert!(parsed["color"].is_string(), "expected color field");
    assert!(parsed["hex"].is_string(), "expected hex field");
}

#[tokio::test]
async fn test_openai_list_models() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("gpt")),
        "expected a gpt model"
    );
}

#[tokio::test]
async fn test_openai_embed() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let req = EmbeddingRequest::new("text-embedding-3-small", vec!["Hello world", "Goodbye world"]);
    let resp = provider.embed(req).await.unwrap();
    assert_eq!(
        resp.embeddings.len(),
        2,
        "should return one vector per input"
    );
    assert!(
        !resp.embeddings[0].is_empty(),
        "embedding vector should not be empty"
    );
}

// ---------------------------------------------------------------------------
// OpenAI Responses API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_responses_complete() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIResponsesProvider::new(api_key);
    let config = default_config("gpt-4o-mini");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    let text = resp.text();
    assert!(!text.is_empty(), "expected non-empty response, got empty");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_openai_responses_stream() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIResponsesProvider::new(api_key);
    let config = default_config("gpt-4o-mini");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
}
