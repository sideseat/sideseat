use chrono::Utc;
use serde_json::json;

use super::*;
use crate::sideml::types::{ChatRole, ContentBlock, FinishReason};
use sideseat_ports::types::MessageSpanRow;

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
        request_frame: String::new(),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_timestamp: ts,
        span_end_timestamp: None,
        messages_json: messages_json.to_string(),
        tool_definitions_json: tool_definitions_json.to_string(),
        tool_names_json: tool_names_json.to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
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
        scope_name: None,
        scope_version: None,
        span_name: None,
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
        span_marks: 0,
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
        request_frame: String::new(),
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        span_timestamp: span_start,
        span_end_timestamp: span_end,
        messages_json: messages_json.to_string(),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
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
        scope_name: None,
        scope_version: None,
        span_name: None,
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
        span_marks: 0,
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
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text, .. } if text == "Hello")
    );
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
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text, .. } if text == "First")
    );
    assert!(
        matches!(&result.messages[1].content, ContentBlock::Text { text, .. } if text == "Second")
    );
}

/// A name repeated with the **same** statement is one tool; a name repeated with a *different* one is two.
///
/// Two descriptions that differ are two statements, and merging them kept whichever scored higher and dropped the
/// other with nothing saying so. Corpus evidence for why that matters: `crewai/swarm`'s agents each declare
/// `Delegate work to coworker` with a description enumerating **their own** coworkers ("Code Reviewer" against
/// "Coding Specialist"), so the merged list told the user the wrong coworkers for one of the two agents - a
/// statement about that agent nobody had made. Two forms where one merely states *less* are still one tool, which
/// is what the merge is for and what the tests below pin.
#[test]
fn test_deduplicate_tools() {
    // Identical statements collapse.
    let deduped = deduplicate_tools(vec![
        json!({"type": "function", "function": {"name": "tool_a", "description": "A"}}),
        json!({"type": "function", "function": {"name": "tool_b", "description": "B"}}),
        json!({"type": "function", "function": {"name": "tool_a", "description": "A"}}),
    ]);
    let names: Vec<_> = deduped
        .iter()
        .filter_map(|t| t.get("function")?.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["tool_a", "tool_b"]);

    // Contradicting statements both survive, in the order they were stated.
    let deduped = deduplicate_tools(vec![
        json!({"type": "function", "function": {"name": "tool_a", "description": "A"}}),
        json!({"type": "function", "function": {"name": "tool_b", "description": "B"}}),
        json!({"type": "function", "function": {"name": "tool_a", "description": "A again"}}),
    ]);
    let described: Vec<_> = deduped
        .iter()
        .filter_map(|t| {
            let f = t.get("function")?;
            Some((f.get("name")?.as_str()?, f.get("description")?.as_str()?))
        })
        .collect();
    assert_eq!(
        described,
        vec![("tool_a", "A"), ("tool_a", "A again"), ("tool_b", "B")],
        "a contradicting definition is a distinct definition, not a version of the same one"
    );

    // And a name stated with nothing else is still the same tool: this is refinement, not contradiction.
    let deduped = deduplicate_tools(vec![
        json!({"type": "function", "function": {"name": "tool_a", "description": "A"}}),
        json!({"type": "function", "function": {"name": "tool_a"}}),
    ]);
    assert_eq!(deduped.len(), 1);
    assert_eq!(
        deduped[0]["function"]["description"].as_str(),
        Some("A"),
        "the fuller statement survives a refinement merge"
    );
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

/// The role filter must be a view over the finished feed, not a stage inside it.
///
/// Filtering during flattening removed blocks the later stages read. `role=tool` deleted the
/// assistant tool calls that correlation uses to give an id-less result its call's id; the two
/// results then fell back to content identity, which is the same for both, and dedup collapsed
/// them - so asking for the tool messages returned *fewer* than the unfiltered feed contains.
///
/// Stated as a property rather than a fixed count: for every role, filtering must return exactly
/// the blocks of that role the unfiltered feed returns.
#[test]
fn role_filter_returns_exactly_the_unfiltered_blocks_of_that_role() {
    // Two sequential calls to the same tool whose results are byte-identical and carry no id -
    // the Gemini/ADK shape. Distinguishable only through their calls.
    let msg = json!([
        {
            "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
            "content": {"role": "user", "content": "Weather in Paris and Lyon?"}
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:01Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call-paris", "name": "get_weather",
                             "input": {"city": "Paris"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:02Z"}},
            "content": {
                "role": "user",
                "content": [{"type": "tool_result", "name": "get_weather", "content": "sunny"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:03Z"}},
            "content": {
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "call-lyon", "name": "get_weather",
                             "input": {"city": "Lyon"}}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.tool.message", "time": "2025-01-01T00:00:04Z"}},
            "content": {
                "role": "user",
                "content": [{"type": "tool_result", "name": "get_weather", "content": "sunny"}]
            }
        },
        {
            "source": {"event": {"name": "gen_ai.choice", "time": "2025-01-01T00:00:05Z"}},
            "content": {"role": "assistant", "content": "Both are sunny."}
        }
    ]);

    let row = || make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");

    let unfiltered = process_spans(vec![row()], &FeedOptions::new());
    assert_eq!(
        unfiltered
            .messages
            .iter()
            .filter(|b| b.role == ChatRole::Tool)
            .count(),
        2,
        "the fixture must produce two distinct tool results, or it cannot detect the collapse"
    );

    for role in ["user", "assistant", "tool", "system"] {
        let expected: Vec<String> = unfiltered
            .messages
            .iter()
            .filter(|b| b.role.as_str() == role)
            .map(|b| format!("{:?}", b.content))
            .collect();

        let filtered = process_spans(
            vec![row()],
            &FeedOptions::new().with_role(Some(role.to_string())),
        );
        let actual: Vec<String> = filtered
            .messages
            .iter()
            .map(|b| format!("{:?}", b.content))
            .collect();

        assert_eq!(
            actual, expected,
            "?role={role} must return exactly the {role} blocks of the unfiltered feed"
        );
        assert_eq!(
            filtered.metadata.block_count,
            filtered.messages.len(),
            "?role={role} reported a block count that does not match what it returned"
        );
    }
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
        ContentBlock::ToolUse {
            id, name, input, ..
        } => {
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
    assert!(
        matches!(&result.messages[0].content, ContentBlock::Text { text, .. } if text == "First")
    );
    assert!(
        matches!(&result.messages[1].content, ContentBlock::Text { text, .. } if text == "Second")
    );
}

#[test]
fn test_metadata() {
    let msg = json!([{
        "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
        "content": {"role": "user", "content": "Test"}
    }]);

    let row = make_span_row("trace1", "span1", None, &msg.to_string(), "[]", "[]");
    let options = FeedOptions::default();
    let result = process_spans(vec![row.clone()], &options);

    assert_eq!(result.metadata.block_count, 1);
    assert_eq!(result.metadata.span_count, 1);
    // A trace's totals are the store's (its counted-usage rule): a reconstruction states none of its own.
    assert_eq!(result.metadata.span_usage, None);

    // A span view states what its one span recorded.
    let span = process_span(vec![row], &options);
    let usage = span.metadata.span_usage.expect("a span view's own usage");
    assert_eq!(usage.total_tokens, 150);
    assert!((usage.total_cost - 0.01).abs() < 0.001);
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
