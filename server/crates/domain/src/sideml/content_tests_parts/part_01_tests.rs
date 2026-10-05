use super::*;

/// A value a tool **returned** is not a message's content block, in an array as much as alone.
///
/// The message chain consults the envelope cases, and one dialect's structured output is a `json` block whose
/// payload sits under `value` - so a returned `{"value": …}` had its own member stripped. The singleton path
/// was already envelope-free; the array path was not, which is one caller further along than the golden
/// fixture that first exposed this.
#[test]
fn a_returned_value_keeps_its_own_members() {
    let single = normalize_tool_result_content(Some(json!({"value": {"amount": 7}})));
    let in_array = normalize_tool_result_content(Some(json!([{"value": {"amount": 7}}])));

    // Alone: kept as it is, because nothing in the provider chain claims it.
    assert_eq!(
        single,
        json!({"value": {"amount": 7}}),
        "a returned object nothing recognises is the result itself"
    );
    // In an array: the same members survive. What matters is that `value` is still there - stripping it
    // would report the tool as having returned `{"amount": 7}`.
    let elements = in_array.as_array().expect("an array of blocks");
    assert_eq!(elements.len(), 1);
    let data = elements[0]
        .get("data")
        .or_else(|| elements[0].get("raw"))
        .expect("the block carries what was returned");
    assert!(
        data.get("value").is_some(),
        "the returned member survived normalisation: {in_array}"
    );
}

/// The declared wrapper cases answer exactly as `try_openinference_message_content` does.
///
/// Every wrapper it unwraps, the reasoning members, the serialisation envelope, and - just as important -
/// the shapes where it declines, since declining is what leaves a block to the rest of the chain.
#[test]
fn the_declared_wrappers_match_the_reader_they_replace() {
    use crate::rules::schema::ChainPosition;

    let plan = &crate::rules::ruleset().content_blocks;
    let cases: Vec<(&str, JsonValue)> = vec![
        (
            "one dialect's content wrapper around text",
            json!({"message_content": {"type": "text", "text": "hello"}}),
        ),
        (
            "the generic wrapper around text",
            json!({"value": {"type": "text", "text": "hello"}}),
        ),
        (
            "both wrappers, where the dialect's own is tried first",
            json!({
                "message_content": {"type": "text", "text": "first"},
                "value": {"type": "text", "text": "second"}
            }),
        ),
        (
            "a wrapper around an image",
            json!({"message_content": {"type": "image_url", "image_url": {"url": "https://x/y.png"}}}),
        ),
        (
            "a wrapper whose content this chain cannot read",
            json!({"message_content": {"utterly": "unknown"}}),
        ),
        (
            "an unreadable wrapper beside a reasoning member - the wrapper still claims the block",
            json!({"message_content": "", "reasoning_content": {"text": "thinking"}}),
        ),
        (
            "an unreadable wrapper beside a serialisation envelope",
            json!({"value": "", "kwargs": {"content": "hello"}}),
        ),
        (
            "a wrapper holding a bare string",
            json!({"message_content": "hello"}),
        ),
        ("a wrapper holding null", json!({"message_content": null})),
        (
            "reasoning with text and a signature",
            json!({"reasoning_content": {"text": "thinking", "signature": "sig"}}),
        ),
        (
            "reasoning with text alone",
            json!({"reasoning_content": {"text": "thinking"}}),
        ),
        (
            "reasoning with a signature alone",
            json!({"reasoning_content": {"signature": "sig"}}),
        ),
        (
            "reasoning under the other SDK's member name",
            json!({"reasoning": {"text": "thinking", "signature": "sig"}}),
        ),
        (
            "a reasoning member holding a bare string",
            json!({"reasoning": "thinking"}),
        ),
        (
            "a reasoning member holding an empty object",
            json!({"reasoning_content": {}}),
        ),
        (
            "a wrapper beside a reasoning member - the wrapper is tried first",
            json!({
                "value": {"type": "text", "text": "hello"},
                "reasoning_content": {"text": "thinking"}
            }),
        ),
        (
            "a serialisation envelope carrying content",
            json!({"kwargs": {"content": "hello", "type": "human"}}),
        ),
        (
            "an envelope carrying only a type",
            json!({"kwargs": {"type": "text", "text": "hello"}}),
        ),
        (
            "an envelope carrying neither",
            json!({"kwargs": {"id": "x"}}),
        ),
        (
            "an envelope whose content this chain cannot read",
            json!({"kwargs": {"content": {"utterly": "unknown"}}}),
        ),
        (
            "a block with none of these members",
            json!({"type": "text", "text": "hello"}),
        ),
        ("a bare string", json!("hello")),
        ("a list", json!([1, 2])),
    ];

    for (what, block) in cases {
        let declared = plan.normalize(&block, ChainPosition::MessageEnvelope);
        let retired = try_openinference_message_content(&block);
        assert_eq!(
            declared, retired,
            "the declared wrappers disagree with the reader they replace: {what}"
        );
    }
}

/// The declared cases for one dialect answer exactly as `try_vercel_format` does.
///
/// Every form it recognises, plus the shapes where it *declines* - because declining is what leaves a block
/// to the rest of the chain, and a case that claims one too eagerly turns an unrelated block into a
/// message. The one stated difference is the last case.
#[test]
fn the_declared_dialect_blocks_match_the_reader_they_replace() {
    use crate::rules::schema::ChainPosition;

    let plan = &crate::rules::ruleset().content_blocks;
    let cases: Vec<(&str, JsonValue)> = vec![
        (
            "a call with both an id and arguments",
            json!({"type": "tool-call", "toolCallId": "c1", "toolName": "search", "input": {"q": "x"}}),
        ),
        (
            "a call whose arguments are under the older member name",
            json!({"type": "tool-call", "toolCallId": "c1", "toolName": "search", "args": {"q": "x"}}),
        ),
        (
            "a call whose newer member is an empty object beside a filled older one",
            json!({"type": "tool-call", "toolName": "search", "input": {}, "args": {"q": "x"}}),
        ),
        (
            "a call with no id, which is still a call",
            json!({"type": "tool-call", "toolName": "search", "input": {"q": "x"}}),
        ),
        (
            "a call with no name, which names nothing to run",
            json!({"type": "tool-call", "toolCallId": "c1", "input": {}}),
        ),
        (
            "a result",
            json!({"type": "tool-result", "toolCallId": "c1", "result": {"ok": true}}),
        ),
        (
            "a result under the other member name, flagged as an error",
            json!({"type": "tool-result", "toolCallId": "c1", "output": "boom", "isError": true}),
        ),
        (
            "a result with the snake-case error flag",
            json!({"type": "tool-result", "toolCallId": "c1", "result": "boom", "is_error": true}),
        ),
        (
            "a result with no content at all",
            json!({"type": "tool-result", "toolCallId": "c1"}),
        ),
        (
            "structured data",
            json!({"type": "json", "value": {"a": 1}}),
        ),
        (
            "structured data that is a list",
            json!({"type": "json", "value": [1, 2]}),
        ),
        ("prose", json!({"type": "text", "value": "hello"})),
        ("a text block with no value", json!({"type": "text"})),
        (
            "a file with the newer media-type member",
            json!({"type": "file", "mediaType": "image/png", "data": "iVBOR"}),
        ),
        (
            "a file with the older one",
            json!({"type": "file", "mimeType": "image/jpeg", "data": "/9j/4"}),
        ),
        (
            "a file with no media type",
            json!({"type": "file", "data": "iVBOR"}),
        ),
        (
            "a file with no data",
            json!({"type": "file", "mediaType": "image/png"}),
        ),
        (
            "an aggregated response with a finish reason",
            json!({"content": "the answer", "finishReason": "stop", "role": "assistant"}),
        ),
        (
            "an aggregated response known only by its provider metadata",
            json!({"content": "the answer", "providerMetadata": {"x": 1}}),
        ),
        (
            "an aggregated response known only by its role",
            json!({"content": "the answer", "role": "assistant"}),
        ),
        (
            "a content string with none of those members",
            json!({"content": "the answer"}),
        ),
        (
            "a content string whose role is not the assistant's",
            json!({"content": "the question", "role": "user"}),
        ),
        (
            "a block whose content is not a string",
            json!({"content": {"parts": []}, "finishReason": "stop"}),
        ),
        (
            "an unrecognised type beside a content string - claimed by neither",
            json!({"type": "reasoning-part", "content": "hmm", "finishReason": "stop"}),
        ),
        (
            "a block with no type and no content",
            json!({"role": "assistant"}),
        ),
        ("a bare string, which is not an object", json!("hello")),
    ];

    for (what, block) in cases {
        let declared = plan.normalize(&block, ChainPosition::AfterProviderFormats);
        let retired = try_vercel_format(&block);
        assert_eq!(
            declared, retired,
            "the declared cases disagree with the reader they replace: {what}"
        );
    }

    // The one stated difference: a `type` that is not a *string*. The retired reader fell through to its
    // aggregated case for such a block; the declared condition asks only whether a type is present, which
    // this vocabulary can spell where "is not a string" cannot be. The difference is the safe direction -
    // the block goes to the rest of the chain instead of being claimed as prose - and no producer of this
    // dialect writes a non-string type.
    let non_string_type = json!({"type": 7, "content": "the answer", "finishReason": "stop"});
    assert_eq!(
        try_vercel_format(&non_string_type),
        Some(json!({"type": "text", "text": "the answer"})),
        "the retired reader claimed it"
    );
    assert_eq!(
        plan.normalize(&non_string_type, ChainPosition::AfterProviderFormats),
        None,
        "and the declared cases leave it to the rest of the chain"
    );
}

// ========== semconv 1.37 part types ==========

#[test]
fn test_semconv_reasoning_part_becomes_thinking() {
    // Before this was handled, a reasoning part fell through to the unknown
    // passthrough and the UI rendered {"type":"unknown"} instead of a thinking block.
    let block = json!({"type": "reasoning", "text": "step by step"});
    let out = normalize_content_block(&block).expect("reasoning part should normalize");
    assert_eq!(out["type"], "thinking");
    assert_eq!(out["text"], "step by step");
}

#[test]
fn test_semconv_reasoning_part_accepts_content_field() {
    let block = json!({"type": "reasoning", "content": "thought"});
    let out = normalize_content_block(&block).expect("reasoning part should normalize");
    assert_eq!(out["type"], "thinking");
    assert_eq!(out["text"], "thought");
}

#[test]
fn test_semconv_blob_part_becomes_document() {
    let block = json!({
        "type": "blob",
        "data": "data:application/pdf;base64,JVBERi0=",
        "media_type": "application/pdf"
    });
    let out = normalize_content_block(&block).expect("blob part should normalize");
    assert_eq!(out["type"], "document");
    assert_eq!(out["media_type"], "application/pdf");
}

#[test]
fn test_semconv_blob_part_falls_back_to_declared_media_type() {
    // No data URL prefix, so the media type can only come from the block itself.
    let block = json!({"type": "blob", "data": "cGxhaW4=", "media_type": "text/csv"});
    let out = normalize_content_block(&block).expect("blob part should normalize");
    assert_eq!(out["media_type"], "text/csv");
}

// ========== OpenInference nested format tests ==========

#[test]
fn test_openinference_message_content_text() {
    // OpenInference stores content blocks with message_content wrapper
    let block = json!({
        "message_content": {
            "type": "text",
            "text": "Hello world"
        }
    });

    let result = try_openinference_message_content(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "text");
    assert_eq!(normalized["text"], "Hello world");
}

#[test]
fn test_openinference_reasoning_content() {
    // OpenInference reasoning_content for extended thinking
    let block = json!({
        "reasoning_content": {
            "text": "Let me think through this...",
            "signature": "abc123"
        }
    });

    let result = try_openinference_message_content(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "thinking");
    assert_eq!(normalized["text"], "Let me think through this...");
    assert_eq!(normalized["signature"], "abc123");
}

#[test]
fn test_openinference_message_content_not_matched() {
    // Regular block without wrapper should not match
    let block = json!({
        "type": "text",
        "text": "Hello world"
    });

    let result = try_openinference_message_content(&block);
    assert!(result.is_none());
}

#[test]
fn test_langchain_kwargs_wrapper() {
    // LangChain serializes messages with kwargs wrapper
    let block = json!({
        "kwargs": {
            "content": "Hello from LangChain",
            "type": "human"
        }
    });

    let result = try_openinference_message_content(&block);
    assert!(result.is_some());
    // kwargs content with type gets normalized
    let normalized = result.unwrap();
    // The kwargs wrapper contains a content field, should normalize to text
    assert!(normalized["type"].as_str().is_some());
}

#[test]
fn test_value_wrapper() {
    // Some SDKs wrap content in a value field
    let block = json!({
        "value": {
            "type": "text",
            "text": "Wrapped text"
        }
    });

    let result = try_openinference_message_content(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();
    assert_eq!(normalized["type"], "text");
    assert_eq!(normalized["text"], "Wrapped text");
}

#[test]
fn test_empty_object_fallback_to_json() {
    // At the individual block level (try_unknown_fallback), empty objects
    // are classified as "json" structured output. However, they are filtered
    // out at the array level in normalize_content() if they appear to be
    // sparse array placeholders from unflatten.
    let block = json!({});
    let result = try_unknown_fallback(&block);
    assert!(result.is_some());
    assert_eq!(result.unwrap()["type"], "json");
}

// ========== Sparse array placeholder tests ==========

#[test]
fn test_sparse_array_placeholder_detection_empty_object() {
    assert!(is_sparse_array_placeholder(&json!({})));
}

#[test]
fn test_sparse_array_placeholder_detection_non_empty_object() {
    // Non-empty objects are NOT placeholders
    assert!(!is_sparse_array_placeholder(&json!({"key": "value"})));
    assert!(!is_sparse_array_placeholder(&json!({"type": "text"})));
    assert!(!is_sparse_array_placeholder(&json!({"empty": null})));
}

#[test]
fn test_sparse_array_placeholder_detection_nested_empty() {
    // Objects containing only empty arrays/objects are placeholders
    assert!(is_sparse_array_placeholder(&json!({"arr": []})));
    assert!(is_sparse_array_placeholder(&json!({"obj": {}})));
    assert!(is_sparse_array_placeholder(&json!({"arr": [{}, {}]})));
}

#[test]
fn test_sparse_array_placeholder_detection_nested_non_empty() {
    // Objects with any non-empty content are NOT placeholders
    assert!(!is_sparse_array_placeholder(&json!({"arr": [1]})));
    assert!(!is_sparse_array_placeholder(&json!({"obj": {"k": "v"}})));
    assert!(!is_sparse_array_placeholder(
        &json!({"arr": [{"type": "text"}]})
    ));
}

#[test]
fn test_sparse_array_placeholder_detection_primitives() {
    // Primitives are NOT placeholders
    assert!(!is_sparse_array_placeholder(&json!("text")));
    assert!(!is_sparse_array_placeholder(&json!(123)));
    assert!(!is_sparse_array_placeholder(&json!(null)));
    assert!(!is_sparse_array_placeholder(&json!(true)));
    assert!(!is_sparse_array_placeholder(&json!([1, 2, 3])));
}

#[test]
fn test_normalize_content_filters_sparse_placeholders() {
    // Array with empty placeholder objects (from unflatten sparse arrays)
    let content = json!([
        {},  // placeholder for missing index 0
        {"type": "text", "text": "Hello"}
    ]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Should filter out empty placeholder");
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "Hello");
}

#[test]
fn test_normalize_content_filters_multiple_placeholders() {
    // Multiple placeholders at different positions
    let content = json!([
        {},  // placeholder
        {"type": "text", "text": "First"},
        {},  // placeholder
        {"type": "text", "text": "Second"},
        {}   // placeholder
    ]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 2, "Should filter all placeholders");
    assert_eq!(arr[0]["text"], "First");
    assert_eq!(arr[1]["text"], "Second");
}

#[test]
fn test_normalize_content_keeps_valid_empty_structured_output() {
    // An object with ANY key (even if value is null/empty) is kept
    // This is valid structured output, not a placeholder
    let content = json!([
        {"status": "success", "data": []},  // Valid: has keys
        {"result": null}                     // Valid: has a key
    ]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 2, "Should keep objects with keys");
    assert_eq!(arr[0]["type"], "json");
    assert_eq!(arr[1]["type"], "json");
}

#[test]
fn test_normalize_content_all_empty_objects_kept_as_structured_output() {
    // Array with ONLY empty objects - could be intentional structured output
    // (unlike sparse arrays which have a MIX of empty and non-empty)
    let content = json!([{}, {}, {}]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    // All empty objects are kept (no non-empty elements to indicate sparse array)
    assert_eq!(
        arr.len(),
        3,
        "Should keep empty objects when no non-empty elements"
    );
    for block in arr {
        assert_eq!(block["type"], "json", "Empty objects become json type");
    }
}

#[test]
fn test_normalize_content_single_empty_object_is_structured_output() {
    // Single empty object - valid structured output (e.g., empty result)
    let content = json!([{}]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Single empty object should be kept");
    assert_eq!(arr[0]["type"], "json");
    assert_eq!(arr[0]["data"], json!({}));
}

#[test]
fn test_openinference_message_content_array() {
    // Array with message_content wrappers should normalize each
    let content = json!([
        { "message_content": { "type": "text", "text": "Hello" } }
    ]);
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "Hello");
}

// ========== Vercel AI SDK format tests ==========

#[test]
fn test_vercel_tool_call_format() {
    // Vercel AI SDK uses type: "tool-call" with camelCase fields
    let block = json!({
        "type": "tool-call",
        "toolCallId": "call_abc123",
        "toolName": "search_knowledge",
        "input": {"query": "test", "num_results": 5}
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["id"], "call_abc123");
    assert_eq!(normalized["name"], "search_knowledge");
    assert_eq!(normalized["input"]["query"], "test");
    assert_eq!(normalized["input"]["num_results"], 5);
}

#[test]
fn test_vercel_tool_result_format() {
    // Vercel AI SDK uses type: "tool-result" with result field
    let block = json!({
        "type": "tool-result",
        "toolCallId": "call_abc123",
        "toolName": "search_knowledge",
        "result": {"type": "text", "value": "Search results here"}
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["tool_use_id"], "call_abc123");
    assert_eq!(normalized["is_error"], false);
}

#[test]
fn test_vercel_tool_result_with_output_field() {
    // Some Vercel AI variants use "output" instead of "result"
    let block = json!({
        "type": "tool-result",
        "toolCallId": "call_xyz",
        "output": {"type": "text", "value": "Output content"}
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["tool_use_id"], "call_xyz");
}

#[test]
fn test_vercel_tool_result_error() {
    let block = json!({
        "type": "tool-result",
        "toolCallId": "call_error",
        "result": "Error: Something went wrong",
        "isError": true
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["is_error"], true);
}

#[test]
fn test_vercel_tool_call_with_args_field() {
    // Alternative format using "args" instead of "input"
    let block = json!({
        "type": "tool-call",
        "toolCallId": "call_alt",
        "toolName": "calculator",
        "args": {"expression": "2+2"}
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["input"]["expression"], "2+2");
}

// ========== Full pipeline normalization tests ==========

#[test]
fn test_normalize_content_vercel_tool_call_in_array() {
    // Full normalization: array with Vercel tool-call
    let content = json!([
        {"type": "tool-call", "toolCallId": "call_1", "toolName": "fn1", "input": {"x": 1}}
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "tool_use");
    assert_eq!(arr[0]["name"], "fn1");
}

#[test]
fn test_normalize_content_mixed_formats() {
    // Mix of formats in one array
    let content = json!([
        {"type": "text", "text": "Hello"},
        {"type": "tool-call", "toolCallId": "call_1", "toolName": "fn1", "input": {}},
        {"toolUse": {"toolUseId": "call_2", "name": "fn2", "input": {}}}
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[1]["type"], "tool_use");
    assert_eq!(arr[1]["name"], "fn1");
    assert_eq!(arr[2]["type"], "tool_use");
    assert_eq!(arr[2]["name"], "fn2");
}

// ========== Provider format tests ==========

#[test]
fn test_bedrock_tool_use_format() {
    let block = json!({
        "toolUse": {
            "toolUseId": "bedrock_call_1",
            "name": "get_weather",
            "input": {"city": "NYC"}
        }
    });

    let result = try_bedrock_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["id"], "bedrock_call_1");
    assert_eq!(normalized["name"], "get_weather");
}

#[test]
fn test_anthropic_tool_use_format() {
    let block = json!({
        "type": "tool_use",
        "id": "anthropic_call_1",
        "name": "calculator",
        "input": {"expression": "1+1"}
    });

    let result = try_anthropic_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["id"], "anthropic_call_1");
}

#[test]
fn test_strands_js_tool_use_format() {
    // Strands JS SDK flat camelCase format
    let block = json!({
        "type": "toolUse",
        "name": "temperature_forecast",
        "toolUseId": "tooluse_8C2XVjwnvbo8lFo7o8ysEL",
        "input": {"city": "New York City", "days": 3}
    });

    let result = normalize_content_block(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["id"], "tooluse_8C2XVjwnvbo8lFo7o8ysEL");
    assert_eq!(normalized["name"], "temperature_forecast");
    assert_eq!(normalized["input"]["city"], "New York City");
}

#[test]
fn test_strands_js_tool_result_format() {
    // Strands JS SDK flat camelCase format for tool results
    let block = json!({
        "type": "toolResult",
        "toolUseId": "tooluse_8C2XVjwnvbo8lFo7o8ysEL",
        "content": [{"text": "72F, partly cloudy"}],
        "status": "success"
    });

    let result = normalize_content_block(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["tool_use_id"], "tooluse_8C2XVjwnvbo8lFo7o8ysEL");
    assert_eq!(normalized["is_error"], false);
}

#[test]
fn test_gemini_function_call_format() {
    let block = json!({
        "functionCall": {
            "name": "search",
            "args": {"query": "test"}
        }
    });

    // Declared now (`content-blocks-gemini.json`), so through the whole chain.
    let normalized = normalize_content_block(&block).expect("a call normalises");

    assert_eq!(normalized["type"], "tool_use");
    assert_eq!(normalized["name"], "search");
    // Gemini generates synthetic IDs
    assert!(
        normalized["id"]
            .as_str()
            .unwrap()
            .starts_with("gemini_search_call_")
    );
}

#[test]
fn test_gemini_adk_thought_block() {
    // ADK sends thinking content as text blocks with thought=true flag
    let block = json!({
        "text": "Let me work through this logic puzzle step by step.",
        "thought": true
    });

    let result = try_gemini_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "thinking");
    assert_eq!(
        normalized["text"],
        "Let me work through this logic puzzle step by step."
    );
    assert_eq!(normalized["signature"], serde_json::Value::Null);
}

#[test]
fn test_gemini_adk_thought_false_not_thinking() {
    // thought=false should NOT be treated as thinking
    let block = json!({
        "text": "Regular text content",
        "thought": false
    });

    let result = try_gemini_format(&block);
    assert!(result.is_none());
}

#[test]
fn test_gemini_thinking_part() {
    // Gemini thinking: {"thinking": "..."}
    let block = json!({"thinking": "Step 1: analyze the problem"});

    let result = try_gemini_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "thinking");
    assert_eq!(normalized["text"], "Step 1: analyze the problem");
}

#[test]
fn test_openai_text_format() {
    let block = json!({"type": "text", "text": "Hello world"});

    let result = try_openai_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "text");
    assert_eq!(normalized["text"], "Hello world");
}

// ========== Edge cases ==========

#[test]
fn test_unknown_type_becomes_unknown() {
    let block = json!({"type": "future_type", "data": "something"});

    let result = normalize_content_block(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "unknown");
    assert_eq!(normalized["raw"]["type"], "future_type");
}

#[test]
fn test_plain_json_object_becomes_json_type() {
    // Structured output without type field
    let block = json!({"temperature": 72, "unit": "F"});

    let result = normalize_content_block(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "json");
    assert_eq!(normalized["data"]["temperature"], 72);
}

// ========== Vercel AI SDK content format tests ==========

#[test]
fn test_vercel_json_content_block() {
    // Vercel AI SDK uses {type: "json", value: ...} for structured data
    let block = json!({
        "type": "json",
        "value": {"status": "success", "data": [1, 2, 3]}
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "json");
    assert_eq!(normalized["data"]["status"], "success");
    assert_eq!(normalized["data"]["data"], json!([1, 2, 3]));
}
