#[test]
fn test_has_meaningful_data_helper() {
    // Unit test for the helper function itself
    // Note: Private helper accessed via super:: from test module

    // Should return false (no meaningful data)
    assert!(!super::has_meaningful_data(&json!(null)));
    assert!(!super::has_meaningful_data(&json!([])));
    assert!(!super::has_meaningful_data(&json!({})));
    assert!(!super::has_meaningful_data(&json!("")));
    assert!(!super::has_meaningful_data(&json!("   "))); // Whitespace-only
    assert!(!super::has_meaningful_data(&json!([null])));
    assert!(!super::has_meaningful_data(&json!([{}])));
    assert!(!super::has_meaningful_data(&json!({"key": null})));
    assert!(!super::has_meaningful_data(&json!({"key": {}})));
    assert!(!super::has_meaningful_data(&json!(false))); // Booleans are metadata
    assert!(!super::has_meaningful_data(&json!(true))); // Booleans are metadata
    assert!(!super::has_meaningful_data(&json!(0))); // Zero is not meaningful
    assert!(!super::has_meaningful_data(&json!(0.0))); // Zero is not meaningful

    // Should return true (has meaningful data)
    assert!(super::has_meaningful_data(&json!("text")));
    assert!(super::has_meaningful_data(&json!(42))); // Non-zero number
    assert!(super::has_meaningful_data(&json!(0.5))); // Non-zero number
    assert!(super::has_meaningful_data(&json!(["text"])));
    assert!(super::has_meaningful_data(&json!({"key": "value"})));
    assert!(super::has_meaningful_data(&json!([{"title": "Citation"}])));
    assert!(super::has_meaningful_data(
        &json!({"nested": {"deep": "value"}})
    ));
}

#[test]
fn test_special_role_with_citations_edge_case() {
    // Edge case: role "context" with Azure-style context object
    // Citations should NOT be extracted (early return for special role)
    // This is acceptable - "context" role is for conversation history, not API responses
    let input = json!({
        "role": "context",  // Special role triggers early return
        "content": {"history": []},
        "context": {  // This Azure-style context is lost (acceptable)
            "citations": [{"title": "Lost citation"}]
        }
    });
    let output = normalize(&input);

    // The message becomes a Context content block (from normalize_context_message)
    assert_eq!(output.role, ChatRole::User); // context role maps to User

    // No citation Context blocks should exist (they weren't extracted)
    let citation_context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        citation_context.is_none(),
        "Citations should NOT be extracted for special role messages (acceptable behavior)"
    );
}

// --- Vertex AI Verification Tests ---

#[test]
fn test_vertex_model_role_maps_to_assistant() {
    let input = json!({"role": "model", "content": "Hello from Gemini"});
    let output = normalize(&input);
    assert_eq!(output.role, ChatRole::Assistant);
}

#[test]
fn test_vertex_gs_uri_preserved_in_file_data() {
    let input = json!({
        "role": "user",
        "content": [{
            "file_data": {
                "mime_type": "image/png",
                "file_uri": "gs://my-bucket/images/photo.png"
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 1);

    // Verify the gs:// URI is preserved
    if let ContentBlock::Image { source, data, .. } = &output.content[0] {
        assert_eq!(source, "url");
        assert_eq!(data, "gs://my-bucket/images/photo.png");
    } else {
        panic!("Expected Image block");
    }
}

#[test]
fn test_vertex_function_declarations_normalized() {
    // Vertex AI uses "functionDeclarations" in tools
    let tools = json!([{
        "functionDeclarations": [{
            "name": "get_weather",
            "description": "Get current weather",
            "parameters": {
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                }
            }
        }]
    }]);
    let normalized = super::tools::normalize_tools(&tools);
    let arr = normalized.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(
        arr[0].get("function").is_some(),
        "Should normalize to OpenAI format"
    );
}

// ============================================================================
// QUERY-TIME EXPANSION TESTS
// ============================================================================

#[test]
fn test_bundled_tool_results_expanded_from_gen_ai_tool_result_event() {
    // Issue: gen_ai.tool.result events don't have role set in raw content
    // (role is derived at query time), so expand_bundled_tool_results must
    // check the event source name, not just the role field.
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        // Note: No "role" field - it's derived from event name at query time
        content: json!({
            "content": [
                {"toolResult": {"toolUseId": "id1", "content": [{"text": "Result 1"}]}},
                {"toolResult": {"toolUseId": "id2", "content": [{"text": "Result 2"}]}}
            ],
            "tool_call_id": "id1"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    // Should expand into 2 separate messages (one per toolResult)
    assert_eq!(
        sideml_messages.len(),
        2,
        "Bundled tool results should be expanded into individual messages"
    );

    // Both should have role "tool" (derived from event name)
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::Tool);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::Tool);

    // Each should have its own tool_use_id
    assert_eq!(
        sideml_messages[0].sideml.tool_use_id.as_deref(),
        Some("id1")
    );
    assert_eq!(
        sideml_messages[1].sideml.tool_use_id.as_deref(),
        Some("id2")
    );
}

#[test]
fn test_bundled_tool_results_single_result_not_expanded() {
    // Single tool result should NOT be expanded (no change)
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "content": [
                {"toolResult": {"toolUseId": "id1", "content": [{"text": "Result 1"}]}}
            ],
            "tool_call_id": "id1"
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    // Should remain as 1 message
    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::Tool);
}

#[test]
fn test_message_array_expanded_from_gen_ai_input_messages() {
    // Issue: Arrays stored at ingestion must be expanded at query time
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        // Array of messages stored as-is at ingestion
        content: json!([
            {"role": "system", "content": "You are helpful"},
            {"role": "user", "content": "Hello"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    // Should expand into 2 separate messages
    assert_eq!(
        sideml_messages.len(),
        2,
        "Message array should be expanded into individual messages"
    );

    // First message is system
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::System);

    // Second message is user
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::User);
}

#[test]
fn test_message_array_expanded_from_gen_ai_output_messages() {
    // Test output messages array expansion
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.output.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!([
            {"role": "assistant", "content": "Here's the weather"},
            {"role": "assistant", "content": "And here's more info"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    // Should expand into 2 separate messages
    assert_eq!(sideml_messages.len(), 2);
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::Assistant);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::Assistant);
}

#[test]
fn test_message_array_single_message_not_expanded() {
    // Single message in array should still become one message
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "content": "Hello"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::User);
}

#[test]
fn test_message_array_with_nested_content_field() {
    // Some frameworks nest the array in a "content" field
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "content": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi!"}
            ]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml(&raw_messages);

    // Should expand the nested array
    assert_eq!(sideml_messages.len(), 2);
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::User);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::Assistant);
}

#[test]
fn test_tool_span_role_derivation_with_gen_ai_choice() {
    // Issue: gen_ai.choice events in tool spans should derive role "tool"
    // This tests the to_sideml_with_context function with is_tool_span=true
    use crate::sideml::to_sideml_with_context;

    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "message": "Tool result: 72F"
        }),
        rendering: false,
    }];

    // In a tool span, gen_ai.choice = tool OUTPUT (role: tool)
    let sideml_messages = to_sideml_with_context(&raw_messages, true);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(
        sideml_messages[0].sideml.role,
        ChatRole::Tool,
        "gen_ai.choice in tool span should derive role 'tool'"
    );
}

#[test]
fn test_chat_span_role_derivation_with_gen_ai_choice() {
    // gen_ai.choice events in chat spans (non-tool) should derive role "assistant"
    use crate::sideml::to_sideml_with_context;

    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "message": "Hello! How can I help?"
        }),
        rendering: false,
    }];

    // In a chat span (not tool), gen_ai.choice = assistant response
    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(
        sideml_messages[0].sideml.role,
        ChatRole::Assistant,
        "gen_ai.choice in chat span should derive role 'assistant'"
    );
}

// === Documents Role Tests ===

#[test]
fn test_documents_role_is_preserved_in_special_roles() {
    use crate::observations::{MessageSource, RawMessage};

    // OpenInference retrieval documents have role="documents"
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "documents",
            "content": [
                {"id": "doc1", "content": "Document 1 text"},
                {"id": "doc2", "content": "Document 2 text"}
            ]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(sideml_messages.len(), 1);
    // "documents" is a special role that should NOT be overridden by event-based derivation
    assert_eq!(
        sideml_messages[0].sideml.role,
        ChatRole::User, // Normalizes to User since documents->User in ChatRole::from_str_normalized
        "documents role should be preserved (normalized to User)"
    );
    // More importantly, the category should be Retrieval
    assert_eq!(
        sideml_messages[0].category,
        MessageCategory::Retrieval,
        "documents role should categorize as Retrieval"
    );
}

#[test]
fn test_documents_role_from_attribute_source() {
    use crate::observations::{MessageSource, RawMessage};

    // Documents from attribute source (e.g., retrieval.documents)
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "retrieval.documents".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "documents",
            "content": [{"id": "doc1", "content": "Retrieved content"}]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(sideml_messages.len(), 1);
    assert_eq!(
        sideml_messages[0].category,
        MessageCategory::Retrieval,
        "documents role from attribute should categorize as Retrieval"
    );
}

// === Message Array Expansion Tests for Different Sources ===

#[test]
fn test_message_array_expanded_from_ai_prompt_messages() {
    use crate::observations::{MessageSource, RawMessage};

    // Vercel AI SDK format: ai.prompt.messages contains array
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "ai.prompt.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "system", "content": "You are a helpful assistant"},
            {"role": "user", "content": "Hello!"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        2,
        "Array from ai.prompt.messages should be expanded"
    );
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::System);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::User);
}

#[test]
fn test_message_array_expanded_from_mlflow_span_inputs() {
    use crate::observations::{MessageSource, RawMessage};

    // MLflow format: mlflow.spanInputs contains messages
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "mlflow.spanInputs".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "content": "What is 2+2?"},
            {"role": "assistant", "content": "4"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        2,
        "Array from mlflow.spanInputs should be expanded"
    );
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::User);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::Assistant);
}

#[test]
fn test_message_array_not_expanded_from_unknown_source() {
    use crate::observations::{MessageSource, RawMessage};

    // Unknown source should NOT be expanded (could be intentionally bundled)
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "some.unknown.source".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "content": "Message 1"},
            {"role": "assistant", "content": "Message 2"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    // Unknown sources should be kept as-is (not expanded)
    // The array becomes a single message with the array as content
    assert_eq!(
        sideml_messages.len(),
        1,
        "Array from unknown source should NOT be expanded"
    );
}

// === Tool Message Role Derivation Edge Cases ===

#[test]
fn test_tool_message_in_tool_span_without_extraction_role() {
    use crate::observations::{MessageSource, RawMessage};

    // Edge case: gen_ai.tool.message in tool span WITHOUT role set during extraction
    // In tool spans, gen_ai.tool.message is tool INPUT (invocation args)
    // Role is derived as Assistant to prevent merging with tool OUTPUT
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"type": "text", "text": "Tool input args"}]
            // Note: NO role field - will be derived from event name + span context
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, true); // is_tool_span=true

    assert_eq!(sideml_messages.len(), 1);
    // In tool span, gen_ai.tool.message is tool INPUT → Assistant role
    // This prevents merging with tool OUTPUT in ToolResultRegistry
    assert_eq!(
        sideml_messages[0].sideml.role,
        ChatRole::Assistant,
        "Tool input in tool span should get Assistant role to prevent merging with output"
    );
}

#[test]
fn test_tool_call_role_preserved_in_tool_span() {
    use crate::observations::{MessageSource, RawMessage};

    // Normal case: gen_ai.tool.message in tool span WITH tool_call role from extraction
    // "tool_call" represents the assistant invoking a tool, so it becomes a ToolUse block
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool_call",  // Set during extraction
            "name": "get_weather",
            "content": [{"type": "text", "text": "{\"location\": \"NYC\"}"}]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, true); // is_tool_span=true

    assert_eq!(sideml_messages.len(), 1);
    // tool_call represents assistant calling a tool, so it normalizes to Assistant with ToolUse
    assert_eq!(
        sideml_messages[0].sideml.role,
        ChatRole::Assistant,
        "tool_call role normalizes to Assistant (tool invocation)"
    );
    // The category should be GenAIToolInput (what's being sent to the tool)
    assert_eq!(
        sideml_messages[0].category,
        MessageCategory::GenAIToolInput,
        "tool_call should categorize as GenAIToolInput"
    );
    // Check that the content is converted to ToolUse block
    assert!(
        sideml_messages[0]
            .sideml
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "get_weather")),
        "tool_call should be converted to ToolUse block"
    );
}

#[test]
fn test_gen_ai_tool_result_role_derivation() {
    use crate::observations::{MessageSource, RawMessage};

    // gen_ai.tool.result event should always derive Tool role
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"toolResult": {"toolUseId": "123", "content": "Result"}}]
        }),
        rendering: false,
    }];

    // Test in both chat span and tool span contexts
    let sideml_in_chat_span = to_sideml_with_context(&raw_messages, false);
    let sideml_in_tool_span = to_sideml_with_context(&raw_messages, true);

    assert_eq!(sideml_in_chat_span[0].sideml.role, ChatRole::Tool);
    assert_eq!(sideml_in_tool_span[0].sideml.role, ChatRole::Tool);
}

// === Special Roles Categorization Tests ===

#[test]
fn test_special_roles_categorization() {
    use crate::observations::{MessageSource, RawMessage};

    let test_cases = vec![
        ("tool_call", MessageCategory::GenAIToolInput),
        ("tools", MessageCategory::GenAIToolDefinitions),
        ("data", MessageCategory::GenAIContext),
        ("context", MessageCategory::GenAIContext),
        ("documents", MessageCategory::Retrieval),
    ];

    for (role, expected_category) in test_cases {
        let raw_messages = vec![RawMessage {
            source: MessageSource::Attribute {
                key: "test".to_string(),
                time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            },
            content: json!({
                "role": role,
                "content": "test content"
            }),
            rendering: false,
        }];

        let sideml_messages = to_sideml_with_context(&raw_messages, false);

        assert_eq!(
            sideml_messages[0].category, expected_category,
            "Role '{}' should categorize as {:?}",
            role, expected_category
        );
    }
}

// === Stable Hash Tests ===

#[test]
fn test_gemini_synthetic_id_is_deterministic() {
    // Test that Gemini synthetic IDs are deterministic (same input = same output)
    // This is critical for tool result correlation
    use crate::sideml::content::normalize_content;

    // Gemini function call format
    let gemini_call = json!({
        "function_call": {
            "name": "get_weather",
            "args": {"location": "NYC"}
        }
    });

    // Normalize twice and check IDs are the same
    let result1 = normalize_content(Some(&gemini_call));
    let result2 = normalize_content(Some(&gemini_call));

    // Both should produce the same synthetic ID
    assert_eq!(
        result1, result2,
        "Gemini synthetic IDs should be deterministic"
    );

    // Verify the result contains a ToolUse block with synthetic ID
    let arr = result1.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let block = &arr[0];
    assert_eq!(block.get("type").unwrap(), "tool_use");
    let id = block.get("id").unwrap().as_str().unwrap();
    assert!(
        id.starts_with("gemini_get_weather_call_"),
        "ID should have deterministic prefix"
    );
}

// === Case-Insensitive SPECIAL_ROLES Tests ===

#[test]
fn test_special_roles_case_insensitive() {
    use crate::observations::{MessageSource, RawMessage};

    // Test that SPECIAL_ROLES check is case-insensitive
    let test_cases = vec![
        ("tool_call", ChatRole::Assistant), // Lowercase
        ("TOOL_CALL", ChatRole::Assistant), // Uppercase
        ("Tool_Call", ChatRole::Assistant), // Mixed case
        ("DOCUMENTS", ChatRole::User),      // Documents uppercase
        ("Documents", ChatRole::User),      // Documents mixed case
    ];

    for (role, expected_role) in test_cases {
        let raw_messages = vec![RawMessage {
            source: MessageSource::Event {
                name: "gen_ai.user.message".to_string(), // This would normally derive User
                time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            },
            content: json!({
                "role": role,
                "content": "test"
            }),
            rendering: false,
        }];

        let sideml_messages = to_sideml_with_context(&raw_messages, false);

        // The special role should be preserved (not overridden by event-based derivation)
        // and then normalized to the expected ChatRole
        assert_eq!(
            sideml_messages[0].sideml.role, expected_role,
            "Role '{}' should be preserved as special role (normalized to {:?})",
            role, expected_role
        );
    }
}

// === Bundled Tool Result Splitting Tests ===

#[test]
fn test_bundled_tool_results_snake_case_format() {
    use crate::observations::{MessageSource, RawMessage};

    // Test snake_case variant: tool_result instead of toolResult
    let bundled_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tool",
            "content": [
                {"tool_result": {"tool_use_id": "id1", "content": "Result 1"}},
                {"tool_result": {"tool_use_id": "id2", "content": "Result 2"}}
            ]
        }),
        rendering: false,
    };

    let sideml_messages = to_sideml_with_context(&[bundled_message], false);

    assert_eq!(
        sideml_messages.len(),
        2,
        "Bundled tool_results (snake_case) should be split"
    );
}

#[test]
fn test_bundled_tool_results_direct_array() {
    use crate::observations::{MessageSource, RawMessage};

    // Test direct array format (content is top-level array, not nested)
    let bundled_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"toolResult": {"toolUseId": "id1", "content": "Result 1"}},
            {"toolResult": {"toolUseId": "id2", "content": "Result 2"}}
        ]),
        rendering: false,
    };

    let sideml_messages = to_sideml_with_context(&[bundled_message], false);

    assert_eq!(
        sideml_messages.len(),
        2,
        "Bundled toolResults in direct array format should be split"
    );
}

// === Content Field Fallbacks Tests ===

#[test]
fn test_content_extraction_from_parts_field() {
    // Test Gemini "parts" field extraction
    let raw = json!({
        "role": "user",
        "parts": [{"text": "Hello from Gemini parts field"}]
    });

    let message = normalize(&raw);

    assert!(
        !message.content.is_empty(),
        "Content should be extracted from 'parts' field"
    );
}

#[test]
fn test_content_extraction_from_text_field() {
    // Test Bedrock "text" field extraction
    let raw = json!({
        "role": "user",
        "text": "Hello from text field"
    });

    let message = normalize(&raw);

    assert!(
        !message.content.is_empty(),
        "Content should be extracted from 'text' field"
    );
    if let ContentBlock::Text { text, .. } = &message.content[0] {
        assert_eq!(text, "Hello from text field");
    } else {
        panic!("Expected Text content block");
    }
}

#[test]
fn test_content_extraction_from_arguments_field() {
    // Test tool call "arguments" field extraction
    let raw = json!({
        "role": "assistant",
        "arguments": {"location": "NYC"}
    });

    let message = normalize(&raw);

    // Arguments should be captured as content
    assert!(
        !message.content.is_empty(),
        "Content should be extracted from 'arguments' field"
    );
}

// === Message Array Expansion Tests ===

#[test]
fn test_message_array_expanded_from_messages_field() {
    use crate::observations::{MessageSource, RawMessage};

    // Test "messages" field expansion (common in many frameworks)
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "messages": [
                {"role": "system", "content": "You are a helper"},
                {"role": "user", "content": "Hello!"}
            ]
        }),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        2,
        "Array from 'messages' field should be expanded"
    );
    assert_eq!(sideml_messages[0].sideml.role, ChatRole::System);
    assert_eq!(sideml_messages[1].sideml.role, ChatRole::User);
}

#[test]
fn test_message_array_expansion_with_gemini_parts() {
    use crate::observations::{MessageSource, RawMessage};

    // Test that Gemini format with "parts" is recognized as message-like
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "parts": [{"text": "Hello from Gemini"}]}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        1,
        "Gemini format with 'parts' should be recognized"
    );
}

#[test]
fn test_message_array_expansion_with_bedrock_text() {
    use crate::observations::{MessageSource, RawMessage};

    // Test that Bedrock format with "text" is recognized as message-like
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "text": "Hello from Bedrock"}
        ]),
        rendering: false,
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        1,
        "Bedrock format with 'text' should be recognized"
    );
}
