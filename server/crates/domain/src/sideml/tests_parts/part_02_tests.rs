
#[test]
fn test_id_field_not_extracted_for_assistant_role() {
    let input = json!({
        "role": "assistant",
        "id": "msg_123",
        "content": "Hello!"
    });
    let output = normalize(&input);
    assert!(output.tool_use_id.is_none());
}

#[test]
fn test_tool_use_id_field_extracted_for_any_role() {
    let input = json!({
        "role": "assistant",
        "tool_call_id": "call_123",
        "content": "Result from tool"
    });
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_123");
}

// === Structured Output Tests ===

#[test]
fn test_openai_structured_output_json() {
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "output_json",
            "json": {
                "name": "Sergey",
                "role": "Senior Solutions Architect",
                "years_experience": 12
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(block_to_json(&output.content[0])["data"]["name"], "Sergey");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["role"],
        "Senior Solutions Architect"
    );
    assert_eq!(
        block_to_json(&output.content[0])["data"]["years_experience"],
        12
    );
}

#[test]
fn test_openai_json_object_type() {
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "json_object",
            "json": {"key": "value", "count": 42}
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(block_to_json(&output.content[0])["data"]["key"], "value");
    assert_eq!(block_to_json(&output.content[0])["data"]["count"], 42);
}

#[test]
fn test_strands_structured_output_pydantic() {
    // Strands structured_output() returns raw Pydantic model JSON
    // This should be recognized as structured output (json type), not unknown
    let input = json!({
        "role": "assistant",
        "content": [{
            "name": "Jane Doe",
            "age": 28,
            "address": {
                "street": "123 Main St",
                "city": "New York",
                "country": "USA"
            },
            "contacts": [{"email": "jane@example.com", "phone": null}],
            "skills": ["systems admin"]
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["name"],
        "Jane Doe"
    );
    assert_eq!(block_to_json(&output.content[0])["data"]["age"], 28);
    assert_eq!(
        block_to_json(&output.content[0])["data"]["address"]["city"],
        "New York"
    );
}

#[test]
fn test_strands_structured_output_as_message_string() {
    // Strands gen_ai.choice events have the message as a JSON string that gets parsed
    // When the content is a single JSON object (not array), it's structured output
    let input = json!({
        "role": "assistant",
        "message": {"result": 42, "status": "success"}
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(block_to_json(&output.content[0])["data"]["result"], 42);
    assert_eq!(
        block_to_json(&output.content[0])["data"]["status"],
        "success"
    );
}

// === Content Type Detection Edge Cases ===

#[test]
fn test_user_type_field_treated_as_unknown() {
    // If structured data has a "type" field (user-defined, like "type": "person"),
    // it's treated as unknown because we can't distinguish it from a malformed content block
    let input = json!({
        "role": "assistant",
        "content": [{"type": "person", "name": "Jane", "age": 28}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "unknown");
    // Raw data is preserved
    assert_eq!(block_to_json(&output.content[0])["raw"]["type"], "person");
    assert_eq!(block_to_json(&output.content[0])["raw"]["name"], "Jane");
}

#[test]
fn test_non_string_type_field_treated_as_json() {
    // If "type" field exists but isn't a string, treat as structured output
    let input = json!({
        "role": "assistant",
        "content": [{"type": 123, "data": "value"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(block_to_json(&output.content[0])["data"]["type"], 123);
}

#[test]
fn test_empty_object_is_structured_output() {
    // Empty object is valid structured output (empty result)
    let input = json!({
        "role": "assistant",
        "content": [{}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(block_to_json(&output.content[0])["data"], json!({}));
}

#[test]
fn test_text_field_with_object_value_is_unknown() {
    // Object with "text" field containing an object (not string) - malformed Bedrock
    // Conservative approach: flag as unknown
    let input = json!({
        "role": "assistant",
        "content": [{"text": {"nested": "data"}}]
    });
    let output = normalize(&input);
    // Has provider field "text" but wrong type - unknown (conservative)
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "unknown");
    assert_eq!(block["raw"]["text"]["nested"], "data");
}

#[test]
fn test_malformed_tool_use_is_unknown() {
    // Object with "toolUse" field but malformed structure is unknown
    let input = json!({
        "role": "assistant",
        "content": [{"toolUse": "not an object"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "unknown");
}

#[test]
fn test_assistant_with_tool_call_id_not_converted_to_tool_result() {
    // Assistant role messages should NOT convert to tool_result even with tool_call_id
    // (tool_call_id on assistant can mean this is a tool input/invocation, not a tool result)
    let input = json!({
        "role": "assistant",
        "tool_call_id": "call_123",
        "content": [{"weather": "sunny", "temp": 72}]
    });
    let output = normalize(&input);
    // Should remain as json block, not convert to tool_result
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["weather"],
        "sunny"
    );
}

#[test]
fn test_assistant_with_tool_use_id_not_converted_to_tool_result() {
    // Assistant role messages should NOT convert to tool_result even with tool_use_id
    // (tool_use_id on assistant can mean this is a tool input/invocation, not a tool result)
    let input = json!({
        "role": "assistant",
        "tool_use_id": "toolu_456",
        "content": [{"weather": "rainy", "temp": 55}]
    });
    let output = normalize(&input);
    // Should remain as json block, not convert to tool_result
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["weather"],
        "rainy"
    );
}

#[test]
fn test_json_converted_to_tool_result_for_tool_role() {
    // Structured output (json type) should convert to tool_result for tool role
    let input = json!({
        "role": "tool",
        "name": "get_weather",
        "content": [{"weather": "sunny", "temp": 72}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["content"]["weather"],
        "sunny"
    );
}

#[test]
fn test_complex_nested_structured_output() {
    // Complex nested structures should be preserved as json
    let input = json!({
        "role": "assistant",
        "content": [{
            "users": [
                {"id": 1, "name": "Alice", "roles": ["admin", "user"]},
                {"id": 2, "name": "Bob", "roles": ["user"]}
            ],
            "metadata": {
                "total": 2,
                "page": 1,
                "filters": {"active": true}
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["users"][0]["name"],
        "Alice"
    );
    assert_eq!(
        block_to_json(&output.content[0])["data"]["metadata"]["total"],
        2
    );
}

#[test]
fn test_text_field_with_extra_fields_is_unknown() {
    // Object with "text" field alongside other fields looks like malformed Bedrock
    // Conservative approach: flag as unknown rather than assume structured output
    let input = json!({
        "role": "assistant",
        "content": [{"text": "Document summary", "confidence": 0.95, "word_count": 150}]
    });
    let output = normalize(&input);
    // Has provider field "text" but extra fields - unknown (conservative)
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "unknown");
    // Raw data is preserved for inspection
    assert_eq!(block["raw"]["text"], "Document summary");
    assert_eq!(block["raw"]["confidence"], 0.95);
}

#[test]
fn test_bedrock_text_only_becomes_text_block() {
    // Pure Bedrock text format: exactly {"text": "..."} with nothing else
    let input = json!({
        "role": "assistant",
        "content": [{"text": "Hello world"}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "text");
    assert_eq!(block["text"], "Hello world");
}

// === Tool Message Categorization Tests ===

#[test]
fn test_categorize_tool_calls_as_input() {
    let msg = json!({
        "role": "assistant",
        "tool_calls": [{"id": "call_1", "function": {"name": "search", "arguments": "{}"}}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolInput
    );
}

#[test]
fn test_categorize_tool_use_content_as_input() {
    let msg = json!({
        "content": [{"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {}}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolInput
    );
}

#[test]
fn test_categorize_bedrock_tool_use_as_input() {
    let msg = json!({
        "content": [{"toolUse": {"toolUseId": "123", "name": "weather", "input": {}}}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolInput
    );
}

#[test]
fn test_categorize_gemini_function_call_as_input() {
    let msg = json!({
        "content": [{"functionCall": {"name": "get_weather", "args": {}}}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolInput
    );
}

#[test]
fn test_categorize_tool_result_as_output() {
    let msg = json!({
        "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "sunny"}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolMessage
    );
}

#[test]
fn test_categorize_bedrock_tool_result_as_output() {
    let msg = json!({
        "content": [{"toolResult": {"toolUseId": "123", "content": [{"text": "result"}]}}]
    });
    assert_eq!(
        categorize_tool_message(&msg),
        MessageCategory::GenAIToolMessage
    );
}

// === Message-level Refusal Tests ===

#[test]
fn test_message_level_refusal() {
    let input = json!({
        "role": "assistant",
        "content": "I cannot help with that.",
        "refusal": "This request violates safety guidelines."
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(block_to_json(&output.content[1])["type"], "refusal");
    assert_eq!(
        block_to_json(&output.content[1])["message"],
        "This request violates safety guidelines."
    );
}

#[test]
fn test_empty_refusal_not_added() {
    let input = json!({
        "role": "assistant",
        "content": "Hello!",
        "refusal": ""
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 1);
}

// === Data URL Media Type Extraction Tests ===

#[test]
fn test_data_url_extracts_media_type() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc123"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["media_type"], "image/png");
    assert_eq!(block_to_json(&output.content[0])["data"], "abc123");
}

#[test]
fn test_data_url_jpeg_media_type() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,xyz789"}}]
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0])["media_type"],
        "image/jpeg"
    );
}

#[test]
fn test_regular_url_no_media_type() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "https://example.com/image.png"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["source"], "url");
    assert!(block_to_json(&output.content[0])["media_type"].is_null());
}

// === tool_choice Field Preservation Tests ===

#[test]
fn test_tool_choice_auto_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "tool_choice": "auto"
    });
    let output = normalize(&input);
    assert!(matches!(output.tool_choice, Some(ToolChoice::Auto)));
}

#[test]
fn test_tool_choice_required_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "tool_choice": "required"
    });
    let output = normalize(&input);
    assert!(matches!(output.tool_choice, Some(ToolChoice::Required)));
}

#[test]
fn test_tool_choice_none_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "tool_choice": "none"
    });
    let output = normalize(&input);
    assert!(matches!(output.tool_choice, Some(ToolChoice::None)));
}

#[test]
fn test_tool_choice_specific_function_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "tool_choice": {"type": "function", "function": {"name": "get_weather"}}
    });
    let output = normalize(&input);
    match output.tool_choice.as_ref().unwrap() {
        ToolChoice::Function { name } => assert_eq!(name, "get_weather"),
        _ => panic!("Expected ToolChoice::Function"),
    }
}

// === response_format Field Preservation Tests ===

#[test]
fn test_response_format_json_object_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "response_format": {"type": "json_object"}
    });
    let output = normalize(&input);
    assert!(matches!(
        output.response_format,
        Some(ResponseFormat::JsonObject)
    ));
}

#[test]
fn test_response_format_json_schema_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "person",
                "schema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "age": {"type": "integer"}
                    },
                    "required": ["name", "age"]
                }
            }
        }
    });
    let output = normalize(&input);
    match output.response_format.as_ref().unwrap() {
        ResponseFormat::JsonSchema { json_schema } => {
            assert_eq!(json_schema.name.as_deref(), Some("person"));
            assert_eq!(json_schema.schema.as_ref().unwrap()["type"], "object");
        }
        _ => panic!("Expected ResponseFormat::JsonSchema"),
    }
}

#[test]
fn test_response_format_text_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "response_format": {"type": "text"}
    });
    let output = normalize(&input);
    assert!(matches!(output.response_format, Some(ResponseFormat::Text)));
}

// === Image Detail Field Tests ===

#[test]
fn test_image_detail_field_preserved() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "https://example.com/img.png", "detail": "high"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["detail"], "high");
}

#[test]
fn test_image_detail_auto() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc", "detail": "auto"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["detail"], "auto");
}

#[test]
fn test_image_detail_low() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "https://example.com/img.png", "detail": "low"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["detail"], "low");
}

#[test]
fn test_image_without_detail() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "https://example.com/img.png"}}]
    });
    let output = normalize(&input);
    // ContentBlock::Image doesn't have a detail field - it's normalized away
    assert!(block_to_json(&output.content[0]).get("detail").is_none());
}

// === Model Field Preservation Tests ===

#[test]
fn test_model_field_preserved() {
    let input = json!({
        "role": "assistant",
        "content": "Hello",
        "model": "gpt-4o"
    });
    let output = normalize(&input);
    assert_eq!(output.model.as_ref().unwrap(), "gpt-4o");
}

// === Cache Control Tests (Anthropic) ===

#[test]
fn test_cache_control_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "cache_control": {"type": "ephemeral"}
    });
    let output = normalize(&input);
    assert_eq!(
        output.cache_control.as_ref().unwrap().cache_type,
        "ephemeral"
    );
}

// === Stop Sequences Tests ===

#[test]
fn test_stop_field_preserved() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "stop": ["END", "STOP"]
    });
    let output = normalize(&input);
    assert_eq!(output.stop.as_ref().unwrap()[0], "END");
    assert_eq!(output.stop.as_ref().unwrap()[1], "STOP");
}

#[test]
fn test_stop_sequences_normalized_to_stop() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "stop_sequences": ["\\n\\nHuman:"]
    });
    let output = normalize(&input);
    assert_eq!(output.stop.as_ref().unwrap()[0], "\\n\\nHuman:");
}

// === Parallel Tool Calls Tests ===

#[test]
fn test_parallel_tool_calls_true() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "parallel_tool_calls": true
    });
    let output = normalize(&input);
    assert!(output.parallel_tool_calls.unwrap());
}

#[test]
fn test_parallel_tool_calls_false() {
    let input = json!({
        "role": "user",
        "content": "Hello",
        "parallel_tool_calls": false
    });
    let output = normalize(&input);
    assert!(!output.parallel_tool_calls.unwrap());
}

// === Refusal Content Block Tests ===

#[test]
fn test_refusal_block_with_message_field() {
    let input = json!({
        "role": "assistant",
        "content": [{"type": "refusal", "message": "I cannot do that"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "refusal");
    assert_eq!(
        block_to_json(&output.content[0])["message"],
        "I cannot do that"
    );
}

#[test]
fn test_refusal_block_with_refusal_field() {
    let input = json!({
        "role": "assistant",
        "content": [{"type": "refusal", "refusal": "Safety violation"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "refusal");
    assert_eq!(
        block_to_json(&output.content[0])["message"],
        "Safety violation"
    );
}

// ============================================================================
// STRANDS AGENTS / BEDROCK INTEGRATION TESTS
// ============================================================================

#[test]
fn test_strands_raw_user_message_with_text_array() {
    // Literal content from gen_ai.user.message event (no metadata)
    let input = json!({
        "content": [{"text": "Provide a 3-day weather forecast for NYC"}],
        "role": "user"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "user");
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Provide a 3-day weather forecast for NYC"
    );
}

#[test]
fn test_strands_raw_choice_with_tool_use() {
    // Literal content from gen_ai.choice event (no metadata)
    let input = json!({
        "message": [{"toolUse": {"toolUseId": "tooluse_abc123", "name": "weather_forecast", "input": {"city": "NYC", "days": 3}}}],
        "finish_reason": "tool_use"
    });
    let output = normalize(&input);
    assert_eq!(output.finish_reason.unwrap().as_str(), "tool_use");
}

#[test]
fn test_strands_raw_tool_result_content() {
    // Literal content from gen_ai.tool.message event (no metadata)
    let input = json!({
        "content": [{"toolResult": {"toolUseId": "tooluse_abc123", "status": "success", "content": [{"text": "Weather: sunny"}]}}],
        "role": "tool"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["tool_use_id"],
        "tooluse_abc123"
    );
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "tooluse_abc123");
}

#[test]
fn test_strands_raw_assistant_with_tool_use_content() {
    // Literal content from gen_ai.assistant.message event (no metadata)
    let input = json!({
        "content": [{"toolUse": {"toolUseId": "tooluse_xyz789", "name": "greeting", "input": {"name": "User"}}}]
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "user");
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[0])["id"], "tooluse_xyz789");
    assert_eq!(block_to_json(&output.content[0])["name"], "greeting");
    assert_eq!(block_to_json(&output.content[0])["input"]["name"], "User");
}

#[test]
fn test_strands_tool_message_with_id_attribute() {
    let input = json!({
        "role": "tool",
        "content": {"city": "NYC", "days": 3},
        "id": "tooluse_abc123"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "tooluse_abc123");
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
}

#[test]
fn test_strands_choice_tool_result_text_format() {
    // Strands gen_ai.choice event with tool result as text (from "message" field)
    // Raw: {"message":[{"text":"Weather: sunny"}],"role":"tool","tool_call_id":"tooluse_abc123"}
    let input = json!({
        "message": [{"text": "Weather forecast for Los Angeles: sunny"}],
        "role": "tool",
        "tool_call_id": "tooluse_abc123"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "tooluse_abc123");
    // Text should be converted to tool_result when role=tool and tool_use_id present
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["content"],
        "Weather forecast for Los Angeles: sunny"
    );
}

#[test]
fn test_strands_choice_does_not_add_tool_result() {
    // Strands gen_ai.choice events include "tool.result" attribute with FULL Bedrock format.
    // However, SideML normalize() does NOT add it to assistant messages because:
    // 1. tool_result should be in tool messages, not assistant messages
    // 2. tool.result is handled at extraction level to create separate tool message
    let input = json!({
        "message": [{"toolUse": {"toolUseId": "tooluse_abc123", "name": "weather_forecast", "input": {"city": "LA"}}}],
        "tool.result": [{"toolResult": {"toolUseId": "tooluse_abc123", "status": "success", "content": [{"text": "Weather: sunny"}]}}],
        "finish_reason": "tool_use"
    });
    let output = normalize(&input);

    // Should have only tool_use (tool_result is NOT added to assistant messages)
    assert_eq!(output.content.len(), 1);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(
        block_to_json(&output.content[0])["name"],
        "weather_forecast"
    );
}

#[test]
fn test_strands_tool_result_rich_format_from_content() {
    // A toolResult takes the canonical content form a tool message's content takes: a lone text is its
    // string, and only a result of several blocks stays a list, so nothing it returned is lost.
    let normalized = |content: serde_json::Value| {
        let output = normalize(&json!({
            "role": "tool",
            "tool_call_id": "tooluse_abc123",
            "content": [{"toolResult": {"toolUseId": "tooluse_abc123", "status": "success", "content": content}}]
        }));
        assert_eq!(output.content.len(), 1);
        let block = block_to_json(&output.content[0]);
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "tooluse_abc123");
        block["content"].clone()
    };
    assert_eq!(normalized(json!([{"text": "Weather: sunny"}])), json!("Weather: sunny"));
    let rich = normalized(json!([{"text": "Weather: sunny"}, {"text": "Wind: light"}]));
    assert!(rich.is_array(), "several blocks stay a list, got: {rich}");
    assert_eq!(rich[0]["text"], "Weather: sunny");
}

// ============================================================================
// AUTOGEN INTEGRATION TESTS
// ============================================================================

#[test]
fn test_autogen_raw_message_format() {
    let input = json!({
        "role": "user",
        "content": "Provide a 3-day weather forecast for NYC"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "user");
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Provide a 3-day weather forecast for NYC"
    );
}

#[test]
fn test_autogen_tool_call_message() {
    let input = json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "id": "call_abc123",
            "type": "function",
            "function": {
                "name": "get_weather",
                "arguments": "{\"city\":\"NYC\"}"
            }
        }]
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "assistant");
    let tool_use = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .unwrap();
    if let ContentBlock::ToolUse { name, input, .. } = tool_use {
        assert_eq!(name, "get_weather");
        assert_eq!(input["city"], "NYC");
    }
}

// ============================================================================
// CREWAI INTEGRATION TESTS
// ============================================================================

#[test]
fn test_crewai_task_output_format() {
    let input = json!({
        "role": "assistant",
        "content": "The weather forecast for NYC is sunny with highs of 75F.",
        "finish_reason": "stop"
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "assistant");
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "The weather forecast for NYC is sunny with highs of 75F."
    );
    assert_eq!(output.finish_reason.unwrap().as_str(), "stop");
}
