//! Integration tests for Bedrock OpenAI-compatible API (Chat Completions + Responses via AWS Bedrock).
//!
//! ```bash
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock_openai
//! ```

#[macro_use]
mod common;
use common::*;

const BEDROCK_OPENAI_MODEL: &str = "openai.gpt-oss-120b";
const BEDROCK_OPENAI_SMALL_MODEL: &str = "openai.gpt-oss-20b";

/// Returns `(api_key, region)` for Bedrock OpenAI-compatible API tests, skipping if no API key is set.
macro_rules! bedrock_openai_env {
    () => {{
        let api_key = match std::env::var("BEDROCK_API_KEY")
            .or_else(|_| std::env::var("AWS_BEARER_TOKEN_BEDROCK"))
        {
            Ok(k) => k,
            Err(_) => return,
        };
        (api_key, bedrock_region())
    }};
}

// ── Chat Completions API ────────────────────────────────────────────────────

#[tokio::test]
async fn test_bedrock_openai_chat_complete() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_stream() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_system_prompt() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello")], config.clone()))
        .await
        .unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("arr"), "expected pirate response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_chat_multi_turn() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
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
    let resp = retry(|| provider.complete(messages.clone(), config.clone())).await.unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_chat_tools() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'mango'")], config)
        .await
        .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_chat_streaming_tools() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_chat_tool_use_loop() {
    // Full two-turn cycle: user → tool_use → tool_result → final text.
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    // Turn 1: model calls the tool
    let resp1 = retry(|| provider.complete(vec![user_msg("Echo 'jackfruit'")], config.clone()))
        .await
        .unwrap();

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
    let resp2 = retry(|| provider.complete(messages.clone(), config.clone())).await.unwrap();

    assert!(!resp2.text().is_empty(), "expected text in turn 2");
}


#[tokio::test]
async fn test_bedrock_openai_chat_small_model() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text().is_empty(), "expected non-empty response from small model");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_small_model_stream() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_count_tokens() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    match provider.count_tokens(vec![user_msg("Hello, world!")], config).await {
        Ok(count) => assert!(count.input_tokens > 0, "expected > 0 input tokens"),
        Err(sideseat::ProviderError::Unsupported(_)) => {
            eprintln!("SKIP: count_tokens not supported by this endpoint");
        }
        Err(e) => panic!("count_tokens failed: {e:?}"),
    }
}


#[tokio::test]
async fn test_bedrock_openai_chat_list_models() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);

    let models = provider.list_models().await.unwrap();

    assert!(!models.is_empty(), "expected at least one model");
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.iter().any(|id| id.starts_with("openai.")),
        "expected at least one openai.* model, got: {ids:?}"
    );
}

// ── Responses API ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_bedrock_openai_responses_multi_turn() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIResponsesProvider::for_bedrock_openai(region, api_key);
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
    let resp = retry(|| provider.complete(messages.clone(), config.clone())).await.unwrap();

    let text = resp.text().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_bedrock_openai_responses_tools() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIResponsesProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'papaya'")], config)
        .await
        .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_responses_streaming_tools() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIResponsesProvider::for_bedrock_openai(region, api_key);
    let mut config = default_config(BEDROCK_OPENAI_MODEL);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_openai_responses_list_models() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIResponsesProvider::for_bedrock_openai(region, api_key);

    let models = provider.list_models().await.unwrap();

    assert!(!models.is_empty(), "expected at least one model");
    assert!(
        models.iter().any(|m| m.id.starts_with("openai.")),
        "expected at least one openai.* model"
    );
}
