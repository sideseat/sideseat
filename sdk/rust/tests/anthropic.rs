//! Integration tests for Anthropic provider (direct, Bedrock, Vertex).

#[macro_use]
mod common;
use common::*;

// ---------------------------------------------------------------------------
// Anthropic direct
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_complete() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let config = default_config("claude-haiku-4-5-20251001");

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(matches!(resp.content[0], ContentBlock::Text(_)));
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_stream() {
    let (server, provider) = mock_anthropic();
    mock_sse(&server, POST, "/messages", ANTHROPIC_STREAM_EVENTS);
    let config = default_config("claude-haiku-4-5-20251001");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_tools() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_TOOL_JSON);
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'pineapple'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
    let tool = resp.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t) } else { None }
    }).unwrap();
    assert_eq!(tool.name, "echo");
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn test_anthropic_system_prompt() {
    let (server, provider) = mock_anthropic();
    let pirate_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"Arrr! Hello there, matey!"}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(pirate_json);
    });
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider.complete(vec![user_msg("Hello")], config).await.unwrap();
    let text = resp.text();
    assert!(text.to_lowercase().contains("arr"), "expected pirate response, got: {text}");
}

#[tokio::test]
async fn test_anthropic_multi_turn() {
    let (server, provider) = mock_anthropic();
    let alex_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"Your name is Alex."}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(alex_json);
    });
    let config = default_config("claude-haiku-4-5-20251001");

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
async fn test_anthropic_streaming_tools() {
    let (server, provider) = mock_anthropic();
    mock_sse(&server, POST, "/messages", ANTHROPIC_STREAM_TOOL_EVENTS);
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Echo 'streaming'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in streaming response, got: {:?}", resp.content);
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn test_anthropic_tool_use_loop() {
    let (server, provider) = mock_anthropic();
    // Turn 2 mock: body has tool_result → return plain text (registered FIRST = checked first)
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages").body_includes("tool_result");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    // Turn 1 fallback: no restriction → return tool_use (registered second = checked if first doesn't match)
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_TOOL_JSON);
    });
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.tools = vec![echo_tool()];

    // Turn 1: model calls the tool
    let resp = provider.complete(vec![user_msg("Echo 'banana'")], config.clone()).await.unwrap();

    let tool_use = resp.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None }
    });
    assert!(tool_use.is_some(), "expected tool_use in turn 1");
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
    let resp2 = provider.complete(messages, config).await.unwrap();

    assert!(!resp2.text().is_empty());
    assert_eq!(resp2.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_anthropic_json_schema_output() {
    let (server, provider) = mock_anthropic();
    // Anthropic uses tool_use trick for JSON schema
    mock_json(&server, POST, "/messages", ANTHROPIC_TOOL_JSON);
    let mut config = default_config("claude-haiku-4-5-20251001");
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

    let resp = provider.complete(vec![user_msg("Give me info about France.")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool || !resp.text().is_empty(), "expected structured output");
}

#[tokio::test]
async fn test_anthropic_thinking() {
    let (server, provider) = mock_anthropic();
    let thinking_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Let me count the r's..."},{"type":"text","text":"There are 3 r's."}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(thinking_json);
    });
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.max_tokens = Some(2048);
    config.thinking_budget = Some(1024);

    let resp = provider.complete(vec![user_msg("How many r's are in 'strawberry'?")], config).await.unwrap();

    let has_thinking = resp.content.iter().any(|b| matches!(b, ContentBlock::Thinking(_)));
    let has_text = resp.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(has_thinking || has_text);
    if has_thinking {
        let thinking = resp.content.iter().find_map(|b| {
            if let ContentBlock::Thinking(t) = b { Some(t) } else { None }
        }).unwrap();
        assert!(!thinking.text.is_empty());
    }
}

#[tokio::test]
async fn test_anthropic_vision() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let config = default_config("claude-haiku-4-5-20251001");

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
    let resp = provider.complete(vec![msg], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_document_input() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let config = default_config("claude-haiku-4-5-20251001");

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

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_anthropic_cache_control() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.system = Some("You are a helpful assistant.".to_string());

    let msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text("Hello".to_string())],
        name: None,
        cache_control: Some(CacheControl::Ephemeral),
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_sampling_params() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.temperature = Some(0.0);
    config.top_k = Some(40);

    let resp = provider.complete(vec![user_msg("Say exactly 'deterministic'")], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_stop_sequences() {
    let (server, provider) = mock_anthropic();
    let stop_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"one, two, three. "}],"stop_reason":"stop_sequence","stop_sequence":"STOP","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(stop_json);
    });
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.stop_sequences = vec!["STOP".to_string()];

    let resp = provider.complete(vec![user_msg("Count: one, two, three. Then say STOP.")], config).await.unwrap();

    let text = resp.text();
    assert!(!text.is_empty());
    assert!(
        matches!(resp.stop_reason, StopReason::StopSequence(_) | StopReason::EndTurn),
        "unexpected stop_reason: {:?}", resp.stop_reason
    );
}

#[tokio::test]
async fn test_anthropic_disable_parallel_tools() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_TOOL_JSON);
    let mut config = default_config("claude-haiku-4-5-20251001");
    config.tools = vec![echo_tool()];
    config.parallel_tool_calls = Some(false);

    let resp = provider.complete(vec![user_msg("Echo 'mango'")], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(resp.warnings.iter().all(|w| !w.contains("parallel_tool_calls")));
}

#[tokio::test]
async fn test_anthropic_list_models() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, GET, "/models", ANTHROPIC_LIST_MODELS_JSON);

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty());
    assert!(models.iter().any(|m| m.id.contains("claude")));
}

#[tokio::test]
async fn test_anthropic_count_tokens() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages/count_tokens", ANTHROPIC_COUNT_TOKENS_JSON);
    let config = default_config("claude-haiku-4-5-20251001");

    let count = provider.count_tokens(vec![user_msg("Hello, how are you?")], config).await.unwrap();
    assert!(count.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// Anthropic via AWS Bedrock (invoke_model / invoke_model_with_response_stream)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_bedrock_complete() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_stream() {
    let (server, provider) = mock_anthropic_bedrock();
    let body = bedrock_anthropic_stream_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke-with-response-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_tools() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_TOOL_JSON);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'lychee'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_bedrock_system_prompt() {
    let (server, provider) = mock_anthropic_bedrock();
    let pirate_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"Arrr!"}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(pirate_json);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider.complete(vec![user_msg("Hello")], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_anthropic_bedrock_cache_control() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

    let msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text("Say 'hello' in one word".to_string())],
        name: None,
        cache_control: Some(CacheControl::Ephemeral),
    };
    let resp = provider.complete(vec![msg], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(!resp.warnings.iter().any(|w| w.contains("cache_control")));
}

#[tokio::test]
async fn test_anthropic_bedrock_streaming_tools() {
    let (server, provider) = mock_anthropic_bedrock();
    let body = bedrock_anthropic_stream_tool_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke-with-response-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Please echo the word 'papaya'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use in stream, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_bedrock_vision() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

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
    let resp = provider.complete(vec![msg], config).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_thinking() {
    let (server, provider) = mock_anthropic_bedrock();
    let thinking_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Let me count..."},{"type":"text","text":"3"}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(thinking_json);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.max_tokens = Some(2048);
    config.thinking_budget = Some(1024);

    let resp = provider.complete(vec![user_msg("How many r's are in 'strawberry'?")], config).await.unwrap();

    let has_thinking = resp.content.iter().any(|b| matches!(b, ContentBlock::Thinking(_)));
    let has_text = resp.content.iter().any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(has_thinking || has_text);
}

#[tokio::test]
async fn test_anthropic_bedrock_document_input() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

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

    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_anthropic_bedrock_multi_turn() {
    let (server, provider) = mock_anthropic_bedrock();
    let alex_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"Your name is Alex."}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(alex_json);
    });
    let config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");

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
    let (server, provider) = mock_anthropic_bedrock();
    // Turn 2 mock: body has tool_result → return plain text (registered FIRST = checked first)
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$").body_includes("tool_result");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    // Turn 1 fallback: no restriction → return tool_use (registered second = checked if first doesn't match)
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_TOOL_JSON);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.tools = vec![echo_tool()];

    // Turn 1
    let resp = provider.complete(vec![user_msg("Echo 'banana'")], config.clone()).await.unwrap();

    let tool_use = resp.content.iter().find_map(|b| {
        if let ContentBlock::ToolUse(t) = b { Some(t.clone()) } else { None }
    });
    assert!(tool_use.is_some(), "expected tool_use in turn 1");
    let tool_use = tool_use.unwrap();

    // Turn 2
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
    assert!(!resp2.text().is_empty());
    assert_eq!(resp2.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_anthropic_bedrock_json_schema_output() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_TOOL_JSON);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
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

    let resp = provider.complete(vec![user_msg("Give me info about France.")], config).await.unwrap();
    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool || !resp.text().is_empty());
}

#[tokio::test]
async fn test_anthropic_bedrock_sampling_params() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_COMPLETE_JSON);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.temperature = Some(0.0);
    config.top_p = Some(0.9);
    config.top_k = Some(40);

    let resp = provider.complete(vec![user_msg("Say exactly 'deterministic'")], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_bedrock_stop_sequences() {
    let (server, provider) = mock_anthropic_bedrock();
    let stop_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"one, two"}],"stop_reason":"stop_sequence","stop_sequence":"STOP","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(stop_json);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.stop_sequences = vec!["STOP".to_string()];

    let resp = provider.complete(vec![user_msg("Count: one, two, three. Then say STOP.")], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(
        matches!(resp.stop_reason, StopReason::StopSequence(_) | StopReason::EndTurn),
        "unexpected stop_reason: {:?}", resp.stop_reason
    );
}

#[tokio::test]
async fn test_anthropic_bedrock_disable_parallel_tools() {
    let (server, provider) = mock_anthropic_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*\/invoke$");
        then.status(200).header("content-type", "application/json").body(ANTHROPIC_TOOL_JSON);
    });
    let mut config = default_config("us.anthropic.claude-haiku-4-5-20251001-v1:0");
    config.tools = vec![echo_tool()];
    config.parallel_tool_calls = Some(false);

    let resp = provider.complete(vec![user_msg("Echo 'mango'")], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(!resp.warnings.iter().any(|w| w.contains("parallel_tool_calls")));
}

// ---------------------------------------------------------------------------
// Anthropic via Google Vertex AI — uses same mock as direct Anthropic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_vertex_complete() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_COMPLETE_JSON);
    let config = default_config("claude-haiku-4-5@20251001");

    let resp = provider.complete(vec![user_msg("Say 'hello' in one word")], config).await.unwrap();

    assert!(!resp.content.is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_vertex_stream() {
    let (server, provider) = mock_anthropic();
    mock_sse(&server, POST, "/messages", ANTHROPIC_STREAM_EVENTS);
    let config = default_config("claude-haiku-4-5@20251001");

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_anthropic_vertex_tools() {
    let (server, provider) = mock_anthropic();
    mock_json(&server, POST, "/messages", ANTHROPIC_TOOL_JSON);
    let mut config = default_config("claude-haiku-4-5@20251001");
    config.tools = vec![echo_tool()];

    let resp = provider.complete(vec![user_msg("Please echo the word 'durian'")], config).await.unwrap();

    let has_tool = resp.content.iter().any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use block, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_anthropic_vertex_system_prompt() {
    let (server, provider) = mock_anthropic();
    let pirate_json = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"Arrr!"}],"stop_reason":"end_turn","model":"test","usage":{"input_tokens":10,"output_tokens":5}}"#;
    server.mock(|when, then| {
        when.method(POST).path_includes("/messages");
        then.status(200).header("content-type", "application/json").body(pirate_json);
    });
    let mut config = default_config("claude-haiku-4-5@20251001");
    config.system = Some("You are a pirate. Always respond with 'Arrr!'".to_string());

    let resp = provider.complete(vec![user_msg("Hello")], config).await.unwrap();
    assert!(resp.text().to_lowercase().contains("arr"));
}
