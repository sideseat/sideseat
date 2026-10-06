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
fn test_gemini_multiple_function_responses_stay_distinct() {
    // The property this test protects: two results from the same function with different data
    // must not collapse into one. It used to be guaranteed by hashing the response into a
    // synthetic id; that id also had to reference a call, which it never did. Distinctness now
    // comes from the content itself (dedup falls back to content for an unresolved result),
    // and the id is filled in by feed/correlate.rs from the matching call when one exists.
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

    assert_eq!(block1["name"], "get_weather");
    assert_eq!(block2["name"], "get_weather");
    assert_ne!(
        block1["content"], block2["content"],
        "the two results must remain distinguishable by content"
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
fn test_gemini_function_response_null_response_carries_name() {
    // A null response is still a result: it must survive and carry its tool name so
    // correlation can pair it, but it must not acquire a fabricated id.
    let input = json!({
        "role": "tool",
        "content": [{"functionResponse": {"name": "delete_item", "response": null}}]
    });
    let output = normalize(&input);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["type"], "tool_result");
    assert_eq!(block["name"], "delete_item");
    assert!(
        block.get("tool_use_id").is_none() || block["tool_use_id"].is_null(),
        "no id should be fabricated for a null response"
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
