use super::*;
use chrono::Utc;

fn event_source(name: &str) -> MessageSource {
    MessageSource::Event {
        name: name.to_string(),
        time: Utc::now(),
    }
}

fn attribute_source() -> MessageSource {
    MessageSource::Attribute {
        key: "test".to_string(),
        time: Utc::now(),
    }
}

#[test]
fn test_is_llm_output_event() {
    assert!(is_llm_output_event("gen_ai.choice"));
    assert!(is_llm_output_event("gen_ai.content.completion"));
    assert!(!is_llm_output_event("gen_ai.user.message"));
    assert!(!is_llm_output_event("gen_ai.assistant.message"));
}

#[test]
fn test_is_special_role() {
    assert!(is_special_role("tool_call"));
    assert!(is_special_role("TOOL_CALL")); // case insensitive
    assert!(is_special_role("tool"));
    assert!(is_special_role("tools"));
    assert!(is_special_role("data"));
    assert!(is_special_role("context"));
    assert!(!is_special_role("user"));
    assert!(!is_special_role("assistant"));
    assert!(!is_special_role("system"));
}

#[test]
fn test_categorize_attribute_message() {
    assert_eq!(
        categorize_attribute_message(Some("user")),
        MessageCategory::GenAIUserMessage
    );
    assert_eq!(
        categorize_attribute_message(Some("assistant")),
        MessageCategory::GenAIAssistantMessage
    );
    assert_eq!(
        categorize_attribute_message(None),
        MessageCategory::GenAIUserMessage
    );
}

#[test]
fn test_determine_category_llm_output_ignores_role() {
    // Even with tool role, gen_ai.choice should be GenAIChoice
    let content = json!({"role": "tool"});
    let source = event_source("gen_ai.choice");
    assert_eq!(
        determine_category(&source, &content),
        MessageCategory::GenAIChoice
    );
}

#[test]
fn test_determine_category_special_role_overrides_event() {
    let content = json!({"role": "tool_call"});
    let source = event_source("gen_ai.assistant.message");
    assert_eq!(
        determine_category(&source, &content),
        MessageCategory::GenAIToolInput
    );
}

#[test]
fn test_determine_category_standard_role_uses_event() {
    let content = json!({"role": "assistant"});
    let source = event_source("gen_ai.assistant.message");
    assert_eq!(
        determine_category(&source, &content),
        MessageCategory::GenAIAssistantMessage
    );
}

#[test]
fn test_determine_category_attribute_uses_role() {
    let content = json!({"role": "system"});
    let source = attribute_source();
    assert_eq!(
        determine_category(&source, &content),
        MessageCategory::GenAISystemMessage
    );
}

#[test]
fn test_expand_message_array_preserves_message_with_string_content() {
    // Individual messages have string "content" field - should NOT be expanded
    // The key insight: only expand if nested content/messages is an ARRAY
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "ai.prompt.messages".to_string(),
            time: Utc::now(),
        },
        content: json!({"role": "system", "content": "You are a helpful assistant."}),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 1, "Single message should be preserved");
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("system")
    );
    assert_eq!(
        result[0].0.content.get("content").and_then(|c| c.as_str()),
        Some("You are a helpful assistant.")
    );
}

#[test]
fn test_expand_message_array_preserves_message_with_array_content() {
    // Messages can have array content (content blocks) - should NOT expand as messages
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "ai.prompt.messages".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "role": "user",
            "content": [{"type": "text", "text": "Hello"}]  // Array of content blocks, not messages
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(
        result.len(),
        1,
        "Single message with content blocks should be preserved"
    );
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("user")
    );
}

#[test]
fn test_expand_message_array_expands_top_level_array() {
    // Top-level array of messages should be expanded
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "ai.prompt.messages".to_string(),
            time: Utc::now(),
        },
        content: json!([
            {"role": "system", "content": "You are helpful"},
            {"role": "user", "content": "Hello"}
        ]),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 2, "Should expand to 2 messages");
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("system")
    );
    assert_eq!(
        result[1].0.content.get("role").and_then(|r| r.as_str()),
        Some("user")
    );
}

#[test]
fn test_expand_message_array_expands_nested_messages_array() {
    // Nested "messages" array should be expanded
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "some.attribute".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"}
            ]
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 2, "Should expand nested messages array");
}

#[test]
fn test_expand_message_array_gemini_parts_format() {
    // Gemini uses "parts" instead of "content" - should be recognized as message
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.prompt".to_string(),
            time: Utc::now(),
        },
        content: json!([
            {"role": "user", "parts": [{"text": "Hello"}]},
            {"role": "model", "parts": [{"text": "Hi"}]}
        ]),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 2, "Gemini format should be expanded");
}

#[test]
fn test_expand_message_array_coalesces_google_genai_stream_chunks() {
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.output.messages".to_string(),
            time: Utc::now(),
        },
        content: json!([
            {
                "role": "assistant",
                "parts": [{"content": "Water boils at 10", "type": "text"}],
                "finish_reason": ""
            },
            {
                "role": "assistant",
                "parts": [{"content": "0°C at sea level.", "type": "text"}],
                "finish_reason": "stop"
            }
        ]),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].0.content,
        json!({
            "role": "assistant",
            "parts": [{"content": "Water boils at 100°C at sea level.", "type": "text"}],
            "finish_reason": "stop"
        })
    );
}

/// A client that runs the tool loop itself streams one response per round, and its span holds them
/// all: the first round's function call, then the answer in chunks. The answer used to stay split,
/// because the function call disqualified the whole array from coalescing.
#[test]
fn test_expand_message_array_coalesces_each_stream_of_a_tool_loop() {
    let call = json!({
        "role": "assistant",
        "parts": [{"type": "tool_call", "id": "c1", "name": "get_weather", "arguments": {"city": "Rome"}}],
        "finish_reason": "stop"
    });
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.output.messages".to_string(),
            time: Utc::now(),
        },
        content: json!([
            call,
            {
                "role": "assistant",
                "parts": [{"content": "Rome is sunny. Wea", "type": "text"}],
                "finish_reason": ""
            },
            {
                "role": "assistant",
                "parts": [{"content": "r light layers.", "type": "text"}],
                "finish_reason": "stop"
            }
        ]),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    let contents: Vec<_> = result.iter().map(|(raw, _)| raw.content.clone()).collect();
    assert_eq!(
        contents,
        vec![
            call,
            json!({
                "role": "assistant",
                "parts": [{"content": "Rome is sunny. Wear light layers.", "type": "text"}],
                "finish_reason": "stop"
            })
        ]
    );
    let positions: Vec<_> = result.iter().map(|(_, path)| path.clone()).collect();
    let root = PositionPath::root(0);
    assert_eq!(positions, vec![root.child_index(0), root.child_index(1)]);
}

#[test]
fn test_expand_message_array_keeps_multiple_finished_candidates_separate() {
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.output.messages".to_string(),
            time: Utc::now(),
        },
        content: json!([
            {
                "role": "assistant",
                "parts": [{"content": "Candidate one", "type": "text"}],
                "finish_reason": "stop"
            },
            {
                "role": "assistant",
                "parts": [{"content": "Candidate two", "type": "text"}],
                "finish_reason": "stop"
            }
        ]),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 2);
}

#[test]
fn test_to_sideml_vercel_ai_system_and_user_messages() {
    // Full pipeline test: Vercel AI system + user messages should both be preserved
    let raw_messages = vec![
        RawMessage {
            source: MessageSource::Attribute {
                key: "ai.prompt.messages".to_string(),
                time: Utc::now(),
            },
            content: json!({"role": "system", "content": "You are a helpful assistant."}),
        },
        RawMessage {
            source: MessageSource::Attribute {
                key: "ai.prompt.messages".to_string(),
                time: Utc::now(),
            },
            content: json!({"role": "user", "content": [{"type": "text", "text": "Hello"}]}),
        },
    ];

    let result = to_sideml(&raw_messages);

    assert_eq!(result.len(), 2, "Should have 2 messages");
    assert_eq!(result[0].sideml.role, ChatRole::System);
    assert_eq!(result[1].sideml.role, ChatRole::User);

    // Verify content is normalized
    assert!(!result[0].sideml.content.is_empty());
    assert!(!result[1].sideml.content.is_empty());
}

#[test]
fn test_flatten_tool_blocks_preserves_order() {
    // Test that non-tool blocks appear at their first occurrence position
    let msg = SideMLMessage {
        position: PositionPath::default(),
        source: event_source("gen_ai.assistant.message"),
        category: MessageCategory::GenAIAssistantMessage,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Before tools".to_string(),
                },
                ContentBlock::ToolUse {
                    id: Some("tool_1".to_string()),
                    name: "search".to_string(),
                    input: json!({"q": "test"}),
                },
                ContentBlock::ToolUse {
                    id: Some("tool_2".to_string()),
                    name: "fetch".to_string(),
                    input: json!({"url": "http://example.com"}),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);

    assert_eq!(result.len(), 3, "Should flatten into 3 messages");
    // Non-tool content should come FIRST (at its original position)
    assert!(matches!(
        result[0].sideml.content.first(),
        Some(ContentBlock::Text { text }) if text == "Before tools"
    ));
    // Tool blocks should follow in order
    assert!(matches!(
        result[1].sideml.content.first(),
        Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_1"
    ));
    assert!(matches!(
        result[2].sideml.content.first(),
        Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_2"
    ));
}

#[test]
fn test_flatten_tool_blocks_text_after_tools() {
    // Test that text after tools stays at the end
    let msg = SideMLMessage {
        position: PositionPath::default(),
        source: event_source("gen_ai.assistant.message"),
        category: MessageCategory::GenAIAssistantMessage,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: Some("tool_1".to_string()),
                    name: "search".to_string(),
                    input: json!({}),
                },
                ContentBlock::ToolUse {
                    id: Some("tool_2".to_string()),
                    name: "fetch".to_string(),
                    input: json!({}),
                },
                ContentBlock::Text {
                    text: "After tools".to_string(),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);

    assert_eq!(result.len(), 3, "Should flatten into 3 messages");
    // Tool blocks should come first (in order)
    assert!(matches!(
        result[0].sideml.content.first(),
        Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_1"
    ));
    assert!(matches!(
        result[1].sideml.content.first(),
        Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_2"
    ));
    // Text should come LAST (at its original position)
    assert!(matches!(
        result[2].sideml.content.first(),
        Some(ContentBlock::Text { text }) if text == "After tools"
    ));
}

/// Two non-tool groups of one message occupy two positions.
///
/// The contract [`PositionPath`] states is that two observations of one payload differ *by
/// construction*, so that identical content is still two occurrences. A group used to take the
/// **parent's** position, and a message shaped `[text, call, call, text]` produces two of them — so
/// both stood at the parent, and since flatten restarts its block index per message, both texts
/// landed on `parent.0`. Identity for plain text is `(trace, role, content)`, separated within one
/// atomic emission only by the position-derived ordinal; equal positions collapse that separation.
///
/// Kept at this level deliberately. No payload in the corpus reaches it — an OTLP content-block list
/// arrives as separate observations, which already carry distinct roots — so an end-to-end fixture
/// cannot state the property, and `_synthetic/text_split_by_parallel_calls` pins that shape's answer
/// rather than this mechanism. What is checked here is the invariant the type promises.
#[test]
fn two_non_tool_groups_of_one_message_occupy_two_positions() {
    let msg = SideMLMessage {
        position: PositionPath::root(0),
        source: event_source("gen_ai.choice"),
        category: MessageCategory::GenAIChoice,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "checking".to_string(),
                },
                ContentBlock::ToolUse {
                    id: Some("call_1".to_string()),
                    name: "lookup".to_string(),
                    input: json!({"q": "a"}),
                },
                ContentBlock::ToolUse {
                    id: Some("call_2".to_string()),
                    name: "lookup".to_string(),
                    input: json!({"q": "b"}),
                },
                ContentBlock::Text {
                    text: "checking".to_string(),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);
    assert_eq!(result.len(), 4, "two groups and two calls");

    let positions: Vec<String> = result.iter().map(|m| m.position.to_string()).collect();
    let distinct: std::collections::BTreeSet<&String> = positions.iter().collect();
    assert_eq!(
        distinct.len(),
        positions.len(),
        "every message of one split occupies its own position, or identical content in two of them \
             is one identity at one ordinal and dedup keeps only the first: {positions:?}"
    );
    assert_eq!(
        positions,
        vec!["0.0", "0.1", "0.2", "0.3"],
        "each takes the position of the block it was made from - a group, the first of its own"
    );
}

/// A call and a result in one message are two messages, not one carrying two tool ids.
///
/// The counts were compared *separately* (`use <= 1 && result <= 1`), so this shape - one of each -
/// passed through whole, and the function's own promise that each message carries at most one tool id
/// was false for it. Downstream, `tool_use_id` is a single field: whichever id the message kept, the
/// other block's correspondence was unrepresentable.
#[test]
fn a_call_and_a_result_in_one_message_are_split() {
    let msg = SideMLMessage {
        position: PositionPath::root(0),
        source: event_source("gen_ai.choice"),
        category: MessageCategory::GenAIChoice,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: Some("call_1".to_string()),
                    name: "search".to_string(),
                    input: json!({"q": "a"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("call_0".to_string()),
                    name: Some("search".to_string()),
                    content: json!("earlier"),
                    is_error: false,
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);
    assert_eq!(result.len(), 2, "one call and one result are two messages");
    for message in &result {
        let tools = message
            .sideml
            .content
            .iter()
            .filter(|b| {
                matches!(
                    b,
                    ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                )
            })
            .count();
        assert_eq!(
            tools, 1,
            "at most one tool block per message is the contract"
        );
    }
}

/// Text between two tool blocks stays between them.
///
/// A one-shot flag emitted the accumulated non-tool blocks only *before the first* tool block, so
/// anything written between the first and second was held to the end: `[text A, call 1, text B, call 2]`
/// came out as `[text A, call 1, call 2, text B]`, moving a model's own commentary past the call it
/// introduced. The group is emitted at every tool block now, and each takes its own first block's
/// position, so two groups of one message are still two.
#[test]
fn text_between_two_tool_blocks_keeps_its_place() {
    let msg = SideMLMessage {
        position: PositionPath::root(0),
        source: event_source("gen_ai.choice"),
        category: MessageCategory::GenAIChoice,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "first, the weather".to_string(),
                },
                ContentBlock::ToolUse {
                    id: Some("call_1".to_string()),
                    name: "weather".to_string(),
                    input: json!({}),
                },
                ContentBlock::Text {
                    text: "then the news".to_string(),
                },
                ContentBlock::ToolUse {
                    id: Some("call_2".to_string()),
                    name: "news".to_string(),
                    input: json!({}),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);
    let order: Vec<String> = result
        .iter()
        .map(|m| match m.sideml.content.first() {
            Some(ContentBlock::Text { text }) => text.clone(),
            Some(ContentBlock::ToolUse { name, .. }) => name.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        order,
        vec![
            "first, the weather".to_string(),
            "weather".to_string(),
            "then the news".to_string(),
            "news".to_string(),
        ],
        "the source order of one message survives the split"
    );
}

#[test]
fn test_flatten_tool_blocks_single_tool_unchanged() {
    // Messages with 0 or 1 tool blocks should pass through unchanged
    let msg = SideMLMessage {
        position: PositionPath::default(),
        source: event_source("gen_ai.assistant.message"),
        category: MessageCategory::GenAIAssistantMessage,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Here's the result".to_string(),
                },
                ContentBlock::ToolUse {
                    id: Some("tool_1".to_string()),
                    name: "search".to_string(),
                    input: json!({}),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);

    assert_eq!(result.len(), 1, "Single tool should not be flattened");
    assert_eq!(
        result[0].sideml.content.len(),
        2,
        "All content blocks preserved"
    );
}

#[test]
fn test_flatten_tool_blocks_multiple_tool_results() {
    // Test flattening multiple tool results (parallel tool execution)
    let msg = SideMLMessage {
        position: PositionPath::default(),
        source: event_source("gen_ai.tool.message"),
        category: MessageCategory::GenAIToolMessage,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Tool,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: Some("call_1".to_string()),
                    name: None,
                    content: json!({"result": "weather data"}),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("call_2".to_string()),
                    name: None,
                    content: json!({"result": "time data"}),
                    is_error: false,
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);

    assert_eq!(result.len(), 2, "Multiple tool results should be flattened");

    // Each message should have exactly one tool result
    for (i, msg) in result.iter().enumerate() {
        assert_eq!(
            msg.sideml.content.len(),
            1,
            "Message {} should have exactly one content block",
            i
        );
        assert!(
            matches!(msg.sideml.content[0], ContentBlock::ToolResult { .. }),
            "Message {} should contain a ToolResult",
            i
        );
    }

    // Verify tool_use_ids are preserved and propagated to message level
    let tool_ids: Vec<_> = result
        .iter()
        .filter_map(|msg| msg.sideml.tool_use_id.clone())
        .collect();
    assert!(tool_ids.contains(&"call_1".to_string()));
    assert!(tool_ids.contains(&"call_2".to_string()));
}

#[test]
fn test_flatten_tool_blocks_mixed_tool_use_and_result() {
    // Edge case: Message with both ToolUse and ToolResult (unusual but possible)
    let msg = SideMLMessage {
        position: PositionPath::default(),
        source: event_source("gen_ai.assistant.message"),
        category: MessageCategory::GenAIAssistantMessage,
        source_type: MessageSourceType::Event,
        timestamp: Utc::now(),
        sideml: ChatMessage {
            role: ChatRole::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: Some("call_1".to_string()),
                    name: "search".to_string(),
                    input: json!({}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("call_0".to_string()),
                    name: None,
                    content: json!("previous result"),
                    is_error: false,
                },
                ContentBlock::ToolUse {
                    id: Some("call_2".to_string()),
                    name: "fetch".to_string(),
                    input: json!({}),
                },
            ],
            ..Default::default()
        },
    };

    let result = flatten_tool_blocks(vec![msg]);

    // Should flatten into 3 separate messages (2 tool uses + 1 tool result)
    assert_eq!(
        result.len(),
        3,
        "Mixed tool uses and results should be flattened"
    );

    // Count block types
    let tool_use_count = result
        .iter()
        .filter(|m| matches!(m.sideml.content.first(), Some(ContentBlock::ToolUse { .. })))
        .count();
    let tool_result_count = result
        .iter()
        .filter(|m| {
            matches!(
                m.sideml.content.first(),
                Some(ContentBlock::ToolResult { .. })
            )
        })
        .count();

    assert_eq!(tool_use_count, 2, "Should have 2 tool use messages");
    assert_eq!(tool_result_count, 1, "Should have 1 tool result message");
}

#[test]
fn test_flattened_tool_results_get_name_enriched() {
    // Regression test: name enrichment must happen AFTER flattening,
    // otherwise flattened tool results won't get their tool names.
    use crate::observations::RawMessage;

    // Create a tool use message and a bundled tool result message
    let tool_use_msg = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.assistant.message".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "call_weather",
                "name": "get_weather",
                "input": {"city": "NYC"}
            }, {
                "type": "tool_use",
                "id": "call_time",
                "name": "get_time",
                "input": {"timezone": "EST"}
            }]
        }),
    };

    // Bundled tool results - will be flattened
    let tool_results_msg = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "role": "tool",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "call_weather",
                "content": "Sunny, 72F"
            }, {
                "type": "tool_result",
                "tool_use_id": "call_time",
                "content": "3:00 PM EST"
            }]
        }),
    };

    let result = to_sideml(&[tool_use_msg, tool_results_msg]);

    // Should have 4 messages: 2 tool uses + 2 tool results (flattened)
    assert_eq!(result.len(), 4, "Should flatten into 4 messages");

    // Find the tool result messages and verify they have names
    let tool_results: Vec<_> = result
        .iter()
        .filter(|m| m.sideml.role == ChatRole::Tool)
        .collect();

    assert_eq!(tool_results.len(), 2, "Should have 2 tool result messages");

    // Each tool result should have a name enriched from the tool use
    for tr in &tool_results {
        assert!(
            tr.sideml.name.is_some(),
            "Tool result should have name enriched, got: {:?}",
            tr.sideml
        );
    }

    // Verify correct names
    let names: Vec<_> = tool_results
        .iter()
        .filter_map(|m| m.sideml.name.clone())
        .collect();
    assert!(names.contains(&"get_weather".to_string()));
    assert!(names.contains(&"get_time".to_string()));
}

#[test]
fn test_request_data_expansion() {
    // request_data wraps messages in {messages: [...], model: "..."}
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "request_data".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ],
            "model": "gpt-4o"
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(
        result.len(),
        2,
        "Should expand messages array from request_data"
    );
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("system")
    );
    assert_eq!(
        result[1].0.content.get("role").and_then(|r| r.as_str()),
        Some("user")
    );
}

#[test]
fn test_response_data_message_unwrap() {
    // response_data non-streaming: {message: {role, content, ...}, usage: {...}}
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "response_data".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "message": {"role": "assistant", "content": "Hi there!"},
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 1, "Should unwrap singular message");
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("assistant")
    );
    assert_eq!(
        result[0].0.content.get("content").and_then(|c| c.as_str()),
        Some("Hi there!")
    );
}
