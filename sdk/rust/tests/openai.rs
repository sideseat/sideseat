//! Integration tests for OpenAI providers (Chat Completions + Responses API).

#[macro_use]
mod common;
use common::*;

// ---------------------------------------------------------------------------
// OpenAI Chat Completions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_complete() {
    let (server, provider) = mock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config("gpt-4o-mini");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.content.is_empty());
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_openai_stream() {
    let (server, provider) = mock_openai_chat();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_EVENTS);
    let config = default_config("gpt-4o-mini");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_openai_tools() {
    let (server, provider) = mock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_TOOL_JSON);
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
    let (server, provider) = mock_openai_chat();
    let json_resp = r##"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"{\"color\":\"blue\",\"hex\":\"#0000FF\"}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"##;
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200)
            .header("content-type", "application/json")
            .body(json_resp);
    });
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
        .complete(vec![user_msg("What color is the sky?")], config)
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
    let (server, provider) = mock_openai_chat();
    mock_json(&server, GET, "/models", OPENAI_LIST_MODELS_JSON);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("gpt")),
        "expected a gpt model"
    );
}

#[tokio::test]
async fn test_openai_embed() {
    let (server, provider) = mock_openai_chat();
    mock_json(&server, POST, "/embeddings", OPENAI_EMBED_JSON);

    let req = EmbeddingRequest::new(
        "text-embedding-3-small",
        vec!["Hello world", "Goodbye world"],
    );
    let resp = provider.embed(req).await.unwrap();
    assert_eq!(resp.embeddings.len(), 1, "canned response has 1 embedding");
    assert!(!resp.embeddings[0].is_empty());
}

// ---------------------------------------------------------------------------
// OpenAI Responses API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_responses_complete() {
    let (server, provider) = mock_openai_responses();
    mock_json(&server, POST, "/responses", OPENAI_RESPONSES_JSON);
    let config = default_config("gpt-4o-mini");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_openai_responses_stream() {
    let (server, provider) = mock_openai_responses();
    mock_sse(&server, POST, "/responses", OPENAI_RESPONSES_STREAM_EVENTS);
    let config = default_config("gpt-4o-mini");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
}
