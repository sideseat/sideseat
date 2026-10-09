#[test]
fn test_unflatten_multiple_tool_calls() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "llm.output_messages.0.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "assistant",
            "tool_calls.0.tool_call.id": "call_1",
            "tool_calls.0.tool_call.function.name": "get_weather",
            "tool_calls.1.tool_call.id": "call_2",
            "tool_calls.1.tool_call.function.name": "get_time"
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    // After flattening, bundled tool calls become individual messages
    assert_eq!(
        result.len(),
        2,
        "Multiple tool calls should be flattened into separate messages"
    );

    // Collect all tool names from both messages
    let tool_uses: Vec<_> = result
        .iter()
        .flat_map(|msg| {
            msg.sideml.content.iter().filter_map(|b| {
                if let ContentBlock::ToolUse { name, .. } = b {
                    Some(name.as_str())
                } else {
                    None
                }
            })
        })
        .collect();
    assert_eq!(
        tool_uses.len(),
        2,
        "Should have two ToolUse content blocks total"
    );
    assert!(tool_uses.contains(&"get_weather"));
    assert!(tool_uses.contains(&"get_time"));
}

#[test]
fn test_unflatten_no_dotted_keys_unchanged() {
    use crate::observations::{MessageSource, RawMessage};

    // Message without dotted keys should pass through unchanged
    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "user",
            "content": "Hello world"
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::User);
    assert_eq!(block_to_json(&result[0].sideml.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&result[0].sideml.content[0])["text"],
        "Hello world"
    );
}

#[test]
fn test_unflatten_nested_object_path() {
    use crate::observations::{MessageSource, RawMessage};

    // Test deeply nested path without array indices
    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "assistant",
            "metadata.provider.name": "openai",
            "metadata.provider.version": "v1"
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // The metadata should be unflattened but won't appear in normalized output
    // since it's not a standard field - this test verifies no crash occurs
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

// ============================================================================
// SPECIAL ROLE TESTS - Tool Definitions (role="tools")
// ============================================================================

#[test]
fn test_normalize_tools_role_with_definitions() {
    let raw = json!({
        "role": "tools",
        "type": "tool_definitions",
        "content": [
            {
                "name": "get_weather",
                "description": "Get current weather",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
            }
        ]
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::System);
    // Content should have ToolDefinitions block
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        ContentBlock::ToolDefinitions { tools, .. } => {
            assert_eq!(tools.len(), 1);
        }
        _ => panic!("Expected ToolDefinitions content block"),
    }
}

#[test]
fn test_normalize_tools_role_with_tool_choice() {
    let raw = json!({
        "role": "tools",
        "content": [{"name": "search"}],
        "tool_choice": "required"
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::System);
    assert_eq!(msg.tool_choice, Some(ToolChoice::Required));
}

#[test]
fn test_normalize_tools_role_agent_tools() {
    // Agent tools are a simple list of tool names
    let raw = json!({
        "role": "tools",
        "type": "agent_tools",
        "content": ["get_weather", "send_email", "search"]
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::System);
    // Content should have ToolDefinitions block with 3 tools
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        ContentBlock::ToolDefinitions { tools, .. } => {
            assert_eq!(tools.len(), 3);
        }
        _ => panic!("Expected ToolDefinitions content block"),
    }
}

#[test]
fn test_is_tools_definition_role() {
    assert!(ChatRole::is_tools_definition_role("tools"));
    assert!(ChatRole::is_tools_definition_role("Tools"));
    assert!(ChatRole::is_tools_definition_role("TOOLS"));
    assert!(!ChatRole::is_tools_definition_role("tool"));
    assert!(!ChatRole::is_tools_definition_role("system"));
    assert!(!ChatRole::is_tools_definition_role("user"));
}

// ============================================================================
// SPECIAL ROLE TESTS - Tool Call (role="tool_call")
// ============================================================================

#[test]
fn test_normalize_tool_call_role() {
    let raw = json!({
        "role": "tool_call",
        "name": "get_weather",
        "tool_call_id": "call_123",
        "content": {"city": "New York"}
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::Assistant);
    assert_eq!(msg.finish_reason, Some(FinishReason::ToolUse));
    assert_eq!(msg.content.len(), 1);

    match &msg.content[0] {
        ContentBlock::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, &Some("call_123".to_string()));
            assert_eq!(name, "get_weather");
            assert_eq!(input["city"], "New York");
        }
        _ => panic!("Expected ToolUse content block"),
    }
}

#[test]
fn test_normalize_tool_call_role_without_id() {
    let raw = json!({
        "role": "tool_call",
        "name": "search",
        "content": {"query": "rust programming"}
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::Assistant);
    assert_eq!(msg.content.len(), 1);

    match &msg.content[0] {
        ContentBlock::ToolUse {
            id, name, input, ..
        } => {
            assert!(id.is_none());
            assert_eq!(name, "search");
            assert_eq!(input["query"], "rust programming");
        }
        _ => panic!("Expected ToolUse content block"),
    }
}

#[test]
fn test_tool_call_role_normalizes_to_assistant() {
    // "tool_call" should normalize to Assistant role
    assert_eq!(
        ChatRole::from_str_normalized("tool_call"),
        ChatRole::Assistant
    );
}

// ============================================================================
// SPECIAL ROLE TESTS - Context and Data Roles
// ============================================================================

#[test]
fn test_data_role_normalizes_to_user() {
    // "data" role (Google ADK) should normalize to User
    assert_eq!(ChatRole::from_str_normalized("data"), ChatRole::User);
}

#[test]
fn test_context_role_normalizes_to_user() {
    // "context" role should normalize to User
    assert_eq!(ChatRole::from_str_normalized("context"), ChatRole::User);
}

#[test]
fn test_normalize_data_role_message() {
    let raw = json!({
        "role": "data",
        "type": "conversation_history",
        "content": {"messages": [{"role": "user", "content": "Hi"}]}
    });

    let msg = normalize(&raw);

    // Data role normalizes to user
    assert_eq!(msg.role, ChatRole::User);
    // Content has Context block
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        ContentBlock::Context { data, context_type } => {
            assert_eq!(context_type, &Some("conversation_history".to_string()));
            assert!(data.get("messages").is_some());
        }
        _ => panic!("Expected Context content block"),
    }
}

#[test]
fn test_category_from_data_role() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "data",
            "type": "conversation_history",
            "content": {"messages": []}
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].category, MessageCategory::GenAIContext);
}

#[test]
fn test_context_type_inferred_from_data_role() {
    // When type is not explicitly set, it should be inferred from role
    let raw = json!({
        "role": "data",
        "content": {"messages": []}
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::User);
    match &msg.content[0] {
        ContentBlock::Context { context_type, .. } => {
            assert_eq!(context_type, &Some("conversation_history".to_string()));
        }
        _ => panic!("Expected Context content block"),
    }
}

#[test]
fn test_context_type_inferred_from_context_role() {
    // When type is not explicitly set, it should be inferred from role
    let raw = json!({
        "role": "context",
        "content": [{"role": "user", "content": "hi"}]
    });

    let msg = normalize(&raw);

    assert_eq!(msg.role, ChatRole::User);
    match &msg.content[0] {
        ContentBlock::Context { context_type, .. } => {
            assert_eq!(context_type, &Some("chat_context".to_string()));
        }
        _ => panic!("Expected Context content block"),
    }
}

// ============================================================================
// PIPELINE CATEGORY TESTS - New Roles
// ============================================================================

#[test]
fn test_category_from_tool_call_role() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool_call",
            "name": "get_weather",
            "content": {"city": "NYC"}
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].category, MessageCategory::GenAIToolInput);
}

#[test]
fn test_tool_call_role_from_event_gets_tool_input_category() {
    // Event source with role="tool_call" (from tool span extraction) should get GenAIToolInput
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool_call",
            "name": "get_weather",
            "tool_call_id": "call_123",
            "content": {"city": "NYC"}
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].category,
        MessageCategory::GenAIToolInput,
        "Event with role=tool_call should get GenAIToolInput category"
    );
}

#[test]
fn test_category_from_tools_role() {
    use crate::observations::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tools",
            "content": [{"name": "search"}]
        }),
        rendering: false,
        direction: None,
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].category, MessageCategory::GenAIToolDefinitions);
}

/// End-to-end pipeline test: tool span messages with correct roles and categories
#[test]
fn test_pipeline_tool_span_messages_end_to_end() {
    use crate::sideml::to_sideml_with_context;

    // Simulate what extraction produces for a tool span:
    // - tool_call message (input TO the tool)
    // - tool message (output FROM the tool)

    let tool_call_msg = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool_call",  // Special role for tool input
            "name": "weather_forecast",
            "tool_call_id": "call_123",
            "content": {"city": "NYC", "days": 3}
        }),
        rendering: false,
        direction: None,
    };

    let tool_result_msg = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 1).unwrap(),
        },
        content: json!({
            "tool_call_id": "call_123",
            "content": [{"text": "Weather is sunny"}]
        }),
        rendering: false,
        direction: None,
    };

    // Use is_tool_span=true for tool span context
    let result = to_sideml_with_context(&[tool_call_msg, tool_result_msg], true);

    assert_eq!(result.len(), 2);

    // Verify tool_call message
    let tool_call = &result[0];
    assert_eq!(
        tool_call.category,
        MessageCategory::GenAIToolInput,
        "tool_call role should get GenAIToolInput category"
    );
    assert_eq!(tool_call.sideml.role, ChatRole::Assistant); // tool_call normalizes to assistant

    // Note: name is extracted from content blocks (tool_use), not from message-level name field
    // For tool_call messages, the name appears in the tool_use content block
    let has_tool_use = tool_call
        .sideml
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "weather_forecast"));
    assert!(
        has_tool_use,
        "tool_call message should have tool_use content block with name"
    );

    // Verify tool result message
    let tool_result = &result[1];
    // gen_ai.choice events always get GenAIChoice category (output, not history)
    // This is important for history filtering: choice events are never filtered
    assert_eq!(
        tool_result.category,
        MessageCategory::GenAIChoice,
        "gen_ai.choice should get GenAIChoice category even with tool role"
    );
    assert_eq!(tool_result.sideml.role, ChatRole::Tool);
    assert_eq!(
        tool_result.sideml.tool_use_id,
        Some("call_123".to_string()),
        "tool result should have tool_use_id for correlation"
    );
}

// ============================================================================
// EXTENDED THINKING / REASONING TESTS
// Universal support for extended thinking across all providers
// ============================================================================

#[test]
fn test_bedrock_reasoning_content_with_signature() {
    // AWS Bedrock format: {"reasoningContent": {"reasoningText": {"text": "...", "signature": "..."}}}
    let input = json!({
        "role": "assistant",
        "content": [{
            "reasoningContent": {
                "reasoningText": {
                    "text": "Let me analyze this step by step...",
                    "signature": "ErYhCkgICxABGAI..."
                }
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Let me analyze this step by step..."
    );
    assert_eq!(
        thinking_signature(&output.content[0]),
        Some("ErYhCkgICxABGAI...")
    );
}

#[test]
fn test_bedrock_reasoning_content_no_signature() {
    let input = json!({
        "role": "assistant",
        "content": [{
            "reasoningContent": {
                "reasoningText": {
                    "text": "Thinking without signature"
                }
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Thinking without signature"
    );
    // Signature should be absent or null when not provided
    let block = block_to_json(&output.content[0]);
    let signature = block.get("signature");
    assert!(
        signature.is_none() || signature.unwrap().is_null(),
        "signature should be absent or null"
    );
}

#[test]
fn test_anthropic_thinking_with_thinking_field() {
    // Anthropic API format: {"type": "thinking", "thinking": "...", "signature": "..."}
    // Note: Anthropic uses "thinking" field, NOT "text" field
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": "Let me work through this problem...",
            "signature": "sig_abc123"
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Let me work through this problem..."
    );
    assert_eq!(thinking_signature(&output.content[0]), Some("sig_abc123"));
}

#[test]
fn test_mistral_nested_thinking_array_concatenates() {
    // Mistral format: {"type": "thinking", "thinking": [{"type": "text", "text": "..."}]}
    // Multiple text blocks should be concatenated
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": [
                {"type": "text", "text": "First, let me consider the problem..."},
                {"type": "text", "text": "Then I'll analyze the constraints..."}
            ]
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "First, let me consider the problem...\n\nThen I'll analyze the constraints..."
    );
}

#[test]
fn test_mistral_empty_thinking_array_is_reasoning_with_no_text() {
    // Mistral's `thinking` array shape is recognised (it must not fall through to
    // {"type":"unknown"}), and an empty one is a reasoning step whose text is not there: thinking with
    // no text, which the view names as such. Extraction from a NON-empty array is covered by
    // test_mistral_mixed_block_types_only_text_extracted.
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": []
        }]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 1, "{:?}", output.content);
    assert!(
        matches!(&output.content[0], ContentBlock::Thinking { text, signature: None } if text.is_empty()),
        "{:?}",
        output.content
    );
}

#[test]
fn test_mistral_mixed_block_types_only_text_extracted() {
    // Only text blocks should be extracted from Mistral's thinking array
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": [
                {"type": "text", "text": "Valid thinking..."},
                {"type": "image", "data": "..."},
                {"type": "text", "text": "More thinking..."}
            ]
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Valid thinking...\n\nMore thinking..."
    );
}

#[test]
fn test_gemini_thinking_part() {
    // Gemini format: {"thinking": "..."} (top-level, similar to {"text": "..."})
    let input = json!({
        "role": "assistant",
        "content": [{"thinking": "My reasoning process here..."}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "My reasoning process here..."
    );
}

#[test]
fn test_mixed_provider_thinking_and_text() {
    // Integration test: mixed Bedrock reasoning + regular text + Anthropic thinking
    let input = json!({
        "role": "assistant",
        "content": [
            {
                "reasoningContent": {
                    "reasoningText": {"text": "Bedrock thinking...", "signature": "sig1"}
                }
            },
            {"text": "Here's the answer."},
            {"type": "thinking", "thinking": "More thinking..."}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 3);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Bedrock thinking..."
    );
    assert_eq!(block_to_json(&output.content[1])["type"], "text");
    assert_eq!(block_to_json(&output.content[2])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[2])["text"],
        "More thinking..."
    );
}

#[test]
fn test_thinking_with_text_field_legacy() {
    // Legacy/SideML internal format: {"type": "thinking", "text": "..."}
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "text": "Legacy format thinking content"
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Legacy format thinking content"
    );
}

#[test]
fn test_bedrock_redacted_thinking() {
    // Bedrock redacted thinking variant
    let input = json!({
        "role": "assistant",
        "content": [{
            "reasoningContent": {
                "redactedContent": {
                    "data": "base64encodedredacteddata..."
                }
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0])["type"],
        "redacted_thinking"
    );
    assert_eq!(
        block_to_json(&output.content[0])["data"],
        "base64encodedredacteddata..."
    );
}

#[test]
fn test_bedrock_unknown_reasoning_variant_preserved() {
    // Future Bedrock format we don't recognize yet - should be preserved
    let input = json!({
        "role": "assistant",
        "content": [{
            "reasoningContent": {
                "newFutureFormat": {
                    "someField": "value"
                }
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "unknown");
    assert!(block_to_json(&output.content[0]).get("raw").is_some());
}

#[test]
fn test_gemini_thinking_not_confused_with_type_tagged() {
    // Type-tagged thinking should be handled by try_openai_format, not try_gemini_format
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": "This has a type field"
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "This has a type field"
    );
}

#[test]
fn test_pydantic_ai_thinking_content_field() {
    // PydanticAI uses "content" instead of "text" or "thinking"
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "content": "Let me analyze this problem..."
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Let me analyze this problem..."
    );
}

#[test]
fn test_thinking_null_field_fallback() {
    // If "thinking" field is null, should fall back to other fields
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": null,
            "text": "Fallback text"
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(block_to_json(&output.content[0])["text"], "Fallback text");
}

#[test]
fn test_thinking_field_priority_order() {
    // "thinking" field should take priority over "text" and "content"
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": "From thinking field",
            "text": "From text field",
            "content": "From content field"
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "From thinking field"
    );
}

// === Tool Result Content Normalization Tests ===

#[test]
fn test_tool_result_content_normalized_to_sideml() {
    // Bedrock toolResult content should be normalized to SideML format
    let input = json!({
        "role": "tool",
        "content": [{
            "toolResult": {
                "toolUseId": "tool_123",
                "content": [
                    {"text": "The image was saved."},
                    {"image": {"format": "jpeg", "source": {"bytes": "abc123"}}}
                ]
            }
        }]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    assert_eq!(block["tool_use_id"], "tool_123");
    // Inner content should be normalized to SideML format (has "type" field)
    let content = block["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "The image was saved.");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["media_type"], "image/jpeg");
}

#[test]
fn test_tool_result_string_content_unchanged() {
    // String content inside tool_result should stay as string
    let input = json!({
        "role": "tool",
        "content": [{"type": "tool_result", "tool_use_id": "123", "content": "Simple text result"}]
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0])["content"],
        "Simple text result"
    );
}

#[test]
fn test_convert_no_tool_result_creates_one() {
    // When tool role has content blocks without tool_result, wrap them in tool_result
    let input = json!({
        "role": "tool",
        "tool_call_id": "tool_id",
        "content": [
            {"type": "text", "text": "Result text"},
            {"type": "image", "source": {"type": "base64", "data": "abc123"}, "media_type": "image/jpeg"}
        ]
    });
    let output = normalize(&input);
    // Should have single tool_result wrapping both blocks
    assert_eq!(output.content.len(), 1);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    let inner = block["content"].as_array().unwrap();
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[0]["type"], "text");
    assert_eq!(inner[1]["type"], "image");
}

#[test]
fn test_convert_merges_siblings_into_existing_tool_result() {
    // When tool_result has sibling blocks, merge siblings into tool_result's content
    let input = json!({
        "role": "tool",
        "content": [
            {"type": "tool_result", "tool_use_id": "123", "content": "Text result"},
            {"type": "image", "source": {"type": "base64", "data": "abc123"}, "media_type": "image/jpeg"}
        ]
    });
    let output = normalize(&input);
    // Should have single tool_result with merged content
    assert_eq!(output.content.len(), 1);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    let inner = block["content"].as_array().unwrap();
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[0]["type"], "text");
    assert_eq!(inner[0]["text"], "Text result");
    assert_eq!(inner[1]["type"], "image");
}
