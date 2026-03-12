//! Integration tests for Cohere provider (direct API + Cohere via Bedrock).

#[macro_use]
mod common;
use common::*;

const COHERE_MODEL: &str = "command-r-08-2024";
const COHERE_EMBED_MODEL: &str = "embed-english-v3.0";

// ---------------------------------------------------------------------------
// Cohere direct API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cohere_complete() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/chat", COHERE_COMPLETE_JSON);
    let config = default_config(COHERE_MODEL);

    let resp = provider
        .complete(vec![user_msg("Say hello in one word.")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_cohere_stream() {
    let (server, provider) = mock_cohere();
    // Cohere streaming: use SSE-like format or plain JSON - provider uses SSE
    let stream_events = concat!(
        "data: {\"type\":\"content-delta\",\"index\":0,\"delta\":{\"message\":{\"content\":{\"text\":\"hello\"}}}}\n\n",
        "data: {\"type\":\"message-end\",\"delta\":{\"finish_reason\":\"COMPLETE\",\"usage\":{\"billed_units\":{\"input_tokens\":10,\"output_tokens\":5},\"tokens\":{\"input_tokens\":10,\"output_tokens\":5}}}}\n\n",
    );
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(stream_events);
    });
    let config = default_config(COHERE_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_system_prompt() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/chat", COHERE_COMPLETE_JSON);
    let mut config = default_config(COHERE_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = provider
        .complete(vec![user_msg("Hello!")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_multi_turn() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/chat", COHERE_COMPLETE_JSON);
    let config = default_config(COHERE_MODEL);

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_tools() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/chat", COHERE_TOOL_JSON);
    let mut config = default_config(COHERE_MODEL);
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
async fn test_cohere_embed() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/embed", COHERE_EMBED_JSON);

    let request = EmbeddingRequest::new(COHERE_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 1, "canned response has 1 embedding");
    assert!(!resp.embeddings[0].is_empty());
}

#[tokio::test]
async fn test_cohere_count_tokens() {
    let (server, provider) = mock_cohere();
    mock_json(&server, POST, "/tokenize", COHERE_TOKENIZE_JSON);
    let config = default_config(COHERE_MODEL);

    let count = provider
        .count_tokens(vec![user_msg("Hello, world!")], config)
        .await
        .unwrap();
    assert!(count.input_tokens > 0);
}

#[tokio::test]
async fn test_cohere_list_models() {
    let (server, provider) = mock_cohere();
    mock_json(&server, GET, "/models", COHERE_LIST_MODELS_JSON);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("command")));
}

// ---------------------------------------------------------------------------
// Cohere via AWS Bedrock (converse API)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cohere_bedrock_complete() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config("cohere.command-r-v1:0");

    let resp = provider
        .complete(vec![user_msg("Say hello in one word.")], config)
        .await
        .unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_bedrock_stream() {
    let (server, provider) = mock_bedrock();
    let body = bedrock_converse_stream_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let config = default_config("cohere.command-r-v1:0");

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_bedrock_system_prompt() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let mut config = default_config("cohere.command-r-v1:0");
    config.system = Some("You are a pirate.".to_string());

    let resp = provider
        .complete(vec![user_msg("Hello!")], config)
        .await
        .unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_bedrock_multi_turn() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config("cohere.command-r-v1:0");

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice!"),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_cohere_bedrock_tools() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_TOOL_JSON);
    });
    let mut config = default_config("cohere.command-r-v1:0");
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
async fn test_cohere_bedrock_embed() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_includes("/invoke");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_COHERE_JSON);
    });

    let model = "cohere.embed-english-v3";
    let request = EmbeddingRequest::new(model, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty());
}
