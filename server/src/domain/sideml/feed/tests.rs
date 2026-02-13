//! Tests for the feed pipeline.
//!
//! These tests verify the correctness of the message processing pipeline
//! using the new flattened block structure.

use chrono::Utc;
use serde_json::json;

use super::*;
use crate::data::types::MessageSpanRow;
use crate::domain::sideml::types::{ChatRole, ContentBlock, FinishReason};

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Fixed timestamp for tests (2025-01-01T00:00:00Z)
fn fixed_time() -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn make_span_row(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    tool_definitions_json: &str,
    tool_names_json: &str,
) -> MessageSpanRow {
    // Use fixed_time() to match the timestamps in test JSON messages
    let ts = fixed_time();
    MessageSpanRow {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_timestamp: ts,
        span_end_timestamp: None,
        messages_json: messages_json.to_string(),
        tool_definitions_json: tool_definitions_json.to_string(),
        tool_names_json: tool_names_json.to_string(),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cost_total: 0.01,
        observation_type: None,
        session_id: None,
        ingested_at: ts,
    }
}

/// Create a span row with explicit timestamps for dedup-aware tests
fn make_span_row_with_timestamps(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    span_start: chrono::DateTime<Utc>,
    span_end: Option<chrono::DateTime<Utc>>,
) -> MessageSpanRow {
    // Default to "generation" for LLM spans to enable history detection
    make_span_row_full(
        trace_id,
        span_id,
        parent_span_id,
        messages_json,
        span_start,
        span_end,
        Some("generation"),
    )
}

/// Create a span row with full control over all fields
fn make_span_row_full(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    span_start: chrono::DateTime<Utc>,
    span_end: Option<chrono::DateTime<Utc>>,
    observation_type: Option<&str>,
) -> MessageSpanRow {
    MessageSpanRow {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_timestamp: span_start,
        span_end_timestamp: span_end,
        messages_json: messages_json.to_string(),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cost_total: 0.01,
        observation_type: observation_type.map(String::from),
        session_id: None,
        ingested_at: span_start,
    }
}

#[allow(dead_code)]
fn get_text(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    }
}

// ============================================================================
// BASIC TESTS
// ============================================================================

#[test]
fn test_process_spans_empty() {
    let rows = vec![];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert!(result.messages.is_empty());
    assert!(result.tool_definitions.is_empty());
    assert!(result.tool_names.is_empty());
}

#[test]
fn test_process_spans_simple_message() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "Hello"}
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, ChatRole::User);
    // Content is now a single block
    assert!(matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "Hello"));
}

#[test]
fn test_process_spans_flattening() {
    // Test that multiple content blocks in one message become multiple BlockEntries
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [
                {"type": "text", "text": "First"},
                {"type": "text", "text": "Second"}
            ]
        }
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have 2 blocks (one per content block)
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, ChatRole::Assistant);
    assert_eq!(result.messages[1].role, ChatRole::Assistant);
    assert_eq!(result.messages[0].entry_index, 0);
    assert_eq!(result.messages[1].entry_index, 1);

    // Verify content
    assert!(matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "First"));
    assert!(matches!(&result.messages[1].content, ContentBlock::Text { text } if text == "Second"));
}

#[test]
fn test_deduplicate_tools() {
    let tools = vec![
        json!({"type": "function", "function": {"name": "tool_a", "description": "A"}}),
        json!({"type": "function", "function": {"name": "tool_b", "description": "B"}}),
        json!({"type": "function", "function": {"name": "tool_a", "description": "A again"}}),
    ];

    let deduped = deduplicate_tools(tools);
    assert_eq!(deduped.len(), 2);

    let names: Vec<_> = deduped
        .iter()
        .filter_map(|t| t.get("function")?.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["tool_a", "tool_b"]);
}

#[test]
fn test_deduplicate_tools_prefers_richer_definition() {
    let tools = vec![
        json!({"type": "function", "function": {"name": "tool_a"}}),
        json!({
            "type": "function",
            "function": {
                "name": "tool_a",
                "description": "Richer definition",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                }
            }
        }),
    ];

    let deduped = deduplicate_tools(tools);
    assert_eq!(deduped.len(), 1);

    let func = &deduped[0]["function"];
    assert_eq!(func["name"].as_str(), Some("tool_a"));
    assert_eq!(func["description"].as_str(), Some("Richer definition"));
    assert!(func.get("parameters").is_some());
}

#[test]
fn test_deduplicate_tools_merges_complementary_fields() {
    let tools = vec![
        json!({
            "type": "function",
            "function": {
                "name": "tool_a",
                "description": "Weather tool"
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tool_a",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                }
            }
        }),
    ];

    let deduped = deduplicate_tools(tools);
    assert_eq!(deduped.len(), 1);

    let func = &deduped[0]["function"];
    assert_eq!(func["name"].as_str(), Some("tool_a"));
    assert_eq!(func["description"].as_str(), Some("Weather tool"));
    assert_eq!(
        func["parameters"]["properties"]["city"]["type"].as_str(),
        Some("string")
    );
}

#[test]
fn test_deduplicate_tools_merges_parameter_properties_and_required() {
    let tools = vec![
        json!({
            "type": "function",
            "function": {
                "name": "tool_a",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tool_a",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "days": {"type": "integer"}
                    },
                    "required": ["days"]
                }
            }
        }),
    ];

    let deduped = deduplicate_tools(tools);
    assert_eq!(deduped.len(), 1);

    let params = &deduped[0]["function"]["parameters"];
    assert_eq!(
        params["properties"]["city"]["type"].as_str(),
        Some("string")
    );
    assert_eq!(
        params["properties"]["days"]["type"].as_str(),
        Some("integer")
    );

    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&json!("city")));
    assert!(required.contains(&json!("days")));
}

#[test]
fn test_deduplicate_names() {
    let names = vec![
        "tool_b".to_string(),
        "tool_a".to_string(),
        "tool_b".to_string(),
        "tool_c".to_string(),
    ];

    let deduped = deduplicate_names(names);
    assert_eq!(deduped, vec!["tool_a", "tool_b", "tool_c"]);
}

#[test]
fn test_role_filter() {
    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "User message"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "Assistant message"}
        }
    ]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");

    // Filter for user messages only
    let options = FeedOptions::new().with_role(Some("user".to_string()));
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, ChatRole::User);
}

#[test]
fn test_block_entry_metadata() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": "Test content"
        }
    }]);

    let mut row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    row.session_id = Some("session1".to_string());
    row.status_code = Some("OK".to_string());

    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    assert_eq!(block.trace_id, "trace1");
    assert_eq!(block.span_id, "span1");
    assert_eq!(block.session_id, Some("session1".to_string()));
    assert_eq!(block.model, Some("gpt-4".to_string()));
    assert_eq!(block.provider, Some("openai".to_string()));
    assert_eq!(block.status_code, Some("OK".to_string()));
    assert!(!block.is_error);
    assert_eq!(block.entry_type, "text");
    assert!(!block.content_hash.is_empty());
}

#[test]
fn test_span_path_computation() {
    // Create a hierarchy: root -> child -> grandchild
    // Each span has a DIFFERENT message to avoid deduplication
    let msg_root = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "Root message"}
    }]);
    let msg_child = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
        "content": {"role": "user", "content": "Child message"}
    }]);
    let msg_grandchild = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:02Z"}},
        "content": {"role": "user", "content": "Grandchild message"}
    }]);

    let rows = vec![
        make_span_row("trace1", "root", None, &msg_root.to_string(), "[]", "[]"),
        make_span_row(
            "trace1",
            "child",
            Some("root"),
            &msg_child.to_string(),
            "[]",
            "[]",
        ),
        make_span_row(
            "trace1",
            "grandchild",
            Some("child"),
            &msg_grandchild.to_string(),
            "[]",
            "[]",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find blocks by span_id
    let root_block = result
        .messages
        .iter()
        .find(|b| b.span_id == "root")
        .unwrap();
    let child_block = result
        .messages
        .iter()
        .find(|b| b.span_id == "child")
        .unwrap();
    let grandchild_block = result
        .messages
        .iter()
        .find(|b| b.span_id == "grandchild")
        .unwrap();

    // Verify span_path
    assert_eq!(root_block.span_path, vec!["root"]);
    assert_eq!(child_block.span_path, vec!["root", "child"]);
    assert_eq!(
        grandchild_block.span_path,
        vec!["root", "child", "grandchild"]
    );
}

#[test]
fn test_tool_use_extraction() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "call_123",
                "name": "search",
                "input": {"query": "test"}
            }]
        }
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    assert_eq!(block.entry_type, "tool_use");
    assert_eq!(block.tool_use_id, Some("call_123".to_string()));
    assert_eq!(block.tool_name, Some("search".to_string()));

    match &block.content {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, &Some("call_123".to_string()));
            assert_eq!(name, "search");
            assert_eq!(input.get("query").unwrap().as_str(), Some("test"));
        }
        _ => panic!("Expected ToolUse content block"),
    }
}

#[test]
fn test_tool_result_extraction() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "tool",
            "tool_use_id": "call_123",
            "content": "Tool output"
        }
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    assert_eq!(block.role, ChatRole::Tool);
    assert_eq!(block.tool_use_id, Some("call_123".to_string()));
}

#[test]
fn test_sorting_by_timestamp_message_entry() {
    // Test that blocks are sorted by (timestamp, message_index, entry_index)
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "First"}
    }]);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": "2025-01-01T00:00:01Z"}},
        "content": {"role": "assistant", "content": "Second"}
    }]);

    let row1 = make_span_row("trace1", "span1", None, &msg1.to_string(), "[]", "[]");
    let row2 = make_span_row("trace1", "span2", None, &msg2.to_string(), "[]", "[]");

    let options = FeedOptions::default();
    let result = process_spans(vec![row2, row1], &options); // Note: reversed order

    assert_eq!(result.messages.len(), 2);
    // Should be sorted by timestamp ASC
    assert!(matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "First"));
    assert!(matches!(&result.messages[1].content, ContentBlock::Text { text } if text == "Second"));
}

#[test]
fn test_metadata() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "Test"}
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.metadata.block_count, 1);
    assert_eq!(result.metadata.span_count, 1);
    assert_eq!(result.metadata.total_tokens, 150);
    assert!((result.metadata.total_cost - 0.01).abs() < 0.001);
}

// ============================================================================
// DEDUPLICATION INTEGRATION TESTS
// ============================================================================
// Unit tests for dedup logic are in dedup.rs. These tests verify pipeline integration.

#[test]
fn test_thinking_blocks_preserved() {
    // Test that thinking blocks (enrichment content) are preserved
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:00Z"}},
        "content": {
            "role": "assistant",
            "content": [
                {"type": "thinking", "text": "Let me think about this..."},
                {"type": "text", "text": "Here is my answer"}
            ],
            "finish_reason": "stop"
        }
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have 2 blocks: thinking + text
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].entry_type, "thinking");
    assert_eq!(result.messages[1].entry_type, "text");
}

#[test]
fn test_history_deduplication() {
    // Test that duplicate history messages are automatically deduplicated
    // Child span has history (user message) that should be filtered as a duplicate
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Original question"}
    }]);

    let child_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Original question"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The answer", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("root"),
            &child_msg.to_string(),
            t0,
            Some(t1),
        ),
    ];

    // History is automatically detected and deduplicated
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    // Root's user message + child's assistant message (duplicate user message filtered)
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].role, ChatRole::User);
    assert_eq!(result.messages[1].role, ChatRole::Assistant);
}

#[test]
fn test_process_feed_multiple_sessions() {
    // Test process_feed with multiple sessions
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "Session 1 message"}
    }]);

    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
        "content": {"role": "user", "content": "Session 2 message"}
    }]);

    let mut row1 = make_span_row("trace1", "span1", None, &msg1.to_string(), "[]", "[]");
    row1.session_id = Some("session1".to_string());

    let mut row2 = make_span_row("trace2", "span2", None, &msg2.to_string(), "[]", "[]");
    row2.session_id = Some("session2".to_string());

    let options = FeedOptions::default();
    let result = process_feed(vec![row1, row2], &options);

    // Both sessions should be processed
    assert_eq!(result.messages.len(), 2);
}

#[test]
fn test_process_feed_same_batch_ordering() {
    // Feed uses DESC order (newest first), but within same-batch blocks
    // (same span + same timestamp), text should still come before tool_use
    let t0 = "2025-01-01T00:00:00Z";

    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t0}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I'll search for that"},
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let mut row = make_span_row("trace1", "span1", None, &messages.to_string(), "[]", "[]");
    row.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let result = process_feed(vec![row], &options);

    // Should have text and tool_use
    assert_eq!(result.messages.len(), 2);

    // Text should come before tool_use (same-batch ordering preserved)
    assert_eq!(result.messages[0].entry_type, "text");
    assert_eq!(result.messages[1].entry_type, "tool_use");
}

#[test]
fn test_span_end_timestamp_used_for_output_ordering() {
    // Test that span_end_timestamp is used for OUTPUT message ordering
    // even when event time is earlier
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "assistant", "content": "Response", "finish_reason": "stop"}
    }]);

    let mut row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    // Set span_end_timestamp to later time
    row.span_end_timestamp = Some(
        chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:05Z")
            .unwrap()
            .with_timezone(&Utc),
    );

    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    // The block should exist and be processed correctly
    assert_eq!(result.messages[0].role, ChatRole::Assistant);
}

// ============================================================================
// REGRESSION TESTS FOR DEDUPLICATION ISSUES
// ============================================================================

/// Helper to create a span row with observation_type for tool spans
fn make_tool_span_row(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    span_start: chrono::DateTime<Utc>,
    span_end: Option<chrono::DateTime<Utc>>,
) -> MessageSpanRow {
    let mut row = make_span_row_with_timestamps(
        trace_id,
        span_id,
        parent_span_id,
        messages_json,
        span_start,
        span_end,
    );
    row.observation_type = Some("tool".to_string());
    row
}

// ----------------------------------------------------------------------------
// ISSUE 1: Historical Context Leaking as New Messages
// ----------------------------------------------------------------------------
// When an LLM span includes conversation history from previous turns,
// those messages appear as separate entries in the feed.

#[test]
fn test_regression_historical_context_not_leaked() {
    // Scenario: Agent trace where user asks about LA, but history includes NYC data
    // The NYC messages should NOT appear in the feed for the LA request
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Root span: user asks about LA
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in Los Angeles?"}
        }
    ]);

    // Child LLM span includes history from previous turn (NYC) as context
    // This is what the LLM received, but shouldn't be in final feed
    let child_msg = json!([
        // Historical context (previous turn about NYC)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in New York?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "New York is sunny today."}
        },
        // Current turn
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in Los Angeles?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Los Angeles is warm and sunny.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t2)),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("root"),
            &child_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT include NYC messages - they're historical context
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("New York")),
        "Historical NYC messages should not appear in feed. Found: {:?}",
        texts
    );

    // Should only have: system, user (LA question), assistant (LA answer)
    assert_eq!(result.messages.len(), 3);
}

// ----------------------------------------------------------------------------
// ISSUE 2: Tool Results Not Deduplicating Due to Structure Differences
// ----------------------------------------------------------------------------
// Same tool result appears with different JSON structures in different spans,
// causing hash mismatch and duplicate entries.

#[test]
fn test_regression_tool_result_structure_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Tool execution span: result as direct object
    let tool_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_123",
                "content": {"result": "sunny", "temp": 25}
            }
        }
    ]);

    // Chat span receives tool result as array with type wrapper
    let chat_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_123",
                "content": [{"type": "json", "data": {"json": {"result": "sunny", "temp": 25}}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather is sunny.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_tool_span_row(
            "trace1",
            "tool_span",
            Some("chat_span"),
            &tool_span_msg.to_string(),
            t1,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "chat_span",
            Some("root"),
            &chat_span_msg.to_string(),
            t0,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool results
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    // Should be 1 tool result, not 2
    assert_eq!(
        tool_results.len(),
        1,
        "Same tool result with different structure should deduplicate. Found {} instead of 1",
        tool_results.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 3: Text Messages Not Deduplicating Due to Whitespace
// ----------------------------------------------------------------------------
// Same text with trailing newline hashes differently, causing duplicates.

#[test]
fn test_regression_text_whitespace_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Child span: response without trailing newline
    let child_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Hello, world!", "finish_reason": "stop"}
    }]);

    // Root span: aggregated response with trailing newline
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Hello, world!\n", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t1)),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("root"),
            &child_msg.to_string(),
            t0,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count assistant responses
    let assistant_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .collect();

    // Should be 1 response, not 2
    assert_eq!(
        assistant_msgs.len(),
        1,
        "Same text with/without trailing newline should deduplicate. Found {}",
        assistant_msgs.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 4: Wrong Message Ordering (Tool Use After Tool Result)
// ----------------------------------------------------------------------------
// Tool use blocks appear after tool result blocks in the output.

#[test]
fn test_regression_tool_ordering() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);

    // LLM span with tool use output
    let llm_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Search for cats"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "cats"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Second LLM span: receives tool result as history, outputs final response
    // The tool result is recorded here even though tool execution happened elsewhere
    let llm2_msg = json!([
        // History includes the tool result
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Found cats!"}
        },
        // Final output
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Here are the cats!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "llm1",
            Some("root"),
            &llm_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "llm2",
            Some("root"),
            &llm2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find positions
    let tool_use_pos = result
        .messages
        .iter()
        .position(|m| m.entry_type == "tool_use");
    let tool_result_pos = result
        .messages
        .iter()
        .position(|m| m.entry_type == "tool_result");

    assert!(
        tool_use_pos.is_some() && tool_result_pos.is_some(),
        "Should have both tool_use and tool_result. Types found: {:?}",
        result
            .messages
            .iter()
            .map(|m| &m.entry_type)
            .collect::<Vec<_>>()
    );

    assert!(
        tool_use_pos.unwrap() < tool_result_pos.unwrap(),
        "tool_use (pos {}) should come before tool_result (pos {})",
        tool_use_pos.unwrap(),
        tool_result_pos.unwrap()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 5: Spurious Json Block from Tool Input
// ----------------------------------------------------------------------------
// Tool input appears as a separate Json message entry.
// This matches the pattern seen in trace 959d2590050265486b5f3a55ae3e2b71
// where span 2983bb7075c0d081 has a Json block with {"city": "Los Angeles", "days": 7}

#[test]
fn test_regression_no_spurious_tool_input_block() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Tool span with Strands-style events:
    // - tool_handler.invoke event with input params
    // - tool_handler.result event with output
    let tool_msg = json!([
        {
            "source": {"event": {"name": "tool_handler.invoke", "time": t0.to_rfc3339()}},
            "content": {"city": "Los Angeles", "days": 7}
        },
        {
            "source": {"event": {"name": "tool_handler.result", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC",
                "content": {"json": {"result": "sunny"}}
            }
        }
    ]);

    let rows = vec![make_tool_span_row(
        "trace1",
        "tool1",
        Some("root"),
        &tool_msg.to_string(),
        t0,
        Some(t1),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect all block types
    let block_types: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.entry_type.as_str())
        .collect();

    // Should NOT have json or text blocks for tool input params
    let non_tool_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type != "tool_result" && m.role != ChatRole::Tool)
        .collect();

    assert!(
        non_tool_blocks.is_empty(),
        "Tool span should only produce tool_result, not extra blocks. Found: {:?}",
        block_types
    );
}

// ----------------------------------------------------------------------------
// ISSUE 6: Tool Results with Same tool_use_id but Different Content Hash
// ----------------------------------------------------------------------------
// Tool results referencing same tool_use_id should deduplicate even if structure differs.

#[test]
fn test_regression_tool_result_same_id_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Tool span result
    let tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "tooluse_ABC123",
            "content": "The forecast shows sunny weather."
        }
    }]);

    // Chat span receives same result (recorded again as input history)
    let chat_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC123",
                "content": [{"type": "text", "text": "The forecast shows sunny weather."}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "It will be sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_tool_span_row(
            "trace1",
            "tool1",
            Some("chat1"),
            &tool_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "chat1",
            Some("root"),
            &chat_msg.to_string(),
            t0,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool results for this tool_use_id
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.tool_use_id.as_deref() == Some("tooluse_ABC123") && m.role == ChatRole::Tool)
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "Same tool result (same tool_use_id) should appear once. Found {}",
        tool_results.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 7: Intermediate Agent Loop Outputs Appearing in Feed
// ----------------------------------------------------------------------------
// Outputs from intermediate agent loop iterations shouldn't appear.

#[test]
fn test_regression_no_intermediate_loop_outputs() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    // First loop iteration (intermediate, not final)
    let loop1_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Get weather for LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "weather", "input": {}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Second loop iteration (final)
    let loop2_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Sunny"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather in LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    // Root span with final output only
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Get weather for LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The weather in LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t3)),
        make_span_row_with_timestamps(
            "trace1",
            "loop1",
            Some("root"),
            &loop1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "loop2",
            Some("root"),
            &loop2_msg.to_string(),
            t1,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count final assistant text responses (not tool_use)
    let final_responses: Vec<_> = result
        .messages
        .iter()
        .filter(|m| {
            m.role == ChatRole::Assistant && m.entry_type == "text" && m.finish_reason.is_some()
        })
        .collect();

    // Should have exactly 1 final response
    assert_eq!(
        final_responses.len(),
        1,
        "Should have exactly 1 final response. Found {}",
        final_responses.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 8: History Assistant Messages Without finish_reason
// ----------------------------------------------------------------------------
// Historical assistant messages lack finish_reason, affecting birth time computation.

#[test]
fn test_regression_history_assistant_detection() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // First span: original conversation
    let first_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!", "finish_reason": "stop"}
        }
    ]);

    // Second span: includes history WITHOUT finish_reason
    let second_msg = json!([
        // History - note: NO finish_reason (stripped when re-sent)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!"}
        },
        // New turn
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "How are you?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "I'm doing well!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "span1",
            None,
            &first_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &second_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count "Hi there!" assistant messages
    let hi_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|m| {
            m.role == ChatRole::Assistant
                && matches!(&m.content, ContentBlock::Text { text } if text == "Hi there!")
        })
        .collect();

    // Should deduplicate to 1
    assert_eq!(
        hi_messages.len(),
        1,
        "History assistant message should deduplicate. Found {}",
        hi_messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 9: Cross-Span Duplicate Tool Use Events
// ----------------------------------------------------------------------------
// Same tool use appears in multiple spans with different timing.

#[test]
fn test_regression_cross_span_tool_use_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // LLM span decides to use tool
    let llm_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_xyz", "name": "search", "input": {"q": "test"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // Tool span receives the same tool use as input context
    let tool_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_xyz", "name": "search", "input": {"q": "test"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_xyz", "content": "Results"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "llm",
            Some("root"),
            &llm_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool",
            Some("llm"),
            &tool_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool_use blocks with this ID
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_use" && m.tool_use_id.as_deref() == Some("call_xyz"))
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Same tool_use across spans should deduplicate. Found {}",
        tool_uses.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 10: Full Agent Trace Integration Test
// ----------------------------------------------------------------------------
// Simulates the exact scenario from trace 959d2590050265486b5f3a55ae3e2b71

#[test]
fn test_regression_full_agent_trace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t4 = t0 + chrono::Duration::milliseconds(400);

    // Root agent span
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {"role": "assistant", "content": "LA is sunny!\n", "finish_reason": "stop"}
        }
    ]);

    // First LLM call - decides to use tool
    let llm1_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool execution
    // Note: In tool spans, gen_ai.choice is used for tool OUTPUT (result)
    // gen_ai.tool.message would be INPUT which is not what we want here
    let tool_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.tool.input", "time": t2.to_rfc3339()}},
            "content": {"city": "LA"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "Sunny, 25C"}
        }
    ]);

    // Second LLM call - produces final response
    let llm2_msg = json!([
        // History
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "LA"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_1",
                "content": [{"type": "text", "text": "Sunny, 25C"}]
            }
        },
        // New output
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {"role": "assistant", "content": "LA is sunny!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "root", None, &root_msg.to_string(), t0, Some(t4)),
        make_span_row_with_timestamps(
            "trace1",
            "llm1",
            Some("root"),
            &llm1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool",
            Some("llm1"),
            &tool_msg.to_string(),
            t1,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "llm2",
            Some("root"),
            &llm2_msg.to_string(),
            t2,
            Some(t4),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Expected conversation flow:
    // 1. System message
    // 2. User: "Weather in LA?"
    // 3. Assistant: [tool_use: get_weather]
    // 4. Tool: "Sunny, 25C"
    // 5. Assistant: "LA is sunny!"

    // Verify no duplicates
    assert_eq!(
        result.messages.len(),
        5,
        "Should have exactly 5 messages. Found {}:\n{:?}",
        result.messages.len(),
        result
            .messages
            .iter()
            .map(|m| format!("{}: {:?}", m.entry_type, m.role))
            .collect::<Vec<_>>()
    );

    // Verify correct ordering - note: system and user have same timestamp,
    // so order between them may vary. Important is semantic ordering:
    // user/system → tool_use → tool_result → assistant_text
    let types: Vec<_> = result
        .messages
        .iter()
        .map(|m| (m.role, m.entry_type.as_str()))
        .collect();

    // First two should be user and system (order may vary as they have same timestamp)
    let first_two: std::collections::HashSet<_> = types[0..2].iter().collect();
    assert!(
        first_two.contains(&(ChatRole::User, "text"))
            && first_two.contains(&(ChatRole::System, "text")),
        "First two should be user and system"
    );
    assert_eq!(
        types[2],
        (ChatRole::Assistant, "tool_use"),
        "Third should be tool_use"
    );
    assert_eq!(
        types[3],
        (ChatRole::Tool, "tool_result"),
        "Fourth should be tool_result"
    );
    assert_eq!(
        types[4],
        (ChatRole::Assistant, "text"),
        "Fifth should be assistant text"
    );

    // No spurious json blocks
    let json_blocks = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "json")
        .count();
    assert_eq!(json_blocks, 0, "Should have no spurious json blocks");
}

// ----------------------------------------------------------------------------
// ISSUE 11: Content Hash Consistency Between Functions
// ----------------------------------------------------------------------------
// compute_block_hash and compute_semantic_hash should produce same results.

#[test]
fn test_regression_hash_function_consistency() {
    use super::dedup::MessageIdentity;

    let t0 = fixed_time();

    // Create a text block
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello world"}
    }]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    // Get the content_hash from the block (computed by compute_block_hash)
    let display_hash = u64::from_str_radix(&block.content_hash, 16).unwrap();

    // Get the identity hash (computed by MessageIdentity::from_block -> compute_semantic_hash)
    let identity = MessageIdentity::from_block(block);
    let identity_hash = match identity {
        MessageIdentity::Regular { semantic_hash, .. } => semantic_hash,
        _ => panic!("Expected Regular identity"),
    };

    // These should match for consistent deduplication
    assert_eq!(
        display_hash, identity_hash,
        "content_hash ({:016x}) should match identity semantic_hash ({:016x})",
        display_hash, identity_hash
    );
}

// ============================================================================
// ADDITIONAL REGRESSION TESTS
// ============================================================================

// ----------------------------------------------------------------------------
// ISSUE 11b: JSON Key Order Should Not Affect Deduplication
// ----------------------------------------------------------------------------
// Same JSON content with different key orders should hash to the same value.

#[test]
fn test_regression_json_key_order_deduplication() {
    use super::compute_block_hash;
    use crate::domain::sideml::ContentBlock;

    // Same data, different key order
    let json1 = serde_json::json!({
        "name": "Jane",
        "age": 28,
        "city": "NYC"
    });

    let json2 = serde_json::json!({
        "city": "NYC",
        "name": "Jane",
        "age": 28
    });

    let block1 = ContentBlock::Json { data: json1 };
    let block2 = ContentBlock::Json { data: json2 };

    let hash1 = compute_block_hash(&block1);
    let hash2 = compute_block_hash(&block2);

    assert_eq!(
        hash1, hash2,
        "JSON blocks with same data but different key order should have same hash"
    );
}

#[test]
fn test_regression_nested_json_key_order_deduplication() {
    use super::compute_block_hash;
    use crate::domain::sideml::ContentBlock;

    // Nested JSON with different key orders at multiple levels
    let json1 = serde_json::json!({
        "person": {
            "name": "Jane",
            "address": {"city": "NYC", "street": "123 Main"}
        },
        "score": 95
    });

    let json2 = serde_json::json!({
        "score": 95,
        "person": {
            "address": {"street": "123 Main", "city": "NYC"},
            "name": "Jane"
        }
    });

    let block1 = ContentBlock::Json { data: json1 };
    let block2 = ContentBlock::Json { data: json2 };

    let hash1 = compute_block_hash(&block1);
    let hash2 = compute_block_hash(&block2);

    assert_eq!(
        hash1, hash2,
        "Nested JSON with different key order should have same hash"
    );
}

// ----------------------------------------------------------------------------
// ISSUE 12: Multiple Parallel Tool Calls in Single Response
// ----------------------------------------------------------------------------
// LLM responds with multiple tool_use blocks; all should be preserved.

#[test]
fn test_regression_parallel_tool_calls() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Compare weather in LA and NYC"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_la", "name": "get_weather", "input": {"city": "Los Angeles"}},
                    {"type": "tool_use", "id": "call_nyc", "name": "get_weather", "input": {"city": "New York"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t1));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have 3 blocks: user + 2 tool_use
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_use")
        .collect();
    assert_eq!(
        tool_uses.len(),
        2,
        "Parallel tool calls should both be preserved. Found {}",
        tool_uses.len()
    );

    // Verify different inputs preserved
    let inputs: std::collections::HashSet<_> = tool_uses
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::ToolUse { input, .. } => input.get("city").and_then(|v| v.as_str()),
            _ => None,
        })
        .collect();
    assert!(inputs.contains("Los Angeles"));
    assert!(inputs.contains("New York"));
}

// ----------------------------------------------------------------------------
// ISSUE 13: Empty Content Blocks Filtered
// ----------------------------------------------------------------------------
// Messages with empty content arrays should not produce blocks.

#[test]
fn test_regression_empty_content_filtered() {
    let t0 = fixed_time();

    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": ""}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": []}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Only the non-empty message should produce a block
    assert_eq!(
        result.messages.len(),
        1,
        "Empty content should be filtered. Found {} messages",
        result.messages.len()
    );
    assert!(matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "Hello"));
}

// ----------------------------------------------------------------------------
// ISSUE 14: Unicode and Special Characters in Content
// ----------------------------------------------------------------------------
// Unicode text should hash consistently and deduplicate properly.

#[test]
fn test_regression_unicode_content_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Same unicode content in two spans
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello 你好 مرحبا 🌍"}
    }]);

    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello 你好 مرحبا 🌍"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should deduplicate to 1
    assert_eq!(
        result.messages.len(),
        1,
        "Unicode content should deduplicate. Found {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 15: System Messages in History
// ----------------------------------------------------------------------------
// System prompts duplicated across spans should deduplicate.

#[test]
fn test_regression_system_message_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let span1_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        }
    ]);

    let span2_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "span1",
            None,
            &span1_msg.to_string(),
            t0,
            Some(t0),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &span2_msg.to_string(),
            t0,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count system messages
    let system_count = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::System)
        .count();
    assert_eq!(
        system_count, 1,
        "System message should deduplicate. Found {}",
        system_count
    );
}

// ----------------------------------------------------------------------------
// ISSUE 16: Cross-Trace Isolation
// ----------------------------------------------------------------------------
// Same content in different traces should NOT deduplicate.

#[test]
fn test_regression_cross_trace_isolation() {
    let t0 = fixed_time();

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "span2", None, &msg.to_string(), t0, Some(t0)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Event-based traces (no input attribute): guard prevents false marking
    // because input_source_count (1) <= accumulated.len() (1). Both preserved.
    assert_eq!(
        result.messages.len(),
        2,
        "Event-based traces with same content: both preserved (guard prevents marking). Found {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 17: Tool Result with Error Flag
// ----------------------------------------------------------------------------
// Tool results with is_error=true should be preserved and marked.

#[test]
fn test_regression_tool_result_error_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Tool result with error - using content array with explicit is_error
    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Run the command"}
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "call_1",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "Error: API rate limit exceeded", "is_error": true}]
            }
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t1));

    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have user message and tool result
    assert_eq!(result.messages.len(), 2);

    // Find the tool result block
    let tool_block = result.messages.iter().find(|m| m.role == ChatRole::Tool);
    assert!(tool_block.is_some(), "Should have a tool result");

    let block = tool_block.unwrap();
    assert_eq!(block.entry_type, "tool_result");

    // Verify error info is preserved in the content
    match &block.content {
        ContentBlock::ToolResult { is_error, .. } => {
            assert!(*is_error, "is_error should be true");
        }
        _ => panic!("Expected ToolResult, got {:?}", block.entry_type),
    }
}

// ----------------------------------------------------------------------------
// ISSUE 18: Deep Span Hierarchy
// ----------------------------------------------------------------------------
// Messages in deeply nested spans should maintain correct span_path.

#[test]
fn test_regression_deep_hierarchy_span_path() {
    let t0 = fixed_time();

    // Create 5-level deep hierarchy
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Deep message"}
    }]);

    let rows = vec![
        make_span_row("trace1", "l1", None, "[]", "[]", "[]"),
        make_span_row("trace1", "l2", Some("l1"), "[]", "[]", "[]"),
        make_span_row("trace1", "l3", Some("l2"), "[]", "[]", "[]"),
        make_span_row("trace1", "l4", Some("l3"), "[]", "[]", "[]"),
        make_span_row("trace1", "l5", Some("l4"), &msg.to_string(), "[]", "[]"),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(result.messages.len(), 1);
    assert_eq!(
        result.messages[0].span_path,
        vec!["l1", "l2", "l3", "l4", "l5"],
        "Deep hierarchy span_path should be correct"
    );
}

// ----------------------------------------------------------------------------
// ISSUE 19: Thinking and Text in Same Message
// ----------------------------------------------------------------------------
// Both thinking and text blocks should be preserved from same message.

#[test]
fn test_regression_thinking_with_text_preserved() {
    let t0 = fixed_time();

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [
                {"type": "thinking", "text": "Let me reason through this..."},
                {"type": "text", "text": "The answer is 42."}
            ],
            "finish_reason": "stop"
        }
    }]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have 2 blocks: thinking + text
    assert_eq!(result.messages.len(), 2);

    let types: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.entry_type.as_str())
        .collect();
    assert!(types.contains(&"thinking"), "Should contain thinking block");
    assert!(types.contains(&"text"), "Should contain text block");

    // Both should have same message_index but different entry_index
    assert_eq!(
        result.messages[0].message_index,
        result.messages[1].message_index
    );
    assert_ne!(
        result.messages[0].entry_index,
        result.messages[1].entry_index
    );
}

// ----------------------------------------------------------------------------
// ISSUE 20: Redacted Thinking Block Handling
// ----------------------------------------------------------------------------
// Redacted thinking blocks should be preserved.

#[test]
fn test_regression_redacted_thinking_preserved() {
    let t0 = fixed_time();

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [
                {"type": "redacted_thinking", "data": "encrypted_data_here"},
                {"type": "text", "text": "Here is my answer."}
            ],
            "finish_reason": "stop"
        }
    }]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    let types: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.entry_type.as_str())
        .collect();
    assert!(
        types.contains(&"redacted_thinking"),
        "Redacted thinking should be preserved. Found: {:?}",
        types
    );
}

// ----------------------------------------------------------------------------
// ISSUE 21: Same Timestamp Different Content
// ----------------------------------------------------------------------------
// Different content at exact same timestamp should both be preserved.

#[test]
fn test_regression_same_timestamp_different_content() {
    let t0 = fixed_time();

    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Both should be preserved (different content)
    assert_eq!(
        result.messages.len(),
        2,
        "Different content at same timestamp should both be preserved"
    );
}

// ----------------------------------------------------------------------------
// ISSUE 22: Very Long Content Hashing
// ----------------------------------------------------------------------------
// Very long content should hash consistently without truncation issues.

#[test]
fn test_regression_long_content_hashing() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Create a long message (10KB)
    let long_text: String = "A".repeat(10_000);

    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": &long_text}
    }]);

    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": &long_text}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should deduplicate
    assert_eq!(
        result.messages.len(),
        1,
        "Long content should deduplicate correctly"
    );
}

// ----------------------------------------------------------------------------
// ISSUE 23: Tool Use Followed by Immediate Text Response
// ----------------------------------------------------------------------------
// When LLM outputs tool_use and text in same response, both should be preserved.

#[test]
fn test_regression_tool_use_with_text_response() {
    let t0 = fixed_time();

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me search for that."},
                {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}
            ],
            "finish_reason": "tool_use"
        }
    }]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    // Should have both blocks
    assert_eq!(result.messages.len(), 2);
    let types: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.entry_type.as_str())
        .collect();
    assert!(types.contains(&"text"));
    assert!(types.contains(&"tool_use"));
}

// ----------------------------------------------------------------------------
// ISSUE 24: Multiple Tool Results for Same Tool Use ID
// ----------------------------------------------------------------------------
// If somehow two different results reference same tool_use_id, both should be handled.

#[test]
fn test_regression_duplicate_tool_use_id_different_content() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // First tool result
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "First result"}
    }]);

    // Second tool result (same ID, different content — anomalous but tool_use_id is identity)
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "Second result"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Same tool_use_id = same logical tool execution → deduped to 1.
    // tool_use_id is the primary identity signal for tool results.
    assert_eq!(
        result.messages.len(),
        1,
        "Same tool_use_id should dedup regardless of content differences"
    );
}

// ----------------------------------------------------------------------------
// ISSUE 25: Context Block Handling
// ----------------------------------------------------------------------------
// Context blocks should be preserved as-is.

#[test]
fn test_regression_context_block_preserved() {
    let t0 = fixed_time();

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "user",
            "content": [
                {"type": "context", "context_type": "file", "data": {"path": "/test.txt", "content": "test content"}}
            ]
        }
    }]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].entry_type, "context");
}

// ----------------------------------------------------------------------------
// ISSUE 26: Event Source vs Attribute Source Both Handled
// ----------------------------------------------------------------------------
// Both event and attribute sources are valid message sources.
// When duplicates exist, quality scoring prefers event source.

#[test]
fn test_regression_event_and_attribute_sources_handled() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Attribute source - needs timestamp for processing
    let msg1 = json!([{
        "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello from attribute"}
    }]);

    // Event source
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello from event"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have 2 messages (different content)
    assert_eq!(
        result.messages.len(),
        2,
        "Should have 2 messages (different content). Found: {:?}",
        result
            .messages
            .iter()
            .map(|m| (&m.source_type, &m.span_id))
            .collect::<Vec<_>>()
    );

    // Verify both source types are represented
    let source_types: std::collections::HashSet<_> = result
        .messages
        .iter()
        .map(|m| m.source_type.as_str())
        .collect();
    assert!(
        source_types.contains("attribute"),
        "Should have attribute source"
    );
    assert!(source_types.contains("event"), "Should have event source");
}

// ----------------------------------------------------------------------------
// ISSUE 27: Finish Reason Preservation in Dedup
// ----------------------------------------------------------------------------
// When deduplicating, the version with finish_reason should be kept.

#[test]
fn test_regression_finish_reason_preserved_in_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Without finish_reason (in later span - will be marked as history duplicate)
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "The answer"}
    }]);

    // With finish_reason (in earlier span - original occurrence)
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "The answer", "finish_reason": "stop"}
    }]);

    let rows = vec![
        // Span with finish_reason comes first (lower timestamp)
        make_span_row_with_timestamps("trace1", "span1", None, &msg2.to_string(), t0, Some(t0)),
        // Span without finish_reason comes later
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg1.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should deduplicate to 1 with finish_reason
    assert_eq!(
        result.messages.len(),
        1,
        "Should deduplicate to 1. Found: {}",
        result.messages.len()
    );
    assert!(
        result.messages[0].finish_reason.is_some(),
        "Version with finish_reason should be kept. finish_reason: {:?}",
        result.messages[0].finish_reason
    );
}

// ----------------------------------------------------------------------------
// ISSUE 28: Model Info Preservation in Dedup
// ----------------------------------------------------------------------------
// When deduplicating, the version with model info should be kept.

#[test]
fn test_regression_model_info_preserved_in_dedup() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);

    // First span with model info
    let mut row1 =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));
    row1.model = Some("claude-3-opus".to_string()); // Has model info

    // Second span without model info (duplicate message)
    let mut row2 = make_span_row_with_timestamps(
        "trace1",
        "span2",
        Some("span1"),
        &msg.to_string(),
        t1,
        Some(t1),
    );
    row2.model = None; // No model info

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2], &options);

    // Should deduplicate to 1 with model info
    assert_eq!(
        result.messages.len(),
        1,
        "Should deduplicate to 1. Found: {}",
        result.messages.len()
    );
    assert!(
        result.messages[0].model.is_some(),
        "Version with model info should be kept. Model: {:?}",
        result.messages[0].model
    );
}

// ----------------------------------------------------------------------------
// Additional helper tests for specific edge cases
// ----------------------------------------------------------------------------

#[test]
fn test_tool_result_text_vs_array_normalization() {
    // Tool result content can be string, object, or array
    // All forms representing same data should produce same identity
    let t0 = fixed_time();

    // Form 1: plain string
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "Result text"}
    }]);

    // Form 2: array with text block
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": [{"type": "text", "text": "Result text"}]}
    }]);

    let row1 =
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0));
    let row2 = make_span_row_with_timestamps(
        "trace1",
        "span2",
        Some("span1"),
        &msg2.to_string(),
        t0,
        Some(t0),
    );

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2], &options);

    // Both forms should deduplicate to 1
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Tool)
        .collect();

    // This test documents current behavior - it may fail if normalization isn't implemented
    assert_eq!(
        tool_results.len(),
        1,
        "Tool results with same semantic content should deduplicate. Found {}:\n{:?}",
        tool_results.len(),
        tool_results
            .iter()
            .map(|m| format!("span={}, hash={}", m.span_id, m.content_hash))
            .collect::<Vec<_>>()
    );
}

// ============================================================================
// PHASE 3 HISTORY DETECTION REGRESSION TESTS
// ============================================================================
// Tests for GenAI input events from non-generation spans being marked as history.
// This catches cross-trace session history that Strands includes in event loop spans.

/// Helper to create a span row with specific observation_type
fn make_span_row_with_observation_type(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    messages_json: &str,
    span_start: chrono::DateTime<Utc>,
    span_end: Option<chrono::DateTime<Utc>>,
    observation_type: &str,
) -> MessageSpanRow {
    let mut row = make_span_row_with_timestamps(
        trace_id,
        span_id,
        parent_span_id,
        messages_json,
        span_start,
        span_end,
    );
    row.observation_type = Some(observation_type.to_string());
    row
}

// ----------------------------------------------------------------------------
// ISSUE 29: Session History in Event Loop Spans
// ----------------------------------------------------------------------------
// Strands includes previous session turns in event loop spans (observation_type="span").
// These should be filtered as history.

#[test]
fn test_regression_session_history_in_event_loop_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Root agent span with current request
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a weather assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in London?"}
        }
    ]);

    // Event loop span with session history (previous NYC request)
    // This is what Strands does - accumulates all previous turns
    let event_loop_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in NYC?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC is sunny today."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in London?"}
        }
    ]);

    // Generation span with actual LLM output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {"role": "assistant", "content": "London is rainy today.", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t2),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t2),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("event_loop"),
            &gen_msg.to_string(),
            t1,
            Some(t2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain NYC messages (session history)
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("NYC")),
        "Session history (NYC) should be filtered. Found: {:?}",
        texts
    );

    // Should only have: system, user (London), assistant (London response)
    assert_eq!(
        result.messages.len(),
        3,
        "Should have 3 messages (system, user, assistant). Found {}:\n{:?}",
        result.messages.len(),
        texts
    );
}

// ----------------------------------------------------------------------------
// ISSUE 30: Generation Span Messages Not Filtered
// ----------------------------------------------------------------------------
// Messages from generation spans should NOT be filtered, even if they have
// GenAI input event names.

#[test]
fn test_regression_generation_span_messages_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Generation span with user input and assistant output
    let gen_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there!", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type("trace1", "root", None, "[]", t0, Some(t1), "agent"),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("root"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Both messages from generation span should be preserved
    assert_eq!(
        result.messages.len(),
        2,
        "Messages from generation span should be preserved. Found {}",
        result.messages.len()
    );

    let roles: Vec<_> = result.messages.iter().map(|m| m.role).collect();
    assert!(roles.contains(&ChatRole::User));
    assert!(roles.contains(&ChatRole::Assistant));
}

// ----------------------------------------------------------------------------
// ISSUE 31: Root Span Messages Not Filtered
// ----------------------------------------------------------------------------
// Messages from root spans should NOT be filtered, regardless of event name.

#[test]
fn test_regression_root_span_messages_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root span with user input (even though it has GenAI input event name)
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Root span user message"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Response", "finish_reason": "stop"}
        }
    ]);

    // Even with observation_type="span", root should not be filtered
    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "root",
        None,
        &root_msg.to_string(),
        t0,
        Some(t1),
        "span",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Root span messages should be preserved
    assert_eq!(
        result.messages.len(),
        2,
        "Root span messages should be preserved. Found {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// ISSUE 32: Chain Span History Filtered
// ----------------------------------------------------------------------------
// GenAI input events from chain spans (observation_type="chain") should be filtered.

#[test]
fn test_regression_chain_span_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root span
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Current request"}
    }]);

    // Chain span with accumulated history
    let chain_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Previous request"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Previous response"}
        }
    ]);

    // Generation span with output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Current response", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "chain",
            Some("root"),
            &chain_msg.to_string(),
            t0,
            Some(t1),
            "chain",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("chain"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain "Previous" messages from chain span
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Previous")),
        "Chain span history should be filtered. Found: {:?}",
        texts
    );

    // Should have: user (Current request), assistant (Current response)
    assert_eq!(result.messages.len(), 2);
}

// ----------------------------------------------------------------------------
// ISSUE 33: Agent Span History Filtered
// ----------------------------------------------------------------------------
// GenAI input events from agent spans (observation_type="agent") in non-root
// position should be filtered.

#[test]
fn test_regression_nested_agent_span_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root agent span with current request
    let root_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Main request"}
    }]);

    // Nested agent span (sub-agent) with history
    let sub_agent_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Sub-agent history"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Sub-agent previous response"}
        }
    ]);

    // Generation span with final output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Final response", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "sub_agent",
            Some("root"),
            &sub_agent_msg.to_string(),
            t0,
            Some(t1),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("sub_agent"),
            &gen_msg.to_string(),
            t0,
            Some(t1),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain sub-agent history
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Sub-agent")),
        "Nested agent span history should be filtered. Found: {:?}",
        texts
    );

    // Should have: user (Main request), assistant (Final response)
    assert_eq!(result.messages.len(), 2);
}

// ----------------------------------------------------------------------------
// ISSUE 34: Output Events Not Filtered
// ----------------------------------------------------------------------------
// gen_ai.choice events should NOT be filtered even from non-generation spans.

#[test]
fn test_regression_output_events_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Span with both input and output events
    let span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History input"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Current output", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type("trace1", "root", None, "[]", t0, Some(t1), "agent"),
        make_span_row_with_observation_type(
            "trace1",
            "span",
            Some("root"),
            &span_msg.to_string(),
            t0,
            Some(t1),
            "span",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // gen_ai.choice output should be preserved (only input filtered)
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.iter().any(|t| t.contains("Current output")),
        "Output events should be preserved. Found: {:?}",
        texts
    );

    // Input should be filtered
    assert!(
        !texts.iter().any(|t| t.contains("History input")),
        "Input events from span should be filtered. Found: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// ISSUE 35: Multi-Turn Session History (Real-World Strands Pattern)
// ----------------------------------------------------------------------------
// Simulates a Strands trace with multiple previous session turns.

#[test]
fn test_regression_multi_turn_session_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(3);

    // Root agent span with current request
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Third question"}
        }
    ]);

    // Event loop span with ALL previous session turns
    let event_loop_msg = json!([
        // Turn 1
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        // Turn 2
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
        },
        // Current turn (also appears here)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Third question"}
        }
    ]);

    // Generation span with current output
    let gen_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Third answer", "finish_reason": "stop"}
    }]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t_end),
            "agent",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t_end),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("event_loop"),
            &gen_msg.to_string(),
            t2,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should NOT contain first/second turn history
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("First")),
        "First turn should be filtered. Found: {:?}",
        texts
    );
    assert!(
        !texts.iter().any(|t| t.contains("Second")),
        "Second turn should be filtered. Found: {:?}",
        texts
    );

    // Should have: system, user (Third question), assistant (Third answer)
    assert_eq!(
        result.messages.len(),
        3,
        "Should have 3 messages for current turn. Found {}:\n{:?}",
        result.messages.len(),
        texts
    );
}

/// Regression #36: System message should appear before user message when timestamps are equal.
///
/// Some frameworks (Strands) record system and user messages with the same timestamp.
/// Semantic ordering should ensure System comes first (sets context), then User (provides input).
#[test]
fn test_regression_system_before_user_same_timestamp() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Both messages have the exact same timestamp
    // System comes first in the array (message_index 0)
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "span1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(result.messages.len(), 2, "Should have 2 messages");

    // System should be first (message_index 0), User second (message_index 1)
    assert_eq!(
        result.messages[0].role,
        ChatRole::System,
        "First message should be System, got {:?}",
        result.messages[0].role
    );
    assert_eq!(
        result.messages[1].role,
        ChatRole::User,
        "Second message should be User, got {:?}",
        result.messages[1].role
    );

    // Verify the content
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text } if text.contains("helpful assistant")),
        "System message content mismatch"
    );
    assert!(
        matches!(&result.messages[1].content, ContentBlock::Text { text } if text == "Hello"),
        "User message content mismatch"
    );
}

// ============================================================================
// OUTPUT CLASSIFICATION AND HISTORY PROTECTION TESTS
// ============================================================================

/// Regression #37: gen_ai.choice events are ALWAYS protected from history marking.
///
/// Even if a gen_ai.choice event appears in a non-generation span with a parent,
/// it should NOT be marked as history. This protects actual LLM outputs.
#[test]
fn test_regression_gen_ai_choice_never_history() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // A gen_ai.choice event in a span with parent - should NOT be marked as history
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
        "content": {"role": "assistant", "content": "LLM response", "finish_reason": "stop"}
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "child_span",
        Some("parent"), // Has parent
        &messages.to_string(),
        t0,
        Some(t_end),
        "span", // Non-generation span type that normally triggers Phase 3
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The gen_ai.choice event should be preserved (protected from history)
    assert_eq!(
        result.messages.len(),
        1,
        "gen_ai.choice should not be filtered"
    );
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text } if text == "LLM response"),
        "gen_ai.choice content should be preserved"
    );
}

/// Regression #38: gen_ai.assistant.message events CAN be marked as history.
///
/// Unlike gen_ai.choice (actual LLM output), gen_ai.assistant.message is used for
/// history re-sends. These SHOULD be marked as history when in non-generation spans.
#[test]
fn test_regression_gen_ai_assistant_message_can_be_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Root span with current request (has gen_ai.choice output)
    let root_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Current request"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Current response", "finish_reason": "stop"}
        }
    ]);

    // Event loop span with history (gen_ai.assistant.message - NOT gen_ai.choice)
    let event_loop_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Previous response from history"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &event_loop_msg.to_string(),
            t0,
            Some(t_end),
            "span", // Non-generation span
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have: user (Current request), assistant (Current response)
    // Should NOT have: "Previous response from history"
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !texts.iter().any(|t| t.contains("Previous response")),
        "gen_ai.assistant.message should be filtered as history. Found: {:?}",
        texts
    );
    assert!(
        texts.iter().any(|t| t.contains("Current response")),
        "gen_ai.choice should be preserved. Found: {:?}",
        texts
    );
}

/// Regression #39: uses_span_end field is correctly set for different block types.
///
/// Verifies the output classification rules:
/// - gen_ai.choice → uses_span_end = true
/// - Assistant text → uses_span_end = true
/// - ToolUse from non-tool span → uses_span_end = true
/// - User message → uses_span_end = false
/// - Tool result → uses_span_end = false
#[test]
fn test_regression_uses_span_end_classification() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Mix of different message types
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "User message"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_end.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Response text"},
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "span1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find blocks by type and verify uses_span_end
    let user_block = result.messages.iter().find(|b| b.role == ChatRole::User);
    let assistant_text = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant && b.entry_type == "text");
    let tool_use = result.messages.iter().find(|b| b.entry_type == "tool_use");

    assert!(user_block.is_some(), "Should have user message");
    assert!(assistant_text.is_some(), "Should have assistant text");
    assert!(tool_use.is_some(), "Should have tool_use");

    // Verify uses_span_end flags
    assert!(
        !user_block.unwrap().uses_span_end,
        "User message should NOT be output"
    );
    assert!(
        assistant_text.unwrap().uses_span_end,
        "Assistant text from gen_ai.choice should be output"
    );
    // NOTE: ToolUse ALWAYS uses event_time (uses_span_end=false), even from gen_ai.choice events.
    // This is critical for correct ordering: ToolUse at event_time T=100 must sort BEFORE
    // ToolResult at span_end T=200. If ToolUse used span_end, it would sort AFTER ToolResult.
    assert!(
        !tool_use.unwrap().uses_span_end,
        "ToolUse should use event_time (not output) for correct ordering"
    );
}

/// Regression #40: ToolUse from tool spans is INPUT, not OUTPUT.
///
/// Tool spans log tool invocation (INPUT). The tool_use is output only if it
/// comes from a gen_ai.choice event (the LLM's completion marker).
#[test]
fn test_regression_tool_use_from_tool_span_is_input() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Tool span with tool_use (logging the call)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}]
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "tool_span",
        Some("parent"),
        &tool_span_msg.to_string(),
        t0,
        Some(t_end),
        "tool", // Tool span
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The tool_use should be present and marked as INPUT
    let tool_use = result.messages.iter().find(|b| b.entry_type == "tool_use");
    assert!(tool_use.is_some(), "Tool use should be preserved");
    assert!(
        !tool_use.unwrap().uses_span_end,
        "ToolUse from tool span should be INPUT (not OUTPUT)"
    );
}

/// Regression #41: ToolResult from tool spans is OUTPUT, uses span_end for ordering.
///
/// Tool results from tool spans represent the actual tool execution result.
/// They should use span_end for effective timestamp (when tool finished).
#[test]
fn test_regression_tool_result_from_tool_span_uses_span_end() {
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(1);

    // Tool span with tool_result (actual execution)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result"}]
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "tool_span",
        Some("parent"),
        &tool_span_msg.to_string(),
        t0,
        Some(t_end),
        "tool", // Tool span
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // The tool_result should be present and marked as OUTPUT
    let tool_result = result
        .messages
        .iter()
        .find(|b| b.entry_type == "tool_result");
    assert!(tool_result.is_some(), "Tool result should be preserved");
    assert!(
        tool_result.unwrap().uses_span_end,
        "ToolResult from tool span should be OUTPUT"
    );
}

/// Regression #42: Tool ordering - tool_use ALWAYS before tool_result.
///
/// Even when history copies of tool_results have earlier timestamps,
/// the ordering should still be: tool_use → tool_result.
/// This is achieved by:
/// 1. ToolResult from tool spans uses span_end (when tool finished)
/// 2. History copies are filtered and don't affect birth_time
#[test]
fn test_regression_tool_use_before_tool_result_with_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t4 = t0 + chrono::Duration::milliseconds(400);
    let t5 = t0 + chrono::Duration::milliseconds(500);

    // Generation span with tool_use (LLM decision)
    let gen_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // Tool span with tool_result (actual execution)
    let tool_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t4.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
        }
    }]);

    // Event loop span with history copy of tool_result (misleading early timestamp!)
    let t_early = t0 + chrono::Duration::milliseconds(50); // Very early!
    let history_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t_early.to_rfc3339()}},
        "content": {
            "role": "tool",
            "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
        }
    }]);

    let rows = vec![
        // Generation span (tool_use)
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("root"),
            &gen_span_msg.to_string(),
            t0,
            Some(t2), // span_end = T2
            "generation",
        ),
        // Tool span (actual tool_result)
        make_span_row_with_observation_type(
            "trace1",
            "tool_span",
            Some("gen_span"),
            &tool_span_msg.to_string(),
            t3,
            Some(t5), // span_end = T5
            "tool",
        ),
        // Event loop span (history copy with early timestamp)
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("root"),
            &history_span_msg.to_string(),
            t0,
            Some(t0 + chrono::Duration::seconds(10)),
            "span", // Non-generation span
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find the final tool_use and tool_result blocks
    let tool_use_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_use");
    let tool_result_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_result");

    assert!(tool_use_idx.is_some(), "Should have tool_use");
    assert!(tool_result_idx.is_some(), "Should have tool_result");

    // CRITICAL: tool_use must come BEFORE tool_result
    assert!(
        tool_use_idx.unwrap() < tool_result_idx.unwrap(),
        "tool_use (index {}) should come before tool_result (index {})",
        tool_use_idx.unwrap(),
        tool_result_idx.unwrap()
    );
}

/// Regression #43: Tool ordering in same span (Strands scenario).
///
/// In Strands and similar frameworks, tool_use and tool_result can appear
/// in the SAME generation span with different event timestamps:
/// - tool_use at T=100 (LLM decided to call tool)
/// - tool_result at T=200 (tool returned)
/// - final_response at T=300 (LLM finished with response)
/// - span_end at T=300
///
/// If tool_use incorrectly used span_end (T=300), it would sort AFTER
/// tool_result (T=200), breaking the conversation flow.
///
/// This test ensures tool_use uses event_time, not span_end.
#[test]
fn test_regression_tool_use_and_tool_result_same_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100); // tool_use event
    let t2 = t0 + chrono::Duration::milliseconds(200); // tool_result event
    let t3 = t0 + chrono::Duration::milliseconds(300); // final_response event
    let t_end = t3; // span_end

    // Single generation span with tool_use, tool_result, and final response
    // This mimics Strands behavior where all messages are in one span
    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "test"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result data"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Here is your result"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have 3 blocks: tool_use, tool_result, final_response
    assert_eq!(result.messages.len(), 3, "Should have 3 blocks");

    // Find indices
    let tool_use_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_use");
    let tool_result_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "tool_result");
    let text_idx = result
        .messages
        .iter()
        .position(|b| b.entry_type == "text" && b.finish_reason.is_some());

    assert!(tool_use_idx.is_some(), "Should have tool_use");
    assert!(tool_result_idx.is_some(), "Should have tool_result");
    assert!(text_idx.is_some(), "Should have final text");

    // CRITICAL: Correct ordering must be: tool_use < tool_result < final_response
    assert!(
        tool_use_idx.unwrap() < tool_result_idx.unwrap(),
        "tool_use (idx {}) must come before tool_result (idx {})",
        tool_use_idx.unwrap(),
        tool_result_idx.unwrap()
    );
    assert!(
        tool_result_idx.unwrap() < text_idx.unwrap(),
        "tool_result (idx {}) must come before final text (idx {})",
        tool_result_idx.unwrap(),
        text_idx.unwrap()
    );

    // Verify uses_span_end classification
    let tool_use = &result.messages[tool_use_idx.unwrap()];
    let tool_result = &result.messages[tool_result_idx.unwrap()];
    let final_text = &result.messages[text_idx.unwrap()];

    // tool_use from gen_ai.assistant.message (not gen_ai.choice) should NOT be uses_span_end
    // because it doesn't have finish_reason and isn't a completion marker
    assert!(
        !tool_use.uses_span_end,
        "tool_use from gen_ai.assistant.message should NOT be uses_span_end"
    );

    // tool_result from generation span should NOT be uses_span_end
    // (only tool_result from tool spans is uses_span_end)
    assert!(
        !tool_result.uses_span_end,
        "tool_result from generation span should NOT be uses_span_end"
    );

    // final_text from gen_ai.choice with finish_reason should be uses_span_end
    assert!(
        final_text.uses_span_end,
        "final_text from gen_ai.choice should be uses_span_end"
    );
}

/// Regression #44: Parallel tool calls ordering (multiple tools same timestamp).
///
/// When LLM calls multiple tools in parallel, all tool_use blocks have
/// the same event_time. They should maintain their message_index order
/// and all come before their corresponding tool_results.
#[test]
fn test_regression_parallel_tools_same_span_ordering() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100); // both tool_use events
    let t2 = t0 + chrono::Duration::milliseconds(200); // both tool_result events
    let t3 = t0 + chrono::Duration::milliseconds(300); // final response
    let t_end = t3;

    let messages = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "temperature", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_2", "name": "precipitation", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "72F"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_2", "content": "20%"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Temperature: 72F, Precipitation: 20%"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect indices by type
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "tool_use")
        .collect();
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "tool_result")
        .collect();
    let final_text: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, b)| b.entry_type == "text" && b.finish_reason.is_some())
        .collect();

    assert_eq!(tool_uses.len(), 2, "Should have 2 tool_use blocks");
    assert_eq!(tool_results.len(), 2, "Should have 2 tool_result blocks");
    assert_eq!(final_text.len(), 1, "Should have 1 final text block");

    // All tool_uses should come before all tool_results
    let max_tool_use_idx = tool_uses.iter().map(|(i, _)| *i).max().unwrap();
    let min_tool_result_idx = tool_results.iter().map(|(i, _)| *i).min().unwrap();
    assert!(
        max_tool_use_idx < min_tool_result_idx,
        "All tool_uses (max idx {}) must come before all tool_results (min idx {})",
        max_tool_use_idx,
        min_tool_result_idx
    );

    // All tool_results should come before final text
    let max_tool_result_idx = tool_results.iter().map(|(i, _)| *i).max().unwrap();
    let final_text_idx = final_text[0].0;
    assert!(
        max_tool_result_idx < final_text_idx,
        "All tool_results (max idx {}) must come before final text (idx {})",
        max_tool_result_idx,
        final_text_idx
    );
}

/// Regression #45: ToolUse without explicit completion marker uses event_time.
///
/// When tool_use comes from gen_ai.assistant.message (not gen_ai.choice),
/// it should use event_time for ordering, not span_end.
/// This is critical for correct tool ordering within a span.
#[test]
fn test_regression_tool_use_from_assistant_message_uses_event_time() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t_end = t0 + chrono::Duration::milliseconds(500); // Much later span_end

    // tool_use from gen_ai.assistant.message (not gen_ai.choice)
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {}}]
        }
    }]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen_span",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(result.messages.len(), 1);
    let block = &result.messages[0];

    // Should NOT be uses_span_end (no gen_ai.choice event, no finish_reason)
    assert!(
        !block.uses_span_end,
        "tool_use from gen_ai.assistant.message should NOT be uses_span_end"
    );

    // Category should be GenAIAssistantMessage, not GenAIChoice
    assert_eq!(
        block.category,
        crate::data::types::MessageCategory::GenAIAssistantMessage
    );
}

/// Regression #46: Intermediate assistant text from generation spans is filtered.
///
/// In Strands tool-use loops:
/// - Generation span produces intermediate text (gen_ai.assistant.message)
/// - Agent span produces final response (gen_ai.choice)
///
/// The intermediate text should be filtered to show only the final response.
/// This prevents duplicate/intermediate outputs during tool-use cycles.
#[test]
fn test_regression_intermediate_assistant_text_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span: intermediate assistant text (NOT the final response)
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Intermediate output during tool use"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {}}]
            }
        }
    ]);

    // Agent span: final response via gen_ai.choice
    let agent_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Final response after tools"}],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        // Agent span (root) with final choice
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        // Generation span with intermediate output
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("agent_span"),
            &gen_span_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Should have tool_use and final text, but NOT intermediate text
    let text_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "text" && b.role == ChatRole::Assistant)
        .collect();

    assert_eq!(
        text_blocks.len(),
        1,
        "Should have exactly 1 assistant text (final only)"
    );

    let final_text = text_blocks[0];
    assert_eq!(
        final_text.category,
        crate::data::types::MessageCategory::GenAIChoice,
        "Should be from GenAIChoice (final response)"
    );
    assert!(
        matches!(&final_text.content, ContentBlock::Text { text } if text.contains("Final response")),
        "Should be the final response text"
    );

    // Tool use should still be present
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();
    assert_eq!(tool_uses.len(), 1, "Should have tool_use");
}

/// Regression #47: Multi-turn session history filtered from generation spans.
///
/// In Strands multi-turn sessions, generation spans contain FULL conversation history
/// including tool calls and results from previous turns. These should be filtered
/// so only the current turn's messages appear.
///
/// Key insight: Tool results in generation spans indicate session history is present.
/// Current turn output uses gen_ai.choice (GenAIChoice category) which is protected.
#[test]
fn test_regression_multi_turn_session_history_in_generation_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span contains:
    // 1. Previous turn tool calls (GenAIAssistantMessage) - HISTORY
    // 2. Previous turn tool results (GenAIToolMessage) - HISTORY
    // 3. Current turn tool calls (GenAIChoice) - CURRENT
    let gen_span_msg = json!([
        // Previous turn tool call (history)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "old_call_1", "name": "search", "input": {"query": "NYC"}}]
            }
        },
        // Previous turn tool result (history) - KEY SIGNAL for session history detection
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_call_id": "old_call_1",
                "content": "NYC weather: sunny"
            }
        },
        // Current turn tool call (gen_ai.choice = protected)
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "new_call_1", "name": "search", "input": {"query": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Agent span: current turn user message and final response
    let agent_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "LA weather"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "LA is sunny"}],
                "finish_reason": "stop"
            }
        }
    ]);

    // Event loop span: current turn tool result (execution output)
    let event_loop_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_call_id": "new_call_1",
            "content": "LA weather: sunny"
        }
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "event_loop",
            Some("agent_span"),
            &event_loop_msg.to_string(),
            t0,
            Some(t3),
            "span",
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("event_loop"),
            &gen_span_msg.to_string(),
            t0,
            Some(t2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count tool_use blocks - should only have LA (current turn), not NYC (history)
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Should have exactly 1 tool_use (current turn only). Found: {:?}",
        tool_uses.iter().map(|b| &b.content).collect::<Vec<_>>()
    );

    // Verify it's the LA tool call (current turn)
    if let ContentBlock::ToolUse { input, .. } = &tool_uses[0].content {
        assert_eq!(
            input.get("query").and_then(|v| v.as_str()),
            Some("LA"),
            "Should be LA tool call (current turn)"
        );
    } else {
        panic!("Expected tool_use content block");
    }

    // Count tool_result blocks - should only have LA (current turn), not NYC (history)
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "Should have exactly 1 tool_result (current turn only)"
    );

    // Should have user message for current turn
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();
    assert_eq!(user_msgs.len(), 1, "Should have 1 user message");

    // Should have final assistant text
    let assistant_text: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant && b.entry_type == "text")
        .collect();
    assert_eq!(
        assistant_text.len(),
        1,
        "Should have 1 final assistant text"
    );
}

// ============================================================================
// REGRESSION TESTS FOR TIMESTAMP-BASED HISTORY DETECTION
// ============================================================================

/// Regression #48: Timestamp-based history detection.
///
/// Messages with timestamp < span_start in child generation spans should be
/// marked as history. This is the fundamental signal for detecting historical
/// context that was passed to the LLM.
#[test]
fn test_regression_timestamp_based_history_detection() {
    let t0 = fixed_time();
    let t_history = t0 - chrono::Duration::seconds(10); // Before span start
    let t_current = t0 + chrono::Duration::seconds(1); // After span start
    let t_end = t0 + chrono::Duration::seconds(2);

    // Generation span with both historical and current content
    let gen_span_msg = json!([
        // Historical message (timestamp before span start) - should be filtered
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t_history.to_rfc3339()}},
            "content": {"role": "user", "content": "Old question from history"}
        },
        // Current message (timestamp after span start) - should be kept
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t_current.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": "Current response",
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen_span",
        Some("parent"),
        &gen_span_msg.to_string(),
        t0, // Span starts at t0
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Historical message should be filtered (timestamp < span_start)
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();
    assert!(
        user_msgs.is_empty(),
        "Historical user message (timestamp < span_start) should be filtered"
    );

    // Current response should be preserved (protected by gen_ai.choice)
    let assistant_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        assistant_msgs.len(),
        1,
        "Current response should be preserved"
    );
}

/// Regression #49: Child spans with different content preserved.
///
/// When parent and child spans have genuinely different content,
/// both should be preserved. The history detection should NOT filter
/// new content just because it doesn't exist in parent spans.
#[test]
fn test_regression_child_span_new_content_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Parent span with one message
    let parent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Question in parent"}
    }]);

    // Child span with different (new) content - NOT a history copy
    let child_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Response in child",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        make_span_row_with_timestamps(
            "trace1",
            "parent",
            None,
            &parent_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "child",
            Some("parent"),
            &child_msg.to_string(),
            t1,
            Some(t2),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Both messages should be preserved (different content)
    assert_eq!(
        result.messages.len(),
        2,
        "Both parent and child content should be preserved when different"
    );

    let roles: Vec<_> = result.messages.iter().map(|m| m.role).collect();
    assert!(roles.contains(&ChatRole::User), "User message should exist");
    assert!(
        roles.contains(&ChatRole::Assistant),
        "Assistant message should exist"
    );
}

/// Regression #50: Tool_use preserved when intermediate text is filtered.
///
/// In generation spans, intermediate assistant text should be filtered
/// but tool_use blocks should be preserved. This is because tool_use
/// represents actual LLM output (a decision to call a tool).
#[test]
fn test_regression_tool_use_preserved_text_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span with intermediate text AND tool_use
    let gen_span_msg = json!([
        // Intermediate text (should be filtered)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Let me search for that"}]
            }
        },
        // Tool use (should be preserved - it's actual output)
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "search", "input": {"query": "test"}}]
            }
        }
    ]);

    // Agent span with final response (triggers has_agent_spans and provides final output)
    let agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Here are the results",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent",
            None,
            &agent_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("agent"),
            &gen_span_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Tool_use should be preserved
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();
    assert_eq!(
        tool_uses.len(),
        1,
        "Tool_use should be preserved even when intermediate text is filtered"
    );

    // Intermediate text should be filtered, but final response (gen_ai.choice) preserved
    let texts: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "text" && b.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        texts.len(),
        1,
        "Should have 1 text (final response), intermediate filtered"
    );
    assert!(
        matches!(&texts[0].content, ContentBlock::Text { text } if text.contains("results")),
        "Should be final response, not intermediate text"
    );
}

/// Regression #51: Multi-turn session history with tool operations.
///
/// In multi-turn sessions (Strands-like), generation spans contain full
/// conversation history including tool calls and results from previous turns.
/// These should be filtered, keeping only current turn output.
///
/// This test specifically covers the original bug where trace
/// e91ae2156c0bb2242e34b507c68374e1 (LA weather) was showing NYC tool
/// calls/results from previous turns.
#[test]
fn test_regression_multi_turn_tool_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t3 = t0 + chrono::Duration::milliseconds(300);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span contains:
    // 1. Previous turn: user question about NYC
    // 2. Previous turn: tool call for NYC
    // 3. Previous turn: tool result for NYC
    // 4. Previous turn: assistant response about NYC
    // 5. Current turn: user question about LA
    // 6. Current turn: tool call for LA (gen_ai.choice - protected)
    let gen_span_msg = json!([
        // PREVIOUS TURN (all should be filtered)
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in NYC?"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "old_call", "name": "weather", "input": {"city": "NYC"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
            "content": {"role": "tool", "tool_call_id": "old_call", "content": "NYC: Rainy"}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC is rainy today."}
        },
        // CURRENT TURN
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "What's the weather in LA?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "new_call", "name": "weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Agent span with current turn user message
    let agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "What's the weather in LA?"}
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent",
            None,
            &agent_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen",
            Some("agent"),
            &gen_span_msg.to_string(),
            t1, // Span starts AFTER previous turn timestamps (t0)
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Check that NYC content is NOT present
    let all_text: Vec<_> = result
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !all_text.iter().any(|t| t.contains("NYC")),
        "NYC messages from previous turn should NOT appear. Found: {:?}",
        all_text
    );

    // Check that LA tool_use IS present
    let tool_uses: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Should have exactly 1 tool_use (LA, current turn)"
    );

    if let ContentBlock::ToolUse { input, .. } = &tool_uses[0].content {
        assert_eq!(
            input.get("city").and_then(|v| v.as_str()),
            Some("LA"),
            "Tool_use should be for LA (current turn), not NYC (history)"
        );
    }

    // Should have only LA user message (from agent span)
    let user_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(user_msgs.len(), 1, "Should have exactly 1 user message");
    assert!(
        user_msgs[0].contains("LA"),
        "User message should be about LA"
    );
}

/// Regression #52: Cross-trace tool_use_id contamination in sessions.
///
/// When processing a session with multiple traces, the orphan tool_result
/// detection should work per-trace. Tool_use_ids from previous traces should
/// NOT be considered "current" for subsequent traces.
///
/// This test simulates a session where:
/// - Trace 1: Has tool_use with ID "call_old"
/// - Trace 2: Has tool_result with ID "call_old" (session history from trace 1),
///   AND tool_use/result with ID "call_new" (current turn)
///
/// The "call_old" tool_result in trace 2 should be filtered as orphan.
#[test]
fn test_regression_cross_trace_tool_id_contamination() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(5);
    let t2 = t1 + chrono::Duration::seconds(1);
    let t_end1 = t0 + chrono::Duration::seconds(4);
    let t_end2 = t1 + chrono::Duration::seconds(2);

    // TRACE 1: Has tool_use with ID "call_old"
    let trace1_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "call_old", "name": "search", "input": {"q": "NYC"}}],
            "finish_reason": "tool_use"
        }
    }]);

    // TRACE 2: Generation span with session history (call_old) and current turn (call_new)
    let trace2_gen_msg = json!([
        // Session history from trace 1 (should be filtered as orphan)
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
            "content": {"role": "tool", "tool_call_id": "call_old", "content": "NYC weather"}
        },
        // Current turn (protected by gen_ai.choice)
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_new", "name": "search", "input": {"q": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    let trace2_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "What's the weather in LA?"}
    }]);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &trace1_agent_msg.to_string(),
            t0,
            Some(t_end1),
            Some("agent"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &trace2_agent_msg.to_string(),
            t1,
            Some(t_end2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen_msg.to_string(),
            t1,
            Some(t_end2),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Filter to trace2 messages only
    let trace2_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.trace_id == "trace2")
        .collect();

    // Should have: user message + current tool_use (call_new)
    // Should NOT have: old tool_result (call_old)
    let tool_results: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert!(
        tool_results.is_empty(),
        "Session history tool_result (call_old) should be filtered. Found: {:?}",
        tool_results
            .iter()
            .map(|m| &m.tool_use_id)
            .collect::<Vec<_>>()
    );

    // Current turn tool_use should be present
    let tool_uses: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_use")
        .collect();

    assert_eq!(
        tool_uses.len(),
        1,
        "Current turn tool_use (call_new) should be present"
    );

    if let ContentBlock::ToolUse { id, .. } = &tool_uses[0].content {
        assert_eq!(
            id.as_deref(),
            Some("call_new"),
            "Should be current turn tool_use"
        );
    }
}

/// Regression #53: Tool_use before tool_result ordering in multi-trace sessions.
///
/// When processing a session with multiple traces, tool_uses must appear
/// before their corresponding tool_results. This tests that orphan tool_results
/// (from session history) are filtered, maintaining correct ordering.
#[test]
fn test_regression_session_tool_ordering_across_traces() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);
    let t4 = t0 + chrono::Duration::seconds(4);
    let t5 = t0 + chrono::Duration::seconds(5);

    // TRACE 1: Complete tool cycle
    let trace1_gen_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "weather", "input": {"city": "NYC"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);
    let trace1_tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "NYC: Sunny"}]}
    }]);

    // TRACE 2: Has session history (call_1 result) + new tool cycle (call_2)
    let trace2_gen_msg = json!([
        // Session history - old tool_result (should be filtered)
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": t3.to_rfc3339()}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "NYC: Sunny"}]}
        },
        // Current turn - new tool_use
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t4.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_2", "name": "weather", "input": {"city": "LA"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);
    let trace2_tool_msg = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t5.to_rfc3339()}},
        "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_2", "content": "LA: Warm"}]}
    }]);

    // Agent spans need content for has_agent_spans detection
    let trace1_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {"role": "assistant", "content": "NYC weather ready.", "finish_reason": "stop"}
    }]);
    let trace2_agent_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t5.to_rfc3339()}},
        "content": {"role": "assistant", "content": "LA weather ready.", "finish_reason": "stop"}
    }]);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &trace1_agent_msg.to_string(),
            t0,
            Some(t2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &trace1_gen_msg.to_string(),
            t0,
            Some(t2),
            "generation",
        ),
        make_span_row_full(
            "trace1",
            "tool1",
            Some("agent1"),
            &trace1_tool_msg.to_string(),
            t1,
            Some(t2),
            Some("tool"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &trace2_agent_msg.to_string(),
            t3,
            Some(t5),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen_msg.to_string(),
            t3,
            Some(t5),
            "generation",
        ),
        make_span_row_full(
            "trace2",
            "tool2",
            Some("agent2"),
            &trace2_tool_msg.to_string(),
            t4,
            Some(t5),
            Some("tool"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Check trace 2 specifically - should have call_2 tool_use before call_2 tool_result
    let trace2_msgs: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.trace_id == "trace2")
        .collect();

    let tool_uses: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_use")
        .collect();
    let tool_results: Vec<_> = trace2_msgs
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    // Should have exactly 1 tool_use (call_2) and 1 tool_result (call_2)
    assert_eq!(
        tool_uses.len(),
        1,
        "Trace 2 should have 1 tool_use (call_2)"
    );
    assert_eq!(
        tool_results.len(),
        1,
        "Trace 2 should have 1 tool_result (call_2, not call_1)"
    );

    // Verify it's call_2, not call_1 (session history)
    assert_eq!(
        tool_results[0].tool_use_id.as_deref(),
        Some("call_2"),
        "Tool result should be call_2 (current turn), not call_1 (session history)"
    );

    // Verify ordering: tool_use before tool_result in the full result
    let trace2_tool_positions: Vec<_> = result
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.trace_id == "trace2" && (m.entry_type == "tool_use" || m.entry_type == "tool_result")
        })
        .map(|(i, m)| (i, &m.entry_type))
        .collect();

    assert!(
        trace2_tool_positions.len() >= 2,
        "Should have both tool_use and tool_result"
    );

    // Find positions
    let tool_use_pos = trace2_tool_positions
        .iter()
        .find(|(_, t)| *t == "tool_use")
        .map(|(i, _)| i);
    let tool_result_pos = trace2_tool_positions
        .iter()
        .find(|(_, t)| *t == "tool_result")
        .map(|(i, _)| i);

    assert!(
        tool_use_pos < tool_result_pos,
        "Tool_use should come before tool_result. Positions: use={:?}, result={:?}",
        tool_use_pos,
        tool_result_pos
    );
}

/// Regression #54: Session with multiple traces - each trace isolated.
///
/// When processing a session, history detection must work per-trace.
/// Tool operations from trace N should not affect filtering in trace N+1.
#[test]
fn test_regression_session_per_trace_isolation() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(5);
    let t2 = t0 + chrono::Duration::seconds(10);
    let t_end0 = t0 + chrono::Duration::seconds(4);
    let t_end1 = t1 + chrono::Duration::seconds(4);
    let t_end2 = t2 + chrono::Duration::seconds(4);

    // Three traces, each with their own tool cycle
    // The tool_use_ids are different in each trace
    let make_trace_msgs = |call_id: &str, city: &str, time: chrono::DateTime<Utc>| {
        let gen_msg = json!([{
            "source": {"event": {"name": "gen_ai.choice", "time": time.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": call_id, "name": "weather", "input": {"city": city}}],
                "finish_reason": "tool_use"
            }
        }]);
        let tool_msg = json!([{
            "source": {"event": {"name": "gen_ai.tool.message", "time": (time + chrono::Duration::seconds(1)).to_rfc3339()}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": call_id, "content": format!("{}: Weather data", city)}]}
        }]);
        (gen_msg, tool_msg)
    };

    let (trace1_gen, trace1_tool) = make_trace_msgs("call_a", "NYC", t0);
    let (trace2_gen, trace2_tool) = make_trace_msgs("call_b", "LA", t1);
    let (trace3_gen, trace3_tool) = make_trace_msgs("call_c", "Chicago", t2);

    let rows = vec![
        // Trace 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            "[]",
            t0,
            Some(t_end0),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &trace1_gen.to_string(),
            t0,
            Some(t_end0),
            "generation",
        ),
        make_span_row_full(
            "trace1",
            "tool1",
            Some("agent1"),
            &trace1_tool.to_string(),
            t0,
            Some(t_end0),
            Some("tool"),
        ),
        // Trace 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            "[]",
            t1,
            Some(t_end1),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &trace2_gen.to_string(),
            t1,
            Some(t_end1),
            "generation",
        ),
        make_span_row_full(
            "trace2",
            "tool2",
            Some("agent2"),
            &trace2_tool.to_string(),
            t1,
            Some(t_end1),
            Some("tool"),
        ),
        // Trace 3
        make_span_row_full(
            "trace3",
            "agent3",
            None,
            "[]",
            t2,
            Some(t_end2),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace3",
            "gen3",
            Some("agent3"),
            &trace3_gen.to_string(),
            t2,
            Some(t_end2),
            "generation",
        ),
        make_span_row_full(
            "trace3",
            "tool3",
            Some("agent3"),
            &trace3_tool.to_string(),
            t2,
            Some(t_end2),
            Some("tool"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Each trace should have its own tool_use and tool_result
    for (trace_id, expected_call_id) in [
        ("trace1", "call_a"),
        ("trace2", "call_b"),
        ("trace3", "call_c"),
    ] {
        let trace_msgs: Vec<_> = result
            .messages
            .iter()
            .filter(|m| m.trace_id == trace_id)
            .collect();

        let tool_uses: Vec<_> = trace_msgs
            .iter()
            .filter(|m| m.entry_type == "tool_use")
            .collect();
        let tool_results: Vec<_> = trace_msgs
            .iter()
            .filter(|m| m.entry_type == "tool_result")
            .collect();

        assert_eq!(
            tool_uses.len(),
            1,
            "{} should have exactly 1 tool_use",
            trace_id
        );
        assert_eq!(
            tool_results.len(),
            1,
            "{} should have exactly 1 tool_result",
            trace_id
        );

        // Verify correct call_id
        if let ContentBlock::ToolUse { id, .. } = &tool_uses[0].content {
            assert_eq!(
                id.as_deref(),
                Some(expected_call_id),
                "{} tool_use should have id {}",
                trace_id,
                expected_call_id
            );
        }
        assert_eq!(
            tool_results[0].tool_use_id.as_deref(),
            Some(expected_call_id),
            "{} tool_result should have id {}",
            trace_id,
            expected_call_id
        );
    }
}

/// Regression #55: Thinking blocks without protection are filtered as history.
///
/// In multi-turn sessions, thinking blocks from previous turns are re-sent as
/// history context. These have GenAIAssistantMessage category (not GenAIChoice)
/// and no finish_reason, so they should be filtered.
///
/// Only thinking blocks with protection markers (GenAIChoice, finish_reason)
/// should be preserved.
#[test]
fn test_regression_thinking_history_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Generation span with:
    // - History thinking (GenAIAssistantMessage, no finish_reason) - should be FILTERED
    // - Current thinking (GenAIChoice, finish_reason) - should be KEPT
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "History thinking from previous turn"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "Current turn thinking"}],
                "finish_reason": "stop"
            }
        }
    ]);

    // Agent span (root) to trigger history detection
    let agent_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Final response"}],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("agent_span"),
            &gen_span_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count thinking blocks
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "thinking")
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        1,
        "Should have exactly 1 thinking block (current turn only)"
    );

    // Verify it's the current turn thinking (protected)
    let thinking = thinking_blocks[0];
    assert!(
        thinking.finish_reason.is_some(),
        "Preserved thinking should have finish_reason (protected)"
    );
    assert!(
        matches!(&thinking.content, ContentBlock::Thinking { text, .. } if text.contains("Current turn")),
        "Should be the current turn thinking"
    );
}

/// Regression #56: User/System messages in non-root generation spans filtered.
///
/// In Strands-like traces with agent spans:
/// - Root agent span has authoritative current-turn messages
/// - Child generation spans receive full context (history + current) for LLM
///
/// User/system messages in child generation spans are history context copies
/// and should be filtered when agent spans exist.
#[test]
fn test_regression_generation_span_history_user_filtered() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::milliseconds(100);
    let t2 = t0 + chrono::Duration::milliseconds(200);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Agent span (root) with current turn messages - use event-based format
    let agent_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are a helpful assistant"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Current question: What is 2+2?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The answer is 4", "finish_reason": "stop"}
        }
    ]);

    // Generation span with history context (copies of previous turns)
    let gen_span_msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History question from turn 1"}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "History question from turn 2"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "Current thinking"}],
                "finish_reason": "stop"
            }
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "agent_span",
            None,
            &agent_span_msg.to_string(),
            t0,
            Some(t_end),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen_span",
            Some("agent_span"),
            &gen_span_msg.to_string(),
            t0,
            Some(t_end),
            "generation",
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Count user messages
    let user_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();

    assert_eq!(
        user_messages.len(),
        1,
        "Should have exactly 1 user message (from root agent span), got {} user messages, {} total blocks",
        user_messages.len(),
        result.messages.len()
    );

    // Verify it's the current turn question
    assert!(
        matches!(&user_messages[0].content, ContentBlock::Text { text } if text.contains("Current question")),
        "Should be the current turn user message"
    );

    // Verify history user messages were filtered
    let has_history = result.messages.iter().any(
        |b| matches!(&b.content, ContentBlock::Text { text } if text.contains("History question")),
    );
    assert!(
        !has_history,
        "History user messages should be filtered from generation span"
    );
}

/// Regression #57: Multi-turn session with thinking - history filtered per trace.
///
/// Each trace (turn) has an agent span and generation span. History in
/// generation spans should be filtered, keeping only protected content.
#[test]
fn test_regression_multi_turn_thinking_session() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);

    // Turn 1: Simple question - no history
    let turn1_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 1", "finish_reason": "stop"}}
    ]);
    let turn1_gen = json!([
        {"source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}], "finish_reason": "stop"}}
    ]);

    // Turn 2: Has history from turn 1 in generation span
    let turn2_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t1.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}}, "content": {"role": "user", "content": "Question 2"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 2", "finish_reason": "stop"}}
    ]);
    let turn2_gen = json!([
        // History thinking (no finish_reason) - should be filtered
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}]}},
        // History user - should be filtered
        {"source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        // Current thinking (protected) - should be kept
        {"source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q2"}], "finish_reason": "stop"}}
    ]);

    // Turn 3: Has history from turns 1 and 2 in generation span
    let turn3_agent = json!([
        {"source": {"event": {"name": "gen_ai.system.message", "time": t2.to_rfc3339()}}, "content": {"role": "system", "content": "System prompt"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 3"}},
        {"source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": "Answer 3", "finish_reason": "stop"}}
    ]);
    let turn3_gen = json!([
        // History thinking from Q1 and Q2 - should be filtered
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q1"}]}},
        {"source": {"event": {"name": "gen_ai.assistant.message", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q2"}]}},
        // History users - should be filtered
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 1"}},
        {"source": {"event": {"name": "gen_ai.user.message", "time": t2.to_rfc3339()}}, "content": {"role": "user", "content": "Question 2"}},
        // Current thinking (protected) - should be kept
        {"source": {"event": {"name": "gen_ai.choice", "time": t2.to_rfc3339()}}, "content": {"role": "assistant", "content": [{"type": "thinking", "thinking": "Thinking for Q3"}], "finish_reason": "stop"}}
    ]);

    let mut rows = vec![
        // Turn 1
        make_span_row_full(
            "trace1",
            "agent1",
            None,
            &turn1_agent.to_string(),
            t0,
            Some(t0 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace1",
            "gen1",
            Some("agent1"),
            &turn1_gen.to_string(),
            t0,
            Some(t0 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
        // Turn 2
        make_span_row_full(
            "trace2",
            "agent2",
            None,
            &turn2_agent.to_string(),
            t1,
            Some(t1 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace2",
            "gen2",
            Some("agent2"),
            &turn2_gen.to_string(),
            t1,
            Some(t1 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
        // Turn 3
        make_span_row_full(
            "trace3",
            "agent3",
            None,
            &turn3_agent.to_string(),
            t2,
            Some(t2 + chrono::Duration::milliseconds(500)),
            Some("agent"),
        ),
        make_span_row_with_observation_type(
            "trace3",
            "gen3",
            Some("agent3"),
            &turn3_gen.to_string(),
            t2,
            Some(t2 + chrono::Duration::milliseconds(500)),
            "generation",
        ),
    ];

    // Add session_id to group all traces as one conversation
    for row in &mut rows {
        row.session_id = Some("test-session".to_string());
    }

    let options = FeedOptions::default();
    let result = process_feed(rows, &options);

    // Should have 3 thinking blocks (one per turn)
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.entry_type == "thinking")
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        3,
        "Should have exactly 3 thinking blocks (one per turn), got {}",
        thinking_blocks.len()
    );

    // Should have 3 user messages (one per turn)
    let user_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::User)
        .collect();

    assert_eq!(
        user_messages.len(),
        3,
        "Should have exactly 3 user messages (one per turn), got {}",
        user_messages.len()
    );

    // Total should be 12 blocks: 3 * (system + user + thinking + response)
    assert_eq!(
        result.messages.len(),
        12,
        "Should have 12 total blocks (4 per turn), got {}",
        result.messages.len()
    );
}

/// Regression #58: Protected content never filtered regardless of timestamp.
///
/// Even if a message has timestamp < span_start, if it's protected
/// (gen_ai.choice, finish_reason), it should NOT be filtered.
#[test]
fn test_regression_protected_ignores_timestamp() {
    let t0 = fixed_time();
    let t_before = t0 - chrono::Duration::seconds(10);
    let t_end = t0 + chrono::Duration::seconds(1);

    // Protected message with timestamp before span start
    // Should NOT be filtered because it's protected
    let gen_span_msg = json!([{
        "source": {"event": {"name": "gen_ai.choice", "time": t_before.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": "Response from LLM",
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen",
        Some("parent"),
        &gen_span_msg.to_string(),
        t0, // Span starts AFTER message timestamp
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Protected message should be preserved despite timestamp
    assert_eq!(
        result.messages.len(),
        1,
        "Protected message should be preserved even with timestamp < span_start"
    );
    assert!(
        result.messages[0].finish_reason.is_some(),
        "Should have finish_reason (protected)"
    );
}

// ----------------------------------------------------------------------------
// ADK Thinking Block Recognition
// ----------------------------------------------------------------------------
// ADK sends thinking content as {"text": "...", "thought": true} which should
// be normalized to ContentBlock::Thinking, not ContentBlock::Unknown.

#[test]
fn test_regression_adk_thinking_blocks_recognized() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t_end = t0 + chrono::Duration::seconds(5);

    // ADK-style messages with thought blocks using event-based format
    let messages = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "user",
            "content": [{"type": "text", "text": "Solve this logic puzzle"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [
                {"text": "Let me think step by step...", "thought": true},
                {"type": "text", "text": "The answer is Alice=Green, Bob=Blue, Carol=Red."}
            ],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert!(
        result.messages.len() >= 3,
        "Expected user + thinking + assistant text, got {}",
        result.messages.len()
    );

    // Find the thinking block
    let thinking_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| matches!(m.content, ContentBlock::Thinking { .. }))
        .collect();

    assert_eq!(
        thinking_blocks.len(),
        1,
        "Should have exactly one thinking block"
    );

    if let ContentBlock::Thinking { ref text, .. } = thinking_blocks[0].content {
        assert_eq!(text, "Let me think step by step...");
    } else {
        panic!("Expected Thinking content block");
    }

    // Verify no unknown blocks (the thought block should not be unknown)
    let unknown_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| matches!(m.content, ContentBlock::Unknown { .. }))
        .collect();

    assert_eq!(
        unknown_blocks.len(),
        0,
        "ADK thought blocks should not produce Unknown content blocks"
    );
}

#[test]
fn test_regression_tool_use_flow_complete() {
    // Verifies the basic tool_use -> tool_result -> assistant flow
    // across all framework patterns
    let t0 = fixed_time();
    let t_end = t0 + chrono::Duration::seconds(5);

    let messages = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {
            "role": "user",
            "content": [{"type": "text", "text": "What is the weather in NYC?"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": (t0 + chrono::Duration::seconds(1)).to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "call_123",
                "name": "get_weather",
                "input": {"city": "NYC"}
            }],
            "finish_reason": "tool_use"
        }
    }, {
        "source": {"event": {"name": "gen_ai.tool.message", "time": (t0 + chrono::Duration::seconds(2)).to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "call_123",
            "content": [{"type": "tool_result", "tool_use_id": "call_123", "content": "Sunny, 75F"}]
        }
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": (t0 + chrono::Duration::seconds(3)).to_rfc3339()}},
        "content": {
            "role": "assistant",
            "content": [{"type": "text", "text": "The weather in NYC is sunny and 75F!"}],
            "finish_reason": "stop"
        }
    }]);

    let rows = vec![make_span_row_with_observation_type(
        "trace1",
        "gen1",
        None,
        &messages.to_string(),
        t0,
        Some(t_end),
        "generation",
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Verify message count and order
    assert!(
        result.messages.len() >= 4,
        "Expected at least 4 messages (user, tool_use, tool_result, assistant), got {}",
        result.messages.len()
    );

    // Verify roles in order
    let roles: Vec<ChatRole> = result.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles[0], ChatRole::User, "First should be user");

    // Find tool_use and tool_result
    let has_tool_use = result
        .messages
        .iter()
        .any(|m| matches!(m.content, ContentBlock::ToolUse { .. }));
    let has_tool_result = result
        .messages
        .iter()
        .any(|m| matches!(m.content, ContentBlock::ToolResult { .. }));

    assert!(has_tool_use, "Should have tool_use block");
    assert!(has_tool_result, "Should have tool_result block");

    // Verify tool_use comes before tool_result
    let tool_use_idx = result
        .messages
        .iter()
        .position(|m| matches!(m.content, ContentBlock::ToolUse { .. }))
        .unwrap();
    let tool_result_idx = result
        .messages
        .iter()
        .position(|m| matches!(m.content, ContentBlock::ToolResult { .. }))
        .unwrap();
    assert!(
        tool_use_idx < tool_result_idx,
        "tool_use should come before tool_result"
    );

    // Verify last message is assistant text
    let last = result.messages.last().unwrap();
    assert_eq!(last.role, ChatRole::Assistant, "Last should be assistant");
    if let ContentBlock::Text { ref text } = last.content {
        assert!(text.contains("sunny"), "Should contain weather response");
    } else {
        panic!("Last message should be text");
    }

    // No duplicates
    let hashes: Vec<_> = result.messages.iter().map(|m| &m.content_hash).collect();
    let unique_hashes: std::collections::HashSet<_> = hashes.iter().collect();
    assert_eq!(
        hashes.len(),
        unique_hashes.len(),
        "Should have no duplicate content hashes"
    );
}

// ============================================================================
// ERROR MESSAGE TESTS
// ============================================================================

#[test]
fn test_error_messages_from_status_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Error span with exception_message -> should get error ParsedMessage
    let mut error_row = make_span_row_full(
        "t1",
        "error-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    error_row.status_code = Some("ERROR".to_string());
    error_row.exception_message = Some("Input is too long".into());

    // Normal span -> should NOT get error ParsedMessage
    let normal_row = make_span_row_full(
        "t1",
        "normal-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );

    let rows = vec![error_row, normal_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    // Should have exactly one error block
    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    assert_eq!(
        error_blocks[0].finish_reason,
        Some(super::super::types::FinishReason::Error)
    );
    assert!(
        !error_blocks[0].is_history,
        "Error blocks should not be marked as history"
    );
}

#[test]
fn test_error_messages_leaf_only() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root error span (has error child -> should NOT get error message)
    let mut root_row =
        make_span_row_full("t1", "root-span", None, "[]", t0, Some(t1), Some("agent"));
    root_row.status_code = Some("ERROR".to_string());
    root_row.exception_message = Some("Root error".to_string());

    // Leaf error span (no error children -> should get error message)
    let mut leaf_row = make_span_row_full(
        "t1",
        "leaf-span",
        Some("root-span"),
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    leaf_row.status_code = Some("ERROR".to_string());
    leaf_row.exception_message = Some("Leaf error".to_string());

    let rows = vec![root_row, leaf_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only leaf error should produce a block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert_eq!(text, "Leaf error"),
        _ => panic!("Expected Text content block"),
    }
}

#[test]
fn test_error_messages_no_status_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Error span without exception fields -> should NOT get error block
    let mut error_row = make_span_row_full(
        "t1",
        "error-span",
        None,
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    error_row.status_code = Some("ERROR".to_string());
    // exception fields are all None

    let rows = vec![error_row];
    let options = FeedOptions::new();
    let result = process_spans(rows, &options);

    assert!(
        result.messages.is_empty(),
        "No error block when exception fields are empty"
    );
}

// ============================================================================
// COMPOSE ERROR TEXT TESTS
// ============================================================================

#[test]
fn test_compose_error_text_type_and_message() {
    let result = compose_error_text(Some("ValueError"), Some("bad input"), None);
    assert_eq!(result, Some("ValueError: bad input".to_string()));
}

#[test]
fn test_compose_error_text_message_only() {
    let result = compose_error_text(None, Some("bad input"), None);
    assert_eq!(result, Some("bad input".to_string()));
}

#[test]
fn test_compose_error_text_type_only() {
    let result = compose_error_text(Some("ValueError"), None, None);
    assert_eq!(result, Some("ValueError".to_string()));
}

#[test]
fn test_compose_error_text_all_none() {
    let result = compose_error_text(None, None, None);
    assert_eq!(result, None);
}

#[test]
fn test_compose_error_text_all_empty() {
    let result = compose_error_text(Some(""), Some(""), Some(""));
    assert_eq!(result, None);
}

#[test]
fn test_compose_error_text_stacktrace_only() {
    let result = compose_error_text(None, None, Some("at main.py:1"));
    assert_eq!(result, Some("```\nat main.py:1\n```".to_string()));
}

#[test]
fn test_compose_error_text_message_and_stacktrace() {
    let result = compose_error_text(None, Some("bad input"), Some("at main.py:1"));
    assert_eq!(
        result,
        Some("bad input\n\n```\nat main.py:1\n```".to_string())
    );
}

#[test]
fn test_compose_error_text_all_fields() {
    let result = compose_error_text(Some("ValueError"), Some("bad input"), Some("at main.py:1"));
    assert_eq!(
        result,
        Some("ValueError: bad input\n\n```\nat main.py:1\n```".to_string())
    );
}

// ============================================================================
// EXCEPTION FIELD REGRESSION TESTS
// ============================================================================

/// Regression: error span with only status_message (no exception fields)
/// should NOT produce an error block. OTEL SDKs propagate status_message
/// up the span tree — only exception events carry real error details.
#[test]
fn test_error_span_with_only_status_code_no_exception_fields() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("agent"));
    row.status_code = Some("ERROR".to_string());
    // No exception fields set — simulates SDK-propagated error status

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    assert!(
        result.messages.is_empty(),
        "No error block for status_code=ERROR without exception fields"
    );
}

/// Regression: exception_type + exception_message produce "Type: Message" header
#[test]
fn test_error_block_with_exception_type_and_message() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("generation"));
    row.status_code = Some("ERROR".to_string());
    row.exception_type = Some("ValueError".to_string());
    row.exception_message = Some("bad input".to_string());

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert_eq!(text, "ValueError: bad input"),
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: exception with stacktrace renders markdown code block
#[test]
fn test_error_block_with_stacktrace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut row = make_span_row_full("t1", "s1", None, "[]", t0, Some(t1), Some("generation"));
    row.status_code = Some("ERROR".to_string());
    row.exception_type = Some("RuntimeError".to_string());
    row.exception_message = Some("crash".to_string());
    row.exception_stacktrace = Some("Traceback:\n  File main.py".to_string());

    let rows = vec![row];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(error_blocks.len(), 1);
    match &error_blocks[0].content {
        ContentBlock::Text { text } => {
            assert!(text.starts_with("RuntimeError: crash"));
            assert!(text.contains("```\nTraceback:\n  File main.py\n```"));
        }
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: parent error span with error child should NOT produce error block
/// (leaf detection) even when parent has exception fields.
#[test]
fn test_error_block_only_on_leaf_with_exception_fields() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let mut parent = make_span_row_full("t1", "parent", None, "[]", t0, Some(t1), Some("agent"));
    parent.status_code = Some("ERROR".to_string());
    parent.exception_type = Some("ValueError".to_string());
    parent.exception_message = Some("parent error".to_string());

    let mut child = make_span_row_full(
        "t1",
        "child",
        Some("parent"),
        "[]",
        t0,
        Some(t1),
        Some("generation"),
    );
    child.status_code = Some("ERROR".to_string());
    child.exception_type = Some("ValueError".to_string());
    child.exception_message = Some("child error".to_string());

    let rows = vec![parent, child];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only leaf error should produce block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert!(text.contains("child error")),
        _ => panic!("Expected Text content block"),
    }
}

/// Regression: two independent error leaf spans (broken hierarchy) should
/// both produce error blocks when both have exception fields.
#[test]
fn test_error_blocks_for_independent_leaf_spans() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    // Root agent span with exception event
    let mut root = make_span_row_full("t1", "root", None, "[]", t0, Some(t1), Some("agent"));
    root.status_code = Some("ERROR".to_string());
    root.exception_type = Some("RuntimeError".to_string());
    root.exception_message = Some("root exception".to_string());

    // Detached leaf span (parent not in trace) — only SDK-propagated status
    let mut detached = make_span_row_full(
        "t1",
        "detached",
        Some("missing-parent"),
        "[]",
        t0,
        Some(t1),
        Some("span"),
    );
    detached.status_code = Some("ERROR".to_string());
    // No exception fields — just propagated error status

    let rows = vec![root, detached];
    let result = process_spans(rows, &FeedOptions::new());

    let error_blocks: Vec<_> = result.messages.iter().filter(|b| b.is_error).collect();
    assert_eq!(
        error_blocks.len(),
        1,
        "Only the span with exception fields should produce an error block"
    );
    match &error_blocks[0].content {
        ContentBlock::Text { text } => assert!(text.contains("root exception")),
        _ => panic!("Expected Text content block"),
    }
}

// ============================================================================
// VERCEL AI SDK toModelOutput DEDUPLICATION
// ============================================================================
// Vercel AI SDK's toModelOutput transforms tool result content before re-sending
// to the model in the next generation span. The raw execute() output appears in
// the tool span, while the transformed format appears in the generation span's
// ai.prompt.messages. These have different content hashes but the same tool_use_id.
//
// Trace 74045ce6aae9e6765f8017028a23ecfe demonstrates this pattern:
//   - Tool spans: raw {path, base64, mimeType}
//   - Generation span: {type: "content", value: [{type: "text"}, {type: "image-data"}]}

#[test]
fn test_regression_vercel_to_model_output_tool_result_dedup() {
    // Vercel AI SDK: toModelOutput transforms content before re-sending to the model.
    // The raw tool result in the tool span differs from the transformed version in the
    // next generation span's ai.prompt.messages attribute.
    //
    // Trace 74045ce6aae9e6765f8017028a23ecfe demonstrates this pattern.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    // Root span (ai.generateText): wraps the whole multi-step flow
    let root_msg = json!([]);

    // First generation span (ai.generateText.doGenerate): produces tool_use
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Read the image"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "tooluse_ABC", "name": "image_reader", "input": {"path": "/tmp/image.png"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool span (ai.toolCall): raw execute() result as attribute
    let tool_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {
            "role": "tool",
            "tool_use_id": "tooluse_ABC",
            "content": {"path": "/tmp/image.png", "base64": "iVBOR...", "mimeType": "image/png"}
        }
    }]);

    // Second generation span: tool result re-sent (toModelOutput format) + final answer
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {
                "role": "tool",
                "tool_use_id": "tooluse_ABC",
                "content": {"type": "content", "value": [
                    {"type": "text", "text": "Image: /tmp/image.png"},
                    {"type": "image-data", "data": "iVBOR...", "mediaType": "image/png"}
                ]}
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool_span",
            Some("root"),
            &tool_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        1,
        "toModelOutput-transformed tool result should dedup with raw version. Found {}",
        tool_results.len()
    );

    // Should keep the tool span version (actual execution)
    assert_eq!(
        tool_results[0].observation_type.as_deref(),
        Some("tool"),
        "Should prefer tool span version over generation span copy"
    );
}

#[test]
fn test_regression_vercel_to_model_output_multiple_tools_dedup() {
    // 3 parallel tool calls: each produces a raw result in its tool span and
    // a toModelOutput-transformed copy in the next generation span.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);
    let t2 = t0 + chrono::Duration::seconds(2);
    let t3 = t0 + chrono::Duration::seconds(3);

    let root_msg = json!([]);

    // First generation span: 3 parallel tool_use calls
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Generate 3 images of a dog"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_1", "name": "image_reader", "input": {"path": "/img1.png"}},
                    {"type": "tool_use", "id": "call_2", "name": "image_reader", "input": {"path": "/img2.png"}},
                    {"type": "tool_use", "id": "call_3", "name": "image_reader", "input": {"path": "/img3.png"}}
                ],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // 3 tool spans with raw results (attribute source, matching real Vercel data)
    let tool1_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": {"path": "/img1.png", "data": "AAA"}}
    }]);
    let tool2_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_2", "content": {"path": "/img2.png", "data": "BBB"}}
    }]);
    let tool3_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_3", "content": {"path": "/img3.png", "data": "CCC"}}
    }]);

    // Second generation span: all 3 re-sent with toModelOutput format + answer
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img1.png"}]}}
        },
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_2",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img2.png"}]}}
        },
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_3",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img3.png"}]}}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Image 1 is best.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "t1",
            Some("root"),
            &tool1_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_tool_span_row(
            "trace1",
            "t2",
            Some("root"),
            &tool2_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_tool_span_row(
            "trace1",
            "t3",
            Some("root"),
            &tool3_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        3,
        "3 tool calls should produce 3 results (not 6). Found {}",
        tool_results.len()
    );

    // All should be from tool spans
    for tr in &tool_results {
        assert_eq!(
            tr.observation_type.as_deref(),
            Some("tool"),
            "All tool results should come from tool spans, not generation span"
        );
    }
}

// ============================================================================
// TIMESTAMP MATERIALIZATION
// ============================================================================
// Attribute-sourced messages inherit span start time as their raw timestamp,
// but their actual production time is span_end (for output blocks).
// After feed processing, block.timestamp must reflect the birth/effective time.
//
// Trace 74045ce6aae9e6765f8017028a23ecfe: final assistant response on root span
// had timestamp = span start (10:03:08) instead of span end (10:03:17).

#[test]
fn test_regression_timestamp_materialized_for_root_span_output() {
    // Vercel AI SDK multi-step: root span wraps gen1 → tools → gen2.
    // The final assistant response appears as an attribute on the root span,
    // inheriting span_start as its timestamp. After processing, its timestamp
    // must be materialized to span_end (when the response was actually produced).
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(3);
    let t2 = t0 + chrono::Duration::seconds(5);
    let t3 = t0 + chrono::Duration::seconds(10);

    // Root span (ai.generateText): final response as attribute
    let root_msg = json!([{
        "source": {"attribute": {"key": "ai.response.text", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
    }]);

    // First gen span: user prompt + tool_use output
    let gen1_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Describe the image"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "image_reader", "input": {"path": "/img.png"}}],
                "finish_reason": "tool_use"
            }
        }
    ]);

    // Tool span: raw result
    let tool_msg = json!([{
        "source": {"attribute": {"key": "ai.toolCall.result", "time": t2.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": {"path": "/img.png", "data": "AAA"}}
    }]);

    // Second gen span: transformed tool result + final answer (gen_ai.choice)
    let gen2_msg = json!([
        {
            "source": {"attribute": {"key": "ai.prompt.messages", "time": t2.to_rfc3339()}},
            "content": {"role": "tool", "tool_use_id": "call_1",
                "content": {"type": "content", "value": [{"type": "text", "text": "Image: /img.png"}]}}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t3.to_rfc3339()}},
            "content": {"role": "assistant", "content": "The image shows a dog.", "finish_reason": "stop"}
        }
    ]);

    let rows = vec![
        make_span_row_with_observation_type(
            "trace1",
            "root",
            None,
            &root_msg.to_string(),
            t0,
            Some(t3),
            "generation",
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen1",
            Some("root"),
            &gen1_msg.to_string(),
            t0,
            Some(t1),
        ),
        make_tool_span_row(
            "trace1",
            "tool1",
            Some("root"),
            &tool_msg.to_string(),
            t2,
            Some(t2),
        ),
        make_span_row_with_timestamps(
            "trace1",
            "gen2",
            Some("root"),
            &gen2_msg.to_string(),
            t2,
            Some(t3),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Find the final assistant response (finish_reason=stop)
    let final_assistant: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant && m.finish_reason == Some(FinishReason::Stop))
        .collect();

    assert_eq!(
        final_assistant.len(),
        1,
        "Should have exactly one final assistant response"
    );

    // The key assertion: timestamp must NOT be span start (t0).
    // It must be materialized to the effective time (span_end = t3).
    assert_ne!(
        final_assistant[0].timestamp, t0,
        "Final assistant timestamp must not be raw span start time"
    );
    assert_eq!(
        final_assistant[0].timestamp, t3,
        "Final assistant timestamp should be span_end (when response was produced)"
    );

    // Tool results should have span_end timestamps too (tool execution completion)
    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(tool_results.len(), 1);
    assert_eq!(
        tool_results[0].timestamp, t2,
        "Tool result timestamp should be tool span_end (execution completion)"
    );

    // Verify chronological ordering: user → tool_use → tool_result → final assistant
    let timestamps: Vec<_> = result.messages.iter().map(|m| m.timestamp).collect();
    for window in timestamps.windows(2) {
        assert!(
            window[0] <= window[1],
            "Messages must be in chronological order: {:?} should be <= {:?}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn test_regression_different_tool_use_ids_preserved() {
    // Different tool_use_ids = different logical tool executions → both preserved.
    // This guards against over-merging: only same tool_use_id triggers dedup.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(1);

    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t0.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_1", "content": "First result"}
    }]);

    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.tool.message", "time": t1.to_rfc3339()}},
        "content": {"role": "tool", "tool_use_id": "call_2", "content": "Second result"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "span1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps(
            "trace1",
            "span2",
            Some("span1"),
            &msg2.to_string(),
            t1,
            Some(t1),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    let tool_results: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.entry_type == "tool_result")
        .collect();

    assert_eq!(
        tool_results.len(),
        2,
        "Different tool_use_ids should both be preserved. Found {}",
        tool_results.len()
    );
}

// ============================================================================
// PR 1: WITHIN-TRACE ADK SUPPORT TESTS
// ============================================================================

/// ADK multi-span trace: Phase 4b marks assistant from input, output-source survives.
///
/// Agent root + 2 generation children.
/// span1 input (llm_request): [sys, userA, asstB_old, userC]
/// span1 output (gen_ai.choice): [toolD(tool_use)]
/// span2 input (llm_request): [sys, userA, asstB_old, userC, toolD, resultE]
/// span2 output (gen_ai.choice): [asstG(stop)]
///
/// Key assertion: Phase 4b marks asstB_old (input-source, assistant) as history.
/// Protected gen_ai.choice output (toolD, asstG) survives.
#[test]
fn test_adk_multi_span_phase4b_and_dedup() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    let span1_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "Previous answer from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Now do something"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "calculator", "input": {"op": "add"}}]
            }
        }
    ]);

    let span1 = make_span_row_full(
        "trace1",
        "gen_span1",
        Some("agent_root"),
        &span1_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let span2_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "assistant", "content": "Previous answer from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "user", "content": "Now do something"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_1", "name": "calculator", "input": {"op": "add"}}]
            }
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "tool", "tool_use_id": "call_1", "content": "4"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:09Z"}},
            "content": {
                "role": "assistant",
                "content": "The answer is 4",
                "finish_reason": "stop"
            }
        }
    ]);

    let span2 = make_span_row_full(
        "trace1",
        "gen_span2",
        Some("agent_root"),
        &span2_msgs.to_string(),
        t0 + dur(6),
        Some(t0 + dur(9)),
        Some("generation"),
    );

    let rows = vec![agent_root, span1, span2];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // "Previous answer from history" should be filtered by Phase 4b
    let has_old_assistant = result.messages.iter().any(|m| {
        matches!(&m.content, ContentBlock::Text { text } if text == "Previous answer from history")
    });
    assert!(
        !has_old_assistant,
        "Phase 4b should mark input-source assistant as history"
    );

    // toolD should survive (from span1 output via gen_ai.choice, protected)
    let tool_uses: Vec<_> = result.messages.iter().filter(|m| m.is_tool_use()).collect();
    assert_eq!(
        tool_uses.len(),
        1,
        "toolD should survive from output source"
    );

    // Final answer should be present (protected by gen_ai.choice + finish_reason)
    let final_answer = result
        .messages
        .iter()
        .any(|m| matches!(&m.content, ContentBlock::Text { text } if text == "The answer is 4"));
    assert!(final_answer, "Final assistant answer should be present");

    // Verify no empty result
    assert!(
        result.messages.len() >= 3,
        "At minimum: toolD, asstG, plus user context. Got {} blocks",
        result.messages.len()
    );
}

/// Phase 4b does not affect event-based frameworks (Strands).
/// Events have their own protection mechanism (gen_ai.choice is protected).
#[test]
fn test_phase4b_no_effect_on_strands_events() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    // Strands pattern: events on generation span
    let gen_msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {
                "role": "assistant",
                "content": "Hi there!",
                "finish_reason": "stop"
            }
        }
    ]);

    let gen_span = make_span_row_full(
        "trace1",
        "gen_span",
        Some("agent_root"),
        &gen_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(3)),
        Some("generation"),
    );

    let rows = vec![agent_root, gen_span];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // gen_ai.choice assistant text is protected — Phase 4b should NOT touch it
    let assistant_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .collect();
    assert_eq!(
        assistant_blocks.len(),
        1,
        "Strands gen_ai.choice should survive (protected)"
    );
}

/// Output-source assistant from llm_response survives while input-source
/// assistant from llm_request is marked as history by Phase 4b.
#[test]
fn test_output_source_assistant_survives_input_source_marked() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(10)),
        Some("agent"),
    );

    // Non-root generation span with both input and output assistant text
    let gen_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "assistant", "content": "Old response from history"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": "New response from this turn",
                "finish_reason": "stop"
            }
        }
    ]);

    let gen_span = make_span_row_full(
        "trace1",
        "gen_span",
        Some("agent_root"),
        &gen_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let rows = vec![agent_root, gen_span];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // "Old response" should be gone (Phase 4b: input-source, assistant, non-root gen)
    let old = result.messages.iter().any(|m| {
        matches!(&m.content, ContentBlock::Text { text } if text == "Old response from history")
    });
    assert!(
        !old,
        "Input-source assistant should be marked as history and filtered"
    );

    // "New response" should survive (output-source, has finish_reason → protected)
    let new = result.messages.iter().any(|m| {
        matches!(&m.content, ContentBlock::Text { text } if text == "New response from this turn")
    });
    assert!(
        new,
        "Output-source assistant with finish_reason should survive"
    );
}

/// Phase 4b marks tool_use from input source as history, output-source tool_use wins dedup.
#[test]
fn test_tool_use_input_vs_output_source_quality() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let agent_root = make_span_row_full(
        "trace1",
        "agent_root",
        None,
        "[]",
        t0,
        Some(t0 + dur(20)),
        Some("agent"),
    );

    // span1 output: gen_ai.choice with tool_use
    let span1_msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:01Z"}},
            "content": {"role": "user", "content": "Do something"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_abc", "name": "my_tool", "input": {"x": 1}}]
            }
        }
    ]);

    let span1 = make_span_row_full(
        "trace1",
        "gen_span1",
        Some("agent_root"),
        &span1_msgs.to_string(),
        t0 + dur(1),
        Some(t0 + dur(3)),
        Some("generation"),
    );

    // span2 input: llm_request re-sends the tool_use as history
    let span2_msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call_abc", "name": "my_tool", "input": {"x": 1}}]
            }
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:06Z"}},
            "content": {"role": "tool", "tool_use_id": "call_abc", "content": "result_val"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:09Z"}},
            "content": {
                "role": "assistant",
                "content": "Done!",
                "finish_reason": "stop"
            }
        }
    ]);

    let span2 = make_span_row_full(
        "trace1",
        "gen_span2",
        Some("agent_root"),
        &span2_msgs.to_string(),
        t0 + dur(6),
        Some(t0 + dur(9)),
        Some("generation"),
    );

    let rows = vec![agent_root, span1, span2];
    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // tool_use should survive (from span1's gen_ai.choice, protected)
    let tool_uses: Vec<_> = result.messages.iter().filter(|m| m.is_tool_use()).collect();
    assert_eq!(tool_uses.len(), 1, "Exactly one tool_use should survive");

    // The surviving tool_use should be output-sourced (from gen_ai.choice event)
    assert!(
        tool_uses[0].is_output_event() || tool_uses[0].is_protected(),
        "Surviving tool_use should be from output/protected source"
    );
}

/// is_input_source and is_output_source classification propagates through pipeline.
/// Verify that blocks from ADK llm_request get is_input_source() = true.
#[test]
fn test_source_classification_propagates_to_blocks() {
    let t0 = fixed_time();
    let dur = chrono::Duration::seconds;

    let msgs = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "input msg"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": "2025-01-01T00:00:05Z"}},
            "content": {
                "role": "assistant",
                "content": "output msg",
                "finish_reason": "stop"
            }
        }
    ]);

    let row = make_span_row_full(
        "trace1",
        "span1",
        None, // root
        &msgs.to_string(),
        t0,
        Some(t0 + dur(5)),
        Some("generation"),
    );

    let options = FeedOptions::default();
    let result = process_spans(vec![row], &options);

    assert_eq!(result.messages.len(), 2);

    let user_block = &result.messages[0];
    assert_eq!(user_block.role, ChatRole::User);
    assert!(
        user_block.is_input_source(),
        "User block from llm_request should be input-source. source_attribute={:?}",
        user_block.source_attribute
    );

    let asst_block = &result.messages[1];
    assert_eq!(asst_block.role, ChatRole::Assistant);
    assert!(
        asst_block.is_output_source(),
        "Assistant block from llm_response should be output-source. source_attribute={:?}",
        asst_block.source_attribute
    );
}

// ============================================================================
// CROSS-TRACE SESSION DEDUP (PREFIX STRIP ENGINE)
// ============================================================================

// ----------------------------------------------------------------------------
// Test: 3 ADK traces with growing prefix → only new content from each
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_accumulated_history() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Trace 1: user("NYC weather") → assistant("NYC sunny")
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        }
    ]);

    // Trace 2: user("NYC weather") + assistant("NYC sunny") [history] + user("London") → assistant("London rain")
    let trace2_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "London weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "London rain"}
        }
    ]);

    // Trace 3: all history + user("Tokyo") → assistant("Tokyo cloudy")
    let trace3_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "NYC weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "NYC sunny"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "London weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "London rain"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "Tokyo weather"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Tokyo cloudy"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &trace1_msg.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &trace2_msg.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace3",
        "s3",
        None,
        &trace3_msg.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2, row3], &options);

    // Within-trace: Phase 4b filters assistant blocks from llm_request (input-source).
    // Cross-trace prefix strip: removes user/tool blocks already seen in prior traces.
    // Trace 1: user("NYC") + asst("NYC sunny") from llm_response = 2
    // Trace 2: prefix [user("NYC")] stripped, asst("NYC sunny") filtered by 4b
    //   → user("London") + asst("London rain") = 2
    // Trace 3: prefix [user("NYC"), user("London")] stripped, assts filtered by 4b
    //   → user("Tokyo") + asst("Tokyo cloudy") = 2
    // Total: 6
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        result.messages.len(),
        6,
        "Expected 6 blocks (2 per trace after prefix strip + 4b). Got {} blocks: {:?}",
        result.messages.len(),
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Single trace is unchanged
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_single_trace_unchanged() {
    let t0 = fixed_time();

    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    let row =
        make_span_row_with_timestamps("trace1", "span1", None, &msg.to_string(), t0, Some(t0));

    let options = FeedOptions::default();
    let result_via_process = process_spans(vec![row.clone()], &options);
    let result_via_trace = process_trace_spans(vec![row], &options);

    assert_eq!(
        result_via_process.messages.len(),
        result_via_trace.messages.len(),
        "Single trace: process_spans should match process_trace_spans"
    );
}

// ----------------------------------------------------------------------------
// Test: Two traces with no overlapping content → 0 stripped
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_no_overlap() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Goodbye"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(
        result.messages.len(),
        2,
        "No overlap: both blocks should be preserved"
    );
}

// ----------------------------------------------------------------------------
// Test: Event-based (Strands) traces are independent → all preserved
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_strands_independent() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Strands: unique event per trace, different content
    let msg1 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Question 1"}
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Answer 1"}
    }]);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Question 2"}
    }, {
        "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
        "content": {"role": "assistant", "content": "Answer 2"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    assert_eq!(
        result.messages.len(),
        4,
        "Strands: unique events per trace, all preserved. Got {}",
        result.messages.len()
    );
}

// ----------------------------------------------------------------------------
// Test: Pure replay trace → 0 new blocks
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_replay_fully_deduped() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "4"}
        }
    ]);

    // Trace2 replays identical content (re-execution)
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "4"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Pure replay should contribute 0 new blocks from trace2.
    // Attribute-based replay is treated as history re-send and stripped.
    let trace1_count = result
        .messages
        .iter()
        .filter(|b| b.trace_id == "trace1")
        .count();
    let trace2_count = result
        .messages
        .iter()
        .filter(|b| b.trace_id == "trace2")
        .count();
    assert!(
        trace1_count > 0,
        "Trace1 should contribute blocks. Got {}",
        trace1_count
    );
    assert!(
        trace2_count == 0,
        "Trace2 (pure replay) should contribute 0 blocks. Got {}",
        trace2_count
    );
}

// ----------------------------------------------------------------------------
// Test: System messages preserved per contributing trace
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_system_per_trace() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        }
    ]);

    // Trace2: same system + history prefix + new content
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Thanks"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Welcome"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace1: system + user + asst = 3
    // Trace2: system (preserved) + user("Thanks") + asst("Welcome") = 3
    // Phase 4b marks assistant from input-source as history within trace2,
    // so "Hi" from llm_request is already filtered by within-trace pipeline
    let system_count = result
        .messages
        .iter()
        .filter(|b| b.role == ChatRole::System)
        .count();
    assert!(
        system_count >= 1,
        "At least one system message should be preserved. Got {}",
        system_count
    );

    // Check that new content from trace2 is present
    let has_thanks = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "Thanks"));
    assert!(has_thanks, "user('Thanks') should be present from trace2");

    let has_welcome = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "Welcome"));
    assert!(has_welcome, "asst('Welcome') should be present from trace2");
}

// ----------------------------------------------------------------------------
// Test: ADK multi-span trace in session + Phase 4b
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_adk_multi_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: single generation span
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    // Trace 2: history + new question
    let trace2_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "How are you?"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "I am fine"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &trace1_msg.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &trace2_msg.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace 1: user("Hello") + asst("Hi there") = 2
    // Trace 2: prefix [user("Hello")] stripped (asst "Hi there" already filtered by 4b)
    //   → user("How are you?") + asst("I am fine") = 2
    // Total: 4
    let has_how = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "How are you?"));
    assert!(has_how, "user('How are you?') from trace2 should survive");

    let has_fine = result
        .messages
        .iter()
        .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "I am fine"));
    assert!(has_fine, "asst('I am fine') from trace2 should survive");
}

// ----------------------------------------------------------------------------
// Test: retain by trace_id simulates the trace endpoint view
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_retain_trace_view() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);

    // Simulate trace endpoint: retain only trace2 blocks
    result.messages.retain(|b| b.trace_id == "trace2");

    // After prefix strip + retain: only trace2's NEW content
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"Second question"),
        "Should have 'Second question'. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Second answer"),
        "Should have 'Second answer'. Got: {:?}",
        texts
    );
    // History should NOT be present
    assert!(
        !texts.contains(&"First question"),
        "Should NOT have 'First question' (history). Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: same-timestamp traces keep first-seen order for prefix strip
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_same_timestamp_trace_ordering() {
    let t0 = fixed_time();

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "First question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "First answer"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Second question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Second answer"}
        }
    ]);

    // Same timestamps and reverse lexical trace IDs: ordering must follow first-seen row
    // order (trace-z first), not trace_id sort (trace-a first).
    let mut row1 = make_span_row_full(
        "trace-z-older",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace-a-newer",
        "s2",
        None,
        &msg2.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2], &options);

    // Simulate trace endpoint retain-by-trace behavior
    result.messages.retain(|b| b.trace_id == "trace-a-newer");
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"Second question"),
        "Expected 'Second question' in target trace. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Second answer"),
        "Expected 'Second answer' in target trace. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"First question"),
        "History prefix should be stripped from target trace. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"First answer"),
        "History prefix should be stripped from target trace. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: system blocks in prefix are transparent (do not break scan)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_system_prefix_transparent() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Q1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A1"}
        }
    ]);

    // Trace2 re-sends history (system + Q1 + A1 in llm_request) then adds new turn.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Q1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Q2"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "A2"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2], &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"Q2"),
        "New user turn should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"A2"),
        "New assistant output should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"Q1"),
        "History user message should be stripped despite leading system block. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: accumulated history matching allows gaps (subsequence, not strict)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_subsequence_match() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace1 accumulated non-system sequence includes an output block ("B")
    // that won't be replayed in trace2 input prefix.
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "B"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D1"}
        }
    ]);

    // Trace2 replays A and C as history, but "B" is absent from prefix.
    // Strict matching would stop at C; subsequence matching should still strip C.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "E"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "F"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D2"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2], &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"E"),
        "New content should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"F"),
        "New content should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"D2"),
        "New assistant response should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"A"),
        "History A should be stripped. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"C"),
        "History C should be stripped even with accumulated gap. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: cross-trace prefix scan applies per span (not only trace start)
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_resets_per_span() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Prior trace contributes A/B to accumulated history.
    let trace1_msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "B"}
        }
    ]);

    // Target trace span1 starts with new content C.
    let trace2_span1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "D"}
        }
    ]);

    // Target trace span2 replays A/C, then adds E.
    // A should be stripped via cross-trace prefix even though it appears
    // in the second span, not at trace start.
    let trace2_span2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "system", "content": "sys"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "A"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "C"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "E"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "F"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &trace1_msg.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &trace2_span1.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace2",
        "s3",
        None,
        &trace2_span2.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2, row3], &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"C"),
        "New C should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"E"),
        "New E should remain. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"F"),
        "Final F should remain. Got: {:?}",
        texts
    );
    assert!(
        !texts.contains(&"A"),
        "Cross-trace replay A should be stripped in span2 prefix. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Replay trace contributes 0 tool defs
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_replay_no_tool_defs() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Use the tool"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Done"}
        }
    ]);

    let tool_defs =
        json!([{"type": "function", "function": {"name": "my_tool", "parameters": {}}}])
            .to_string();

    // Trace2 is pure replay
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Use the tool"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Done"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    row1.tool_definitions_json = tool_defs.clone();

    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    row2.tool_definitions_json = tool_defs;

    let options = FeedOptions::default();
    let result = process_spans(vec![row1, row2], &options);

    // Both traces contribute (guard prevents marking for pure replay).
    // Tool defs are deduplicated by name, so still 1 unique tool.
    assert_eq!(
        result.tool_definitions.len(),
        1,
        "Should have exactly 1 unique tool def after dedup. Got {}",
        result.tool_definitions.len()
    );
}

// ----------------------------------------------------------------------------
// Test: Repeated content AFTER prefix break is safe
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_repeated_content_safe() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: user("yes") + asst("confirmed")
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "confirmed"}
        }
    ]);

    // Trace 2: different prefix + user("yes") after break
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "new question"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "done"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace2 prefix: "new question" NOT in accumulated → STOP immediately → 0 stripped
    // So trace2 keeps: user("new question") + user("yes") + asst("done")
    let yes_count = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "yes"))
        .count();
    assert!(
        yes_count >= 2,
        "Both 'yes' should be preserved (different contexts). Found {}",
        yes_count
    );
}

// ----------------------------------------------------------------------------
// Test: cross-trace prefix matching is role-sensitive
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_role_sensitive() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: assistant says "yes"
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Question 1"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    // Trace 2: user says "yes" as new input (same content, different role)
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "yes"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Acknowledged"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        texts.contains(&"yes"),
        "User 'yes' in trace2 must not be stripped by assistant 'yes' from trace1. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Acknowledged"),
        "Trace2 assistant output should remain. Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: mixed event+attribute duplicates keep event copy in target trace
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_mixed_source_event_survives() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1 contributes "repeat" to accumulated history.
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "first"}
        }
    ]);

    // Trace 2 has the same user text from both event and attribute sources.
    // Cross-trace prefix must only mark the attribute copy as history.
    let msg2 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "repeat"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "fresh"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);
    result.messages.retain(|b| b.trace_id == "trace2");

    let repeat_blocks: Vec<_> = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "repeat"))
        .collect();

    assert_eq!(
        repeat_blocks.len(),
        1,
        "Exactly one 'repeat' should survive in trace2 after dedup. Got {:?}",
        result
            .messages
            .iter()
            .filter_map(|b| match &b.content {
                ContentBlock::Text { text } => Some((text.as_str(), b.source_type.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        repeat_blocks[0].source_type,
        source_type::EVENT,
        "Event-sourced user message should win over attribute history copy"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|b| matches!(&b.content, ContentBlock::Text { text } if text == "fresh")),
        "New assistant output should remain"
    );
}

// ----------------------------------------------------------------------------
// Test: repeated matches are bounded by accumulated occurrence count
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_prefix_occurrence_count_bounded() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    // Trace 1 contributes one "ping".
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a1"}
        }
    ]);

    // Trace 2 contributes a second "ping" after a prefix break.
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "barrier"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a2"}
        }
    ]);

    // Trace 3 starts with three "ping" entries.
    // Only first two should be stripped (bounded by accumulated count = 2).
    let msg3 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "ping"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t2.to_rfc3339()}},
            "content": {"role": "user", "content": "tail"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t2.to_rfc3339()}},
            "content": {"role": "assistant", "content": "a3"}
        }
    ]);

    let mut row1 = make_span_row_full(
        "trace1",
        "s1",
        None,
        &msg1.to_string(),
        t0,
        Some(t0),
        Some("generation"),
    );
    let mut row2 = make_span_row_full(
        "trace2",
        "s2",
        None,
        &msg2.to_string(),
        t1,
        Some(t1),
        Some("generation"),
    );
    let mut row3 = make_span_row_full(
        "trace3",
        "s3",
        None,
        &msg3.to_string(),
        t2,
        Some(t2),
        Some("generation"),
    );
    row1.session_id = Some("session1".to_string());
    row2.session_id = Some("session1".to_string());
    row3.session_id = Some("session1".to_string());

    let options = FeedOptions::default();
    let mut result = process_spans(vec![row1, row2, row3], &options);
    result.messages.retain(|b| b.trace_id == "trace3");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    let ping_count = texts.iter().filter(|&&t| t == "ping").count();
    assert_eq!(
        ping_count, 1,
        "Only one non-history 'ping' should remain in trace3. Got {:?}",
        texts
    );
    assert!(
        texts.contains(&"tail"),
        "Content after prefix break should remain. Got {:?}",
        texts
    );
    assert!(
        texts.contains(&"a3"),
        "New assistant output should remain. Got {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Strands traces with "yes" in both (different turns) → both preserved
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_strands_repeated_yes() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Strands: unique events per trace
    let msg1 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Do you agree?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    let msg2 = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Confirm again?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "yes"}
        }
    ]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg1.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // "Do you agree?" not in accumulated → STOP immediately → 0 stripped from trace2
    assert_eq!(
        result.messages.len(),
        4,
        "Strands: all 4 blocks preserved. Got {}",
        result.messages.len()
    );

    let yes_count = result
        .messages
        .iter()
        .filter(|b| matches!(&b.content, ContentBlock::Text { text } if text == "yes"))
        .count();
    assert_eq!(yes_count, 2, "Both 'yes' should be preserved");
}

// ----------------------------------------------------------------------------
// Test: Multi-trace detection routing check
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_multi_trace_detection() {
    let t0 = fixed_time();

    // Single trace: should go through process_trace_spans path
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
        "content": {"role": "user", "content": "Hello"}
    }]);

    let single_row =
        make_span_row_with_timestamps("trace1", "s1", None, &msg.to_string(), t0, Some(t0));
    let options = FeedOptions::default();

    // Single trace
    let r1 = process_spans(vec![single_row.clone()], &options);
    let r2 = process_trace_spans(vec![single_row], &options);
    assert_eq!(r1.messages.len(), r2.messages.len());

    // Two traces with same content
    let t1 = t0 + chrono::Duration::seconds(10);
    let msg2 = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": t1.to_rfc3339()}},
        "content": {"role": "user", "content": "Different"}
    }]);

    let rows = vec![
        make_span_row_with_timestamps("trace1", "s1", None, &msg.to_string(), t0, Some(t0)),
        make_span_row_with_timestamps("trace2", "s2", None, &msg2.to_string(), t1, Some(t1)),
    ];

    let r3 = process_spans(rows, &options);
    // Multi-trace path: both unique → both preserved
    assert_eq!(r3.messages.len(), 2);
}

// ----------------------------------------------------------------------------
// Test: Genuine repeated user message preserved (the reported bug)
// User asks the same question in trace 2 as in trace 1.
// The history re-send copy should be stripped but the genuine copy preserved.
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_genuine_repeat_preserved() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    // Trace 1: user("Hello") → assistant("Hi")
    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        }
    ]);

    // Trace 2: history re-send [user("Hello"), asst("Hi")] + genuine repeat user("Hello")
    // Framework re-sends full history as prefix of llm_request, then adds new message.
    // The new message happens to be "Hello" again (user asks the same question).
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi there"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hello again!"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Trace 1: user("Hello") + asst("Hi there") = 2
    // Trace 2: Cross-trace prefix marks first user("Hello") and asst("Hi there") as history.
    //   Within-trace dedup: user("Hello") has history copy + genuine copy → non-history wins.
    //   asst("Hi there") from llm_request is history-only → dropped.
    //   Result: user("Hello") + asst("Hello again!") = 2
    // Total: 4
    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    // The genuine "Hello" from trace 2 MUST be preserved
    let hello_count = texts.iter().filter(|&&t| t == "Hello").count();
    assert_eq!(
        hello_count, 2,
        "Both 'Hello' messages (trace1 + trace2 genuine) must be preserved. Got: {:?}",
        texts
    );

    // The new response from trace 2 MUST be present
    assert!(
        texts.contains(&"Hello again!"),
        "asst('Hello again!') from trace2 must be preserved. Got: {:?}",
        texts
    );

    // History re-send of "Hi there" from trace2's llm_request should be dropped
    let hi_count = texts.iter().filter(|&&t| t == "Hi there").count();
    assert_eq!(
        hi_count, 1,
        "Only trace1's 'Hi there' should survive (trace2's is history). Got: {:?}",
        texts
    );
}

// ----------------------------------------------------------------------------
// Test: Trace endpoint view with genuine repeated content
// Simulates the exact bug scenario: viewing a single trace via the trace endpoint
// where the trace has messages with same content as prior traces.
// ----------------------------------------------------------------------------

#[test]
fn test_cross_trace_retain_genuine_repeat_trace_view() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(10);

    let msg1 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        }
    ]);

    // Trace 2: re-sends history + user says "Hello" again
    let msg2 = json!([
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_request", "time": t1.to_rfc3339()}},
            "content": {"role": "user", "content": "Hello"}
        },
        {
            "source": {"attribute": {"key": "gcp.vertex.agent.llm_response", "time": t1.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Hi again"}
        }
    ]);

    let rows = vec![
        make_span_row_full(
            "trace1",
            "s1",
            None,
            &msg1.to_string(),
            t0,
            Some(t0),
            Some("generation"),
        ),
        make_span_row_full(
            "trace2",
            "s2",
            None,
            &msg2.to_string(),
            t1,
            Some(t1),
            Some("generation"),
        ),
    ];

    let options = FeedOptions::default();
    let mut result = process_spans(rows, &options);

    // Simulate trace endpoint: retain only trace2 blocks
    result.messages.retain(|b| b.trace_id == "trace2");

    let texts: Vec<&str> = result
        .messages
        .iter()
        .filter_map(|b| match &b.content {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    // The genuine "Hello" from trace2 MUST be present (this was the reported bug)
    assert!(
        texts.contains(&"Hello"),
        "Genuine 'Hello' from trace2 must be preserved in trace view. Got: {:?}",
        texts
    );
    assert!(
        texts.contains(&"Hi again"),
        "New response 'Hi again' from trace2 must be present. Got: {:?}",
        texts
    );
}

// ============================================================================
// LOGFIRE / OPENAI AGENTS: ASSISTANT PROMOTION IN CHOICELESS GENERATION SPANS
// ============================================================================
// Logfire stores LLM output as gen_ai.assistant.message (not gen_ai.choice).
// Without promotion, assistant messages sort by array index alongside inputs,
// causing incorrect ordering (system=0, assistant=1, user=2 → assistant before user).

#[test]
fn test_logfire_assistant_promoted_when_no_choice() {
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(2);

    // Generation span with parent, no gen_ai.choice — only gen_ai.*.message events
    let msgs = json!([
        {
            "source": {"event": {"name": "gen_ai.system.message", "time": t0.to_rfc3339()}},
            "content": {"role": "system", "content": "You are helpful."}
        },
        {
            "source": {"event": {"name": "gen_ai.assistant.message", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": "Here is the answer."}
        },
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "What is 2+2?"}
        }
    ]);

    let rows = vec![make_span_row_with_timestamps(
        "trace1",
        "gen-span",
        Some("parent-span"),
        &msgs.to_string(),
        t0,
        Some(t1),
    )];

    let options = FeedOptions::default();
    let result = process_spans(rows, &options);

    // Collect roles in order
    let roles: Vec<ChatRole> = result.messages.iter().map(|b| b.role).collect();

    // Assistant should come AFTER user (promoted to span_end timestamp)
    assert_eq!(
        roles,
        vec![ChatRole::System, ChatRole::User, ChatRole::Assistant],
        "Expected system -> user -> assistant, got {:?}",
        roles
    );

    // Verify the promoted assistant block has correct flags
    let assistant_block = result
        .messages
        .iter()
        .find(|b| b.role == ChatRole::Assistant)
        .expect("Should have assistant block");
    assert!(
        assistant_block.uses_span_end,
        "Promoted assistant should use span_end"
    );
    assert!(
        assistant_block.is_protected(),
        "Promoted assistant should be protected from history marking"
    );
}

#[test]
fn test_no_promotion_when_choice_exists() {
    // Verify promotion is suppressed when gen_ai.choice is present.
    // Uses classify_blocks directly to test the classification logic
    // without dedup/history phases interfering.
    let t0 = fixed_time();
    let t1 = t0 + chrono::Duration::seconds(2);

    let span_timestamps = std::collections::HashMap::from([(
        "gen-span".to_string(),
        super::dedup::SpanTimestamps {
            span_start: t0,
            span_end: Some(t1),
        },
    )]);

    // Build blocks manually: gen_ai.assistant.message + gen_ai.choice in same gen span
    let assistant_block = BlockEntry {
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: "Previous response.".to_string(),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: "gen-span".to_string(),
        session_id: None,
        message_index: 1,
        entry_index: 0,
        parent_span_id: Some("parent-span".to_string()),
        span_path: vec!["parent-span".to_string(), "gen-span".to_string()],
        timestamp: t0,
        observation_type: Some("generation".to_string()),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        name: None,
        finish_reason: None,
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: Some("gen_ai.assistant.message".to_string()),
        source_attribute: None,
        category: crate::data::types::MessageCategory::GenAIAssistantMessage,
        content_hash: "hash_prev".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
    };

    let choice_block = BlockEntry {
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: "4".to_string(),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: "gen-span".to_string(),
        session_id: None,
        message_index: 3,
        entry_index: 0,
        parent_span_id: Some("parent-span".to_string()),
        span_path: vec!["parent-span".to_string(), "gen-span".to_string()],
        timestamp: t1,
        observation_type: Some("generation".to_string()),
        model: Some("gpt-4".to_string()),
        provider: Some("openai".to_string()),
        name: None,
        finish_reason: Some(FinishReason::Stop),
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "event".to_string(),
        event_name: Some("gen_ai.choice".to_string()),
        source_attribute: None,
        category: crate::data::types::MessageCategory::GenAIChoice,
        content_hash: "hash_4".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
    };

    let mut blocks = vec![assistant_block.clone(), choice_block.clone()];
    super::classify_blocks(&mut blocks, &span_timestamps);

    // gen_ai.choice should be classified normally (uses_span_end from is_protected)
    let choice = &blocks[1];
    assert!(choice.is_protected(), "gen_ai.choice should be protected");
    assert!(choice.uses_span_end, "gen_ai.choice should use span_end");

    // gen_ai.assistant.message should NOT be promoted (choice exists in this span)
    let asst = &blocks[0];
    assert_eq!(
        asst.category,
        crate::data::types::MessageCategory::GenAIAssistantMessage,
        "gen_ai.assistant.message should keep original category when choice exists"
    );
    assert!(
        !asst.uses_span_end,
        "gen_ai.assistant.message should NOT use span_end when choice exists"
    );
}
