//! Tests for SideML normalization

use super::*;
use chrono::{TimeZone, Utc};
use serde_json::json;

use crate::data::types::{MessageCategory, MessageSourceType};
use crate::domain::traces::{MessageSource, RawMessage};
use normalize::categorize_tool_message;

/// Helper to convert ContentBlock to JsonValue for testing
fn block_to_json(block: &ContentBlock) -> JsonValue {
    serde_json::to_value(block).unwrap()
}

// === ChatRole Tests ===

#[test]
fn test_chat_role_normalization() {
    assert_eq!(ChatRole::from_str_normalized("system"), ChatRole::System);
    assert_eq!(ChatRole::from_str_normalized("SYSTEM"), ChatRole::System);
    assert_eq!(ChatRole::from_str_normalized("user"), ChatRole::User);
    assert_eq!(ChatRole::from_str_normalized("human"), ChatRole::User);
    assert_eq!(
        ChatRole::from_str_normalized("assistant"),
        ChatRole::Assistant
    );
    assert_eq!(ChatRole::from_str_normalized("ai"), ChatRole::Assistant);
    assert_eq!(ChatRole::from_str_normalized("model"), ChatRole::Assistant);
    assert_eq!(ChatRole::from_str_normalized("tool"), ChatRole::Tool);
    assert_eq!(ChatRole::from_str_normalized("function"), ChatRole::Tool);
    assert_eq!(ChatRole::from_str_normalized("unknown"), ChatRole::User);
}

#[test]
fn test_chat_role_as_str() {
    assert_eq!(ChatRole::System.as_str(), "system");
    assert_eq!(ChatRole::User.as_str(), "user");
    assert_eq!(ChatRole::Assistant.as_str(), "assistant");
    assert_eq!(ChatRole::Tool.as_str(), "tool");
}

#[test]
fn test_is_tool_role() {
    assert!(ChatRole::is_tool_role("tool"));
    assert!(ChatRole::is_tool_role("Tool"));
    assert!(ChatRole::is_tool_role("TOOL"));
    assert!(ChatRole::is_tool_role("function"));
    assert!(ChatRole::is_tool_role("Function"));
    assert!(!ChatRole::is_tool_role("user"));
    assert!(!ChatRole::is_tool_role("assistant"));
    assert!(!ChatRole::is_tool_role("system"));
    assert!(!ChatRole::is_tool_role("unknown"));
}

// === FinishReason Tests ===

#[test]
fn test_finish_reason_normalization() {
    assert_eq!(
        FinishReason::from_str_normalized("stop"),
        Some(FinishReason::Stop)
    );
    assert_eq!(
        FinishReason::from_str_normalized("end_turn"),
        Some(FinishReason::Stop)
    );
    assert_eq!(
        FinishReason::from_str_normalized("STOP"),
        Some(FinishReason::Stop)
    );
    assert_eq!(
        FinishReason::from_str_normalized("length"),
        Some(FinishReason::Length)
    );
    assert_eq!(
        FinishReason::from_str_normalized("max_tokens"),
        Some(FinishReason::Length)
    );
    assert_eq!(
        FinishReason::from_str_normalized("tool_calls"),
        Some(FinishReason::ToolUse)
    );
    assert_eq!(
        FinishReason::from_str_normalized("tool-calls"), // Vercel AI SDK
        Some(FinishReason::ToolUse)
    );
    assert_eq!(
        FinishReason::from_str_normalized("tool_use"),
        Some(FinishReason::ToolUse)
    );
    assert_eq!(
        FinishReason::from_str_normalized("content_filter"),
        Some(FinishReason::ContentFilter)
    );
    assert_eq!(
        FinishReason::from_str_normalized("safety"),
        Some(FinishReason::ContentFilter)
    );
    assert_eq!(FinishReason::from_str_normalized("unknown"), None);
}

#[test]
fn test_finish_reason_as_str() {
    assert_eq!(FinishReason::Stop.as_str(), "stop");
    assert_eq!(FinishReason::Length.as_str(), "length");
    assert_eq!(FinishReason::ToolUse.as_str(), "tool_use");
    assert_eq!(FinishReason::ContentFilter.as_str(), "content_filter");
}

/// Regression test: Vercel AI SDK uses camelCase `finishReason` instead of snake_case
#[test]
fn test_finish_reason_camel_case_vercel_ai() {
    // Vercel AI SDK format with camelCase finishReason
    let input = json!({
        "role": "assistant",
        "content": "Hello, world!",
        "finishReason": "stop"
    });
    let output = normalize(&input);
    assert_eq!(output.finish_reason, Some(FinishReason::Stop));

    // Also test with tool_use value
    let input = json!({
        "role": "assistant",
        "content": "Using a tool",
        "finishReason": "tool-calls"
    });
    let output = normalize(&input);
    assert_eq!(output.finish_reason, Some(FinishReason::ToolUse));

    // Snake_case should still work
    let input = json!({
        "role": "assistant",
        "content": "Done",
        "finish_reason": "stop"
    });
    let output = normalize(&input);
    assert_eq!(output.finish_reason, Some(FinishReason::Stop));
}

// === ContentBlock Serialization Tests ===

#[test]
fn test_content_block_text_serialization() {
    let block = ContentBlock::Text {
        text: "Hello".to_string(),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "text");
    assert_eq!(json["text"], "Hello");
}

#[test]
fn test_content_block_image_serialization() {
    let block = ContentBlock::Image {
        media_type: Some("image/jpeg".to_string()),
        source: "base64".to_string(),
        data: "abc123".to_string(),
        detail: None,
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "image");
    assert_eq!(json["media_type"], "image/jpeg");
    assert_eq!(json["source"], "base64");
}

#[test]
fn test_content_block_tool_use_serialization() {
    let block = ContentBlock::ToolUse {
        id: Some("tool_1".to_string()),
        name: "get_weather".to_string(),
        input: json!({"city": "NYC"}),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "tool_use");
    assert_eq!(json["id"], "tool_1");
    assert_eq!(json["name"], "get_weather");
    assert_eq!(json["input"]["city"], "NYC");
}

#[test]
fn test_content_block_refusal_serialization() {
    let block = ContentBlock::Refusal {
        message: "I cannot help with that.".to_string(),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "refusal");
    assert_eq!(json["message"], "I cannot help with that.");
}

#[test]
fn test_content_block_json_serialization() {
    let block = ContentBlock::Json {
        data: json!({"name": "Sergey", "role": "Architect"}),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "json");
    assert_eq!(json["data"]["name"], "Sergey");
    assert_eq!(json["data"]["role"], "Architect");
}

#[test]
fn test_content_block_thinking_serialization() {
    let block = ContentBlock::Thinking {
        text: "Let me think about this...".to_string(),
        signature: Some("sig123".to_string()),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "thinking");
    assert_eq!(json["text"], "Let me think about this...");
    assert_eq!(json["signature"], "sig123");
}

#[test]
fn test_content_block_redacted_thinking_serialization() {
    let block = ContentBlock::RedactedThinking {
        data: "redacted_data_123".to_string(),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "redacted_thinking");
    assert_eq!(json["data"], "redacted_data_123");
}

// === ChatMessage Tests ===

#[test]
fn test_chat_message_builder() {
    let msg = ChatMessage::new(ChatRole::Assistant)
        .with_text("Hello!")
        .with_finish_reason(FinishReason::Stop);

    assert_eq!(msg.role, ChatRole::Assistant);
    assert_eq!(msg.content.len(), 1);
    assert_eq!(block_to_json(&msg.content[0])["type"], "text");
    assert_eq!(block_to_json(&msg.content[0])["text"], "Hello!");
    assert_eq!(msg.finish_reason, Some(FinishReason::Stop));
}

// === Text Content Tests ===

#[test]
fn test_string_content_to_array() {
    let input = json!({"role": "user", "content": "Hello"});
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(block_to_json(&output.content[0])["text"], "Hello");
}

#[test]
fn test_openai_text_block() {
    let input = json!({"role": "user", "content": [{"type": "text", "text": "Hello"}]});
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(block_to_json(&output.content[0])["text"], "Hello");
}

#[test]
fn test_bedrock_text_block() {
    let input = json!({"role": "user", "content": [{"text": "Hello"}]});
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "text");
    assert_eq!(block_to_json(&output.content[0])["text"], "Hello");
}

// === Image Content Tests ===

#[test]
fn test_openai_image_url() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "https://example.com/img.png"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "url");
    assert_eq!(
        block_to_json(&output.content[0])["data"],
        "https://example.com/img.png"
    );
}

#[test]
fn test_openai_data_url_image() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc123"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["data"], "abc123");
}

#[test]
fn test_anthropic_image() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["media_type"], "image/png");
}

#[test]
fn test_anthropic_document() {
    let input = json!({
        "role": "user",
        "content": [{"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "pdfdata"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "document");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(
        block_to_json(&output.content[0])["media_type"],
        "application/pdf"
    );
    assert_eq!(block_to_json(&output.content[0])["data"], "pdfdata");
}

#[test]
fn test_anthropic_image_url() {
    let input = json!({
        "role": "user",
        "content": [{"type": "image", "source": {"type": "url", "url": "https://example.com/img.png"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "url");
    assert_eq!(
        block_to_json(&output.content[0])["data"],
        "https://example.com/img.png"
    );
}

#[test]
fn test_gemini_inline_data() {
    let input = json!({
        "role": "user",
        "content": [{"inline_data": {"mime_type": "image/jpeg", "data": "xyz"}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(
        block_to_json(&output.content[0])["media_type"],
        "image/jpeg"
    );
}

#[test]
fn test_bedrock_image() {
    let input = json!({
        "role": "user",
        "content": [{"image": {"format": "png", "source": {"bytes": "base64data"}}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "image");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["media_type"], "image/png");
}

#[test]
fn test_bedrock_video() {
    let input = json!({
        "role": "user",
        "content": [{"video": {"format": "mp4", "source": {"bytes": "videobytes"}}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "video");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["data"], "videobytes");
    assert_eq!(block_to_json(&output.content[0])["media_type"], "video/mp4");
}

#[test]
fn test_bedrock_document() {
    let input = json!({
        "role": "user",
        "content": [{"document": {"format": "pdf", "name": "report.pdf", "source": {"bytes": "docbytes"}}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "document");
    assert_eq!(block_to_json(&output.content[0])["source"], "base64");
    assert_eq!(block_to_json(&output.content[0])["data"], "docbytes");
    assert_eq!(
        block_to_json(&output.content[0])["media_type"],
        "application/pdf"
    );
    assert_eq!(block_to_json(&output.content[0])["name"], "report.pdf");
}

// === Tool Use Tests ===

#[test]
fn test_bedrock_tool_use() {
    let input = json!({
        "role": "assistant",
        "content": [{"toolUse": {"toolUseId": "123", "name": "weather", "input": {"city": "NYC"}}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[0])["id"], "123");
    assert_eq!(block_to_json(&output.content[0])["name"], "weather");
}

#[test]
fn test_anthropic_tool_use() {
    let input = json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[0])["id"], "toolu_1");
    assert_eq!(block_to_json(&output.content[0])["name"], "search");
}

#[test]
fn test_gemini_function_call() {
    let input = json!({
        "role": "assistant",
        "content": [{"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_use");
    assert_eq!(block_to_json(&output.content[0])["name"], "get_weather");
}

#[test]
fn test_gemini_function_response() {
    let input = json!({
        "role": "tool",
        "content": [{"functionResponse": {"name": "get_weather", "response": {"temp": 72, "conditions": "sunny"}}}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    // Gemini generates synthetic tool_use_id from function name + response hash
    let tool_use_id = block["tool_use_id"].as_str().unwrap();
    assert!(
        tool_use_id.starts_with("gemini_get_weather_result_"),
        "Expected synthetic ID starting with 'gemini_get_weather_result_', got: {}",
        tool_use_id
    );
    assert_eq!(block["content"]["temp"], 72);
}

// === Thinking Content Tests ===

#[test]
fn test_thinking_block_normalization() {
    let input = json!({
        "role": "assistant",
        "content": [{"type": "thinking", "text": "Let me analyze...", "signature": "sig_abc"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(
        block_to_json(&output.content[0])["text"],
        "Let me analyze..."
    );
    assert_eq!(block_to_json(&output.content[0])["signature"], "sig_abc");
}

#[test]
fn test_redacted_thinking_block_normalization() {
    let input = json!({
        "role": "assistant",
        "content": [{"type": "redacted_thinking", "data": "redacted_xyz"}]
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0])["type"],
        "redacted_thinking"
    );
    assert_eq!(block_to_json(&output.content[0])["data"], "redacted_xyz");
}

// === Tool Calls Tests (tool_calls -> content[].tool_use) ===

#[test]
fn test_tool_calls_nested_to_content_block() {
    let input = json!({
        "role": "assistant",
        "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "search", "arguments": "{}"}}]
    });
    let output = normalize(&input);
    // Tool calls are converted to content blocks
    let tool_use = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolUse { .. }));
    assert!(tool_use.is_some(), "Should have ToolUse content block");
    if let ContentBlock::ToolUse { name, id, .. } = tool_use.unwrap() {
        assert_eq!(name, "search");
        assert_eq!(id.as_deref(), Some("call_1"));
    }
}

#[test]
fn test_tool_calls_flat_to_content_block() {
    let input = json!({
        "role": "assistant",
        "tool_calls": [{"id": "call_1", "type": "function", "name": "search", "arguments": "{}"}]
    });
    let output = normalize(&input);
    let tool_use = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolUse { .. }));
    assert!(tool_use.is_some(), "Should have ToolUse content block");
    if let ContentBlock::ToolUse { name, .. } = tool_use.unwrap() {
        assert_eq!(name, "search");
    }
}

// === Tool Result Tests ===

#[test]
fn test_bedrock_tool_result() {
    let input = json!({
        "role": "tool",
        "content": [{"toolResult": {"toolUseId": "123", "status": "success", "content": [{"text": "result"}]}}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(block_to_json(&output.content[0])["tool_use_id"], "123");
}

// === Role Normalization Tests ===

#[test]
fn test_role_normalization() {
    assert_eq!(normalize(&json!({"role": "human"})).role.as_str(), "user");
    assert_eq!(normalize(&json!({"role": "ai"})).role.as_str(), "assistant");
    assert_eq!(
        normalize(&json!({"role": "model"})).role.as_str(),
        "assistant"
    );
    assert_eq!(
        normalize(&json!({"role": "function"})).role.as_str(),
        "tool"
    );
}

// === Field Preservation Tests ===

#[test]
fn test_preserves_name_field() {
    let input = json!({"role": "tool", "name": "get_weather", "content": "result"});
    let output = normalize(&input);
    assert_eq!(output.name.as_deref(), Some("get_weather"));
}

#[test]
fn test_preserves_finish_reason() {
    let input = json!({"role": "assistant", "content": "done", "finish_reason": "end_turn"});
    let output = normalize(&input);
    assert_eq!(output.finish_reason.unwrap().as_str(), "stop");
}

#[test]
fn test_preserves_index() {
    let input = json!({"role": "assistant", "content": "text", "index": 0});
    let output = normalize(&input);
    assert_eq!(output.index.unwrap(), 0);
}

// === Tool Use ID Extraction Tests ===

#[test]
fn test_extract_tool_use_id_direct() {
    let input = json!({"role": "tool", "tool_call_id": "abc123", "content": "result"});
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "abc123");
}

#[test]
fn test_extract_tool_use_id_from_bedrock_content() {
    let input = json!({
        "role": "tool",
        "content": [{"toolResult": {"toolUseId": "bedrock_123", "content": [{"text": "result"}]}}]
    });
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "bedrock_123");
}

// === Unknown to Tool Result Conversion Tests ===

#[test]
fn test_unknown_converted_to_tool_result_with_tool_use_id() {
    let input = json!({
        "role": "tool",
        "tool_call_id": "call_123",
        "content": [{"query": "AI agents", "max_results": 3}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(block_to_json(&output.content[0])["tool_use_id"], "call_123");
    assert_eq!(
        block_to_json(&output.content[0])["content"]["query"],
        "AI agents"
    );
    assert_eq!(
        block_to_json(&output.content[0])["content"]["max_results"],
        3
    );
    // is_error defaults to false and is skipped when false
    assert!(block_to_json(&output.content[0])["is_error"].is_null());
}

#[test]
fn test_unknown_not_converted_to_tool_result_for_assistant_with_tool_use_id() {
    // Assistant role should NOT convert to tool_result even with tool_use_id
    // Only tool role messages should have content wrapped in tool_result
    let input = json!({
        "role": "assistant",
        "tool_call_id": "tooluse_abc123",
        "content": [{"query": "AI agents", "results": [{"title": "Result 1"}]}]
    });
    let output = normalize(&input);
    // Should remain as json block, not convert to tool_result
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["query"],
        "AI agents"
    );
}

#[test]
fn test_plain_json_becomes_structured_output_for_user() {
    // Plain JSON objects without type/provider fields are treated as structured output
    let input = json!({
        "role": "user",
        "content": [{"custom_data": "value"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["custom_data"],
        "value"
    );
}

#[test]
fn test_plain_json_becomes_structured_output_for_assistant() {
    // Plain JSON objects without type/provider fields are treated as structured output
    // (e.g., Strands structured_output(), OpenAI json_mode without wrapper)
    let input = json!({
        "role": "assistant",
        "content": [{"custom_data": "value"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "json");
    assert_eq!(
        block_to_json(&output.content[0])["data"]["custom_data"],
        "value"
    );
}

#[test]
fn test_unknown_content_block_preserves_raw_data() {
    // Test that unknown content blocks preserve their original data (fix for data loss bug)
    let input = json!({
        "role": "user",
        "content": [
            {"type": "future_content_type", "data": {"nested": "value"}, "extra_field": 123}
        ]
    });
    let output = normalize(&input);

    assert_eq!(output.content.len(), 1);
    let block_json = block_to_json(&output.content[0]);
    assert_eq!(block_json["type"], "unknown");

    // Verify raw data is preserved
    let raw = &block_json["raw"];
    assert_eq!(raw["type"], "future_content_type");
    assert_eq!(raw["data"]["nested"], "value");
    assert_eq!(raw["extra_field"], 123);
}

#[test]
fn test_unknown_content_block_roundtrip() {
    // Test that unknown blocks can be serialized and deserialized without data loss
    use crate::domain::sideml::ContentBlock;

    let original = json!({
        "type": "experimental_block",
        "payload": {"key": "value"},
        "metadata": [1, 2, 3]
    });

    // Deserialize into ContentBlock
    let block: ContentBlock = serde_json::from_value(original.clone()).unwrap();

    // Should be Unknown variant
    match &block {
        ContentBlock::Unknown { raw } => {
            assert_eq!(raw["type"], "experimental_block");
            assert_eq!(raw["payload"]["key"], "value");
            assert_eq!(raw["metadata"], json!([1, 2, 3]));
        }
        _ => panic!("Expected Unknown variant, got {:?}", block),
    }

    // Serialize back to JSON
    let serialized = serde_json::to_value(&block).unwrap();

    // Should preserve all original data
    assert_eq!(serialized["type"], "unknown");
    assert_eq!(serialized["raw"]["type"], "experimental_block");
    assert_eq!(serialized["raw"]["payload"]["key"], "value");
    assert_eq!(serialized["raw"]["metadata"], json!([1, 2, 3]));
}

#[test]
fn test_nested_arrays_preserved_in_unknown() {
    // Test that nested arrays in content blocks are preserved, not silently dropped
    // This was a bug where arrays would return None from try_unknown_fallback
    let input = json!({
        "role": "user",
        "content": [
            ["nested", "array", "values"],
            [{"complex": "object"}, {"in": "array"}],
            [1, 2, 3]
        ]
    });
    let output = normalize(&input);

    // All three nested arrays should be preserved as unknown blocks
    assert_eq!(
        output.content.len(),
        3,
        "All nested arrays should be preserved"
    );

    // First nested array: ["nested", "array", "values"]
    let block1 = block_to_json(&output.content[0]);
    assert_eq!(block1["type"], "unknown");
    assert_eq!(block1["raw"], json!(["nested", "array", "values"]));

    // Second nested array: [{"complex": "object"}, {"in": "array"}]
    let block2 = block_to_json(&output.content[1]);
    assert_eq!(block2["type"], "unknown");
    assert_eq!(block2["raw"][0]["complex"], "object");
    assert_eq!(block2["raw"][1]["in"], "array");

    // Third nested array: [1, 2, 3]
    let block3 = block_to_json(&output.content[2]);
    assert_eq!(block3["type"], "unknown");
    assert_eq!(block3["raw"], json!([1, 2, 3]));
}

#[test]
fn test_primitive_values_preserved_in_unknown() {
    // Primitive values in content arrays: strings become text blocks,
    // non-string primitives preserved as unknown (not dropped).
    let input = json!({
        "role": "user",
        "content": [
            "just a string",
            42,
            true,
            null
        ]
    });
    let output = normalize(&input);

    // All primitives in array are preserved (not dropped)
    assert_eq!(
        output.content.len(),
        4,
        "All primitives should be preserved"
    );

    // String in array becomes text
    let block1 = block_to_json(&output.content[0]);
    assert_eq!(block1["type"], "text");
    assert_eq!(block1["text"], "just a string");

    // Number preserved as unknown
    let block2 = block_to_json(&output.content[1]);
    assert_eq!(block2["type"], "unknown");
    assert_eq!(block2["raw"], 42);

    // Boolean preserved as unknown
    let block3 = block_to_json(&output.content[2]);
    assert_eq!(block3["type"], "unknown");
    assert_eq!(block3["raw"], true);

    // Null preserved as unknown
    let block4 = block_to_json(&output.content[3]);
    assert_eq!(block4["type"], "unknown");
    assert!(block4["raw"].is_null());
}

#[test]
fn test_unknown_converted_for_tool_role_without_tool_use_id() {
    let input = json!({
        "role": "tool",
        "name": "get_weather",
        "content": [{"city": "NYC", "forecast": "sunny"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(block_to_json(&output.content[0])["content"]["city"], "NYC");
    // is_error defaults to false and is skipped when false
    assert!(block_to_json(&output.content[0])["is_error"].is_null());
    assert!(block_to_json(&output.content[0])["tool_use_id"].is_null());
}

#[test]
fn test_unknown_converted_for_function_role() {
    let input = json!({
        "role": "function",
        "name": "get_weather",
        "content": [{"city": "NYC", "forecast": "sunny"}]
    });
    let output = normalize(&input);
    assert_eq!(output.role.as_str(), "tool");
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(block_to_json(&output.content[0])["content"]["city"], "NYC");
}

#[test]
fn test_tool_result_preserved_not_double_converted() {
    let input = json!({
        "role": "tool",
        "tool_call_id": "call_123",
        "content": [{"type": "tool_result", "tool_use_id": "call_123", "content": {"result": "ok"}, "is_error": false}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(block_to_json(&output.content[0])["content"]["result"], "ok");
}

#[test]
fn test_text_content_preserved_in_tool_message() {
    // Tool messages with text content should be converted to tool_result
    let input = json!({
        "role": "tool",
        "tool_call_id": "call_123",
        "content": [{"type": "text", "text": "The weather is sunny"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["content"],
        "The weather is sunny"
    );
    assert_eq!(block_to_json(&output.content[0])["tool_use_id"], "call_123");
}

// === Centralized tool_use_id extraction tests ===

#[test]
fn test_tool_use_id_extracted_from_id_field() {
    let input = json!({
        "role": "tool",
        "id": "call_abc123",
        "content": "Result data"
    });
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_abc123");
}

#[test]
fn test_tool_use_id_extracted_from_call_id_field() {
    let input = json!({
        "role": "function",
        "call_id": "call_xyz789",
        "content": "Result data"
    });
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "call_xyz789");
}

#[test]
fn test_tool_use_id_priority_order() {
    let input = json!({
        "role": "tool",
        "tool_call_id": "primary_id",
        "id": "secondary_id",
        "call_id": "tertiary_id",
        "content": "Result"
    });
    let output = normalize(&input);
    assert_eq!(output.tool_use_id.as_deref().unwrap(), "primary_id");
}

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
    // When tool message has toolResult in content (from extraction-level tool.result handling),
    // normalize produces tool_result with RICH content (array format).
    let input = json!({
        "role": "tool",
        "tool_call_id": "tooluse_abc123",
        "content": [{"toolResult": {"toolUseId": "tooluse_abc123", "status": "success", "content": [{"text": "Weather: sunny"}]}}]
    });
    let output = normalize(&input);

    assert_eq!(output.content.len(), 1);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    assert_eq!(
        block_to_json(&output.content[0])["tool_use_id"],
        "tooluse_abc123"
    );
    // Content should be the ARRAY format from Bedrock
    let content = &block_to_json(&output.content[0])["content"];
    assert!(
        content.is_array(),
        "content should be array, got: {}",
        content
    );
    assert_eq!(content[0]["text"], "Weather: sunny");
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
    if let ContentBlock::ToolUse { id, name, input } = tool_use.unwrap() {
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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[user_message]);

    assert_eq!(result.len(), 1, "Non-tool messages should not be split");
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

// === Special Role Preservation Tests ===

#[test]
fn test_special_role_tool_call_preserved() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // tool_call normalizes to Assistant (tool invocation by assistant)
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

#[test]
fn test_special_role_tools_preserved() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // tools role normalizes to System with tool definitions
    assert_eq!(result[0].sideml.role, ChatRole::System);
}

#[test]
fn test_special_role_data_preserved() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // data role normalizes to User with Context block
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_special_role_context_preserved() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // context role normalizes to User with Context block
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_standard_role_overridden_by_event() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    // Event-derived role (Assistant) takes precedence over explicit "tool"
    assert_eq!(result[0].sideml.role, ChatRole::Assistant);
}

// === Tool Name Enrichment Tests ===

#[test]
fn test_tool_result_gets_name_from_matching_tool_use() {
    use crate::domain::sideml::to_sideml_with_context;
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[tool_result]);

    assert_eq!(result.len(), 1);
    // No matching tool_use, so name should be None
    assert_eq!(result[0].sideml.name, None);
}

// === Tool Span vs Chat Span Context Tests ===

#[test]
fn test_gen_ai_choice_in_tool_span_becomes_tool_role() {
    use crate::domain::sideml::to_sideml_with_context;
    use crate::domain::traces::{MessageSource, RawMessage};

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Tool output result"}]
        }),
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
    use crate::domain::sideml::to_sideml_with_context;
    use crate::domain::traces::{MessageSource, RawMessage};

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Assistant response"}]
        }),
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
    use crate::domain::sideml::to_sideml_with_context;
    use crate::domain::traces::{MessageSource, RawMessage};

    let message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"text": "Tool result"}]
        }),
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
    use crate::domain::traces::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "message": "Hello, I can help you!",
            "finish_reason": "end_turn"
        }),
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
    use crate::domain::traces::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.user.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": "What's the weather?"
        }),
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::User);
}

#[test]
fn test_to_sideml_derives_tool_role_from_tool_message_event() {
    use crate::domain::traces::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.message".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": "Weather is sunny",
            "id": "tool123"
        }),
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].sideml.role, ChatRole::Tool);
}

#[test]
fn test_to_sideml_event_derived_role_takes_precedence() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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

#[test]
fn test_unflatten_multiple_tool_calls() {
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
        ContentBlock::ToolUse { id, name, input } => {
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
        ContentBlock::ToolUse { id, name, input } => {
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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].category, MessageCategory::GenAIToolInput);
}

#[test]
fn test_tool_call_role_from_event_gets_tool_input_category() {
    // Event source with role="tool_call" (from tool span extraction) should get GenAIToolInput
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

    let raw_message = RawMessage {
        source: MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "role": "tools",
            "content": [{"name": "search"}]
        }),
    };

    let result = to_sideml(&[raw_message]);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].category, MessageCategory::GenAIToolDefinitions);
}

/// End-to-end pipeline test: tool span messages with correct roles and categories
#[test]
fn test_pipeline_tool_span_messages_end_to_end() {
    use crate::domain::sideml::to_sideml_with_context;

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
        block_to_json(&output.content[0])["signature"],
        "ErYhCkgICxABGAI..."
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
    assert_eq!(block_to_json(&output.content[0])["signature"], "sig_abc123");
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
fn test_mistral_empty_thinking_array_fallback() {
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "thinking",
            "thinking": []
        }]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "thinking");
    assert_eq!(block_to_json(&output.content[0])["text"], "");
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

#[test]
fn test_convert_keeps_multiple_tool_results_unchanged() {
    // Multiple tool_results should be kept as-is (parallel tool calls)
    let input = json!({
        "role": "tool",
        "content": [
            {"type": "tool_result", "tool_use_id": "A", "content": "Result A"},
            {"type": "tool_result", "tool_use_id": "B", "content": "Result B"}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);
    assert_eq!(block_to_json(&output.content[0])["tool_use_id"], "A");
    assert_eq!(block_to_json(&output.content[1])["tool_use_id"], "B");
}

#[test]
fn test_single_text_becomes_string_content() {
    // Single text block in tool role should become string content
    let input = json!({
        "role": "tool",
        "tool_call_id": "id",
        "content": [{"type": "text", "text": "Simple result"}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["type"], "tool_result");
    // Content should be string, not array
    assert_eq!(
        block_to_json(&output.content[0])["content"],
        "Simple result"
    );
}

#[test]
fn test_anthropic_tool_result_content_normalized() {
    // Anthropic tool_result with array content should normalize inner blocks
    let input = json!({
        "role": "tool",
        "content": [{
            "type": "tool_result",
            "tool_use_id": "toolu_123",
            "content": [
                {"type": "text", "text": "Found 5 results"},
                {"type": "image", "source": {"type": "base64", "data": "xyz"}, "media_type": "image/png"}
            ]
        }]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    let content = block["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
}

#[test]
fn test_gemini_function_response_content_normalized() {
    // Gemini functionResponse content should be normalized to SideML format
    let input = json!({
        "role": "tool",
        "content": [{
            "functionResponse": {
                "name": "search",
                "response": [
                    {"text": "Search complete"},
                    {"inline_data": {"mime_type": "image/png", "data": "base64data"}}
                ]
            }
        }]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    // Inner content should be normalized to SideML format
    let content = block["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Search complete");
    assert_eq!(content[1]["type"], "image");
}

// === Gemini Synthetic ID Tests ===
// Gemini doesn't provide tool call IDs, so we generate synthetic IDs to prevent
// deduplication collisions when the same function is called multiple times.

#[test]
fn test_gemini_function_call_synthetic_id() {
    // Verify Gemini function calls get synthetic IDs based on name + args hash
    let input = json!({
        "role": "assistant",
        "content": [{"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_use");
    assert_eq!(block["name"], "get_weather");
    // Synthetic ID should be gemini_{name}_call_{hash}
    let id = block["id"].as_str().unwrap();
    assert!(
        id.starts_with("gemini_get_weather_call_"),
        "Expected synthetic ID starting with 'gemini_get_weather_call_', got: {}",
        id
    );
}

#[test]
fn test_gemini_multiple_function_calls_unique_ids() {
    // Multiple calls to the same function with different args should get unique IDs
    let input = json!({
        "role": "assistant",
        "content": [
            {"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}},
            {"functionCall": {"name": "get_weather", "args": {"city": "LA"}}}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);

    let block1 = block_to_json(&output.content[0]);
    let block2 = block_to_json(&output.content[1]);

    let id1 = block1["id"].as_str().unwrap();
    let id2 = block2["id"].as_str().unwrap();

    // Both should be synthetic IDs for get_weather
    assert!(id1.starts_with("gemini_get_weather_call_"));
    assert!(id2.starts_with("gemini_get_weather_call_"));

    // But they should be DIFFERENT (different args hash)
    assert_ne!(
        id1, id2,
        "Multiple calls with different args should have different IDs"
    );
}

#[test]
fn test_gemini_multiple_function_responses_unique_ids() {
    // Multiple responses from the same function with different results should get unique IDs
    let input = json!({
        "role": "tool",
        "content": [
            {"functionResponse": {"name": "get_weather", "response": {"temp": 72, "city": "NYC"}}},
            {"functionResponse": {"name": "get_weather", "response": {"temp": 85, "city": "LA"}}}
        ]
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 2);

    let block1 = block_to_json(&output.content[0]);
    let block2 = block_to_json(&output.content[1]);

    let id1 = block1["tool_use_id"].as_str().unwrap();
    let id2 = block2["tool_use_id"].as_str().unwrap();

    // Both should be synthetic IDs for get_weather
    assert!(id1.starts_with("gemini_get_weather_result_"));
    assert!(id2.starts_with("gemini_get_weather_result_"));

    // But they should be DIFFERENT (different response hash)
    assert_ne!(
        id1, id2,
        "Multiple responses with different data should have different IDs"
    );
}

#[test]
fn test_gemini_identical_calls_same_id() {
    // Identical calls (same name + args) should get the same ID (for dedup)
    let input1 = json!({
        "role": "assistant",
        "content": [{"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}}]
    });
    let input2 = json!({
        "role": "assistant",
        "content": [{"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}}]
    });
    let output1 = normalize(&input1);
    let output2 = normalize(&input2);

    let block1 = block_to_json(&output1.content[0]);
    let block2 = block_to_json(&output2.content[0]);
    let id1 = block1["id"].as_str().unwrap();
    let id2 = block2["id"].as_str().unwrap();

    assert_eq!(
        id1, id2,
        "Identical calls should have the same synthetic ID"
    );
}

#[test]
fn test_gemini_function_call_empty_args() {
    // Function call with no args should still get a valid synthetic ID
    let input = json!({
        "role": "assistant",
        "content": [{"functionCall": {"name": "list_items"}}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    let id = block["id"].as_str().unwrap();
    assert!(
        id.starts_with("gemini_list_items_call_"),
        "Expected synthetic ID for empty args, got: {}",
        id
    );
}

#[test]
fn test_gemini_function_response_null_response() {
    // Function response with null response should still get a valid synthetic ID
    let input = json!({
        "role": "tool",
        "content": [{"functionResponse": {"name": "delete_item", "response": null}}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    let id = block["tool_use_id"].as_str().unwrap();
    assert!(
        id.starts_with("gemini_delete_item_result_"),
        "Expected synthetic ID for null response, got: {}",
        id
    );
}

#[test]
fn test_tool_span_raw_bedrock_multimodal_wrapped() {
    // Tool span gen_ai.choice flow: raw Bedrock format → normalize → wrap in tool_result
    // This is the key scenario from the plan's flow trace
    let input = json!({
        "role": "tool",
        "tool_call_id": "tool_123",
        "content": [
            {"text": "Image generated successfully"},
            {"image": {"format": "jpeg", "source": {"bytes": "abc123base64data"}}}
        ]
    });
    let output = normalize(&input);
    // Should have single tool_result wrapping both normalized blocks
    assert_eq!(output.content.len(), 1);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    assert_eq!(block["tool_use_id"], "tool_123");
    // Inner content is normalized to SideML format (has "type" field)
    let content = block["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Image generated successfully");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["media_type"], "image/jpeg");
}

#[test]
fn test_single_bedrock_object_in_tool_result_normalized() {
    // Edge case: single Bedrock-format object (not array) should be normalized
    // This tests the fix for provider-format objects passed as tool result content
    let input = json!({
        "role": "tool",
        "content": [{
            "toolResult": {
                "toolUseId": "tool_123",
                "content": {"text": "Single text block"}  // Object, not array
            }
        }]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    // Should be normalized to SideML format
    assert_eq!(block["content"]["type"], "text");
    assert_eq!(block["content"]["text"], "Single text block");
}

#[test]
fn test_structured_object_in_tool_result_preserved() {
    // Structured data objects should NOT be normalized (no provider format match)
    let input = json!({
        "role": "tool",
        "content": [{
            "functionResponse": {
                "name": "get_weather",
                "response": {"temp": 72, "conditions": "sunny"}  // Structured data
            }
        }]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    // Should be kept as-is (no "type" field added)
    assert_eq!(block["content"]["temp"], 72);
    assert_eq!(block["content"]["conditions"], "sunny");
}

#[test]
fn test_llm_span_and_tool_span_produce_same_sideml_format() {
    // Both paths should produce identical SideML format for tool result content

    // LLM span tool.result path (Bedrock toolResult wrapper)
    let llm_span_input = json!({
        "role": "tool",
        "content": [{
            "toolResult": {
                "toolUseId": "tool_xyz",
                "content": [
                    {"text": "Result text"},
                    {"image": {"format": "png", "source": {"bytes": "imgdata"}}}
                ]
            }
        }]
    });

    // Tool span gen_ai.choice path (raw Bedrock blocks)
    let tool_span_input = json!({
        "role": "tool",
        "tool_call_id": "tool_xyz",
        "content": [
            {"text": "Result text"},
            {"image": {"format": "png", "source": {"bytes": "imgdata"}}}
        ]
    });

    let llm_output = normalize(&llm_span_input);
    let tool_output = normalize(&tool_span_input);

    // Both should produce single tool_result
    assert_eq!(llm_output.content.len(), 1);
    assert_eq!(tool_output.content.len(), 1);

    let llm_block = block_to_json(&llm_output.content[0]);
    let tool_block = block_to_json(&tool_output.content[0]);

    // Both should have tool_result type
    assert_eq!(llm_block["type"], "tool_result");
    assert_eq!(tool_block["type"], "tool_result");

    // Both should have same tool_use_id
    assert_eq!(llm_block["tool_use_id"], "tool_xyz");
    assert_eq!(tool_block["tool_use_id"], "tool_xyz");

    let llm_content = llm_block["content"].as_array().unwrap();
    let tool_content = tool_block["content"].as_array().unwrap();

    assert_eq!(llm_content.len(), 2);
    assert_eq!(tool_content.len(), 2);

    // Both should produce identical SideML format (has "type" field)
    assert_eq!(llm_content[0]["type"], "text");
    assert_eq!(llm_content[0]["text"], "Result text");
    assert_eq!(llm_content[1]["type"], "image");

    assert_eq!(tool_content[0]["type"], "text");
    assert_eq!(tool_content[0]["text"], "Result text");
    assert_eq!(tool_content[1]["type"], "image");

    // Content should be identical between both paths
    assert_eq!(
        llm_content, tool_content,
        "Both paths should produce identical content"
    );
}

// === Error Handling Tests (Using Existing Patterns) ===

#[test]
fn test_finish_reason_error_mapping() {
    // New FinishReason::Error variant
    assert_eq!(
        FinishReason::from_str_normalized("error"),
        Some(FinishReason::Error)
    );
    assert_eq!(
        FinishReason::from_str_normalized("failed"),
        Some(FinishReason::Error)
    );
    assert_eq!(
        FinishReason::from_str_normalized("failure"),
        Some(FinishReason::Error)
    );
}

#[test]
fn test_api_error_captured_as_context() {
    let input = json!({
        "role": "assistant",
        "error": {
            "code": "rate_limit_exceeded",
            "message": "Rate limit exceeded"
        }
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("api_error"))
    });
    assert!(
        context.is_some(),
        "API errors should be captured as Context for debugging"
    );
}

#[test]
fn test_existing_refusal_still_works() {
    // Verify we didn't break existing Refusal handling
    let input = json!({
        "role": "assistant",
        "refusal": "I cannot help with that request"
    });
    let output = normalize(&input);

    let refusal = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::Refusal { .. }));
    assert!(refusal.is_some(), "Refusal should still be captured");
}

#[test]
fn test_existing_tool_result_is_error_still_works() {
    // Verify we didn't break existing ToolResult.is_error handling
    let input = json!({
        "role": "tool",
        "tool_use_id": "test_123",
        "content": [{"type": "text", "text": "Tool failed"}],
        "is_error": true
    });
    let output = normalize(&input);

    // Tool results captured via existing tool handling
    assert!(output.tool_use_id.is_some());
}

// === Universal Citation/Grounding Tests ===

// --- Google Gemini/Vertex AI Grounding Tests ---

#[test]
fn test_gemini_grounding_metadata_as_context() {
    let input = json!({
        "role": "model",
        "content": "According to recent sources...",
        "groundingMetadata": {
            "webSearchQueries": ["climate change effects 2024"],
            "searchEntryPoint": {
                "renderedContent": "<search widget html>"
            },
            "groundingChunks": [{
                "web": {
                    "uri": "https://example.com/article",
                    "title": "Climate Report 2024"
                }
            }],
            "groundingSupports": [{
                "segment": {"startIndex": 0, "endIndex": 30},
                "groundingChunkIndices": [0],
                "confidenceScores": [0.95]
            }]
        }
    });
    let output = normalize(&input);

    let grounding = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    assert!(
        grounding.is_some(),
        "groundingMetadata should be captured as Context block"
    );
}

#[test]
fn test_gemini_citation_metadata_as_context() {
    let input = json!({
        "role": "model",
        "content": "The study found that...",
        "citationMetadata": {
            "citations": [{
                "startIndex": 0,
                "endIndex": 20,
                "uri": "https://example.com/study.pdf",
                "title": "Research Study",
                "license": "CC-BY-4.0"
            }]
        }
    });
    let output = normalize(&input);

    let citations = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        citations.is_some(),
        "citationMetadata should be captured as Context block"
    );
}

// --- Azure OpenAI Tests ---

#[test]
fn test_azure_data_sources_as_context() {
    let input = json!({
        "role": "user",
        "content": "What is the company policy?",
        "data_sources": [{
            "type": "azure_search",
            "parameters": {
                "index_name": "policies",
                "endpoint": "https://search.windows.net"
            }
        }]
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("data_sources"))
    });
    assert!(
        context.is_some(),
        "data_sources should be captured as Context block"
    );
}

#[test]
fn test_azure_citations_as_context() {
    let input = json!({
        "role": "assistant",
        "content": "According to the policy [doc1]...",
        "context": {
            "citations": [{
                "title": "HR Policy",
                "url": "https://example.com/policy.pdf",
                "content": "The policy states that...",
                "filepath": "policies/hr.pdf",
                "chunk_id": "chunk_1"
            }],
            "intent": "policy_lookup"
        }
    });
    let output = normalize(&input);

    // Citations should be extracted as Context block
    let citations = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        citations.is_some(),
        "citations should be captured as Context block"
    );

    // Other context fields should also be captured
    let azure_context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("azure_context"))
    });
    assert!(
        azure_context.is_some(),
        "other context fields should be captured"
    );
}

#[test]
fn test_azure_search_results_as_context() {
    let input = json!({
        "role": "assistant",
        "content": "Based on the search results...",
        "search_results": [{
            "title": "Product Manual",
            "url": "https://docs.example.com/manual.pdf",
            "content": "The product supports...",
            "score": 0.92
        }]
    });
    let output = normalize(&input);

    let search = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("search_results"))
    });
    assert!(
        search.is_some(),
        "search_results should be captured as Context block"
    );
}

#[test]
fn test_azure_content_filter_uses_existing_refusal() {
    // Azure content filtering already works via existing Refusal block
    let input = json!({
        "role": "assistant",
        "content": "",
        "refusal": "Content blocked by Azure content safety"
    });
    let output = normalize(&input);

    let refusal = output
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::Refusal { .. }));
    assert!(
        refusal.is_some(),
        "refusal field should create Refusal block"
    );
}

// --- Cohere Tests ---

#[test]
fn test_cohere_citations_as_context() {
    let input = json!({
        "role": "assistant",
        "content": "The answer is 42.",
        "citations": [{
            "start": 0,
            "end": 17,
            "text": "The answer is 42.",
            "document_ids": ["doc-hitchhiker-1"]
        }]
    });
    let output = normalize(&input);

    let citations = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        citations.is_some(),
        "Cohere citations should be captured as Context block"
    );
}

// --- AWS Bedrock Tests ---

#[test]
fn test_bedrock_attributions_as_context() {
    let input = json!({
        "role": "assistant",
        "content": "According to the knowledge base...",
        "attributions": [{
            "content": {"text": "Source document content..."},
            "score": 0.88,
            "location": {
                "type": "S3",
                "s3Location": {"uri": "s3://bucket/doc.pdf"}
            }
        }]
    });
    let output = normalize(&input);

    let attr = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("attributions"))
    });
    assert!(
        attr.is_some(),
        "Bedrock attributions should be captured as Context block"
    );
}

// --- Multiple Citation Sources Test ---

#[test]
fn test_multiple_citation_sources_create_separate_blocks() {
    // Message with both Gemini grounding AND Cohere-style citations
    let input = json!({
        "role": "assistant",
        "content": "Response with multiple citation sources",
        "groundingMetadata": {
            "webSearchQueries": ["query"],
            "groundingChunks": [{"web": {"uri": "https://example.com"}}]
        },
        "citations": [{
            "start": 0,
            "end": 10,
            "document_ids": ["doc1"]
        }]
    });
    let output = normalize(&input);

    // Should have BOTH context blocks, not merged
    let grounding = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    let citations = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });

    assert!(
        grounding.is_some(),
        "groundingMetadata should create separate Context block"
    );
    assert!(
        citations.is_some(),
        "citations should create separate Context block"
    );

    // Count Context blocks - should be exactly 2
    let context_count = output
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Context { .. }))
        .count();
    assert_eq!(
        context_count, 2,
        "Multiple citation sources should create multiple Context blocks"
    );
}

// --- Empty/Null Guard Tests ---

#[test]
fn test_empty_citations_not_captured() {
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "citations": []  // Empty array
    });
    let output = normalize(&input);

    // Should NOT create Context block for empty array
    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        context.is_none(),
        "Empty citations should not create Context block"
    );
}

#[test]
fn test_null_grounding_not_captured() {
    let input = json!({
        "role": "model",
        "content": "Response text",
        "groundingMetadata": null
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    assert!(
        context.is_none(),
        "Null grounding should not create Context block"
    );
}

#[test]
fn test_empty_object_grounding_not_captured() {
    let input = json!({
        "role": "model",
        "content": "Response text",
        "groundingMetadata": {}
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    assert!(
        context.is_none(),
        "Empty grounding object should not create Context block"
    );
}

#[test]
fn test_empty_error_object_not_captured() {
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "error": {}  // Empty error object
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("api_error"))
    });
    assert!(
        context.is_none(),
        "Empty error object should not create Context block"
    );
}

// --- Deep Validation Tests (has_meaningful_data) ---

#[test]
fn test_array_with_null_not_captured() {
    // [null] is technically non-empty but contains no useful data
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "citations": [null]
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        context.is_none(),
        "Array with only null should not create Context block"
    );
}

#[test]
fn test_array_with_empty_objects_not_captured() {
    // [{}] is technically non-empty but contains no useful data
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "citations": [{}]
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        context.is_none(),
        "Array with only empty objects should not create Context block"
    );
}

#[test]
fn test_object_with_null_values_not_captured() {
    // {"key": null} is technically non-empty but contains no useful data
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "groundingMetadata": {"webSearchQueries": null, "groundingChunks": null}
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    assert!(
        context.is_none(),
        "Object with only null values should not create Context block"
    );
}

#[test]
fn test_deeply_nested_empty_not_captured() {
    // Deeply nested empty structure should not create Context block
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "groundingMetadata": {
            "groundingChunks": [{"web": {}}],  // Empty nested object
            "groundingSupports": []
        }
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("grounding"))
    });
    assert!(
        context.is_none(),
        "Deeply nested empty structures should not create Context block"
    );
}

#[test]
fn test_meaningful_data_is_captured() {
    // Verify that actual meaningful data IS captured
    let input = json!({
        "role": "assistant",
        "content": "Response text",
        "citations": [{"title": "Real Citation", "url": "https://example.com"}]
    });
    let output = normalize(&input);

    let context = output.content.iter().find(|b| {
        matches!(b, ContentBlock::Context { context_type, .. }
            if context_type.as_deref() == Some("citations"))
    });
    assert!(
        context.is_some(),
        "Meaningful citation data should create Context block"
    );
}

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
    use crate::domain::sideml::to_sideml_with_context;

    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "message": "Tool result: 72F"
        }),
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
    use crate::domain::sideml::to_sideml_with_context;

    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: json!({
            "message": "Hello! How can I help?"
        }),
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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

    // gen_ai.tool.result event should always derive Tool role
    let raw_messages = vec![RawMessage {
        source: MessageSource::Event {
            name: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!({
            "content": [{"toolResult": {"toolUseId": "123", "content": "Result"}}]
        }),
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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::sideml::content::normalize_content;

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    if let ContentBlock::Text { text } = &message.content[0] {
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
    use crate::domain::traces::{MessageSource, RawMessage};

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
    use crate::domain::traces::{MessageSource, RawMessage};

    // Test that Gemini format with "parts" is recognized as message-like
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "parts": [{"text": "Hello from Gemini"}]}
        ]),
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
    use crate::domain::traces::{MessageSource, RawMessage};

    // Test that Bedrock format with "text" is recognized as message-like
    let raw_messages = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.input.messages".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        },
        content: json!([
            {"role": "user", "text": "Hello from Bedrock"}
        ]),
    }];

    let sideml_messages = to_sideml_with_context(&raw_messages, false);

    assert_eq!(
        sideml_messages.len(),
        1,
        "Bedrock format with 'text' should be recognized"
    );
}

// === Edge Case Tests ===

#[test]
fn test_empty_tool_name_handling() {
    // Test that empty tool names are handled gracefully
    let raw = json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "call_123",
            "function": {
                "name": "",
                "arguments": "{}"
            }
        }]
    });

    let message = normalize(&raw);

    // Should not panic, and tool call should still be created
    assert!(!message.content.is_empty());
}

#[test]
fn test_null_role_handling() {
    // Test that null role defaults correctly
    let raw = json!({
        "role": null,
        "content": "Hello"
    });

    let message = normalize(&raw);

    // Null role should default to User
    assert_eq!(message.role, ChatRole::User);
}

#[test]
fn test_non_string_role_handling() {
    // Test that non-string role (edge case) defaults correctly
    let raw = json!({
        "role": 123,
        "content": "Hello"
    });

    let message = normalize(&raw);

    // Non-string role should default to User
    assert_eq!(message.role, ChatRole::User);
}

// === Vercel AI SDK Structured Output Tests ===

#[test]
fn test_vercel_ai_response_object_as_content() {
    // Vercel AI SDK stores structured output in "object" field (from ai.response.object)
    // This is extracted by try_vercel_ai and should be normalized as content
    let raw = json!({
        "role": "assistant",
        "object": {
            "recipe": {
                "name": "Chocolate Chip Cookies",
                "ingredients": ["flour", "sugar", "butter", "chocolate chips"],
                "steps": ["Mix dry ingredients", "Add wet ingredients", "Bake at 350F"]
            }
        }
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(
        !message.content.is_empty(),
        "Content should not be empty for object field"
    );

    // The object should be normalized as a json content block
    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert!(data.get("recipe").is_some(), "Should contain recipe object");
            assert_eq!(data["recipe"]["name"], "Chocolate Chip Cookies");
        }
        other => panic!("Expected Json content block, got {:?}", other),
    }
}

#[test]
fn test_vercel_ai_response_object_simple_value() {
    // Vercel AI can return simple values as structured output
    let raw = json!({
        "role": "assistant",
        "object": {
            "answer": 42,
            "confidence": 0.95
        }
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(!message.content.is_empty());

    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert_eq!(data["answer"], 42);
            assert_eq!(data["confidence"], 0.95);
        }
        other => panic!("Expected Json content block, got {:?}", other),
    }
}

#[test]
fn test_vercel_ai_content_takes_precedence_over_object() {
    // When both content and object are present, content should take precedence
    let raw = json!({
        "role": "assistant",
        "content": "Hello from content field",
        "object": {"ignored": true}
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(!message.content.is_empty());

    // Content field should be used, not object
    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert_eq!(text, "Hello from content field");
        }
        other => panic!("Expected Text content block, got {:?}", other),
    }
}

// ============================================================================
// REGRESSION TESTS
// ============================================================================
// Tests for specific real-world issues that were fixed.
// Each test documents the original issue with trace ID when available.

#[test]
fn regression_langgraph_contents_field_with_message_content_wrapper() {
    // Regression test for trace 0bda91d1d9f955fd3e2dab4b9664333b
    // Issue: LangGraph/ChatBedrockConverse uses OpenInference format with:
    // - "contents" field (plural) instead of "content"
    // - Nested "message_content" wrapper around actual content
    // - Sparse arrays from unflatten (missing indices create empty {})
    //
    // This combination caused assistant messages to show as empty.
    let raw = json!({
        "role": "assistant",
        "contents": [
            {},  // Sparse array placeholder (index 0 was missing)
            {
                "message_content": {
                    "type": "text",
                    "text": "Hello! Here's your 3-day weather forecast for New York City..."
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert_eq!(
        message.content.len(),
        1,
        "Should have exactly 1 content block"
    );

    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert!(
                text.contains("weather forecast"),
                "Should extract text from message_content wrapper"
            );
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

#[test]
fn regression_empty_json_block_from_sparse_array() {
    // Regression test for trace 771e923f9f7781491b41abc00d9d21fa
    // Issue: Assistant message displayed as "{}" because sparse array
    // placeholder was normalized to json type instead of being filtered.
    let raw = json!({
        "role": "assistant",
        "contents": [
            {
                "message_content": {
                    "type": "text",
                    "text": "I'll help you with the weather forecast."
                }
            },
            {}  // This placeholder was incorrectly showing as "{}"
        ]
    });

    let message = normalize(&raw);

    assert_eq!(
        message.content.len(),
        1,
        "Empty placeholder should be filtered"
    );

    // Verify no json blocks with empty data
    for block in &message.content {
        if let ContentBlock::Json { data } = block {
            assert!(
                !data.as_object().is_some_and(|o| o.is_empty()),
                "Should not have empty json blocks from sparse array placeholders"
            );
        }
    }
}

#[test]
fn regression_openinference_tool_use_with_sparse_array() {
    // Regression test: Tool calls from LangGraph with sparse indices
    // Real pattern: llm.output_messages.0.message.contents.N.message_content
    // where N has gaps (e.g., 0 and 2 exist, but 1 doesn't)
    let raw = json!({
        "role": "assistant",
        "contents": [
            {},  // Placeholder
            {
                "message_content": {
                    "type": "tool_use",
                    "id": "tooluse_RHFjqfWtcReRCoWcxaIv4n",
                    "name": "temperature_forecast",
                    "input": {"city": "New York City", "days": 3}
                }
            },
            {
                "message_content": {
                    "type": "tool_use",
                    "id": "tooluse_kKnD6F5gFUOkblpMf6V81i",
                    "name": "precipitation_forecast",
                    "input": {"city": "New York City", "days": 3}
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 2, "Should have 2 tool_use blocks");

    let tool_names: Vec<&str> = message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();

    assert!(tool_names.contains(&"temperature_forecast"));
    assert!(tool_names.contains(&"precipitation_forecast"));
}

#[test]
fn regression_openinference_reasoning_content_thinking() {
    // Regression test: Extended thinking from OpenInference format
    // Pattern: reasoning_content wrapper with text and signature
    let raw = json!({
        "role": "assistant",
        "contents": [
            {
                "reasoning_content": {
                    "text": "Let me analyze the weather data for NYC...",
                    "signature": "thinking_sig_abc123"
                }
            },
            {
                "message_content": {
                    "type": "text",
                    "text": "Based on my analysis, here's the forecast."
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 2, "Should have thinking + text");

    // First block should be thinking
    match &message.content[0] {
        ContentBlock::Thinking { text, signature } => {
            assert!(text.contains("analyze the weather"));
            assert_eq!(signature.as_deref(), Some("thinking_sig_abc123"));
        }
        other => panic!("Expected Thinking, got {:?}", other),
    }

    // Second block should be text
    match &message.content[1] {
        ContentBlock::Text { text } => {
            assert!(text.contains("Based on my analysis"));
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

#[test]
fn regression_unflatten_creates_nested_sparse_arrays() {
    // Regression test: Unflatten with complex nested sparse structures
    // Can occur with deeply nested OpenInference message formats
    //
    // The unflatten algorithm creates placeholders at multiple levels when
    // indexed attributes have gaps.
    let raw = json!({
        "role": "assistant",
        // After unflatten of something like:
        // "contents.2.message_content.text": "Hello"
        // We get:
        "contents": [
            {},  // placeholder for index 0
            {},  // placeholder for index 1
            {
                "message_content": {
                    "type": "text",
                    "text": "Hello after two sparse indices"
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 1, "Should filter both placeholders");

    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert_eq!(text, "Hello after two sparse indices");
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

// === Plain Data / Structured Output Tests ===

#[test]
fn test_is_plain_data_value() {
    // Plain data objects → true
    assert!(is_plain_data_value(&json!({"name": "Jane", "age": 28})));
    assert!(is_plain_data_value(
        &json!({"score": 0.95, "label": "positive"})
    ));
    assert!(is_plain_data_value(&json!({"items": [1, 2, 3]})));

    // Message-structure objects → false
    assert!(!is_plain_data_value(
        &json!({"role": "user", "content": "hi"})
    ));
    assert!(!is_plain_data_value(&json!({"content": "hello"})));
    assert!(!is_plain_data_value(&json!({"type": "text", "text": "hi"})));
    assert!(!is_plain_data_value(&json!({"tool_calls": []})));
    assert!(!is_plain_data_value(&json!({"finish_reason": "stop"})));
    assert!(!is_plain_data_value(&json!({"toolUse": {}})));
    assert!(!is_plain_data_value(&json!({"functionCall": {}})));
    assert!(!is_plain_data_value(&json!({"parts": []})));
    assert!(!is_plain_data_value(&json!({"choices": []})));

    // Non-objects → false
    assert!(!is_plain_data_value(&json!("string")));
    assert!(!is_plain_data_value(&json!(42)));
    assert!(!is_plain_data_value(&json!([1, 2])));
    assert!(!is_plain_data_value(&json!(null)));
    assert!(!is_plain_data_value(&json!({})));
}

#[test]
fn test_normalize_plain_data_object() {
    let raw = json!({"name": "Jane Doe", "age": 28});
    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::User);
    assert_eq!(message.content.len(), 1);
    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert_eq!(data["name"], "Jane Doe");
            assert_eq!(data["age"], 28);
        }
        other => panic!("Expected Json block, got {:?}", other),
    }
}

#[test]
fn test_normalize_plain_data_doesnt_affect_messages() {
    // A proper message with "content" key should still work normally
    let raw = json!({"role": "assistant", "content": "Hello!"});
    let message = normalize(&raw);
    assert_eq!(message.role, ChatRole::Assistant);
    assert_eq!(message.content.len(), 1);
    match &message.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello!"),
        other => panic!("Expected Text, got {:?}", other),
    }

    // A message with tool_calls should still work
    let raw = json!({"tool_calls": [{"id": "1", "function": {"name": "test", "arguments": "{}"}}]});
    let message = normalize(&raw);
    assert_eq!(message.role, ChatRole::Assistant);
    assert!(
        message
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    );
}
