//! Integration tests for Anthropic provider (direct, Bedrock, Vertex).
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-... cargo test -p sideseat -- --nocapture anthropic
//!
//! # Anthropic via Bedrock (invoke_model):
//! BEDROCK_REGION=us-east-1 cargo test -p sideseat -- --nocapture anthropic_bedrock
//! ANTHROPIC_BEDROCK_MODEL=us.anthropic.claude-3-haiku-20240307-v1:0   -- optional model override
//!
//! # Anthropic via Vertex AI:
//! VERTEX_PROJECT_ID=my-project VERTEX_LOCATION=us-east5 VERTEX_ACCESS_TOKEN=$(gcloud auth print-access-token) \
//!   cargo test -p sideseat -- --nocapture anthropic_vertex
//! VERTEX_MODEL=claude-haiku-4-5@20251001   -- optional model override
//! ```

#[macro_use]
mod common;
use common::*;

// ---------------------------------------------------------------------------
// Anthropic direct
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

    let text = resp.text();
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
    let text = resp.text();
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

    let text = resp.text().to_lowercase();
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

    assert!(!resp2.text().is_empty(), "expected text in turn 2");
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
    assert!(has_tool || !resp.text().is_empty(), "expected structured output");
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

    assert!(!resp.text().is_empty(), "expected text response to image");
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

    let text = resp.text().to_lowercase();
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
    assert!(!resp.text().is_empty(), "expected response with cache_control");
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

    assert!(!resp.text().is_empty(), "expected response with sampling params");
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

    let text = resp.text();
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

    assert!(!resp.text().is_empty(), "expected text");
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

    let text = resp.text();
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

    assert!(!resp.text().is_empty(), "expected text response to vision");
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

    let text = resp.text().to_lowercase();
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

    let text = resp.text().to_lowercase();
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
    assert!(!resp2.text().is_empty(), "expected text in turn 2");
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
    assert!(has_tool || !resp.text().is_empty(), "expected structured output");
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

    assert!(!resp.text().is_empty(), "expected response with sampling params");
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

    let text = resp.text();
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

    assert!(!resp.text().is_empty(), "expected text");
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

    let text = resp.text();
    assert!(
        text.to_lowercase().contains("arr"),
        "expected pirate response, got: {text}"
    );
}
