
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
    use crate::sideml::ContentBlock;

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
    use crate::sideml::ContentBlock;

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

/// One attachment reported twice - with its filename by one instrumentation, without it by another -
/// is one document. Hashing the name made the trace show the same PDF twice.
#[test]
fn a_document_is_identified_by_its_bytes_not_its_name() {
    use crate::sideml::types::ContentBlock;
    let document = |name: Option<&str>| ContentBlock::Document {
        media_type: Some("application/pdf".to_string()),
        name: name.map(str::to_string),
        source: "base64".to_string(),
        data: "JVBERi0xLjMKJcTl8uXrp/Og0MTGCg==".to_string(),
    };

    assert_eq!(
        super::extraction::compute_block_hash(&document(Some("task"))),
        super::extraction::compute_block_hash(&document(None))
    );
}
