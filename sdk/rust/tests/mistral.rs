//! Integration tests for Mistral provider (direct API + Mistral models via Bedrock).

#[macro_use]
mod common;
use common::*;

const MISTRAL_MODEL: &str = "mistral-small-latest";
const MISTRAL_EMBED_MODEL: &str = "mistral-embed";

fn bedrock_mistral_small() -> &'static str {
    "mistral.mistral-small-2402-v1:0"
}

fn bedrock_mistral_large() -> &'static str {
    "mistral.mistral-large-2407-v1:0"
}

// ---------------------------------------------------------------------------
// Mistral direct API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mistral_complete() {
    let (server, provider) = mock_mistral();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(MISTRAL_MODEL);

    let resp = provider.complete(vec![user_msg("Say hello in one word.")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
    assert!(resp.id.is_some());
}

#[tokio::test]
async fn test_mistral_stream() {
    let (server, provider) = mock_mistral();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_EVENTS);
    let config = default_config(MISTRAL_MODEL);

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_mistral_system_prompt() {
    let (server, provider) = mock_mistral();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let mut config = default_config(MISTRAL_MODEL);
    config.system = Some("You are a pirate. Respond only in pirate speak.".to_string());

    let resp = provider.complete(vec![user_msg("Hello!")], config).await.unwrap();

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_multi_turn() {
    let (server, provider) = mock_mistral();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(MISTRAL_MODEL);

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice! Nice to meet you."),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_tools() {
    let (server, provider) = mock_mistral();
    mock_json(&server, POST, "/chat/completions", OPENAI_TOOL_JSON);
    let mut config = default_config(MISTRAL_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'mango'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_mistral_structured_output() {
    use sideseat::types::ResponseFormat;
    let json_body = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"{\"name\":\"Bob\",\"age\":30}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let (server, provider) = mock_mistral();
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(json_body);
    });
    let mut config = default_config(MISTRAL_MODEL);
    config.response_format = Some(ResponseFormat::Json);

    let resp = provider
        .complete(
            vec![user_msg("Return a JSON object with fields 'name' and 'age'.")],
            config,
        )
        .await
        .unwrap();

    let text = resp.text();
    assert!(!text.is_empty());
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    assert!(parsed.is_ok(), "expected valid JSON, got: {text}");
}

#[tokio::test]
async fn test_mistral_embed() {
    let (server, provider) = mock_mistral();
    mock_json(&server, POST, "/embeddings", MISTRAL_EMBED_JSON);

    let request = EmbeddingRequest::new(MISTRAL_EMBED_MODEL, vec!["Hello, world!", "Rust is great"]);
    let resp = provider.embed(request).await.unwrap();

    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty());
}

#[tokio::test]
async fn test_mistral_list_models() {
    let (server, provider) = mock_mistral();
    mock_json(&server, GET, "/models", MISTRAL_LIST_MODELS_JSON);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("mistral")));
}

// ---------------------------------------------------------------------------
// Mistral via AWS Bedrock (converse API)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mistral_bedrock_complete() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200).header("content-type", "application/json").body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(bedrock_mistral_small());

    let resp = provider.complete(vec![user_msg("Say hello in one word.")], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_bedrock_stream() {
    let (server, provider) = mock_bedrock();
    let body = bedrock_converse_stream_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let config = default_config(bedrock_mistral_small());

    let stream = provider.stream(vec![user_msg("Say hello in one word.")], config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_bedrock_system_prompt() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200).header("content-type", "application/json").body(BEDROCK_COMPLETE_JSON);
    });
    let mut config = default_config(bedrock_mistral_small());
    config.system = Some("You are a pirate.".to_string());

    let resp = provider.complete(vec![user_msg("Hello!")], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_bedrock_multi_turn() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200).header("content-type", "application/json").body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(bedrock_mistral_small());

    let messages = vec![
        user_msg("My name is Alice."),
        Message::assistant("Hello Alice!"),
        user_msg("What is my name?"),
    ];

    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_mistral_bedrock_tools() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/converse$");
        then.status(200).header("content-type", "application/json").body(BEDROCK_TOOL_JSON);
    });
    let mut config = default_config(bedrock_mistral_large());
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'mango'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}
