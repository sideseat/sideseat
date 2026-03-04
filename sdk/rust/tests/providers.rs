//! Integration tests for all LLM providers.
//!
//! Each test checks for a required environment variable. If not set, the
//! test is skipped (returns immediately with a pass). To run a specific
//! provider's tests, set the corresponding env var:
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-... cargo test -p sideseat -- --nocapture anthropic
//! OPENAI_API_KEY=sk-... cargo test -p sideseat -- --nocapture openai
//! GEMINI_API_KEY=AIza... cargo test -p sideseat -- --nocapture gemini
//! AWS_DEFAULT_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock_api_key
//! ```

use sideseat::{
    Provider, collect_stream,
    providers::{
        AnthropicProvider, BedrockProvider, GeminiAuth, GeminiProvider, OpenAIChatProvider,
        OpenAIResponsesProvider,
    },
    types::{ContentBlock, EmbeddingRequest, Message, ProviderConfig, Role, StopReason, Tool},
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text(text.to_string())],
        cache_control: None,
    }
}

fn default_config(model: &str) -> ProviderConfig {
    ProviderConfig {
        model: model.to_string(),
        max_tokens: Some(256),
        ..Default::default()
    }
}

fn echo_tool() -> Tool {
    Tool::new(
        "echo",
        "Echoes the provided message back.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "Message to echo"}
            },
            "required": ["message"]
        }),
    )
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_complete() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let config = default_config("claude-3-5-haiku-20241022");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.content.is_empty(), "response should have content");
    let ContentBlock::Text(text) = &resp.content[0] else {
        panic!("expected text block")
    };
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
    assert!(resp.usage.input_tokens > 0, "should have input tokens");
    assert!(resp.usage.output_tokens > 0, "should have output tokens");
}

#[tokio::test]
async fn test_anthropic_stream() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let config = default_config("claude-3-5-haiku-20241022");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "should have text content");
    assert!(
        resp.usage.output_tokens > 0,
        "should report tokens after stream"
    );
}

#[tokio::test]
async fn test_anthropic_tools() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let mut config = default_config("claude-3-5-haiku-20241022");
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'pineapple'")], config)
        .await
        .unwrap();

    let has_tool_use = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(
        has_tool_use,
        "expected a tool_use block; got: {:?}",
        resp.content
    );

    let tool = resp
        .content
        .iter()
        .find_map(|b| {
            if let ContentBlock::ToolUse(t) = b {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(tool.name, "echo");
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn test_anthropic_system_prompt() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let mut config = default_config("claude-3-5-haiku-20241022");
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider
        .complete(vec![user_msg("Hello")], config)
        .await
        .unwrap();
    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("arr"),
        "expected pirate response, got: {text}"
    );
}

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
    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
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

    let text = resp.text_content();
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

    let text = resp.text_content();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("response should be valid JSON");
    assert!(parsed["color"].is_string(), "expected color field");
    assert!(parsed["hex"].is_string(), "expected hex field");
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

    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
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

    assert!(!resp.text_content().is_empty());
}

// ---------------------------------------------------------------------------
// Google Gemini
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_gemini_complete() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config("gemini-2.0-flash");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_gemini_stream() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config("gemini-2.0-flash");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_gemini_tools() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let mut config = default_config("gemini-2.0-flash");
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'papaya'")], config)
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_gemini_structured_output() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let mut config = default_config("gemini-2.0-flash");
    config.extra.insert(
        "output_schema".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "animal": {"type": "string"},
                "sound": {"type": "string"}
            },
            "required": ["animal", "sound"]
        }),
    );

    let resp = provider
        .complete(
            vec![user_msg("Name an animal and the sound it makes.")],
            config,
        )
        .await
        .unwrap();

    let text = resp.text_content();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("response should be valid JSON");
    assert!(parsed["animal"].is_string());
    assert!(parsed["sound"].is_string());
}

// ---------------------------------------------------------------------------
// AWS Bedrock (SDK / IAM credentials)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_complete() {
    let Ok(region) =
        std::env::var("AWS_DEFAULT_REGION").or_else(|_| std::env::var("BEDROCK_REGION"))
    else {
        return;
    };
    let provider = BedrockProvider::from_env(region).await;
    let config = default_config("us.amazon.nova-lite-v1:0");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_stream() {
    let Ok(region) =
        std::env::var("AWS_DEFAULT_REGION").or_else(|_| std::env::var("BEDROCK_REGION"))
    else {
        return;
    };
    let provider = BedrockProvider::from_env(region).await;
    let config = default_config("us.amazon.nova-lite-v1:0");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_tools() {
    let Ok(region) =
        std::env::var("AWS_DEFAULT_REGION").or_else(|_| std::env::var("BEDROCK_REGION"))
    else {
        return;
    };
    let provider = BedrockProvider::from_env(region).await;
    let mut config = default_config("us.anthropic.claude-3-5-haiku-20241022-v1:0");
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'jackfruit'")], config)
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use, got: {:?}", resp.content);
}

// ---------------------------------------------------------------------------
// List models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_list_models() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("claude")),
        "expected a claude model"
    );
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
async fn test_gemini_list_models() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("gemini")),
        "expected a gemini model"
    );
}

#[tokio::test]
async fn test_bedrock_list_models() {
    let Ok(region) =
        std::env::var("AWS_DEFAULT_REGION").or_else(|_| std::env::var("BEDROCK_REGION"))
    else {
        return;
    };
    let provider = BedrockProvider::from_env(region).await;
    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return foundation models");
}

// ---------------------------------------------------------------------------
// Count tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_count_tokens() {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = AnthropicProvider::new(api_key);
    let config = default_config("claude-3-5-haiku-20241022");
    let count = provider
        .count_tokens(vec![user_msg("Hello, how are you?")], config)
        .await
        .unwrap();
    assert!(count.input_tokens > 0, "should report non-zero token count");
}

#[tokio::test]
async fn test_gemini_count_tokens() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let config = default_config("gemini-2.0-flash");
    let count = provider
        .count_tokens(vec![user_msg("Hello, how are you?")], config)
        .await
        .unwrap();
    assert!(count.input_tokens > 0, "should report non-zero token count");
}

// ---------------------------------------------------------------------------
// Embeddings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_embed() {
    let Ok(api_key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let provider = OpenAIChatProvider::new(api_key);
    let req = EmbeddingRequest::new(vec!["Hello world", "Goodbye world"]);
    let resp = provider.embed(req, "text-embedding-3-small").await.unwrap();
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

#[tokio::test]
async fn test_gemini_embed() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let req = EmbeddingRequest::new(vec!["Hello world"]);
    let resp = provider.embed(req, "gemini-embedding-001").await.unwrap();
    assert_eq!(resp.embeddings.len(), 1);
    assert!(
        !resp.embeddings[0].is_empty(),
        "embedding vector should not be empty"
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock API key (plain HTTP)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_api_key_complete() {
    let Ok(api_key) = std::env::var("BEDROCK_API_KEY") else {
        return;
    };
    let Ok(region) = std::env::var("BEDROCK_REGION") else {
        return;
    };
    let provider = BedrockProvider::with_api_key(api_key, region);
    let config = default_config("us.amazon.nova-lite-v1:0");

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("hello"),
        "expected 'hello' in: {text}"
    );
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_stream() {
    let Ok(api_key) = std::env::var("BEDROCK_API_KEY") else {
        return;
    };
    let Ok(region) = std::env::var("BEDROCK_REGION") else {
        return;
    };
    let provider = BedrockProvider::with_api_key(api_key, region);
    let config = default_config("us.amazon.nova-lite-v1:0");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty());
    assert!(resp.usage.output_tokens > 0);
}
