
#[tokio::test]
async fn agent_loop_max_steps_exceeded() {
    use sideseat::DefaultHooks;
    // Provider always returns ToolUse → loop never ends
    let provider = MockProvider::new()
        .with_response(MockResponse::ToolCall {
            id: "c1".into(),
            name: "tool".into(),
            input: serde_json::json!({}),
        })
        .with_response(MockResponse::ToolCall {
            id: "c2".into(),
            name: "tool".into(),
            input: serde_json::json!({}),
        })
        .with_response(MockResponse::ToolCall {
            id: "c3".into(),
            name: "tool".into(),
            input: serde_json::json!({}),
        });

    let config = ProviderConfig::new("mock").with_tools(vec![Tool::new(
        "tool",
        "a tool",
        serde_json::json!({}),
    )]);

    let err = run_agent_loop_with_hooks(
        &provider,
        vec![Message::user("go")],
        config,
        |tools| async move {
            tools
                .into_iter()
                .map(|t| (t.id, vec![sideseat::ContentBlock::text("result")]))
                .collect()
        },
        &DefaultHooks,
        Some(2), // max 2 steps
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidRequest(_)));
}

#[tokio::test]
async fn extract_reasoning_multiple_blocks() {
    use sideseat::ContentBlock;
    let mw = ExtractReasoningMiddleware::new();
    // Text with two <think> blocks
    let text = "<think>first</think>middle<think>second</think>end".to_string();
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
            delta: sideseat::types::ContentDelta::Text { text },
        }),
        Ok(sideseat::types::StreamEvent::ContentBlockStop { index: 0 }),
        Ok(sideseat::types::StreamEvent::MessageStop {
            stop_reason: sideseat::StopReason::EndTurn,
        }),
    ])))
    .await
    .unwrap();
    let result = mw
        .after_complete(response, &[], &ProviderConfig::new("mock"))
        .await
        .unwrap();
    let thinking_count = result
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Thinking(_)))
        .count();
    let text_count = result
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Text(_)))
        .count();
    assert_eq!(thinking_count, 2, "expected 2 thinking blocks");
    assert!(text_count >= 1, "expected at least 1 text block");
}

#[tokio::test]
async fn agent_loop_with_hooks_step_count() {
    use async_trait::async_trait;
    use sideseat::AgentStep;
    use std::sync::{Arc, Mutex};

    struct CountingHooks {
        steps: Arc<Mutex<Vec<usize>>>,
    }
    #[async_trait]
    impl AgentHooks for CountingHooks {
        async fn on_step_finish(&self, step: &AgentStep) {
            self.steps.lock().unwrap().push(step.step_number);
        }
    }

    let steps = Arc::new(Mutex::new(Vec::new()));
    let hooks = CountingHooks {
        steps: steps.clone(),
    };

    // Two tool use steps, then end turn
    let provider = MockProvider::new()
        .with_response(MockResponse::ToolCall {
            id: "c1".into(),
            name: "t".into(),
            input: serde_json::json!({}),
        })
        .with_response(MockResponse::ToolCall {
            id: "c2".into(),
            name: "t".into(),
            input: serde_json::json!({}),
        })
        .with_text("done");

    let config =
        ProviderConfig::new("mock").with_tools(vec![Tool::new("t", "tool", serde_json::json!({}))]);

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
        &hooks,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.steps.len(), 2);
    let recorded = steps.lock().unwrap().clone();
    assert_eq!(recorded, vec![0, 1]);
}

#[tokio::test]
async fn agent_loop_needs_approval_blocks_tool() {
    use async_trait::async_trait;
    use sideseat::ToolUseBlock;

    struct RejectAll;
    #[async_trait]
    impl AgentHooks for RejectAll {
        async fn needs_approval(&self, _tool: &ToolUseBlock) -> bool {
            true
        }
    }

    let provider = MockProvider::new()
        .with_response(MockResponse::ToolCall {
            id: "c1".into(),
            name: "dangerous".into(),
            input: serde_json::json!({}),
        })
        .with_text("done");

    let config = ProviderConfig::new("mock").with_tools(vec![Tool::new(
        "dangerous",
        "a tool",
        serde_json::json!({}),
    )]);

    let result = run_agent_loop_with_hooks(
        &provider,
        vec![Message::user("go")],
        config,
        |tools| {
            let results: Vec<(String, Vec<sideseat::ContentBlock>)> = tools
                .into_iter()
                .map(|t| (t.id, vec![sideseat::ContentBlock::text("should_not_run")]))
                .collect();
            async move { results }
        },
        &RejectAll,
        None,
    )
    .await
    .unwrap();

    // The step should have recorded the approval-blocked result
    assert_eq!(result.steps.len(), 1);
    let step = &result.steps[0];
    assert!(step.tool_results.iter().any(|(_, blocks)| {
        blocks
            .iter()
            .any(|b| b.as_text().is_some_and(|t| t.contains("approval")))
    }));
}

#[tokio::test]
async fn default_hooks_are_noops() {
    // DefaultHooks should not modify config, block tools, or fail in any way
    let provider = MockProvider::new().with_text("done");
    let config = ProviderConfig::new("mock");

    let result = run_agent_loop_with_hooks(
        &provider,
        vec![Message::user("hi")],
        config,
        |tools| async move {
            tools
                .into_iter()
                .map(|t| (t.id, vec![sideseat::ContentBlock::text("ok")]))
                .collect()
        },
        &DefaultHooks,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.steps.len(), 0);
    assert!(result.response.first_text().is_some());
}

// ---------------------------------------------------------------------------
// New API (round 4 review)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_ext_ask_accepts_str_literal() {
    use sideseat::ProviderExt;
    let provider = MockProvider::new().with_text("response");
    let response = provider
        .ask("hello", ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("response"));
}

#[tokio::test]
async fn provider_ext_ask_text_returns_string() {
    use sideseat::ProviderExt;
    let provider = MockProvider::new().with_text("hi there");
    let text = provider
        .ask_text("hello", ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(text, "hi there");
}

#[test]
fn message_system_constructor() {
    use sideseat::{Message, Role};
    let m = Message::system("You are helpful.");
    assert_eq!(m.role, Role::System);
    assert_eq!(m.content[0].as_text(), Some("You are helpful."));
}

#[test]
fn conversation_builder_build_messages() {
    use sideseat::{ConversationBuilder, Role};
    let msgs = ConversationBuilder::new()
        .system("sys")
        .user("hello")
        .assistant("hi")
        .build_messages();
    assert_eq!(msgs.len(), 2); // system not included
    assert_eq!(msgs[0].role, Role::User);
}

#[test]
fn provider_config_with_active_tools() {
    use sideseat::ProviderConfig;
    let config = ProviderConfig::new("m").with_active_tools(vec!["search".to_string()]);
    assert_eq!(config.active_tools, Some(vec!["search".to_string()]));
}

#[tokio::test]
async fn fallback_provider_push() {
    use sideseat::FallbackProvider;
    let provider = MockProvider::new().with_text("ok");
    let mut fb = FallbackProvider::new(vec![]);
    fb.push(provider);
    let result = fb
        .complete(vec![Message::user("hi")], ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(result.first_text(), Some("ok"));
}

#[test]
fn stream_event_serializes_to_json() {
    use sideseat::types::{StopReason, StreamEvent};
    let event = StreamEvent::MessageStop {
        stop_reason: StopReason::EndTurn,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("message_stop") || json.contains("EndTurn"));
    let _back: StreamEvent = serde_json::from_str(&json).unwrap();
}

#[test]
fn prompt_template_render_single_string() {
    use sideseat::PromptTemplate;
    use std::collections::HashMap;
    let tmpl = PromptTemplate::new("Hello {{name}}, you are {{age}} years old.");
    let mut vars = HashMap::new();
    vars.insert("name", "Alice");
    vars.insert("age", "30");
    let result = tmpl.render(&vars).unwrap();
    assert_eq!(result, "Hello Alice, you are 30 years old.");
}

#[test]
fn prompt_template_render_substitution_with_placeholder_chars() {
    use sideseat::PromptTemplate;
    use std::collections::HashMap;
    // Ensure substituted value containing {{ does not get processed again
    let tmpl = PromptTemplate::new("{{a}} {{b}}");
    let mut vars = HashMap::new();
    vars.insert("a", "{{not_a_var}}");
    vars.insert("b", "end");
    let result = tmpl.render(&vars).unwrap();
    assert_eq!(result, "{{not_a_var}} end");
}

#[test]
fn display_impl_for_enums() {
    use sideseat::types::{ImageQuality, ImageSize, ReasoningEffort, VideoAspectRatio};
    assert_eq!(format!("{}", ImageSize::S1024x1024), "1024x1024");
    assert_eq!(format!("{}", ImageQuality::Hd), "hd");
    assert_eq!(format!("{}", VideoAspectRatio::Landscape16x9), "16:9");
    assert_eq!(format!("{}", ReasoningEffort::High), "high");
}

#[test]
fn image_video_enums_partial_eq() {
    use sideseat::types::{ImageQuality, ImageSize, ImageStyle, VideoAspectRatio};
    assert_eq!(ImageSize::S1024x1024, ImageSize::S1024x1024);
    assert_ne!(ImageSize::S256x256, ImageSize::S512x512);
    assert_eq!(ImageQuality::Hd, ImageQuality::Hd);
    assert_eq!(ImageStyle::Vivid, ImageStyle::Vivid);
    assert_ne!(
        VideoAspectRatio::Landscape16x9,
        VideoAspectRatio::Portrait9x16
    );
}

#[tokio::test]
async fn fallback_trigger_no_any_error_variant() {
    // FallbackTrigger::AnyError was removed; use FallbackStrategy::AnyError instead
    // Timeout triggers fallback; Auth does not
    use sideseat::{FallbackStrategy, FallbackTrigger};
    let strategy = FallbackStrategy::OnTriggers(vec![FallbackTrigger::Timeout]);
    let primary = MockProvider::new().with_response(MockResponse::Error(ProviderError::Timeout {
        ms: Some(5000),
    }));
    let secondary = MockProvider::new().with_text("recovered");
    let mut p = FallbackProvider::with_strategy(vec![Box::new(primary)], strategy);
    p.push(secondary);
    let resp = p
        .complete(vec![Message::user("hi")], ProviderConfig::new("m"))
        .await
        .unwrap();
    assert_eq!(resp.first_text(), Some("recovered"));
}

// ---------------------------------------------------------------------------
// Telemetry tests
// ---------------------------------------------------------------------------

#[test]
fn telemetry_config_default() {
    let c = TelemetryConfig::default();
    assert!(!c.capture_content);
    assert!(c.record_metrics);
    assert_eq!(c.tracer_name, "sideseat");
}

#[test]
fn provider_name_mock() {
    let p = MockProvider::new();
    assert_eq!(p.provider_name(), "mock");
}

#[test]
fn provider_name_retry_delegates() {
    let inner = MockProvider::new();
    let retry = RetryProvider::new(inner, 1);
    assert_eq!(retry.provider_name(), "mock");
}

#[test]
fn provider_name_fallback_empty() {
    let fb = FallbackProvider::new(vec![]);
    assert_eq!(fb.provider_name(), "unknown");
}

#[test]
fn provider_name_fallback_first() {
    let fb = FallbackProvider::new(vec![Box::new(MockProvider::new())]);
    assert_eq!(fb.provider_name(), "mock");
}

#[test]
fn provider_name_middleware_stack() {
    let stack = MiddlewareStack::new(MockProvider::new());
    assert_eq!(stack.provider_name(), "mock");
}

#[test]
fn provider_name_instrumented_delegates() {
    let p = InstrumentedProvider::new(MockProvider::new());
    assert_eq!(p.provider_name(), "mock");
}

#[test]
fn provider_name_anthropic() {
    use sideseat::providers::AnthropicProvider;
    let p = AnthropicProvider::new("fake-key");
    assert_eq!(p.provider_name(), "anthropic");
}

#[test]
fn provider_name_openai() {
    use sideseat::providers::OpenAIChatProvider;
    let p = OpenAIChatProvider::new("fake-key");
    assert_eq!(p.provider_name(), "openai");
}

#[test]
fn provider_name_bedrock() {
    use sideseat::providers::BedrockProvider;
    let p = BedrockProvider::with_api_key("fake-key", "us-east-1");
    assert_eq!(p.provider_name(), "aws_bedrock");
}

#[tokio::test]
async fn instrumented_complete_returns_same_response() {
    // Uses global noop tracer/meter (default when no provider is installed)
    let inner = MockProvider::new().with_text("hello from instrumented");
    let provider = InstrumentedProvider::new(inner);
    let config = ProviderConfig::new("mock-model");
    let response = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap();
    assert_eq!(response.first_text(), Some("hello from instrumented"));
}

#[tokio::test]
async fn instrumented_stream_passes_through() {
    let inner = MockProvider::new().with_text("streamed");
    let provider = InstrumentedProvider::new(inner);
    let config = ProviderConfig::new("mock-model");
    let stream = provider.stream(vec![Message::user("hi")], config);
    let response = sideseat::collect_stream(stream).await.unwrap();
    assert_eq!(response.first_text(), Some("streamed"));
}

#[tokio::test]
async fn instrumented_complete_propagates_error() {
    let inner =
        MockProvider::new().with_response(MockResponse::Error(ProviderError::Auth("bad".into())));
    let provider = InstrumentedProvider::new(inner);
    let config = ProviderConfig::new("mock-model");
    let err = provider
        .complete(vec![Message::user("hi")], config)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Auth(_)));
}

#[test]
fn sideseat_fluent_builder() {
    let s = SideSeat::new()
        .with_endpoint("http://custom:1234")
        .with_project_id("myproject")
        .with_api_key("sk-test")
        .with_capture_content(true);
    assert_eq!(s.endpoint(), "http://custom:1234");
    assert_eq!(s.project_id(), "myproject");
    assert_eq!(s.api_key(), Some("sk-test"));
    assert!(s.captures_content());
}

#[test]
fn sideseat_telemetry_config_bridge() {
    let config = SideSeat::new()
        .with_capture_content(true)
        .telemetry_config();
    assert!(config.capture_content);
    assert!(config.record_metrics);
    assert_eq!(config.tracer_name, "sideseat");
}

// ---------------------------------------------------------------------------
// timeout_ms — structural round-trip (firing is tested in providers.rs integration tests)
// ---------------------------------------------------------------------------

#[test]
fn provider_config_timeout_ms_round_trips() {
    let mut config = ProviderConfig::new("model");
    config.timeout_ms = Some(5000);
    assert_eq!(config.timeout_ms, Some(5000));
}

#[test]
fn provider_config_timeout_ms_none_by_default() {
    let config = ProviderConfig::new("model");
    assert!(config.timeout_ms.is_none());
}

// ---------------------------------------------------------------------------
// cache_control on Message (structural check — real wire test is in providers.rs)
// ---------------------------------------------------------------------------

#[test]
fn message_cache_control_set() {
    use sideseat::{CacheControl, Message};
    let mut msg = Message::user("context to cache");
    msg.cache_control = Some(CacheControl::Ephemeral);
    assert_eq!(msg.cache_control, Some(CacheControl::Ephemeral));
}

#[test]
fn message_cache_control_system() {
    use sideseat::{CacheControl, Message, Role};
    let mut msg = Message::system("system prompt to cache");
    msg.cache_control = Some(CacheControl::Ephemeral);
    assert_eq!(msg.role, Role::System);
    assert_eq!(msg.cache_control, Some(CacheControl::Ephemeral));
}

// ---------------------------------------------------------------------------
// MediaSource::Text — Bedrock document text source
// ---------------------------------------------------------------------------

#[test]
fn media_source_text_round_trips() {
    use sideseat::MediaSource;
    let src = MediaSource::Text("hello world".to_string());
    if let MediaSource::Text(t) = src {
        assert_eq!(t, "hello world");
    } else {
        panic!("wrong variant");
    }
}

// ---------------------------------------------------------------------------
// Bedrock extra params — structural checks (real wire tests require live AWS)
// ---------------------------------------------------------------------------

#[test]
fn bedrock_guardrail_extra_keys() {
    let mut config = ProviderConfig::new("claude-3");
    config
        .extra
        .insert("guardrail_id".into(), serde_json::json!("gr-123"));
    config
        .extra
        .insert("guardrail_version".into(), serde_json::json!("1"));
    config
        .extra
        .insert("guardrail_trace".into(), serde_json::json!("enabled"));
    assert_eq!(config.extra["guardrail_id"], "gr-123");
    assert_eq!(config.extra["guardrail_trace"], "enabled");
}

#[test]
fn bedrock_performance_config_extra_key() {
    let mut config = ProviderConfig::new("claude-3");
    config.extra.insert(
        "performance_config_latency".into(),
        serde_json::json!("optimized"),
    );
    assert_eq!(config.extra["performance_config_latency"], "optimized");
}

#[test]
fn bedrock_request_metadata_extra_key() {
    let mut config = ProviderConfig::new("claude-3");
    config.extra.insert(
        "request_metadata".into(),
        serde_json::json!({"session_id": "abc123", "user_id": "u1"}),
    );
    assert!(config.extra["request_metadata"]["session_id"] == "abc123");
}

#[test]
fn bedrock_prompt_variables_extra_key() {
    let mut config = ProviderConfig::new("claude-3");
    config.extra.insert(
        "prompt_variables".into(),
        serde_json::json!({"topic": "Rust programming"}),
    );
    assert_eq!(
        config.extra["prompt_variables"]["topic"],
        "Rust programming"
    );
}

#[test]
fn bedrock_amr_paths_extra_key() {
    let mut config = ProviderConfig::new("claude-3");
    config.extra.insert(
        "additional_model_response_field_paths".into(),
        serde_json::json!(["/path/to/field1", "/path/to/field2"]),
    );
    let arr = config.extra["additional_model_response_field_paths"]
        .as_array()
        .unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0], "/path/to/field1");
}

#[test]
fn bedrock_system_tools_extra_key() {
    let mut config = ProviderConfig::new("claude-3");
    config.extra.insert(
        "system_tools".into(),
        serde_json::json!(["computer", "bash"]),
    );
    let arr = config.extra["system_tools"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0], "computer");
}
