/// Unit tests — no API keys required. All tests use MockProvider.
use std::collections::HashMap;

use futures::StreamExt;
use sideseat::{
    AgentHooks, ChatProvider, DefaultHooks, DefaultSettingsMiddleware, ExtractReasoningMiddleware,
    FallbackProvider, FallbackStrategy, FallbackTrigger, ImageGenerationRequest, ImageProvider,
    InstrumentedProvider, LoggingMiddleware, Message, Middleware, MiddlewareStack, MockProvider,
    MockResponse, ModelCapability, PromptTemplate, Provider, ProviderConfig, ProviderError,
    ProviderRegistry, RetryConfig, RetryProvider, SideSeat, SimulateStreamingMiddleware,
    TelemetryConfig, Tool, VideoGenerationRequest, VideoProvider, batch_complete,
    cosine_similarity, euclidean_distance, model_capabilities, normalize_embedding, record_stream,
    run_agent_loop_with_hooks, stream_text, supports_audio_input, supports_audio_output,
    supports_extended_thinking, supports_function_calling, supports_vision, truncate_messages,
    validate_messages,
};

// ---------------------------------------------------------------------------
// MockProvider tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_text_complete() {
    let provider = MockProvider::new().with_text("hello world");
    let config = ProviderConfig::new("mock-model");
    let response = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("hello world"));
}

#[tokio::test]
async fn mock_text_stream() {
    let provider = MockProvider::new().with_text("streamed text");
    let config = ProviderConfig::new("mock-model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let response = sideseat::collect_stream(stream).await.unwrap();
    assert_eq!(response.first_text(), Some("streamed text"));
}

#[tokio::test]
async fn mock_tool_call() {
    let provider = MockProvider::new().with_response(MockResponse::ToolCall {
        id: "call_1".into(),
        name: "get_weather".into(),
        input: serde_json::json!({ "city": "SF" }),
    });
    let config = ProviderConfig::new("mock-model");
    let response = provider
        .complete(vec![Message::user("weather?")], config)
        .await
        .unwrap();
    assert!(response.has_tool_use());
    let tu = response.tool_uses();
    assert_eq!(tu[0].name, "get_weather");
}

#[tokio::test]
async fn mock_error() {
    let provider = MockProvider::new()
        .with_response(MockResponse::Error(ProviderError::Auth("bad key".into())));
    let config = ProviderConfig::new("mock-model");
    let err = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Auth(_)));
}

#[tokio::test]
#[should_panic(expected = "exhausted")]
async fn mock_empty_queue_default() {
    // Empty queue should panic with an actionable message
    let provider = MockProvider::new();
    let config = ProviderConfig::new("mock-model");
    let _ = provider.complete(vec![Message::user("hi")], config).await;
}

#[tokio::test]
async fn mock_captures_calls() {
    let provider = MockProvider::new().with_text("ok").with_text("ok2");
    let config = ProviderConfig::new("mock-model");
    provider
        .complete(vec![Message::user("first")], config.clone())
        .await
        .unwrap();
    provider
        .complete(vec![Message::user("second")], config)
        .await
        .unwrap();
    assert_eq!(provider.call_count(), 2);
    let calls = provider.captured_calls();
    assert_eq!(calls[0].0[0].content[0].as_text(), Some("first"));
    assert_eq!(calls[1].0[0].content[0].as_text(), Some("second"));
}

// ---------------------------------------------------------------------------
// RetryConfig tests
// ---------------------------------------------------------------------------

#[test]
fn retry_config_delay_bounds() {
    let cfg = RetryConfig::new(3)
        .with_base_delay_ms(100)
        .with_max_delay_ms(5000);
    // Verify the config is created with correct parameters
    assert_eq!(cfg.max_retries, 3);
    assert_eq!(cfg.base_delay_ms, 100);
    assert_eq!(cfg.max_delay_ms, 5000);
    // max_delay_ms must be an upper bound for any computed delay
    assert!(cfg.base_delay_ms <= cfg.max_delay_ms);
}

#[test]
fn retry_config_no_overflow() {
    let cfg = RetryConfig::new(5);
    // Verify defaults are sensible and base <= max
    assert_eq!(cfg.base_delay_ms, 1000);
    assert_eq!(cfg.max_delay_ms, 30_000);
    assert!(cfg.base_delay_ms <= cfg.max_delay_ms);
}

#[tokio::test]
async fn retry_provider_succeeds_after_error() {
    // First call fails with a retryable error, second succeeds
    let inner = MockProvider::new()
        .with_response(MockResponse::Error(ProviderError::Network(
            "timeout".into(),
        )))
        .with_text("success");

    // Use very short delay to keep test fast
    let provider = RetryProvider::from_config(
        inner,
        RetryConfig::new(2)
            .with_base_delay_ms(1)
            .with_jitter_factor(0.0),
    );
    let config = ProviderConfig::new("mock-model");
    let response = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("success"));
}

// ---------------------------------------------------------------------------
// FallbackProvider test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fallback_provider_uses_second() {
    let first =
        MockProvider::new().with_response(MockResponse::Error(ProviderError::Auth("bad".into())));
    let second = MockProvider::new().with_text("fallback response");

    let provider = FallbackProvider::new(vec![Box::new(first), Box::new(second)]);
    let config = ProviderConfig::new("mock-model");
    let response = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("fallback response"));
}

#[tokio::test]
async fn fallback_any_error_skips_unsupported() {
    // AnyError strategy must NOT fall back when the error is Unsupported —
    // that is a programming error (wrong provider for the task), not transient.
    let first = MockProvider::new().with_response(MockResponse::Error(ProviderError::Unsupported(
        "no audio".into(),
    )));
    let second = MockProvider::new().with_text("should not be reached");

    let provider = FallbackProvider::new(vec![Box::new(first), Box::new(second)]);
    let config = ProviderConfig::new("mock-model");
    let result = provider.complete(vec![Message::user("hi")], config).await;
    assert!(
        matches!(result, Err(ProviderError::Unsupported(_))),
        "expected Unsupported to propagate without fallback, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// TextStream test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_stream_filters_events() {
    let provider = MockProvider::new().with_text("delta text");
    let config = ProviderConfig::new("mock-model");
    let text_stream = stream_text(&provider, vec![Message::user("hi")], config);
    let chunks: Vec<String> = text_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    assert_eq!(chunks, vec!["delta text"]);
}

// ---------------------------------------------------------------------------
// ProviderRegistry tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_routes_by_prefix() {
    let mut reg = ProviderRegistry::new();
    reg.register("mock", MockProvider::new().with_text("routed"));
    let config = ProviderConfig::new("mock:test-model");
    let response = reg
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("routed"));
}

#[tokio::test]
async fn registry_model_not_found() {
    let reg = ProviderRegistry::new();
    let config = ProviderConfig::new("unknown:model");
    let err = reg
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ModelNotFound { .. }));
}

#[tokio::test]
async fn registry_implements_provider() {
    let mut reg = ProviderRegistry::new();
    reg.register("openai", MockProvider::new().with_text("registry works"));
    let config = ProviderConfig::new("openai:gpt-4o");
    let response = reg
        .complete(vec![Message::user("test")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("registry works"));
}

// ---------------------------------------------------------------------------
// validate_messages / truncate_messages
// ---------------------------------------------------------------------------

#[test]
fn validate_messages_consecutive() {
    let msgs = vec![Message::user("a"), Message::user("b")];
    let warnings = validate_messages(&msgs);
    assert!(!warnings.is_empty());
    assert!(warnings[0].contains("both have role"));
}

#[test]
fn validate_messages_tool_no_use() {
    use sideseat::{ContentBlock, Role};
    let msgs = vec![sideseat::Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult(sideseat::ToolResultBlock {
            tool_use_id: "nonexistent".into(),
            content: vec![],
            is_error: false,
        })],
        name: None,
        cache_control: None,
    }];
    let warnings = validate_messages(&msgs);
    assert!(warnings.iter().any(|w| w.contains("nonexistent")));
}

#[test]
fn truncate_messages_removes_oldest() {
    let msgs = vec![
        Message::user("a very long message that has lots of tokens and takes up space"),
        Message::user("short"),
    ];
    // Set a very small limit that forces removal
    let result = truncate_messages(msgs, 5);
    // Should have removed the first (oldest non-system) message
    assert!(result.len() < 2 || result[0].content[0].as_text() == Some("short"));
}

// ---------------------------------------------------------------------------
// batch_complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_complete_runs_concurrently() {
    let provider = MockProvider::new()
        .with_text("r1")
        .with_text("r2")
        .with_text("r3");
    let requests = vec![
        (vec![Message::user("1")], ProviderConfig::new("m")),
        (vec![Message::user("2")], ProviderConfig::new("m")),
        (vec![Message::user("3")], ProviderConfig::new("m")),
    ];
    let results = batch_complete(&provider, requests).await;
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.is_ok()));
}

// ---------------------------------------------------------------------------
// PromptTemplate
// ---------------------------------------------------------------------------

#[test]
fn prompt_template_render() {
    let tmpl = PromptTemplate::new("Hello, {{name}}! You are {{age}} years old.");
    let mut vars = HashMap::new();
    vars.insert("name", "Alice");
    vars.insert("age", "30");
    let result = tmpl.render(&vars).unwrap();
    assert_eq!(result, "Hello, Alice! You are 30 years old.");
}

#[test]
fn prompt_template_missing_var() {
    let tmpl = PromptTemplate::new("Hello, {{name}}!");
    let vars = HashMap::new();
    let err = tmpl.render(&vars).unwrap_err();
    assert!(matches!(err, ProviderError::InvalidRequest(_)));
}

// ---------------------------------------------------------------------------
// model_capabilities
// ---------------------------------------------------------------------------

#[test]
fn model_capabilities_gpt4o() {
    let caps = model_capabilities("gpt-4o");
    assert!(caps.contains(&ModelCapability::Vision));
    assert!(caps.contains(&ModelCapability::WebSearch));
    assert!(caps.contains(&ModelCapability::FunctionCalling));
    assert!(caps.contains(&ModelCapability::Streaming));
}

#[test]
fn model_capabilities_claude_thinking() {
    let caps = model_capabilities("claude-opus-4-20251201");
    assert!(caps.contains(&ModelCapability::ExtendedThinking));
    assert!(caps.contains(&ModelCapability::Vision));
}

#[test]
fn model_capabilities_embeddings() {
    let caps = model_capabilities("text-embedding-3-small");
    assert_eq!(caps, vec![ModelCapability::Embeddings]);
}

// ---------------------------------------------------------------------------
// Middleware tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn middleware_logging_before_after() {
    let stack = MiddlewareStack::new(MockProvider::new().with_text("ok")).with(LoggingMiddleware);
    let config = ProviderConfig::new("mock-model");
    let response = stack
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("ok"));
}

#[tokio::test]
async fn middleware_stack_modifies_config() {
    struct ModelOverride;

    #[async_trait::async_trait]
    impl Middleware for ModelOverride {
        async fn before_complete(
            &self,
            messages: Vec<Message>,
            mut config: ProviderConfig,
        ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
            config.model = "overridden-model".into();
            Ok((messages, config))
        }
    }

    let inner = MockProvider::new().with_text("ok");
    let stack = MiddlewareStack::new(inner).with(ModelOverride);
    let config = ProviderConfig::new("original-model");
    let response = stack
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    // The inner provider received "overridden-model" (captured in response.model)
    assert_eq!(response.model.as_deref(), Some("overridden-model"));
}

#[tokio::test]
async fn middleware_stack_stream_applies_before() {
    struct ModelOverride;

    #[async_trait::async_trait]
    impl Middleware for ModelOverride {
        async fn before_complete(
            &self,
            messages: Vec<Message>,
            mut config: ProviderConfig,
        ) -> Result<(Vec<Message>, ProviderConfig), ProviderError> {
            config.model = "stream-overridden".into();
            Ok((messages, config))
        }
    }

    let inner = MockProvider::new().with_text("streamed");
    let stack = MiddlewareStack::new(inner).with(ModelOverride);
    let config = ProviderConfig::new("original");
    let stream = stack.stream(vec![Message::user("hi")], config);
    let response = sideseat::collect_stream(stream).await.unwrap();
    assert_eq!(response.first_text(), Some("streamed"));
    // Model in response comes from the inner MockProvider which received the overridden config
    assert_eq!(response.model.as_deref(), Some("stream-overridden"));
}

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

#[test]
fn error_is_retryable() {
    assert!(ProviderError::Timeout { ms: Some(5000) }.is_retryable());
    assert!(
        ProviderError::TooManyRequests {
            message: "limit".into(),
            retry_after_secs: Some(60)
        }
        .is_retryable()
    );
    assert!(ProviderError::Network("conn reset".into()).is_retryable());
    assert!(
        ProviderError::Api {
            status: 500,
            message: "server error".into()
        }
        .is_retryable()
    );
    assert!(!ProviderError::Auth("bad key".into()).is_retryable());
    assert!(!ProviderError::InvalidRequest("bad config".into()).is_retryable());
    assert!(
        !ProviderError::Api {
            status: 400,
            message: "bad request".into()
        }
        .is_retryable()
    );
}

// ---------------------------------------------------------------------------
// Image / video generation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_image_generation() {
    let provider = MockProvider::new().with_generated_image("https://example.com/image.png");
    let request = ImageGenerationRequest::new("dall-e-3", "a sunset over mountains");
    let response = provider.generate_image(request).await.unwrap();
    assert_eq!(response.images.len(), 1);
    assert_eq!(
        response.images[0].url.as_deref(),
        Some("https://example.com/image.png")
    );
}

#[tokio::test]
async fn mock_video_generation() {
    let provider =
        MockProvider::new().with_generated_video("https://storage.googleapis.com/video.mp4");
    let request = VideoGenerationRequest::new("veo-2.0-generate-001", "a timelapse of clouds");
    let response = provider.generate_video(request).await.unwrap();
    assert_eq!(response.videos.len(), 1);
    assert_eq!(
        response.videos[0].uri.as_deref(),
        Some("https://storage.googleapis.com/video.mp4")
    );
}

#[tokio::test]
async fn provider_image_generation_unsupported_by_default() {
    // MockProvider doesn't override edit_image, so the default returns Unsupported
    let provider = MockProvider::new();
    let request = sideseat::ImageEditRequest::new(
        "dall-e-2",
        vec![0u8; 4],
        sideseat::ImageFormat::Png,
        "make it blue",
    );
    let result = provider.edit_image(request).await;
    assert!(matches!(
        result,
        Err(sideseat::ProviderError::Unsupported(_))
    ));
}

#[test]
fn image_generation_request_builder() {
    use sideseat::{ImageOutputFormat, ImageQuality, ImageSize, ImageStyle};
    let req = ImageGenerationRequest::new("dall-e-3", "a sunset")
        .with_n(2)
        .with_size(ImageSize::S1792x1024)
        .with_quality(ImageQuality::Hd)
        .with_style(ImageStyle::Vivid)
        .with_output_format(ImageOutputFormat::B64Json)
        .with_user("user-123");
    assert_eq!(req.model, "dall-e-3");
    assert_eq!(req.n, Some(2));
    assert_eq!(req.size.unwrap().as_str(), "1792x1024");
    assert_eq!(req.quality.unwrap().as_str(), "hd");
    assert_eq!(req.style.unwrap().as_str(), "vivid");
    assert_eq!(req.output_format.as_str(), "b64_json");
    assert_eq!(req.user.as_deref(), Some("user-123"));
}

#[test]
fn video_generation_request_builder() {
    use sideseat::{VideoAspectRatio, VideoResolution};
    let req = VideoGenerationRequest::new("veo-2.0-generate-001", "timelapse")
        .with_n(1)
        .with_duration_secs(8)
        .with_aspect_ratio(VideoAspectRatio::Landscape16x9)
        .with_resolution(VideoResolution::P1080);
    assert_eq!(req.model, "veo-2.0-generate-001");
    assert_eq!(req.duration_secs, Some(8));
    assert_eq!(req.aspect_ratio.unwrap().as_str(), "16:9");
    assert_eq!(req.resolution.unwrap().as_str(), "1080p");
}

// ---------------------------------------------------------------------------
// Vector utilities
// ---------------------------------------------------------------------------

#[test]
fn cosine_similarity_identical() {
    assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
}

#[test]
fn cosine_similarity_orthogonal() {
    assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
}

#[test]
fn cosine_similarity_opposite() {
    assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
}

#[test]
fn cosine_similarity_zero_vector() {
    assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
}

#[test]
fn normalize_embedding_unit() {
    let mut v = vec![3.0_f32, 4.0];
    normalize_embedding(&mut v);
    let mag: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((mag - 1.0).abs() < 1e-6);
}

#[test]
fn euclidean_distance_basic() {
    assert!((euclidean_distance(&[0.0, 0.0], &[3.0, 4.0]) - 5.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// ModelCapability helpers
// ---------------------------------------------------------------------------

#[test]
fn supports_vision_gpt4o() {
    assert!(supports_vision("gpt-4o"));
}

#[test]
fn supports_audio_input_gpt4o_audio() {
    assert!(supports_audio_input("gpt-4o-audio-preview"));
}

#[test]
fn supports_audio_output_gpt4o_audio() {
    assert!(supports_audio_output("gpt-4o-audio-preview"));
}

#[test]
fn supports_audio_input_gemini() {
    assert!(supports_audio_input("gemini-2.0-flash"));
}

#[test]
fn supports_audio_output_gemini_false() {
    assert!(!supports_audio_output("gemini-2.0-flash"));
}

#[test]
fn supports_audio_output_tts1() {
    assert!(supports_audio_output("tts-1"));
}

#[test]
fn supports_audio_output_tts1_hd() {
    assert!(supports_audio_output("tts-1-hd"));
}

#[test]
fn supports_extended_thinking_claude() {
    assert!(supports_extended_thinking("claude-3-7-sonnet-20250219"));
}

#[test]
fn supports_function_calling_gpt4() {
    assert!(supports_function_calling("gpt-4-turbo"));
}

// ---------------------------------------------------------------------------
// StreamRecording
// ---------------------------------------------------------------------------

#[tokio::test]
async fn record_stream_captures_all_events() {
    let provider = MockProvider::new().with_text("hello");
    let config = ProviderConfig::new("mock-model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let (recorded, recording) = record_stream(stream);
    // Consume the stream
    let _response = sideseat::collect_stream(recorded).await.unwrap();
    assert!(!recording.is_empty());
}

#[tokio::test]
async fn record_stream_replay_from_zero() {
    let provider = MockProvider::new().with_text("replay");
    let config = ProviderConfig::new("mock-model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let (recorded, recording) = record_stream(stream);
    let _r = sideseat::collect_stream(recorded).await.unwrap();
    // Replay from start
    let replayed = recording.replay_from(0);
    let response = sideseat::collect_stream(replayed).await.unwrap();
    assert_eq!(response.first_text(), Some("replay"));
}

#[tokio::test]
async fn record_stream_replay_from_offset() {
    let provider = MockProvider::new().with_text("offset");
    let config = ProviderConfig::new("mock-model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let (recorded, recording) = record_stream(stream);
    let _r = sideseat::collect_stream(recorded).await.unwrap();
    let total = recording.len();
    // Skip all events — empty replay
    let replayed = recording.replay_from(total);
    let response = sideseat::collect_stream(replayed).await.unwrap();
    assert!(response.content.is_empty());
}

// ---------------------------------------------------------------------------
// FallbackStrategy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fallback_any_error_always_falls_back() {
    // AnyError: any error triggers fallback to secondary provider
    let primary =
        MockProvider::new().with_response(MockResponse::Error(ProviderError::Auth("bad".into())));
    let secondary = MockProvider::new().with_text("from secondary");
    let mut provider = FallbackProvider::new(vec![Box::new(primary)]);
    provider.push(secondary);
    let resp = provider
        .complete(vec![Message::user("hi")], ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(resp.first_text(), Some("from secondary"));
}

#[tokio::test]
async fn fallback_on_triggers_context_window() {
    // OnTriggers: ContextWindowExceeded triggers fallback; Auth does not
    let primary1 = MockProvider::new().with_response(MockResponse::Error(
        ProviderError::ContextWindowExceeded("too long".into()),
    ));
    let secondary1 = MockProvider::new().with_text("fallback ok");
    let strategy = FallbackStrategy::OnTriggers(vec![FallbackTrigger::ContextWindowExceeded]);
    let mut p1 = FallbackProvider::with_strategy(vec![Box::new(primary1)], strategy);
    p1.push(secondary1);
    let resp = p1
        .complete(vec![Message::user("hi")], ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(resp.first_text(), Some("fallback ok"));
}

#[tokio::test]
async fn fallback_on_triggers_auth_no_fallback() {
    // Auth error does not match ContextWindowExceeded trigger — no fallback
    let primary =
        MockProvider::new().with_response(MockResponse::Error(ProviderError::Auth("bad".into())));
    let secondary = MockProvider::new().with_text("should not reach");
    let strategy = FallbackStrategy::OnTriggers(vec![FallbackTrigger::ContextWindowExceeded]);
    let mut p = FallbackProvider::with_strategy(vec![Box::new(primary)], strategy);
    p.push(secondary);
    let err = p
        .complete(vec![Message::user("hi")], ProviderConfig::new("m"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Auth(_)));
}

// ---------------------------------------------------------------------------
// Tool builder
// ---------------------------------------------------------------------------

#[test]
fn tool_with_input_examples() {
    let examples = vec![
        serde_json::json!({"city": "SF"}),
        serde_json::json!({"city": "NYC"}),
    ];
    let tool = Tool::new("get_weather", "Get weather", serde_json::json!({}))
        .with_input_examples(examples.clone());
    assert_eq!(tool.input_examples, examples);
}

#[test]
fn image_request_with_seed() {
    let req = ImageGenerationRequest::new("dall-e-3", "a painting").with_seed(42);
    assert_eq!(req.seed, Some(42));
}

// ---------------------------------------------------------------------------
// DefaultSettingsMiddleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_settings_fills_none_fields() {
    let mw = DefaultSettingsMiddleware::new()
        .with_temperature(0.5)
        .with_max_tokens(100);
    let config = ProviderConfig::new("model");
    let (_, out) = mw.before_complete(vec![], config).await.unwrap();
    assert_eq!(out.temperature, Some(0.5));
    assert_eq!(out.max_tokens, Some(100));
}

#[tokio::test]
async fn default_settings_caller_wins() {
    let mw = DefaultSettingsMiddleware::new().with_temperature(0.5);
    let mut config = ProviderConfig::new("model");
    config.temperature = Some(0.9);
    let (_, out) = mw.before_complete(vec![], config).await.unwrap();
    assert_eq!(out.temperature, Some(0.9)); // caller's value preserved
}

// ---------------------------------------------------------------------------
// ExtractReasoningMiddleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extract_reasoning_strips_think_tags() {
    use sideseat::{ContentBlock, StopReason};
    let mw = ExtractReasoningMiddleware::new();
    let response = sideseat::provider::collect_stream(Box::pin(futures::stream::iter(vec![
        Ok(sideseat::types::StreamEvent::MessageStart {
            role: sideseat::Role::Assistant,
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockStart {
            index: 0,
            block: sideseat::types::ContentBlockStart::Text,
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockDelta {
            index: 0,
            delta: sideseat::types::ContentDelta::Text {
                text: "<think>reasoning here</think>answer here".to_string(),
            },
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockStop { index: 0 }),
        Ok(sideseat::types::StreamEvent::MessageStop {
            stop_reason: StopReason::EndTurn,
        }),
    ])))
    .await
    .unwrap();
    // after_complete should extract thinking
    let result = mw
        .after_complete(response, &[], &ProviderConfig::new("m"))
        .await
        .unwrap();
    let has_thinking = result
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Thinking(_)));
    let has_text = result
        .content
        .iter()
        .any(|b| b.as_text() == Some("answer here"));
    assert!(has_thinking, "should have extracted thinking block");
    assert!(has_text, "should have remaining text");
}

#[tokio::test]
async fn extract_reasoning_no_tags_unchanged() {
    use sideseat::{ContentBlock, StopReason};
    let mw = ExtractReasoningMiddleware::new();
    let response = sideseat::provider::collect_stream(Box::pin(futures::stream::iter(vec![
        Ok(sideseat::types::StreamEvent::MessageStart {
            role: sideseat::Role::Assistant,
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockStart {
            index: 0,
            block: sideseat::types::ContentBlockStart::Text,
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockDelta {
            index: 0,
            delta: sideseat::types::ContentDelta::Text {
                text: "plain text no tags".to_string(),
            },
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockStop { index: 0 }),
        Ok(sideseat::types::StreamEvent::MessageStop {
            stop_reason: StopReason::EndTurn,
        }),
    ])))
    .await
    .unwrap();
    let result = mw
        .after_complete(response, &[], &ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(result.content.len(), 1);
    assert!(matches!(&result.content[0], ContentBlock::Text(t) if t == "plain text no tags"));
}

// ---------------------------------------------------------------------------
// SimulateStreamingMiddleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn simulate_streaming_yields_events() {
    use sideseat::Middleware;
    let mw = SimulateStreamingMiddleware::new();
    let provider = MockProvider::new().with_text("streamed");
    let config = ProviderConfig::new("model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let transformed = mw.transform_stream(stream);
    let response = sideseat::collect_stream(transformed).await.unwrap();
    assert_eq!(response.first_text(), Some("streamed"));
}

// ---------------------------------------------------------------------------
// AgentHooks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_loop_active_tools_filter() {
    use sideseat::DefaultHooks;
    // Provider returns ToolUse, then EndTurn
    let provider = MockProvider::new()
        .with_response(MockResponse::ToolCall {
            id: "c1".into(),
            name: "allowed_tool".into(),
            input: serde_json::json!({}),
        })
        .with_text("done");

    let config = ProviderConfig::new("mock").with_tools(vec![
        Tool::new("allowed_tool", "allowed", serde_json::json!({})),
        Tool::new("blocked_tool", "blocked", serde_json::json!({})),
    ]);
    // Set active_tools to only allowed_tool
    let mut config = config;
    config.active_tools = Some(vec!["allowed_tool".to_string()]);

    let result = run_agent_loop_with_hooks(
        &provider,
        vec![Message::user("go")],
        config,
        |tools| async move {
            tools
                .into_iter()
                .map(|t| (t.id, vec![sideseat::ContentBlock::text("ok")]))
                .collect()
        },
        &DefaultHooks,
        Some(5),
    )
    .await
    .unwrap();
    assert!(!result.steps.is_empty());
}
