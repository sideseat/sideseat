#[test]
fn test_response_data_tool_calls() {
    // response_data with tool_calls preserved
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "response_data".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "call_1", "function": {"name": "get_weather", "arguments": "{}"}}]
            },
            "usage": {"prompt_tokens": 10}
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(result.len(), 1, "Should unwrap message with tool_calls");
    assert!(
        result[0].0.content.get("tool_calls").is_some(),
        "tool_calls should be preserved"
    );
}

#[test]
fn test_response_data_streaming() {
    // response_data streaming: {combined_chunk_content: "...", chunk_count: N}
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "response_data".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "combined_chunk_content": "Hello from streaming!",
            "chunk_count": 5
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(
        result.len(),
        1,
        "Should synthesize message from streaming content"
    );
    assert_eq!(
        result[0].0.content.get("role").and_then(|r| r.as_str()),
        Some("assistant")
    );
    assert_eq!(
        result[0].0.content.get("content").and_then(|c| c.as_str()),
        Some("Hello from streaming!")
    );
}

#[test]
fn test_response_data_empty_streaming_skipped() {
    // Empty combined_chunk_content should not be synthesized
    let raw = RawMessage {
        source: MessageSource::Attribute {
            key: "response_data".to_string(),
            time: Utc::now(),
        },
        content: json!({
            "combined_chunk_content": "",
            "chunk_count": 0
        }),
    };

    let mut result = Vec::new();
    expand_message_array(&mut result, &raw, &PositionPath::root(0));

    assert_eq!(
        result.len(),
        1,
        "Should keep as-is when streaming content is empty"
    );
    // Kept as-is (the original wrapper object)
    assert!(result[0].0.content.get("combined_chunk_content").is_some());
}
