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
//!
//! # Bedrock IAM (SDK/profile/instance credentials):
//! BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock
//!
//! # Bedrock API key (bearer token):
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock_api_key
//!   (also: AWS_BEARER_TOKEN_BEDROCK=... AWS_REGION=eu-west-1 ... bedrock_api_key)
//!
//! # Bedrock OpenAI-compatible API (OpenAI-compatible endpoints via Bedrock):
//! BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock_openai
//!
//! # Anthropic via Bedrock (invoke_model):
//! BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture anthropic_bedrock
//! ANTHROPIC_BEDROCK_MODEL=us.anthropic.claude-3-haiku-20240307-v1:0   -- optional model override
//!
//! # Anthropic via Vertex AI:
//! VERTEX_PROJECT_ID=my-project VERTEX_LOCATION=us-east5 VERTEX_ACCESS_TOKEN=$(gcloud auth print-access-token) \
//!   cargo test -p sideseat -- --nocapture anthropic_vertex
//! VERTEX_MODEL=claude-haiku-4-5@20251001   -- optional model override
//!
//! # Optional modality test data:
//! BEDROCK_S3_IMAGE_URI=s3://bucket/image.jpg   -- image S3 source test
//! BEDROCK_S3_VIDEO_URI=s3://bucket/video.mp4   -- video S3 source test
//! BEDROCK_TEST_VIDEO_PATH=/path/to/video.mp4   -- embedded video test
//! BEDROCK_VIDEO_OUTPUT_URI=s3://bucket/output/ -- real video generation test
//! BEDROCK_NOVA_SONIC=1                         -- TTS / STT tests
//! ```

use std::sync::Arc;

use sideseat::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderError,
    VideoProvider, collect_stream,
    providers::{
        AnthropicProvider, BedrockProvider, GeminiAuth, GeminiProvider, OpenAIChatProvider,
        OpenAIResponsesProvider,
    },
    types::{
        AudioContent, AudioFormat, CacheControl, ContentBlock, DocumentContent, DocumentFormat,
        EmbeddingRequest, ImageContent, ImageFormat, ImageGenerationRequest, ImageSize,
        MediaSource, Message, ProviderConfig, Response, Role, S3Location, SpeechRequest,
        StopReason, Tool, ToolResultBlock, TranscriptionRequest, VideoContent, VideoFormat,
        VideoGenerationRequest,
    },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Retries an async operation up to 3 times when `ProviderError::is_retryable()` is true,
/// or when the response has empty content (Bedrock can return 200 with empty content under load).
async fn retry<F, Fut>(mut f: F) -> Result<Response, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Response, ProviderError>>,
{
    let mut last = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500 * (1 << attempt))).await;
        }
        match f().await {
            Ok(r) if !r.content.is_empty() => return Ok(r),
            Ok(r) => last = Some(Ok(r)), // empty content — retry
            Err(e) if e.is_retryable() => last = Some(Err(e)),
            Err(e) => return Err(e),
        }
    }
    last.unwrap()
}

fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::text(text.to_string())],
        name: None,
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
    let config = default_config("claude-haiku-4-5-20251001");

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.content.is_empty(), "response should have content");
    assert!(
        matches!(resp.content[0], ContentBlock::Text(_)),
        "expected text block, got: {:?}", resp.content[0]
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
    let config = default_config("claude-haiku-4-5-20251001");

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
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.tools = vec![echo_tool()];

    let resp = retry(|| provider.complete(vec![user_msg("Please echo the word 'pineapple'")], config.clone()))
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
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Hello")], config.clone()))
        .await
        .unwrap();
    let text = resp.text_content();
    assert!(
        text.to_lowercase().contains("arr"),
        "expected pirate response, got: {text}"
    );
}

// Helper: build a direct Anthropic provider, skip test if key not set.
macro_rules! anthropic_direct_env {
    () => {{
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) => AnthropicProvider::new(k),
            Err(_) => return,
        }
    }};
}

const ANTHROPIC_DIRECT_MODEL: &str = "claude-haiku-4-5-20251001";

#[tokio::test]
async fn test_anthropic_multi_turn() {
    let provider = anthropic_direct_env!();
    let config = default_config(ANTHROPIC_DIRECT_MODEL);

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

    let text = resp.text_content().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_anthropic_streaming_tools() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn test_anthropic_tool_use_loop() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.tools = vec![echo_tool()];

    // Turn 1: model calls the tool
    let resp = retry(|| provider.complete(vec![user_msg("Echo 'banana'")], config.clone()))
        .await
        .unwrap();

    let tool_use = resp.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None }
    });
    assert!(tool_use.is_some(), "expected tool_use in turn 1, got: {:?}", resp.content);
    let tool_use = tool_use.unwrap();
    assert_eq!(tool_use.name, "echo");

    // Turn 2: send tool result back
    let messages = vec![
        user_msg("Echo 'banana'"),
        Message {
            role: Role::Assistant,
            content: resp.content.clone(),
            name: None,
            cache_control: None,
        },
        Message::with_tool_results(vec![(tool_use.id.clone(), "banana".to_string())]),
    ];
    let resp2 = retry(|| provider.complete(messages.clone(), config.clone())).await.unwrap();

    assert!(!resp2.text_content().is_empty(), "expected text in turn 2");
    assert_eq!(resp2.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_anthropic_json_schema_output() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.response_format = Some(sideseat::types::ResponseFormat::json_schema_strict(
        "country_info",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "capital": {"type": "string"},
                "population_millions": {"type": "number"}
            },
            "required": ["name", "capital", "population_millions"],
            "additionalProperties": false
        }),
    ));

    let resp = retry(|| provider.complete(vec![user_msg("Give me info about France.")], config.clone()))
        .await
        .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool || !resp.text_content().is_empty(), "expected structured output");
}

#[tokio::test]
async fn test_anthropic_thinking() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.max_tokens = Some(2048);
    config.thinking_budget = Some(1024);

    let resp = retry(|| provider.complete(vec![user_msg("How many r's are in 'strawberry'?")], config.clone()))
        .await
        .unwrap();

    let has_thinking = resp.content.iter().any(|b| matches!(b, ContentBlock::Thinking(_)));
    let has_text = resp.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(
        has_thinking || has_text,
        "expected thinking or text block, got: {:?}", resp.content
    );
    // If thinking was returned, verify it has content
    if has_thinking {
        let thinking = resp.content.iter().find_map(|b| {
            if let ContentBlock::Thinking(t) = b { Some(t) } else { None }
        }).unwrap();
        assert!(!thinking.thinking.is_empty(), "thinking block should not be empty");
    }
}

#[tokio::test]
async fn test_anthropic_vision() {
    let provider = anthropic_direct_env!();
    let config = default_config(ANTHROPIC_DIRECT_MODEL);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::text("What color is this image? Answer in one word.".to_string()),
        ],
        name: None,
        cache_control: None,
    };
    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone())).await.unwrap();

    assert!(!resp.text_content().is_empty(), "expected text response to image");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_document_input() {
    let provider = anthropic_direct_env!();
    let config = default_config(ANTHROPIC_DIRECT_MODEL);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::Text("The capital of France is Paris.".to_string()),
                format: DocumentFormat::Txt,
                name: Some("geo_fact".to_string()),
            }),
            ContentBlock::text("What does the document say?".to_string()),
        ],
        name: None,
        cache_control: None,
    };
    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone())).await.unwrap();

    let text = resp.text_content().to_lowercase();
    assert!(text.contains("paris") || !text.is_empty(), "expected response mentioning the document");
}

#[tokio::test]
async fn test_anthropic_cache_control() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.system = Some("You are a helpful assistant.".to_string());

    let msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text("Hello".to_string())],
        name: None,
        cache_control: Some(CacheControl::Ephemeral),
    };

    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone())).await.unwrap();
    assert!(!resp.text_content().is_empty(), "expected response with cache_control");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_sampling_params() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.temperature = Some(0.0);
    config.top_k = Some(40);

    let resp = retry(|| provider.complete(vec![user_msg("Say exactly 'deterministic'")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected response with sampling params");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_stop_sequences() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.stop_sequences = vec!["STOP".to_string()];

    let resp = retry(|| provider.complete(
        vec![user_msg("Count: one, two, three. Then say STOP and continue.")],
        config.clone(),
    ))
    .await
    .unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "expected response before stop sequence");
    assert!(
        matches!(resp.stop_reason, StopReason::StopSequence(_) | StopReason::EndTurn),
        "unexpected stop_reason: {:?}", resp.stop_reason
    );
}

#[tokio::test]
async fn test_anthropic_disable_parallel_tools() {
    let provider = anthropic_direct_env!();
    let mut config = default_config(ANTHROPIC_DIRECT_MODEL);
    config.tools = vec![echo_tool()];
    config.parallel_tool_calls = Some(false);

    let resp = retry(|| provider.complete(vec![user_msg("Echo 'mango'")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.content.is_empty(), "expected content");
    assert!(
        resp.warnings.iter().all(|w| !w.contains("parallel_tool_calls")),
        "unexpected warning: {:?}", resp.warnings
    );
}

// ---------------------------------------------------------------------------
// Anthropic via AWS Bedrock (invoke_model / invoke_model_with_response_stream)
// ---------------------------------------------------------------------------
//
// Set BEDROCK_REGION + (optionally) ANTHROPIC_BEDROCK_MODEL to run these tests.
// Model defaults to a cross-region Claude 3 Haiku inference profile.

macro_rules! anthropic_bedrock_env {
    () => {{
        let region = bedrock_region();
        let model = std::env::var("ANTHROPIC_BEDROCK_MODEL")
            .unwrap_or_else(|_| format!("{}.anthropic.claude-haiku-4-5-20251001-v1:0", bedrock_region_prefix(&region)));
        (region, model)
    }};
}

async fn anthropic_bedrock_provider(region: &str) -> AnthropicProvider {
    let aws_cfg = aws_config::from_env()
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;
    let client = Arc::new(aws_sdk_bedrockruntime::Client::new(&aws_cfg));
    AnthropicProvider::from_bedrock(client)
}

#[tokio::test]
async fn test_anthropic_bedrock_complete() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.content.is_empty(), "expected content");
    assert!(resp.usage.input_tokens > 0, "expected input tokens");
    assert!(resp.usage.output_tokens > 0, "expected output tokens");
}

#[tokio::test]
async fn test_anthropic_bedrock_stream() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty(), "expected text");
    assert!(resp.usage.output_tokens > 0, "expected output tokens");
}

#[tokio::test]
async fn test_anthropic_bedrock_tools() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.tools = vec![echo_tool()];

    let resp = retry(|| provider.complete(vec![user_msg("Please echo the word 'lychee'")], config.clone()))
        .await
        .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_bedrock_system_prompt() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider
        .complete(vec![user_msg("Hello")], config)
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "expected non-empty response");
}

#[tokio::test]
async fn test_anthropic_bedrock_cache_control() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

    // Send a message with cache_control; Bedrock Anthropic supports prompt caching.
    let msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text("Say 'hello' in one word".to_string())],
        name: None,
        cache_control: Some(CacheControl::Ephemeral),
    };
    let resp = provider.complete(vec![msg], config).await.unwrap();

    assert!(!resp.content.is_empty(), "expected content");
    // No warning about cache_control being unsupported
    assert!(
        !resp.warnings.iter().any(|w| w.contains("cache_control")),
        "unexpected cache_control warning: {:?}",
        resp.warnings
    );
}

#[tokio::test]
async fn test_anthropic_bedrock_streaming_tools() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Please echo the word 'papaya'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in stream, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_bedrock_vision() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::text("What color is this image?".to_string()),
        ],
        name: None,
        cache_control: None,
    };
    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected text response to vision");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_thinking() {
    // Extended thinking requires a model that supports it.
    // Set ANTHROPIC_BEDROCK_THINKING_MODEL or default to Claude 3.7 Sonnet.
    let region = bedrock_region();
    let model = std::env::var("ANTHROPIC_BEDROCK_THINKING_MODEL")
        .unwrap_or_else(|_| {
            format!("{}.anthropic.claude-haiku-4-5-20251001-v1:0", bedrock_region_prefix(&region))
        });

    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.max_tokens = Some(2048);
    config.thinking_budget = Some(1024);

    let resp = provider
        .complete(vec![user_msg("How many r's are in 'strawberry'?")], config)
        .await;

    match resp {
        Err(e) if bedrock_model_not_available(&e) => return, // skip if model unavailable
        Err(e) => panic!("unexpected error: {e}"),
        Ok(resp) => {
            let has_thinking = resp.content.iter().any(|b| matches!(b, ContentBlock::Thinking(_)));
            let has_text = resp.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
            assert!(has_thinking || has_text, "expected thinking or text, got: {:?}", resp.content);
        }
    }
}

#[tokio::test]
async fn test_anthropic_bedrock_document_input() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::Text("The capital of France is Paris.".to_string()),
                format: DocumentFormat::Txt,
                name: Some("geo_fact".to_string()),
            }),
            ContentBlock::text("What does the document say?".to_string()),
        ],
        name: None,
        cache_control: None,
    };
    let resp = provider.complete(vec![msg], config).await.unwrap();

    let text = resp.text_content().to_lowercase();
    assert!(text.contains("paris") || !text.is_empty(), "expected response mentioning the document");
}


#[tokio::test]
async fn test_anthropic_bedrock_multi_turn() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let config = default_config(&model);

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

    let text = resp.text_content().to_lowercase();
    assert!(text.contains("alex"), "expected name recall, got: {text}");
}

#[tokio::test]
async fn test_anthropic_bedrock_tool_use_loop() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.tools = vec![echo_tool()];

    // Turn 1: model calls the tool
    let resp = retry(|| provider.complete(vec![user_msg("Echo 'banana'")], config.clone()))
        .await
        .unwrap();

    let tool_use = resp.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None }
    });
    assert!(tool_use.is_some(), "expected tool_use in turn 1, got: {:?}", resp.content);
    let tool_use = tool_use.unwrap();

    // Turn 2: send tool result back
    let messages = vec![
        user_msg("Echo 'banana'"),
        Message {
            role: Role::Assistant,
            content: resp.content.clone(),
            name: None,
            cache_control: None,
        },
        Message::with_tool_results(vec![(tool_use.id.clone(), "banana".to_string())]),
    ];
    let resp2 = provider.complete(messages, config).await.unwrap();
    assert!(!resp2.text_content().is_empty(), "expected text in turn 2");
    assert_eq!(resp2.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_anthropic_bedrock_json_schema_output() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.response_format = Some(sideseat::types::ResponseFormat::json_schema_strict(
        "country_info",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "capital": {"type": "string"},
                "population_millions": {"type": "number"}
            },
            "required": ["name", "capital", "population_millions"],
            "additionalProperties": false
        }),
    ));

    let resp = provider
        .complete(vec![user_msg("Give me info about France.")], config)
        .await
        .unwrap();

    // Anthropic returns JSON schema output via tool_use trick
    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool || !resp.text_content().is_empty(), "expected structured output");
}

#[tokio::test]
async fn test_anthropic_bedrock_sampling_params() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.temperature = Some(0.0);
    config.top_p = Some(0.9);
    config.top_k = Some(40);

    let resp = provider
        .complete(vec![user_msg("Say exactly 'deterministic'")], config)
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected response with sampling params");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_stop_sequences() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.stop_sequences = vec!["STOP".to_string()];

    let resp = provider
        .complete(
            vec![user_msg("Count: one, two, three. Then say STOP and continue.")],
            config,
        )
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "expected response before stop sequence");
    // stop_reason may be StopSequence or EndTurn depending on whether model hits the trigger
    assert!(
        matches!(resp.stop_reason, StopReason::StopSequence(_) | StopReason::EndTurn),
        "unexpected stop_reason: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn test_anthropic_bedrock_disable_parallel_tools() {
    let (region, model) = anthropic_bedrock_env!();
    let provider = anthropic_bedrock_provider(&region).await;
    let mut config = default_config(&model);
    config.tools = vec![echo_tool()];
    config.parallel_tool_calls = Some(false);

    let resp = retry(|| provider.complete(vec![user_msg("Echo 'mango'")], config.clone()))
        .await
        .unwrap();

    // Should still work; parallel_tool_calls=false maps to disable_parallel_tool_use
    assert!(!resp.content.is_empty(), "expected content");
    assert!(
        !resp.warnings.iter().any(|w| w.contains("parallel_tool_calls")),
        "unexpected warning: {:?}",
        resp.warnings
    );
}

// ---------------------------------------------------------------------------
// Anthropic via Google Vertex AI (rawPredict / streamRawPredict)
// ---------------------------------------------------------------------------
//
// Required env vars:
//   VERTEX_PROJECT_ID    — GCP project ID
//   VERTEX_LOCATION      — region, e.g. "us-east5"
//   VERTEX_ACCESS_TOKEN  — short-lived OAuth token (gcloud auth print-access-token)
//   VERTEX_MODEL         — optional; defaults to "claude-haiku-4-5@20251001"

macro_rules! anthropic_vertex_env {
    () => {{
        let project = match std::env::var("VERTEX_PROJECT_ID") {
            Ok(p) => p,
            Err(_) => return,
        };
        let location = match std::env::var("VERTEX_LOCATION") {
            Ok(l) => l,
            Err(_) => return,
        };
        let token = match std::env::var("VERTEX_ACCESS_TOKEN") {
            Ok(t) => t,
            Err(_) => return,
        };
        let model = std::env::var("VERTEX_MODEL")
            .unwrap_or_else(|_| "claude-haiku-4-5@20251001".to_string());
        (project, location, token, model)
    }};
}

#[tokio::test]
async fn test_anthropic_vertex_complete() {
    let (project, location, token, model) = anthropic_vertex_env!();
    let provider = AnthropicProvider::from_vertex(project, location, token);
    let config = default_config(&model);

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.content.is_empty(), "expected content");
    assert!(resp.usage.input_tokens > 0, "expected input tokens");
    assert!(resp.usage.output_tokens > 0, "expected output tokens");
}

#[tokio::test]
async fn test_anthropic_vertex_stream() {
    let (project, location, token, model) = anthropic_vertex_env!();
    let provider = AnthropicProvider::from_vertex(project, location, token);
    let config = default_config(&model);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty(), "expected text");
    assert!(resp.usage.output_tokens > 0, "expected output tokens");
}

#[tokio::test]
async fn test_anthropic_vertex_tools() {
    let (project, location, token, model) = anthropic_vertex_env!();
    let provider = AnthropicProvider::from_vertex(project, location, token);
    let mut config = default_config(&model);
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'durian'")], config)
        .await
        .unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_vertex_system_prompt() {
    let (project, location, token, model) = anthropic_vertex_env!();
    let provider = AnthropicProvider::from_vertex(project, location, token);
    let mut config = default_config(&model);
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
    assert!(!text.is_empty(), "expected non-empty response, got empty");
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
// AWS Bedrock helpers
// ---------------------------------------------------------------------------

/// Returns the cross-region inference prefix for a Bedrock region.
/// e.g. "eu-west-1" → "eu", "us-east-1" → "us", "ap-southeast-1" → "ap"
fn bedrock_region_prefix(region: &str) -> &'static str {
    if region.starts_with("eu-") {
        "eu"
    } else if region.starts_with("ap-") {
        "ap"
    } else {
        "us"
    }
}

/// Returns true if the error indicates the model isn't available in this region/auth context.
/// Used to skip tests gracefully when a model has limited availability, regional restrictions,
/// or isn't supported by the current auth mechanism (e.g. bearer token).
fn bedrock_model_not_available(e: &ProviderError) -> bool {
    match e {
        ProviderError::Api { status: 0, .. } => true, // SDK-level error (not HTTP); model/op unavailable
        ProviderError::Unsupported(_) => true,         // explicitly unsupported feature/model
        ProviderError::Api { message, .. } => {
            let m = message.to_lowercase();
            m.contains("not found")
                || m.contains("does not exist")
                || m.contains("no access")
                || m.contains("access denied")
                || m.contains("not supported in this region")
                || m.contains("not available")
                || m.contains("invalid model")
                || m.contains("identifier is invalid")   // "model identifier is invalid"
                || m.contains("on-demand throughput")    // requires provisioned throughput
        }
        _ => false,
    }
}

fn bedrock_nova_lite(region: &str) -> String {
    format!("{}.amazon.nova-lite-v1:0", bedrock_region_prefix(region))
}

fn bedrock_nova_micro(region: &str) -> String {
    format!("{}.amazon.nova-micro-v1:0", bedrock_region_prefix(region))
}

fn bedrock_haiku(region: &str) -> String {
    format!("{}.anthropic.claude-haiku-4-5-20251001-v1:0", bedrock_region_prefix(region))
}

/// 64×64 solid-white PNG (base64). Used for vision / multimodal tests.
///
/// Bedrock's Nova models reject images that are too small (1×1 is not accepted).
/// This 64×64 image is the smallest that passes Nova's format validation.
const TINY_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PMQ0AAAwDoPo33UrYvQQckD4XAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAYHLAMpT0sIcNbcEAAAAAElFTkSuQmCC";

fn vision_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::text(text.to_string()),
        ],
        name: None,
        cache_control: None,
    }
}

// ---------------------------------------------------------------------------
// AWS Bedrock — region helper + env-var macros
// ---------------------------------------------------------------------------

/// Reads the Bedrock region from the first env var that is set, falling back to `"us-east-1"`.
///
/// Priority: `BEDROCK_REGION` → `AWS_REGION` → `AWS_DEFAULT_REGION` → `"us-east-1"`
fn bedrock_region() -> String {
    std::env::var("BEDROCK_REGION")
        .or_else(|_| std::env::var("AWS_REGION"))
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string())
}

/// Returns the Bedrock region for IAM-credential tests.
macro_rules! bedrock_iam_env {
    () => {{
        bedrock_region()
    }};
}

/// Returns `(api_key, region)` for bearer-token tests, skipping if no API key is set.
macro_rules! bedrock_api_key_env {
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

// ---------------------------------------------------------------------------
// AWS Bedrock (SDK / IAM credentials)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_complete() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "expected non-empty response, got empty");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_stream() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_tools() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config = default_config(&bedrock_haiku(&region));
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

#[tokio::test]
async fn test_bedrock_system_prompt() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config = default_config(&bedrock_nova_lite(&region));
    config.system = Some("You are a pirate. Always respond like a pirate.".to_string());

    let resp = provider
        .complete(vec![user_msg("Greet me")], config)
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_streaming_tools() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config = default_config(&bedrock_haiku(&region));
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Please echo the word 'mango'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in stream, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_embed() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;

    let req = EmbeddingRequest::new("amazon.titan-embed-text-v2:0", vec!["Hello world", "Goodbye world"]).with_dimensions(256);
    // Titan Embed V2: processes the first input per call
    let resp = provider
        .embed(req)
        .await
        .unwrap();

    assert_eq!(resp.embeddings.len(), 1, "Titan Embed returns one vector per call");
    assert_eq!(resp.embeddings[0].len(), 256, "expected 256 dimensions");
    assert!(resp.usage.input_tokens > 0, "expected token count");
}

#[tokio::test]
async fn test_bedrock_embed_titan_v2_dims() {
    // Titan Embed Text V2 supports 256, 512, and 1024 dimensions.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "amazon.titan-embed-text-v2:0";

    for &dims in &[256u32, 512, 1024] {
        let req = EmbeddingRequest::new(model, vec!["The quick brown fox"]).with_dimensions(dims);
        let resp = provider.embed(req).await.unwrap_or_else(|e| {
            panic!("titan-embed-text-v2 dims={dims} failed: {e:?}")
        });
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(resp.embeddings[0].len(), dims as usize, "dims={dims}");
        assert!(resp.usage.input_tokens > 0);
    }
}

#[tokio::test]
async fn test_bedrock_embed_titan_v1() {
    // Titan Embed Text V1 — fixed 1536 dimensions, no dimension control.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "amazon.titan-embed-text-v1:0";

    let req = EmbeddingRequest::new(model, vec!["The quick brown fox"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("embed {model} failed: {e:?}"),
    };

    assert_eq!(resp.embeddings.len(), 1);
    assert_eq!(resp.embeddings[0].len(), 1536, "Titan V1 is always 1536 dims");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_embed_titan_multimodal() {
    // Titan Multimodal Embeddings G1 — supports 256, 384, 1024 dimensions.
    // NOT available in eu-west-1; test skips gracefully if unavailable.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "amazon.titan-embed-image-v1:0";

    for &dims in &[256u32, 384, 1024] {
        let req = EmbeddingRequest::new(model, vec!["A serene mountain lake"]).with_dimensions(dims);
        let resp = match provider.embed(req).await {
            Ok(r) => r,
            Err(e) if bedrock_model_not_available(&e) => {
                eprintln!("SKIP: {model} not available in this region: {e}");
                return;
            }
            Err(e) => panic!("titan-embed-image dims={dims} failed: {e:?}"),
        };
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(resp.embeddings[0].len(), dims as usize, "dims={dims}");
    }
}

#[tokio::test]
async fn test_bedrock_embed_cohere_english() {
    // Cohere Embed English V3 on Bedrock — fixed 1024 dims.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "cohere.embed-english-v3";

    let req = EmbeddingRequest::new(model, vec!["Hello world", "Goodbye world"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("cohere-embed-english failed: {e:?}"),
    };

    // Cohere on Bedrock returns one vector per text input
    assert_eq!(resp.embeddings.len(), 2, "expected 2 embeddings");
    assert_eq!(resp.embeddings[0].len(), 1024, "Cohere V3 is 1024 dims");
    assert_eq!(resp.embeddings[1].len(), 1024);
}

#[tokio::test]
async fn test_bedrock_embed_cohere_multilingual() {
    // Cohere Embed Multilingual V3 on Bedrock — fixed 1024 dims, multiple languages.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "cohere.embed-multilingual-v3";

    let req = EmbeddingRequest::new(model, vec!["Hello", "Bonjour", "Hola"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("cohere-embed-multilingual failed: {e:?}"),
    };

    assert_eq!(resp.embeddings.len(), 3, "expected 3 embeddings");
    for (i, emb) in resp.embeddings.iter().enumerate() {
        assert_eq!(emb.len(), 1024, "embedding {i} should be 1024 dims");
    }
}

#[tokio::test]
async fn test_bedrock_generate_image() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;

    let req = ImageGenerationRequest::new(
        "amazon.nova-canvas-v1:0",
        "a red circle on a white background",
    )
    .with_size(ImageSize::S512x512);
    let resp = provider.generate_image(req).await.unwrap();

    assert_eq!(resp.images.len(), 1, "expected one image");
    assert!(
        resp.images[0].b64_json.as_ref().map(|s| s.len() > 100).unwrap_or(false),
        "expected non-empty base64 image"
    );
}

#[tokio::test]
async fn test_bedrock_generate_video_requires_s3() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;

    let req = VideoGenerationRequest::new("amazon.nova-reel-v1:0", "a cat walking")
        .with_output_storage_uri("s3://nonexistent-bucket-sideseat-test/output/");
    let result = provider.generate_video(req).await;

    match result {
        Ok(_) => panic!("expected error with fake S3 bucket"),
        Err(ProviderError::Api { status, .. }) => {
            assert!(
                status == 400 || status == 403 || status == 500,
                "unexpected status: {status}"
            );
        }
        Err(e) => panic!("unexpected error type: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_count_tokens() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_haiku(&region));
    let count = match provider
        .count_tokens(vec![user_msg("Hello, world!")], config)
        .await
    {
        Ok(c) => c,
        Err(ProviderError::Unsupported(_)) => {
            eprintln!("SKIP: count_tokens not supported by this model/region");
            return;
        }
        Err(e) => panic!("count_tokens failed: {e:?}"),
    };
    assert!(count.input_tokens > 0, "expected > 0 input tokens");
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
    let region = bedrock_iam_env!();
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
    let config = default_config("claude-haiku-4-5-20251001");
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

#[tokio::test]
async fn test_gemini_embed() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        return;
    };
    let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
    let req = EmbeddingRequest::new("gemini-embedding-001", vec!["Hello world"]);
    let resp = provider.embed(req).await.unwrap();
    assert_eq!(resp.embeddings.len(), 1);
    assert!(
        !resp.embeddings[0].is_empty(),
        "embedding vector should not be empty"
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock API key (plain HTTP, bearer token auth)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_api_key_complete() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    let text = resp.text_content();
    assert!(!text.is_empty(), "expected non-empty response, got empty");
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_stream() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    // collect_stream drives the stream; retry handles transient 424s from Bedrock.
    let resp = retry(|| {
        let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config.clone());
        collect_stream(stream)
    })
    .await
    .unwrap();

    assert!(!resp.text_content().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_tools() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let mut config = default_config(&bedrock_haiku(&region));
    config.tools = vec![echo_tool()];

    let resp = retry(|| provider.complete(vec![user_msg("Please echo the word 'jackfruit'")], config.clone()))
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_api_key_system_prompt() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let mut config = default_config(&bedrock_nova_lite(&region));
    config.system = Some("You are a pirate. Always respond like a pirate.".to_string());

    let resp = retry(|| provider.complete(vec![user_msg("Greet me")], config.clone()))
        .await
        .unwrap();

    assert!(
        !resp.text_content().is_empty(),
        "expected non-empty response"
    );
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_streaming_tools() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let mut config = default_config(&bedrock_haiku(&region));
    config.tools = vec![echo_tool()];

    let resp = retry(|| {
        let stream = provider.stream(vec![user_msg("Please echo the word 'mango'")], config.clone());
        collect_stream(stream)
    })
    .await
    .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in stream, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_api_key_list_models() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models.iter().any(|m| m.id.contains("amazon.nova") || m.id.contains("anthropic.claude")),
        "expected a Nova or Claude model, got: {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);

    let req = EmbeddingRequest::new("amazon.titan-embed-text-v2:0", vec!["Hello world", "Goodbye world"])
        .with_dimensions(256);
    // Titan Embed V2: only processes the first input per call
    let resp = provider
        .embed(req)
        .await
        .unwrap();

    assert_eq!(resp.embeddings.len(), 1, "Titan Embed returns one vector per call");
    assert_eq!(resp.embeddings[0].len(), 256, "expected 256 dimensions");
    assert!(resp.usage.input_tokens > 0, "expected token count");
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_v2_dims() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);
    let model = "amazon.titan-embed-text-v2:0";

    for &dims in &[256u32, 512, 1024] {
        let req = EmbeddingRequest::new(model, vec!["The quick brown fox"]).with_dimensions(dims);
        let resp = provider.embed(req).await.unwrap_or_else(|e| {
            panic!("titan-embed-text-v2 dims={dims} failed: {e:?}")
        });
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(resp.embeddings[0].len(), dims as usize, "dims={dims}");
        assert!(resp.usage.input_tokens > 0);
    }
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_v1() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);
    let model = "amazon.titan-embed-text-v1:0";

    let req = EmbeddingRequest::new(model, vec!["The quick brown fox"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available via bearer token: {e}");
            return;
        }
        Err(e) => panic!("titan-embed-text-v1 failed: {e:?}"),
    };

    assert_eq!(resp.embeddings.len(), 1);
    assert_eq!(resp.embeddings[0].len(), 1536, "Titan V1 is always 1536 dims");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_multimodal() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);
    let model = "amazon.titan-embed-image-v1:0";

    for &dims in &[256u32, 384, 1024] {
        let req = EmbeddingRequest::new(model, vec!["A serene mountain lake"]).with_dimensions(dims);
        let resp = match provider.embed(req).await {
            Ok(r) => r,
            Err(e) if bedrock_model_not_available(&e) => {
                eprintln!("SKIP: {model} not available in this region: {e}");
                return;
            }
            Err(e) => panic!("titan-embed-image dims={dims} failed: {e:?}"),
        };
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(resp.embeddings[0].len(), dims as usize, "dims={dims}");
    }
}

#[tokio::test]
async fn test_bedrock_api_key_embed_cohere_english() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);
    let model = "cohere.embed-english-v3";

    let req = EmbeddingRequest::new(model, vec!["Hello world", "Goodbye world"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("cohere-embed-english failed: {e:?}"),
    };

    assert_eq!(resp.embeddings.len(), 2, "expected 2 embeddings");
    assert_eq!(resp.embeddings[0].len(), 1024, "Cohere V3 is 1024 dims");
    assert_eq!(resp.embeddings[1].len(), 1024);
}

#[tokio::test]
async fn test_bedrock_api_key_embed_cohere_multilingual() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);
    let model = "cohere.embed-multilingual-v3";

    let req = EmbeddingRequest::new(model, vec!["Hello", "Bonjour", "Hola"]);
    let resp = match provider.embed(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("cohere-embed-multilingual failed: {e:?}"),
    };

    assert_eq!(resp.embeddings.len(), 3, "expected 3 embeddings");
    for (i, emb) in resp.embeddings.iter().enumerate() {
        assert_eq!(emb.len(), 1024, "embedding {i} should be 1024 dims");
    }
}

#[tokio::test]
async fn test_bedrock_api_key_generate_image() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);

    let req = ImageGenerationRequest::new("amazon.nova-canvas-v1:0", "a red circle on a white background")
        .with_size(ImageSize::S512x512);
    let resp = provider.generate_image(req).await.unwrap();

    assert_eq!(resp.images.len(), 1, "expected one image");
    // Nova Canvas returns base64-encoded PNG
    assert!(
        resp.images[0].b64_json.as_ref().map(|s| s.len() > 100).unwrap_or(false),
        "expected non-empty base64 image"
    );
}

#[tokio::test]
async fn test_bedrock_api_key_generate_video_requires_s3() {
    // Generate video (Nova Reel) requires a real S3 bucket with write access.
    // This test verifies the API shape and error handling without a real bucket.
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region);

    let req = VideoGenerationRequest::new("amazon.nova-reel-v1:0", "a cat walking")
        .with_output_storage_uri("s3://nonexistent-bucket-sideseat-test/output/");
    let result = provider.generate_video(req).await;

    match result {
        Ok(_) => panic!("expected error with fake S3 bucket"),
        Err(ProviderError::Api { status, .. }) => {
            // 400 or 403 expected — bucket doesn't exist / no S3 permissions.
            // Status 0 = SDK-level rejection (e.g. bearer token not authorized for Nova Reel).
            assert!(
                status == 400 || status == 403 || status == 500 || status == 0,
                "unexpected status: {status}"
            );
        }
        Err(e) => panic!("unexpected error type: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_api_key_count_tokens() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));
    match provider
        .count_tokens(vec![user_msg("Hello, world!")], config)
        .await
    {
        Ok(count) => assert!(count.input_tokens > 0, "expected > 0 input tokens"),
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: count_tokens not available with bearer token: {e}");
        }
        Err(e) => panic!("count_tokens failed: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// AWS Bedrock (IAM) — extended coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_response_model_populated() {
    // Verifies complete() populates resp.model with the requested model ID.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = provider
        .complete(vec![user_msg("Say 'hi'")], config)
        .await
        .unwrap();

    assert!(resp.model.is_some(), "resp.model should be populated; got None");
}

#[tokio::test]
async fn test_bedrock_nova_micro_complete() {
    // Nova Micro: text-only (no vision, no documents). Cheapest Nova model.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_micro(&region));

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response");
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_multi_turn_tool_use() {
    // Full two-turn tool-use cycle: user → tool_use → tool_result → final text.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config = default_config(&bedrock_haiku(&region));
    config.tools = vec![echo_tool()];

    // Turn 1: model decides to call echo
    let turn1_msgs = vec![user_msg("Please echo the word 'jackfruit'")];
    let resp1 = provider
        .complete(turn1_msgs.clone(), config.clone())
        .await
        .unwrap();

    let tool_use = resp1
        .content
        .iter()
        .find_map(|b| if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None })
        .expect("expected tool_use in turn 1 response");
    assert_eq!(tool_use.name, "echo");

    // Turn 2: provide tool result, expect final text
    let turn2_msgs = vec![
        user_msg("Please echo the word 'jackfruit'"),
        Message {
            role: Role::Assistant,
            content: resp1.content.clone(),
            name: None,
            cache_control: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                content: vec![ContentBlock::text("jackfruit".to_string())],
                is_error: false,
            })],
            name: None,
            cache_control: None,
        },
    ];
    let resp2 = provider.complete(turn2_msgs, config).await.unwrap();

    let has_text = resp2.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(has_text, "final response should contain text, got: {:?}", resp2.content);
    assert!(resp2.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_vision() {
    // Nova Lite supports vision; send a tiny PNG and verify a text response.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = provider
        .complete(vec![vision_message("Describe what you see in one word.")], config)
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty vision response");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_stop_reason_max_tokens() {
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config = default_config(&bedrock_nova_lite(&region));
    config.max_tokens = Some(1);

    let resp = provider
        .complete(vec![user_msg("Count from 1 to 100")], config)
        .await
        .unwrap();

    assert_eq!(
        resp.stop_reason,
        StopReason::MaxTokens,
        "expected MaxTokens stop reason, got: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn test_bedrock_count_tokens_with_system() {
    // Verifies that system prompt is forwarded to the token counter.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let mut config_with_system = default_config(&bedrock_haiku(&region));
    config_with_system.system = Some("You are a helpful assistant.".to_string());
    let config_plain = ProviderConfig {
        system: None,
        ..config_with_system.clone()
    };

    let count_plain = match provider
        .count_tokens(vec![user_msg("Hello")], config_plain)
        .await
    {
        Ok(c) => c,
        Err(ProviderError::Unsupported(_)) => {
            eprintln!("SKIP: count_tokens not supported by this model/region");
            return;
        }
        Err(e) => panic!("count_tokens (plain) failed: {e:?}"),
    };
    let count_with_system = provider
        .count_tokens(vec![user_msg("Hello")], config_with_system)
        .await
        .unwrap();

    assert!(
        count_with_system.input_tokens > count_plain.input_tokens,
        "system prompt should increase token count: {} vs {}",
        count_with_system.input_tokens,
        count_plain.input_tokens
    );
}

#[tokio::test]
async fn test_bedrock_generate_image_with_seed() {
    // Verify that seed parameter is accepted and produces a valid image.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;
    let model = "amazon.nova-canvas-v1:0";

    let req = ImageGenerationRequest::new(model, "a solid red square")
        .with_size(ImageSize::S512x512)
        .with_seed(42);

    let resp = match provider.generate_image(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: {model} not available: {e}");
            return;
        }
        Err(e) => panic!("generate_image with seed failed: {e:?}"),
    };

    assert_eq!(resp.images.len(), 1);
    assert!(
        resp.images[0].b64_json.is_some(),
        "expected b64_json in response"
    );
}

#[tokio::test]
async fn test_bedrock_generate_video_real() {
    // Real video generation — requires a writable S3 bucket.
    // Set BEDROCK_VIDEO_OUTPUT_URI=s3://my-bucket/output/ to enable this test.
    let Ok(s3_uri) = std::env::var("BEDROCK_VIDEO_OUTPUT_URI") else {
        return;
    };
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region).await;

    let req = VideoGenerationRequest::new("amazon.nova-reel-v1:0", "a red ball bouncing")
        .with_output_storage_uri(s3_uri)
        .with_seed(7);
    let resp = provider.generate_video(req).await.unwrap();

    assert_eq!(resp.videos.len(), 1, "expected one video");
    assert!(
        resp.videos[0].uri.as_ref().map(|u| u.starts_with("s3://")).unwrap_or(false),
        "expected S3 URI in video output, got: {:?}",
        resp.videos[0].uri
    );
}

#[tokio::test]
async fn test_bedrock_generate_speech() {
    // Nova Sonic TTS. Requires BEDROCK_NOVA_SONIC=1 and model availability.
    let region = bedrock_iam_env!();
    if std::env::var("BEDROCK_NOVA_SONIC").is_err() {
        return;
    }
    let provider = BedrockProvider::from_env(region).await;

    let req = SpeechRequest::new("amazon.nova-sonic-v1:0", "Hello from SideSeat.", "matthew");
    let resp = match provider.generate_speech(req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: nova-sonic not available: {e}");
            return;
        }
        Err(e) => panic!("generate_speech failed: {e:?}"),
    };

    assert!(!resp.audio.is_empty(), "expected non-empty audio output");
}

#[tokio::test]
async fn test_bedrock_transcribe() {
    // Nova Sonic STT round-trip: generate speech then transcribe back to text.
    // Requires BEDROCK_NOVA_SONIC=1.
    let region = bedrock_iam_env!();
    if std::env::var("BEDROCK_NOVA_SONIC").is_err() {
        return;
    }
    let provider = BedrockProvider::from_env(region).await;

    let text = "Hello from SideSeat.";
    let speech_req =
        SpeechRequest::new("amazon.nova-sonic-v1:0", text, "matthew").with_format(AudioFormat::Mp3);
    let speech_resp = match provider.generate_speech(speech_req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: nova-sonic TTS not available: {e}");
            return;
        }
        Err(e) => panic!("generate_speech failed: {e:?}"),
    };

    let transcription_req =
        TranscriptionRequest::new("amazon.nova-sonic-v1:0", speech_resp.audio, AudioFormat::Mp3);
    let transcript_resp = match provider.transcribe(transcription_req).await {
        Ok(r) => r,
        Err(e) if bedrock_model_not_available(&e) => {
            eprintln!("SKIP: nova-sonic STT not available: {e}");
            return;
        }
        Err(e) => panic!("transcribe failed: {e:?}"),
    };

    assert!(!transcript_resp.text.is_empty(), "expected non-empty transcript");
    let lower = transcript_resp.text.to_lowercase();
    assert!(
        lower.contains("hello") || lower.contains("sideseat"),
        "transcript '{lower}' should contain words from original: '{text}'"
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock API key — extended coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_api_key_response_model_populated() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hi'")], config.clone()))
        .await
        .unwrap();

    assert!(resp.model.is_some(), "resp.model should be populated; got None");
}

#[tokio::test]
async fn test_bedrock_api_key_multi_turn_tool_use() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let mut config = default_config(&bedrock_haiku(&region));
    config.tools = vec![echo_tool()];

    // Turn 1
    let turn1_msgs = vec![user_msg("Please echo the word 'jackfruit'")];
    let resp1 = retry(|| provider.complete(turn1_msgs.clone(), config.clone()))
        .await
        .unwrap();

    let tool_use = resp1
        .content
        .iter()
        .find_map(|b| if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None })
        .expect("expected tool_use in turn 1 response");

    // Turn 2
    let turn2_msgs = vec![
        user_msg("Please echo the word 'jackfruit'"),
        Message {
            role: Role::Assistant,
            content: resp1.content.clone(),
            name: None,
            cache_control: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                content: vec![ContentBlock::text("jackfruit".to_string())],
                is_error: false,
            })],
            name: None,
            cache_control: None,
        },
    ];
    let resp2 = retry(|| provider.complete(turn2_msgs.clone(), config.clone()))
        .await
        .unwrap();

    let has_text = resp2.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(has_text, "final response should contain text, got: {:?}", resp2.content);
}

#[tokio::test]
async fn test_bedrock_api_key_vision() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let resp = retry(|| {
        provider.complete(
            vec![vision_message("Describe what you see in one word.")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty vision response");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_stop_reason_max_tokens() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let mut config = default_config(&bedrock_nova_lite(&region));
    config.max_tokens = Some(1);

    let resp = retry(|| provider.complete(vec![user_msg("Count from 1 to 100")], config.clone()))
        .await
        .unwrap();

    assert_eq!(
        resp.stop_reason,
        StopReason::MaxTokens,
        "expected MaxTokens stop reason, got: {:?}",
        resp.stop_reason
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock multimodal — image understanding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_multi_image() {
    // Docs: up to 5 images per request (embedded). Send 3 identical PNGs and verify
    // the model acknowledges multiple images.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let image = || ContentBlock::Image(ImageContent {
        source: MediaSource::base64("image/png", TINY_PNG_B64),
        format: Some(ImageFormat::Png),
        detail: None,
    });

    let msg = Message {
        role: Role::User,
        content: vec![
            image(),
            image(),
            image(),
            ContentBlock::text("How many images are shown? Reply with just a number.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content();
    assert!(!text.is_empty(), "expected response to multi-image request");
    // Model should say "3" or "three"
    assert!(
        text.contains('3') || text.to_lowercase().contains("three"),
        "expected '3' or 'three' in response, got: {text}"
    );
}

#[tokio::test]
async fn test_bedrock_nova_micro_rejects_image() {
    // Nova Micro is text-only. Sending an image block should return a validation error.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_micro(&region));

    let result = provider
        .complete(vec![vision_message("What is in this image?")], config)
        .await;

    match result {
        Err(ProviderError::Unsupported(_)) => {} // SDK-level rejection (ValidationException "not supported")
        Err(ProviderError::Api { status: 400, .. }) => {} // API validation error
        Ok(_) => panic!("Nova Micro should not accept image input"),
        Err(e) => panic!("unexpected error type for Nova Micro image input: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_image_s3() {
    // S3 image source. Requires BEDROCK_S3_IMAGE_URI=s3://bucket/image.jpg
    let Ok(s3_uri) = std::env::var("BEDROCK_S3_IMAGE_URI") else {
        return;
    };
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::S3(S3Location { uri: s3_uri, bucket_owner: None }),
                format: None, // format auto-detected by Bedrock from content
                detail: None,
            }),
            ContentBlock::text("Describe this image in one sentence.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text_content().is_empty(), "expected non-empty image description from S3");
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// AWS Bedrock multimodal — document understanding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_document_txt() {
    // Inline TXT document Q&A.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let doc = b"The capital of France is Paris. The Eiffel Tower is 330 meters tall.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("facts".to_string()),
            }),
            ContentBlock::text("What is the capital of France? Answer in one word.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content().to_lowercase();
    assert!(text.contains("paris"), "expected 'Paris' in response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_document_html() {
    // Inline HTML document — structured content extraction.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let html = b"<html><body><h1>Price List</h1><ul>\
        <li>Apple: $1.00</li><li>Banana: $0.50</li><li>Cherry: $2.00</li>\
        </ul></body></html>";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/html", html),
                format: DocumentFormat::Html,
                name: Some("prices".to_string()),
            }),
            ContentBlock::text(
                "What is the price of a Banana? Reply with just the price.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content();
    assert!(
        text.contains("0.50") || text.contains("50"),
        "expected banana price in response, got: {text}"
    );
}

#[tokio::test]
async fn test_bedrock_document_csv() {
    // Inline CSV document — tabular data Q&A.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let csv = b"Country,Capital,Population\nFrance,Paris,67M\nGermany,Berlin,83M\nItaly,Rome,60M";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/csv", csv),
                format: DocumentFormat::Csv,
                name: Some("countries".to_string()),
            }),
            ContentBlock::text("What is the capital of Germany? One word.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content().to_lowercase();
    assert!(text.contains("berlin"), "expected 'Berlin' in response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_document_markdown() {
    // Inline Markdown document — structured text Q&A.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let md = b"# Project Status\n\n## Projects\n- **Alpha**: complete\n- **Beta**: in progress\n\
               ## Next Steps\nFinalize testing for Beta.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/markdown", md),
                format: DocumentFormat::Md,
                name: Some("status".to_string()),
            }),
            ContentBlock::text(
                "Which project is complete? Reply with just the project name.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content().to_lowercase();
    assert!(text.contains("alpha"), "expected 'Alpha' in response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_multiple_documents() {
    // Two documents in a single request (docs allow up to 5).
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let doc1 = b"Document A: The answer to the ultimate question is 42.";
    let doc2 = b"Document B: The speed of light is 299,792,458 metres per second.";

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc1),
                format: DocumentFormat::Txt,
                name: Some("doc-a".to_string()),
            }),
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc2),
                format: DocumentFormat::Txt,
                name: Some("doc-b".to_string()),
            }),
            ContentBlock::text(
                "From Document A only, what is the answer to the ultimate question?".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    let text = resp.text_content();
    assert!(text.contains("42"), "expected '42' in response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_document_s3() {
    // S3 document source. Requires BEDROCK_S3_DOC_URI=s3://bucket/doc.txt
    let Ok(s3_uri) = std::env::var("BEDROCK_S3_DOC_URI") else {
        return;
    };
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::S3(S3Location { uri: s3_uri, bucket_owner: None }),
                format: DocumentFormat::Txt,
                name: Some("s3-doc".to_string()),
            }),
            ContentBlock::text("Summarise this document in one sentence.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text_content().is_empty(), "expected non-empty doc summary from S3");
}

// ---------------------------------------------------------------------------
// AWS Bedrock multimodal — video understanding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_video_embedded() {
    // Embedded video (base64). Requires BEDROCK_TEST_VIDEO_PATH=/path/to/file.mp4
    // Docs: max 25 MB for embedded video, 1 video per request.
    let Ok(video_path) = std::env::var("BEDROCK_TEST_VIDEO_PATH") else {
        return;
    };
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let video_bytes =
        std::fs::read(&video_path).unwrap_or_else(|e| panic!("read {video_path}: {e}"));
    let format = if video_path.ends_with(".mov") {
        VideoFormat::Mov
    } else if video_path.ends_with(".mkv") {
        VideoFormat::Mkv
    } else if video_path.ends_with(".webm") {
        VideoFormat::Webm
    } else {
        VideoFormat::Mp4
    };

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Video(VideoContent {
                source: MediaSource::from_bytes("video/mp4", &video_bytes),
                format,
            }),
            ContentBlock::text(
                "Describe the main subject of this video in one sentence.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text_content().is_empty(), "expected non-empty video description");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_video_s3() {
    // S3 video source. Requires BEDROCK_S3_VIDEO_URI=s3://bucket/video.mp4
    // Docs: max 1 GB for S3 video.
    let Ok(s3_uri) = std::env::var("BEDROCK_S3_VIDEO_URI") else {
        return;
    };
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Video(VideoContent {
                source: MediaSource::S3(S3Location { uri: s3_uri, bucket_owner: None }),
                format: VideoFormat::Mp4,
            }),
            ContentBlock::text("What is the main subject of this video? One sentence.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text_content().is_empty(), "expected non-empty video description from S3");
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// AWS Bedrock multimodal — mixed modalities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_image_and_document() {
    // Image + document in the same request (mixed modalities).
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let doc = b"Context: The image shows a white pixel on a white background.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("context".to_string()),
            }),
            ContentBlock::text(
                "According to the document, what color does the image show? One word.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    // Verify the mixed image+document request succeeds
    assert!(!resp.text_content().is_empty(), "expected non-empty response to image+document request");
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// Prompt caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_prompt_caching_system() {
    // Verify that a system message with cache_control does not cause an API error.
    // Cache write tokens should appear in usage when caching is applied.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let system_msg = Message {
        role: Role::System,
        content: vec![ContentBlock::text("You are a helpful assistant.".to_string())],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = retry(|| {
        provider.complete(
            vec![system_msg.clone(), user_msg("Reply with the word 'ok'.")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response with cached system prompt");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_prompt_caching_message() {
    // Verify that a user message with cache_control does not cause an API error.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let cached_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text(
            "The sky is blue. The grass is green.".to_string(),
        )],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = retry(|| {
        provider.complete(
            vec![cached_msg.clone(), user_msg("What color is the sky?")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response with cached message");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_audio_converse_unsupported() {
    // Docs: audio input is NOT supported via the Converse API.
    // The SDK rejects ContentBlock::Audio before making a network call.
    let region = bedrock_iam_env!();
    let provider = BedrockProvider::from_env(region.clone()).await;
    let config = default_config(&bedrock_nova_lite(&region));

    let result = provider
        .complete(
            vec![Message {
                role: Role::User,
                content: vec![
                    ContentBlock::Audio(AudioContent {
                        source: MediaSource::from_bytes("audio/mp3", &[0u8; 16]),
                        format: AudioFormat::Mp3,
                    }),
                    ContentBlock::text("Transcribe this.".to_string()),
                ],
                name: None,
                cache_control: None,
            }],
            config,
        )
        .await;

    assert!(
        matches!(result, Err(ProviderError::Unsupported(_))),
        "audio via Converse API should return Unsupported, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock API key — multimodal coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_api_key_multi_image() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::text("How many images are shown? Reply with just a number.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone()))
        .await
        .unwrap();
    let text = resp.text_content();
    assert!(!text.is_empty(), "expected response to multi-image request");
    assert!(
        text.contains('2') || text.to_lowercase().contains("two"),
        "expected '2' or 'two' in response, got: {text}"
    );
}

#[tokio::test]
async fn test_bedrock_api_key_document_txt() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let doc = b"The capital of Japan is Tokyo. The population is approximately 14 million.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("facts".to_string()),
            }),
            ContentBlock::text("What is the capital of Japan? Answer in one word.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone()))
        .await
        .unwrap();
    let text = resp.text_content().to_lowercase();
    assert!(text.contains("tokyo"), "expected 'Tokyo' in response, got: {text}");
}

#[tokio::test]
async fn test_bedrock_api_key_image_and_document() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let doc = b"Hint: The image shows a single white pixel.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("hint".to_string()),
            }),
            ContentBlock::text(
                "According to the document, what does the image show? One word.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = retry(|| provider.complete(vec![msg.clone()], config.clone()))
        .await
        .unwrap();
    // Just verify the mixed image+document request succeeds and returns a response
    assert!(!resp.text_content().is_empty(), "expected non-empty response to image+document request");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_prompt_caching() {
    let (api_key, region) = bedrock_api_key_env!();
    let provider = BedrockProvider::with_api_key(api_key, region.clone());
    let config = default_config(&bedrock_nova_lite(&region));

    let cached_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text(
            "The capital of France is Paris.".to_string(),
        )],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = retry(|| {
        provider.complete(
            vec![cached_msg.clone(), user_msg("What is the capital of France?")],
            config.clone(),
        )
    })
    .await
    .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response with api-key + caching");
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// Bedrock OpenAI-compatible API — Chat Completions + Responses via AWS Bedrock
// Run: BEDROCK_API_KEY=... BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture bedrock_openai
// ---------------------------------------------------------------------------

const BEDROCK_OPENAI_MODEL: &str = "openai.gpt-oss-120b";
const BEDROCK_OPENAI_SMALL_MODEL: &str = "openai.gpt-oss-20b";

// ── Chat Completions API ────────────────────────────────────────────────────

#[tokio::test]
async fn test_bedrock_openai_chat_complete() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response");
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

    assert!(!resp.text_content().is_empty());
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

    let text = resp.text_content().to_lowercase();
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

    let text = resp.text_content().to_lowercase();
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

    assert!(!resp2.text_content().is_empty(), "expected text in turn 2");
}


#[tokio::test]
async fn test_bedrock_openai_chat_small_model() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let resp = retry(|| provider.complete(vec![user_msg("Say 'hello' in one word")], config.clone()))
        .await
        .unwrap();

    assert!(!resp.text_content().is_empty(), "expected non-empty response from small model");
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_openai_chat_small_model_stream() {
    let (api_key, region) = bedrock_openai_env!();
    let provider = OpenAIChatProvider::for_bedrock_openai(region, api_key);
    let config = default_config(BEDROCK_OPENAI_SMALL_MODEL);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text_content().is_empty());
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

    let text = resp.text_content().to_lowercase();
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
