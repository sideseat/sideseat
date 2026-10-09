#[test]
fn test_crewai_tool_call_format() {
    let input = json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": "call_crewai_123",
            "type": "function",
            "function": {
                "name": "search_web",
                "arguments": "{\"query\":\"weather NYC\"}"
            }
        }]
    });
    let output = normalize(&input);
    let tool_use = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .unwrap();
    if let ContentBlock::ToolUse { name, id, .. } = tool_use {
        assert_eq!(name, "search_web");
        assert_eq!(id.as_deref(), Some("call_crewai_123"));
    }
}

#[test]
fn test_crewai_tool_result_format() {
    // CrewAI tool results should be normalized to tool_result type
    let input = json!({
        "role": "tool",
        "tool_call_id": "call_crewai_123",
        "name": "search_web",
        "content": "Weather in NYC: sunny, 75F"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_crewai_123");
    assert_eq!(output.name.as_deref(), Some("search_web"));
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["content"],
        "Weather in NYC: sunny, 75F"
    );
}

// ============================================================================
// LANGGRAPH / OPENINFERENCE INTEGRATION TESTS
// ============================================================================

#[test]
fn test_langgraph_indexed_message_format() {
    let input = json!({
        "role": "user",
        "content": "What's the weather in NYC?"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "user");
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "What's the weather in NYC?"
    );
}

#[test]
fn test_langgraph_tool_calls_format() {
    let input = json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "id": "call_langgraph_456",
            "name": "get_weather",
            "arguments": {"city": "NYC"}
        }]
    });
    let output = normalize(&input);
    // Tool calls should be converted to content[].tool_use
    let tool_use = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolUse { .. }));
    assert!(tool_use.is_some(), "Should have ToolUse content block");
    if let ContentBlock::ToolUse {
        id, name, input, ..
    } = tool_use.unwrap()
    {
        assert_eq!(name, "get_weather");
        assert_eq!(id.as_deref(), Some("call_langgraph_456"));
        assert_eq!(input, &json!({"city": "NYC"}));
    }
}

#[test]
fn test_openinference_function_message() {
    let input = json!({
        "role": "function",
        "call_id": "call_oi_789",
        "name": "weather_tool",
        "content": "Sunny, 72F"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_oi_789");
    assert_eq!(output.name.as_deref(), Some("weather_tool"));
}

// ============================================================================
// LOGFIRE INTEGRATION TESTS
// ============================================================================

#[test]
fn test_logfire_tool_message_with_id() {
    let input = json!({
        "role": "tool",
        "id": "call_logfire_123",
        "content": "Tool result data"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_logfire_123");
}

// ============================================================================
// INTEGRATION TESTS: to_sideml FUNCTION
// ============================================================================

#[test]
fn test_to_sideml_strands_user_message() {
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Hello, assistant!"}],
            "role": "user"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(
        sideml_messages[0].category,
        MessageCategory::GenAIUserMessage
    );
    assert_eq!(sideml_messages[0].source_type, MessageSourceType::Event);
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::User);
    assert_eq!(
        block_to_json(&sideml_messages[0].sideml.content[0])["type"],
        "text"
    );
    assert_eq!(
        block_to_json(&sideml_messages[0].sideml.content[0])["text"],
        "Hello, assistant!"
    );
}

#[test]
fn test_to_sideml_strands_tool_message_categorization() {
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "content": [{"toolResult": {"toolUseId": "abc", "status": "success", "content": [{"text": "Result"}]}}],
            "role": "tool"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(
        sideml_messages[0].category,
        MessageCategory::GenAIToolMessage
    );
    assert_eq!(
        block_to_json(&sideml_messages[0].sideml.content[0])["type"],
        "tool_result"
    );
}

#[test]
fn test_to_sideml_tool_input_categorization() {
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "content": [{"toolUse": {"toolUseId": "abc", "name": "weather", "input": {}}}]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(sideml_messages[0].category, MessageCategory::GenAIToolInput);
}

#[test]
fn test_to_sideml_attribute_source_uses_span_timestamp() {
    let attr_time = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.prompt.0.content".to_string(),
            time: attr_time,
        },
        content: json!({
            "role": "user",
            "content": "Hello"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(sideml_messages[0].source_type, MessageSourceType::Attribute);
    assert_eq!(sideml_messages[0].timestamp, attr_time);
}

#[test]
fn test_to_sideml_choice_event_categorization() {
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "role": "assistant",
            "content": "I'll help you with that.",
            "finish_reason": "stop"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(sideml_messages[0].category, MessageCategory::GenAIChoice);
}

// ============================================================================
// MIXED CONTENT TESTS
// ============================================================================

#[test]
fn test_strands_mixed_text_and_tool_use() {
    let input = json!({
        "role": "assistant",
        "content": [
            {"text": "I'll check the weather for you."},
            {"toolUse": {"toolUseId": "tool123", "name": "get_weather", "input": {"city": "NYC"}}}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(block_to_json(&output.content[1])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[1])["id"], "tool123");
}

#[test]
fn test_anthropic_mixed_thinking_and_text() {
    let input = json!({
        "role": "assistant",
        "content": [
            {"type": "thinking", "text": "Let me analyze this..."},
            {"type": "text", "text": "The answer is 42."}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(block_to_json(&output.content[1])["type"], "text");
}

// === Role from Event Name Tests ===

#[test]
fn test_role_from_event_name_system() {
    // System role is the same regardless of span context
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.system.message", false),
        Some(ChatRole::System)
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.system.message", true),
        Some(ChatRole::System)
    );
}

#[test]
fn test_role_from_event_name_user() {
    // User role is the same regardless of span context
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.user.message", false),
        Some(ChatRole::User)
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.content.prompt", false),
        Some(ChatRole::User)
    );
}

#[test]
fn test_role_from_event_name_assistant_in_chat_span() {
    // In chat spans: gen_ai.choice -> assistant
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.assistant.message", false),
        Some(ChatRole::Assistant)
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.choice", false),
        Some(ChatRole::Assistant)
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.content.completion", false),
        Some(ChatRole::Assistant)
    );
}

#[test]
fn test_role_from_event_name_tool_output_in_tool_span() {
    // In tool spans: gen_ai.choice -> tool (tool output)
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.choice", true),
        Some(ChatRole::Tool)
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.content.completion", true),
        Some(ChatRole::Tool)
    );
}

#[test]
fn test_role_from_event_name_tool_in_chat_span() {
    // In chat spans: gen_ai.tool.message -> tool (tool result)
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.tool.message", false),
        Some(ChatRole::Tool)
    );
}

#[test]
fn test_role_from_event_name_tool_input_in_tool_span() {
    // In tool spans: gen_ai.tool.message is tool INPUT (invocation args)
    // Returns Assistant to prevent merging with tool OUTPUT in ToolResultRegistry
    // (tool_call role also maps to Assistant, so this is semantically consistent)
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.tool.message", true),
        Some(ChatRole::Assistant)
    );
}

#[test]
fn test_role_from_event_name_unknown() {
    assert_eq!(
        normalize::role_from_event_name_with_context("unknown.event", false),
        None
    );
    assert_eq!(
        normalize::role_from_event_name_with_context("gen_ai.other", false),
        None
    );
}

// === Bundled Tool Result Expansion Tests ===

#[test]
fn test_bundled_tool_results_are_split_into_separate_messages() {
    use crate::observations::{MessageSource, RawMessage};

    // Strands/Bedrock format: multiple toolResult objects in one message
    let bundled_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool",
            "content": [
                {"toolResult": {"toolUseId": "id1", "status": "success", "content": [{"text": "Result 1"}]}},
                {"toolResult": {"toolUseId": "id2", "status": "success", "content": [{"json": {"temp": 25}}]}}
            ]
        }),
        rendering: false,
    };

    let result = to_sideml(&[bundled_message]);

    // Should be split into 2 separate messages
    assert_eq!(
        result.len(),
        2,
        "Bundled tool results should be split into separate messages"
    );

    // First message should have id1
    assert_eq!(result[0].sideml.role, ChatRole::Tool);
    assert_eq!(result[0].sideml.tool_use_id, Some("id1".to_string()));

    // Second message should have id2
    assert_eq!(result[1].sideml.role, ChatRole::Tool);
    assert_eq!(result[1].sideml.tool_use_id, Some("id2".to_string()));
}

#[test]
fn test_single_tool_result_not_split() {
    use crate::observations::{MessageSource, RawMessage};

    // Single toolResult should not be modified
    let single_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool",
            "content": [
                {"toolResult": {"toolUseId": "id1", "status": "success", "content": [{"text": "Result"}]}}
            ]
        }),
        rendering: false,
    };

    let result = to_sideml(&[single_message]);

    assert_eq!(
        result.len(),
        1,
        "Single tool result should remain as one message"
    );
    assert_eq!(result[0].sideml.tool_use_id, Some("id1".to_string()));
}

#[test]
fn test_non_tool_messages_not_affected_by_bundling_logic() {
    use crate::observations::{MessageSource, RawMessage};

    // User message should pass through unchanged
    let user_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "user",
            "content": [{"text": "Hello"}, {"text": "World"}]
        }),
        rendering: false,
    };

    let result = to_sideml(&[user_message]);

    assert_eq!(result.len(), 1, "Non-tool messages should not be split");
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

// === Special Role Preservation Tests ===

#[test]
fn test_special_role_tool_call_preserved() {
    use crate::observations::{MessageSource, RawMessage};

    // tool_call is a special role that should NOT be overridden by event-derived role
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(), // Would derive to Tool in non-tool span
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool_call",  // Special role - should be preserved
            "name": "get_weather",
            "tool_call_id": "call_123",
            "content": {"city": "NYC"}
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // tool_call normalizes to Assistant (tool invocation by assistant)
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

#[test]
fn test_special_role_tools_preserved() {
    use crate::observations::{MessageSource, RawMessage};

    // tools role for tool definitions should be preserved
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(), // Would derive to Assistant
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tools",  // Special role - should be preserved
            "content": [{"name": "get_weather", "description": "Get weather"}]
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // tools role normalizes to System with tool definitions
    assert_eq!(result[0].sideml.role, ChatRole::System);
}

#[test]
fn test_special_role_data_preserved() {
    use crate::observations::{MessageSource, RawMessage};

    // data role for conversation history should be preserved
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(), // Would derive to Assistant
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "data",  // Special role - should be preserved
            "content": {"history": [{"user": "hi"}, {"assistant": "hello"}]}
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // data role normalizes to User with Context block
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_special_role_context_preserved() {
    use crate::observations::{MessageSource, RawMessage};

    // context role should be preserved
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(), // Would derive to Assistant
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "context",  // Special role - should be preserved
            "content": {"chat_history": "previous messages"}
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // context role normalizes to User with Context block
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_standard_role_overridden_by_event() {
    use crate::observations::{MessageSource, RawMessage};

    // Standard roles (user, assistant, tool, system) should be overridden by event-derived role
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(), // Derives to Assistant
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool",  // Standard role - should be overridden
            "content": "This should be assistant"
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // Event-derived role (Assistant) takes precedence over explicit "tool"
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

// === Tool Name Enrichment Tests ===

#[test]
fn test_tool_result_gets_name_from_matching_tool_use() {
    use crate::observations::{MessageSource, RawMessage};
    use crate::sideml::to_sideml_with_context;

    // Tool call with name
    let tool_call = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "assistant",
            "content": [{"toolUse": {"toolUseId": "call_abc", "name": "get_weather", "input": {}}}]
        }),
        rendering: false,
    };

    // Tool result without name but with matching tool_use_id
    let tool_result = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(), // Tool result event
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 1).unwrap(),
        },
        content: json!({
            "tool_call_id": "call_abc",
            "content": [{"text": "Sunny, 25C"}]
        }),
        rendering: false,
    };

    let result = to_sideml_with_context(&[tool_call, tool_result], false);

    assert_eq!(result.len(), 2);

    // Tool result should have name enriched from tool call
    let tool_result_msg = &result[1];
    assert_eq!(tool_result_msg.sideml.role, ChatRole::Tool);
    assert_eq!(
        tool_result_msg.sideml.name,
        Some("get_weather".to_string()),
        "Tool result should get name from matching tool_use"
    );
}

#[test]
fn test_tool_result_no_name_when_no_matching_tool_use() {
    use crate::observations::{MessageSource, RawMessage};

    // Tool result with no matching tool call
    let tool_result = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool",
            "tool_call_id": "orphan_id",
            "content": [{"text": "Result"}]
        }),
        rendering: false,
    };

    let result = to_sideml(&[tool_result]);

    assert_eq!(result.len(), 1);
    // No matching tool_use, so name should be None
    assert_eq!(result[0].sideml.name, None);
}

// === Tool Span vs Chat Span Context Tests ===

#[test]
fn test_gen_ai_choice_in_tool_span_becomes_tool_role() {
    use crate::observations::{MessageSource, RawMessage};
    use crate::sideml::to_sideml_with_context;

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Tool output result"}]
        }),
        rendering: false,
    };

    // In tool span: gen_ai.choice is tool OUTPUT
    let result = to_sideml_with_context(&[message], true);

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].sideml.role,
        ChatRole::Tool,
        "gen_ai.choice in tool span should be Tool role"
    );
}

#[test]
fn test_gen_ai_choice_in_chat_span_becomes_assistant_role() {
    use crate::observations::{MessageSource, RawMessage};
    use crate::sideml::to_sideml_with_context;

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Assistant response"}]
        }),
        rendering: false,
    };

    // In chat span: gen_ai.choice is assistant response
    let result = to_sideml_with_context(&[message], false);

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].sideml.role,
        ChatRole::Assistant,
        "gen_ai.choice in chat span should be Assistant role"
    );
}

#[test]
fn test_gen_ai_tool_message_in_chat_span_becomes_tool_role() {
    use crate::observations::{MessageSource, RawMessage};
    use crate::sideml::to_sideml_with_context;

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Tool result"}]
        }),
        rendering: false,
    };

    // In chat span: gen_ai.tool.message is tool result
    let result = to_sideml_with_context(&[message], false);

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].sideml.role,
        ChatRole::Tool,
        "gen_ai.tool.message in chat span should be Tool role"
    );
}

// === Message Attribute Fallback Tests ===

#[test]
fn test_normalize_message_uses_message_attribute_when_no_content() {
    // Strands uses "message" attribute instead of "content" for choice events
    let input = json!({
        "message": [{"toolUse": {"toolUseId": "123", "name": "weather", "input": {"city": "NYC"}}}],
        "finish_reason": "tool_use"
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 1);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[0])["name"], "weather");
}

#[test]
fn test_normalize_message_prefers_content_over_message() {
    // When both "content" and "message" are present, "content" should be used
    let input = json!({
        "content": "Hello from content",
        "message": "Hello from message"
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Hello from content"
    );
}

#[test]
fn test_normalize_message_with_message_string() {
    let input = json!({
        "message": "The weather is sunny."
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "The weather is sunny."
    );
}

// === to_sideml Role Derivation Tests ===

#[test]
fn test_to_sideml_derives_assistant_role_from_choice_event() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "message": "Hello, I can help you!",
            "finish_reason": "end_turn"
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
    assert_eq!(
        block_to_json(&result[0].sideml.content[0])["text"],
        "Hello, I can help you!"
    );
}

#[test]
fn test_to_sideml_derives_user_role_from_user_message_event() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": "What's the weather?"
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_to_sideml_derives_tool_role_from_tool_message_event() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": "Weather is sunny",
            "id": "tool123"
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::Tool);
}

#[test]
fn test_to_sideml_event_derived_role_takes_precedence() {
    use crate::observations::{MessageSource, RawMessage};

    // Event-derived role takes precedence over explicit role in content
    // (except for special roles like tool_call, tools, data, context)
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "user",  // Will be overridden by event-derived role
            "content": "Hello"
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // gen_ai.choice in non-tool span → Assistant (event-derived role takes precedence)
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

// ============================================================================
// UNFLATTEN DOTTED KEYS TESTS
// ============================================================================

#[test]
fn test_unflatten_tool_calls_from_openinference() {
    use crate::observations::{MessageSource, RawMessage};

    // OpenInference stores tool calls as flattened attributes
    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "llm.output_messages.0.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "assistant",
            "tool_calls.0.tool_call.id": "call_abc123",
            "tool_calls.0.tool_call.function.name": "get_weather",
            "tool_calls.0.tool_call.function.arguments": {"city": "NYC", "days": 3}
        }),
        rendering: false,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);

    // Tool calls should be converted to content[].tool_use
    let tool_uses: Vec<_> = result[0]
        .sideml
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .collect();
    assert_eq!(tool_uses.len(), 1, "Should have one ToolUse content block");
    if let ContentBlock::ToolUse { id, name, .. } = tool_uses[0] {
        assert_eq!(name, "get_weather");
        assert_eq!(id.as_deref(), Some("call_abc123"));
    }
}
