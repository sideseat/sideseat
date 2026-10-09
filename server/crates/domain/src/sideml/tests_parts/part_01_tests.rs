use super::*;
use chrono::{TimeZone, Utc};
use serde_json::json;

use crate::observations::{MessageSource, RawMessage};
use normalize::categorize_tool_message;
use sideseat_ports::types::{MessageCategory, MessageSourceType};

/// Helper to convert ContentBlock to JsonValue for testing
fn block_to_json(block: &ContentBlock) -> JsonValue {
    serde_json::to_value(block).unwrap()
}

/// The signature a thinking block keeps, which no view serialises.
fn thinking_signature(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Thinking { signature, .. } => signature.as_deref(),
        _ => None,
    }
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
        FinishReason::from_str_normalized("TOOL_EXECUTION"), // LangChain4j
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
        citations: Vec::new(),
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
        provider_executed: false,
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
    // The view says the reasoning is signed; the signature's bytes stay in the raw store.
    assert_eq!(json["signed"], true);
    assert!(json.get("signature").is_none());
    assert!(!json.to_string().contains("sig123"));
    let unsigned = ContentBlock::Thinking {
        text: "Hmm.".to_string(),
        signature: None,
    };
    assert!(
        serde_json::to_value(&unsigned)
            .unwrap()
            .get("signed")
            .is_none()
    );
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

/// Regression: a streamed call's input arrives as the JSON text its deltas accumulated.
///
/// LangChain's streamed Bedrock message kept `"input": "{\"city\": \"Rome\"}"`, and the same call from
/// the completed response on that span carried the object, so the trace showed the call twice.
#[test]
fn test_tool_use_input_streamed_as_json_text_is_decoded() {
    let input = json!({
        "role": "assistant",
        "content": [{
            "type": "tool_use", "id": "toolu_1", "name": "get_weather", "index": 0,
            "input": "{\"city\": \"Rome\", \"days\": 1}"
        }]
    });
    let output = normalize(&input);
    assert_eq!(
        block_to_json(&output.content[0]),
        json!({"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "Rome", "days": 1}})
    );

    // Text that is not a JSON value is the input as sent and stays a string.
    let input = json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": "toolu_2", "name": "echo", "input": "{\"city\": "}]
    });
    let output = normalize(&input);
    assert_eq!(block_to_json(&output.content[0])["input"], "{\"city\": ");
}

/// Regression: Haystack's reasoning part keeps its text under `reasoning_text`.
///
/// Read through the shared reasoning envelope, which looks for `text`, the reasoning came out empty
/// and was dropped, so a reasoning model's trace showed only the answer.
#[test]
fn test_haystack_reasoning_part_is_thinking() {
    let input = json!({
        "role": "assistant",
        "content": [
            {"reasoning": {"reasoning_text": "Send the two slowest together.", "extra": {"signature": "sig"}}},
            {"text": "17 minutes."}
        ]
    });
    let output = normalize(&input);
    let first = block_to_json(&output.content[0]);
    assert_eq!(first["type"], "thinking");
    assert_eq!(first["text"], "Send the two slowest together.");
    assert_eq!(thinking_signature(&output.content[0]), Some("sig"));
}

/// Regression: a tool message whose content is a number written as text keeps it as the result.
///
/// CrewAI recorded the calculator's `"395.0"` this way. Read as a JSON number it had no content
/// block, so the result vanished and its call looked unanswered.
#[test]
fn test_tool_message_with_numeric_text_content() {
    let input = json!({
        "role": "tool",
        "content": "395.0",
        "tool_call_id": "tooluse_1",
        "name": "calculate"
    });
    let output = normalize(&input);
    assert_eq!(output.content.len(), 1, "{:?}", output.content);
    let result = block_to_json(&output.content[0]);
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "tooluse_1");
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
    // Gemini supplies no call id, so none is invented here. Deriving one from the RESPONSE (as
    // this used to) produced an id that no call ever had, leaving every ADK tool result
    // pointing at nothing. The name is carried instead, and feed/correlate.rs resolves the id
    // from the matching call.
    assert!(
        block.get("tool_use_id").is_none() || block["tool_use_id"].is_null(),
        "no id should be fabricated at normalization, got: {}",
        block["tool_use_id"]
    );
    assert_eq!(block["name"], "get_weather");
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
    assert_eq!(thinking_signature(&output.content[0]), Some("sig_abc"));
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
    use crate::sideml::ContentBlock;

    let original = json!({
        "type": "experimental_block",
        "payload": {"key": "value"},
        "metadata": [1, 2, 3]
    });

    // Deserialize into ContentBlock
    let block: ContentBlock = serde_json::from_value(original).unwrap();

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
