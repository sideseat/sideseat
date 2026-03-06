//! Integration tests for Bedrock OpenAI-compatible API (Chat Completions + Responses via AWS Bedrock).

#[macro_use]
mod common;
use common::*;

const BEDROCK_OPENAI_MODEL: &str = "openai.gpt-oss-120b";
const BEDROCK_OPENAI_SMALL_MODEL: &str = "openai.gpt-oss-20b";

// ── Chat Completions API ────────────────────────────────────────────────────

#[tokio::test]
async fn test_bedrock_openai_chat_complete() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_stream() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_EVENTS);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_system_prompt() {
    let (server, provider) = mock_bedrock_openai_chat();
    let pirate_json = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"Arrr! Hello there!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(pirate_json);
    });
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider.complete(vec![user_msg("Hello")], config).await.unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("arr"), "expected pirate response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_chat_multi_turn() {
    let (server, provider) = mock_bedrock_openai_chat();
    let alex_json = r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"Your name is Alex."},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(alex_json);
    });
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let messages = vec![
        user_msg("My name is Alex. Remember it."),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text("Got it, your name is Alex.".to_string())],
            name: None,
            cache_control: None,
        },
        user_msg("What is my name?"),
    ];
    let resp = provider.complete(messages, config).await.unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_chat_tools() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_TOOL_JSON);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'mango'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_chat_streaming_tools() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_TOOL_EVENTS);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_chat_tool_use_loop() {
    let (server, provider) = mock_bedrock_openai_chat();
    // Turn 2: body has tool_call_id (tool result) → return complete (checked first = FIFO priority)
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions").body_includes("tool_call_id");
        then.status(200).header("content-type", "application/json").body(OPENAI_COMPLETE_JSON);
    });
    // Turn 1 fallback: no restriction → tool call (checked second)
    server.mock(|when, then| {
        when.method(POST).path_includes("/chat/completions");
        then.status(200).header("content-type", "application/json").body(OPENAI_TOOL_JSON);
    });
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    // Turn 1: model calls the tool
    let resp1 = provider.complete(vec![user_msg("Echo 'jackfruit'")], config.clone()).await.unwrap();

    let tool_use = resp1.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None }
    }).expect("expected tool_use in turn 1");
    assert_eq!(tool_use.name, "echo");

    // Turn 2: send tool result, expect plain text
    let messages = vec![
        user_msg("Echo 'jackfruit'"),
        Message {
            role: Role::Assistant,
            content: resp1.content.clone(),
            name: None,
            cache_control: None,
        },
        Message::with_tool_results(vec![(tool_use.id.clone(), "jackfruit".to_string())]),
    ];
    let resp2 = provider.complete(messages, config).await.unwrap();

    assert!(!resp2.text().is_empty());
}

#[tokio::test]
async fn test_bedrock_openai_chat_small_model() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_small_model_stream() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_sse(&server, POST, "/chat/completions", OPENAI_STREAM_EVENTS);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_count_tokens() {
    let (server, provider) = mock_bedrock_openai_chat();
    mock_json(&server, POST, "/chat/completions", OPENAI_COMPLETE_JSON);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    match provider.count_tokens(vec![user_msg("Hello, world!")], config).await {
        Ok(count) => assert!(count.input_tokens > 0),
        Err(sideseat::ProviderError::Unsupported(_)) => {
            // count_tokens may not be supported by this endpoint
        }
        Err(e) => panic!("count_tokens failed: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_openai_chat_list_models() {
    let (server, provider) = mock_bedrock_openai_chat();
    let openai_models = r#"{"object":"list","data":[{"id":"openai.gpt-oss-120b","object":"model","created":1234567890,"owned_by":"openai"},{"id":"openai.gpt-oss-20b","object":"model","created":1234567890,"owned_by":"openai"}]}"#;
    server.mock(|when, then| {
        when.method(GET).path_includes("/models");
        then.status(200).header("content-type", "application/json").body(openai_models);
    });

    let models = provider.list_models().await.unwrap();

    assert!(!models.is_empty());
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.iter().any(|id| id.starts_with("openai.")),
        "expected at least one openai.* model, got: {ids:?}"
    );
}

// ── Responses API ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_bedrock_openai_responses_multi_turn() {
    let (server, provider) = mock_bedrock_openai_responses();
    let alex_json = r#"{"id":"resp_test","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Your name is Alex."}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/responses");
        then.status(200).header("content-type", "application/json").body(alex_json);
    });
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let messages = vec![
        user_msg("My name is Alex. Remember it."),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text("Got it, your name is Alex.".to_string())],
            name: None,
            cache_control: None,
        },
        user_msg("What is my name?"),
    ];
    let resp = provider.complete(messages, config).await.unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_responses_tools() {
    let (server, provider) = mock_bedrock_openai_responses();
    mock_json(&server, POST, "/responses", OPENAI_RESPONSES_TOOL_JSON);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'papaya'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_responses_streaming_tools() {
    let (server, provider) = mock_bedrock_openai_responses();
    mock_sse(&server, POST, "/responses", OPENAI_RESPONSES_STREAM_TOOL_EVENTS);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_responses_list_models() {
    let (server, provider) = mock_bedrock_openai_responses();
    let openai_models = r#"{"object":"list","data":[{"id":"openai.gpt-oss-120b","object":"model","created":1234567890,"owned_by":"openai"}]}"#;
    server.mock(|when, then| {
        when.method(GET).path_includes("/models");
        then.status(200).header("content-type", "application/json").body(openai_models);
    });

    let models = provider.list_models().await.unwrap();

    assert!(!models.is_empty());
    assert!(
        models.iter().any(|m| m.id.starts_with("openai.")),
        "expected at least one openai.* model"
    );
}
