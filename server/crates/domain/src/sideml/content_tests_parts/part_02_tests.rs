#[test]
fn test_vercel_text_with_value_field() {
    // Vercel AI SDK alternative text format uses "value" instead of "text"
    let block = json!({
        "type": "text",
        "value": "Hello world"
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "text");
    assert_eq!(normalized["text"], "Hello world");
}

#[test]
fn test_tool_result_deduplication() {
    // Vercel AI SDK sends both raw data and wrapped format
    // After normalization, these should deduplicate to a single block
    let content = json!([
        {"status": "success", "content": [{"json": {"city": "NYC"}}]},
        {"type": "json", "value": {"status": "success", "content": [{"json": {"city": "NYC"}}]}}
    ]);

    let result = normalize_tool_result_content(Some(content));
    let arr = result.as_array().unwrap();

    // Both blocks normalize to the same {type: "json", data: ...}
    // Deduplication should keep only one
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "json");
}

/// Two identical parts are two occurrences, not one datum written twice.
///
/// The collapse above exists for a carrier that writes one result in two encodings, which normalisation
/// makes identical. A tool that genuinely returned the same part twice writes two *identical* sources, and
/// collapsing those loses an occurrence the carrier's position is evidence of - the same distinction the
/// feed draws between a repeated tool call and a re-sent one.
#[test]
fn identical_tool_result_parts_are_both_kept() {
    let content = json!([
        {"type": "text", "text": "retry"},
        {"type": "text", "text": "retry"}
    ]);

    let result = normalize_tool_result_content(Some(content));
    let arr = result.as_array().unwrap();
    assert_eq!(
        arr.len(),
        2,
        "a tool that returned the same part twice returned two parts: {arr:?}"
    );
}

#[test]
fn test_tool_result_different_content_not_deduplicated() {
    // Different content should NOT be deduplicated
    let content = json!([
        {"status": "success", "data": "result1"},
        {"status": "error", "data": "result2"}
    ]);

    let result = normalize_tool_result_content(Some(content));
    let arr = result.as_array().unwrap();

    // Different content, should keep both
    assert_eq!(arr.len(), 2);
}

#[test]
fn test_vercel_tool_result_exact_format() {
    // Exact format from trace ac5d340c732c90544960cd2cb39a4113
    let first_item = json!({
        "status": "success",
        "content": [{"json": {"city": "London", "days": 7}}]
    });

    let second_item = json!({
        "type": "json",
        "value": {"status": "success", "content": [{"json": {"city": "London", "days": 7}}]}
    });

    // Test each item individually
    let first_result = normalize_content_block(&first_item);
    let second_result = normalize_content_block(&second_item);

    assert!(first_result.is_some(), "First item should normalize");
    assert!(second_result.is_some(), "Second item should normalize");

    let first_norm = first_result.unwrap();
    let second_norm = second_result.unwrap();

    // Both should become {type: "json", data: ...}
    assert_eq!(first_norm["type"], "json", "First should be json type");
    assert_eq!(second_norm["type"], "json", "Second should be json type");

    // They should be identical
    assert_eq!(
        first_norm, second_norm,
        "Both should normalize to identical json blocks"
    );
}

#[test]
fn test_double_encoded_json_string_with_tool_use() {
    // Content stored as JSON string (double-encoded) - common from some SDKs
    let content = JsonValue::String(
            r#"[{"toolUse": {"toolUseId": "call_123", "name": "get_weather", "input": {"city": "NYC"}}}]"#
                .to_string(),
        );

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Should have one tool_use block");
    assert_eq!(arr[0]["type"], "tool_use", "Should be tool_use type");
    assert_eq!(arr[0]["id"], "call_123");
    assert_eq!(arr[0]["name"], "get_weather");
}

#[test]
fn test_double_encoded_json_string_with_text() {
    // Text content stored as JSON string (double-encoded)
    let content = JsonValue::String(r#"[{"text": "Hello world"}]"#.to_string());

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Should have one text block");
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "Hello world");
}

#[test]
fn test_plain_string_not_json() {
    // Plain text string (not JSON) should become text block
    let content = JsonValue::String("This is just plain text".to_string());

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Should have one text block");
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "This is just plain text");
}

#[test]
fn test_vercel_ai_response_format() {
    // Vercel AI SDK aggregated response: {"content": "...", "finishReason": "stop", "role": "assistant"}
    let content = json!({
        "content": "Perfect! Here's your weather forecast...",
        "finishReason": "stop",
        "role": "assistant",
        "providerMetadata": {"bedrock": {"usage": {}}}
    });

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Should have one text block");
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(
        arr[0]["text"], "Perfect! Here's your weather forecast...",
        "Should extract text from content field"
    );
}

// ========== Vercel AI file format tests ==========

#[test]
fn test_vercel_file_format_image() {
    // Vercel AI SDK file format for images
    let block = json!({
        "type": "file",
        "mediaType": "image/jpeg",
        "data": "base64encodeddata..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some(), "Should handle Vercel file format");
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "image");
    assert_eq!(normalized["media_type"], "image/jpeg");
    assert_eq!(normalized["source"], "base64");
    assert_eq!(normalized["data"], "base64encodeddata...");
}

#[test]
fn test_vercel_file_format_pdf() {
    // Vercel AI SDK file format for PDFs
    let block = json!({
        "type": "file",
        "mediaType": "application/pdf",
        "data": "pdfbase64data..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some(), "Should handle Vercel PDF file format");
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "document");
    assert_eq!(normalized["media_type"], "application/pdf");
    assert_eq!(normalized["source"], "base64");
}

#[test]
fn test_vercel_file_format_audio() {
    // Vercel AI SDK file format for audio
    let block = json!({
        "type": "file",
        "mediaType": "audio/mp3",
        "data": "audiobase64data..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some(), "Should handle Vercel audio file format");
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "audio");
    assert_eq!(normalized["media_type"], "audio/mp3");
    assert_eq!(normalized["source"], "base64");
}

#[test]
fn test_vercel_file_format_with_file_reference() {
    // Vercel AI SDK file with #!B64!#:: content-addressed reference
    let block = json!({
        "type": "file",
        "mediaType": "image/png",
        "data": "#!B64!#::abc123hash"
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "image");
    assert_eq!(normalized["source"], "file", "Should detect file reference");
    assert_eq!(normalized["data"], "#!B64!#::abc123hash");
}

#[test]
fn test_vercel_file_format_with_mime_type_variant() {
    // Some versions may use "mimeType" instead of "mediaType"
    let block = json!({
        "type": "file",
        "mimeType": "image/webp",
        "data": "webpdata..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some(), "Should handle mimeType variant");
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "image");
    assert_eq!(normalized["media_type"], "image/webp");
}

#[test]
fn test_vercel_file_format_video() {
    // Vercel AI SDK file format for video
    let block = json!({
        "type": "file",
        "mediaType": "video/mp4",
        "data": "videobase64data..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "video");
    assert_eq!(normalized["media_type"], "video/mp4");
}

#[test]
fn test_vercel_file_format_unknown_mime() {
    // Unknown MIME type should fall back to "file" content type
    let block = json!({
        "type": "file",
        "mediaType": "application/octet-stream",
        "data": "binarydata..."
    });

    let result = try_vercel_format(&block);
    assert!(result.is_some());
    let normalized = result.unwrap();

    assert_eq!(normalized["type"], "file");
    assert_eq!(normalized["media_type"], "application/octet-stream");
}

// ========== REGRESSION TESTS ==========
// Tests for specific real-world issues that were fixed

#[test]
fn regression_openinference_message_content_wrapper_langgraph() {
    // Regression test for trace 0bda91d1d9f955fd3e2dab4b9664333b
    // LangGraph/ChatBedrockConverse stores content as:
    // llm.output_messages.0.message.contents.1.message_content.text
    //
    // After unflatten, this becomes:
    // {"contents": [{}, {"message_content": {"text": "..."}}]}
    //
    // The message_content wrapper must be unwrapped and the sparse placeholder filtered.
    let content = json!([
        {},  // Sparse array placeholder (index 0 missing)
        {
            "message_content": {
                "type": "text",
                "text": "Hello! Here's your 3-day weather forecast..."
            }
        }
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(
        arr.len(),
        1,
        "Should filter sparse placeholder and keep real content"
    );
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(
        arr[0]["text"], "Hello! Here's your 3-day weather forecast...",
        "Should extract text from message_content wrapper"
    );
}

#[test]
fn regression_sparse_array_with_reasoning_content() {
    // Regression test: OpenInference extended thinking with sparse array
    // llm.output_messages.0.message.contents.0.reasoning_content.text exists
    // but contents.1 might be sparse or have message_content
    let content = json!([
        {
            "reasoning_content": {
                "text": "Let me think about this weather forecast...",
                "signature": "sig123"
            }
        },
        {},  // Sparse placeholder
        {
            "message_content": {
                "type": "text",
                "text": "Based on my analysis..."
            }
        }
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(
        arr.len(),
        2,
        "Should filter placeholder, keep thinking and text"
    );
    assert_eq!(arr[0]["type"], "thinking");
    assert_eq!(
        arr[0]["text"],
        "Let me think about this weather forecast..."
    );
    assert_eq!(arr[0]["signature"], "sig123");
    assert_eq!(arr[1]["type"], "text");
    assert_eq!(arr[1]["text"], "Based on my analysis...");
}

#[test]
fn regression_empty_assistant_message_771e923f() {
    // Regression test for trace 771e923f9f7781491b41abc00d9d21fa
    // Issue: Assistant message showed "{}" due to sparse array placeholder
    // being normalized to json type instead of filtered
    //
    // This specific case had contents like:
    // [{"message_content": {"type": "text", "text": "..."}}, {}]
    let content = json!([
        {
            "message_content": {
                "type": "text",
                "text": "I can help with that weather forecast!"
            }
        },
        {}  // This was showing as "{}" in the UI
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 1, "Empty placeholder should be filtered out");
    assert_eq!(arr[0]["type"], "text");
    assert_ne!(
        arr[0]["type"], "json",
        "Should NOT have json block for empty placeholder"
    );
}

#[test]
fn regression_tool_use_with_sparse_contents() {
    // Regression test: Tool use blocks from OpenInference with sparse arrays
    // Real data pattern from LangGraph traces
    let content = json!([
        {},  // Sparse placeholder
        {
            "message_content": {
                "type": "tool_use",
                "id": "tooluse_abc123",
                "name": "temperature_forecast",
                "input": {"city": "NYC", "days": 3}
            }
        },
        {},  // Another sparse placeholder
        {
            "message_content": {
                "type": "tool_use",
                "id": "tooluse_xyz789",
                "name": "precipitation_forecast",
                "input": {"city": "NYC", "days": 3}
            }
        }
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 2, "Should have exactly 2 tool_use blocks");
    assert_eq!(arr[0]["type"], "tool_use");
    assert_eq!(arr[0]["name"], "temperature_forecast");
    assert_eq!(arr[1]["type"], "tool_use");
    assert_eq!(arr[1]["name"], "precipitation_forecast");
}

#[test]
fn regression_nested_sparse_arrays() {
    // Regression test: Deeply nested sparse array structures
    // Can occur with complex OpenInference message structures
    let content = json!([
        {
            "message_content": {
                "type": "text",
                "text": "First message"
            }
        },
        {
            // Object with only empty nested structures - should be treated as placeholder
            "nested": [{}, {}],
            "also_empty": {}
        },
        {
            "message_content": {
                "type": "text",
                "text": "Second message"
            }
        }
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 2, "Should filter nested empty structure");
    assert_eq!(arr[0]["text"], "First message");
    assert_eq!(arr[1]["text"], "Second message");
}

// ========== Python repr parsing tests ==========

#[test]
fn test_python_repr_simple_dict() {
    let input = "{'status': 'success', 'value': 42}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["value"], 42);
}

#[test]
fn test_python_repr_booleans_and_none() {
    let input = "{'active': True, 'deleted': False, 'data': None}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["active"], true);
    assert_eq!(parsed["deleted"], false);
    assert!(parsed["data"].is_null());
}

#[test]
fn test_python_repr_nested_openai_agents_tool_result() {
    // Exact format from OpenAI Agents SDK trace 019c31ff
    let input = "{'status': 'success', 'content': [{'json': {'city': 'New York City', 'days': 3, 'forecast': [{'day': 1, 'condition': 'Sunny', 'high': 25, 'low': 15}]}}]}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["status"], "success");
    let forecast = &parsed["content"][0]["json"]["forecast"][0];
    assert_eq!(forecast["condition"], "Sunny");
    assert_eq!(forecast["high"], 25);
}

#[test]
fn test_python_repr_list() {
    let input = "[{'name': 'tool1'}, {'name': 'tool2'}]";
    let parsed = try_parse_python_repr(input).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "tool1");
}

#[test]
fn test_python_repr_not_triggered_for_plain_text() {
    assert!(try_parse_python_repr("This is just plain text").is_none());
    assert!(try_parse_python_repr("True story").is_none());
    assert!(try_parse_python_repr("").is_none());
}

#[test]
fn test_python_repr_valid_json_passthrough() {
    // Valid JSON also parses (upstream checks JSON first, this is a safety check)
    let input = r#"{"key": "value"}"#;
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["key"], "value");
}

#[test]
fn test_python_repr_double_quoted_string_with_apostrophe() {
    // Python uses double quotes for strings containing single quotes
    let input = r#"{'message': "it's raining", 'count': 5}"#;
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["message"], "it's raining");
    assert_eq!(parsed["count"], 5);
}

#[test]
fn test_python_repr_single_quoted_string_with_double_quotes() {
    // Double quotes inside single-quoted Python string → must be escaped in JSON
    let input = "{'key': 'he said \"hello\"'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["key"], r#"he said "hello""#);
}

#[test]
fn test_python_repr_literal_word_boundary_in_identifier() {
    // "Trueness" should NOT be replaced, only standalone "True"
    let input = "{'flag': True, 'name': 'Trueness'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["flag"], true);
    assert_eq!(parsed["name"], "Trueness");
}

#[test]
fn test_python_repr_literal_not_replaced_inside_strings() {
    // "True" inside a quoted string must NOT be replaced
    let input = "{'label': 'True', 'value': True}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["label"], "True", "String 'True' must stay as string");
    assert_eq!(parsed["value"], true, "Bare True must become boolean");
}

#[test]
fn test_python_repr_none_inside_string_preserved() {
    let input = "{'msg': 'None of the above', 'val': None}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["msg"], "None of the above");
    assert!(parsed["val"].is_null());
}

#[test]
fn test_python_repr_false_inside_string_preserved() {
    let input = "{'label': 'False positive', 'flag': False}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["label"], "False positive");
    assert_eq!(parsed["flag"], false);
}

#[test]
fn test_python_repr_underscore_boundary() {
    // _True should NOT be replaced (word boundary includes underscore)
    let input = "{'_True': 1, 'True_val': 2, 'ok': True}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["_True"], 1);
    assert_eq!(parsed["True_val"], 2);
    assert_eq!(parsed["ok"], true);
}

#[test]
fn test_python_repr_backslash_in_string() {
    // Python path with backslashes
    let input = r"{'path': 'C:\\temp\\file.txt'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["path"], "C:\\temp\\file.txt");
}

#[test]
fn test_python_repr_escaped_single_quote() {
    // Python \' inside single-quoted string → literal single quote
    let input = r"{'msg': 'it\'s fine'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["msg"], "it's fine");
}

#[test]
fn test_python_repr_newline_escape() {
    let input = r"{'text': 'line1\nline2'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["text"], "line1\nline2");
}

#[test]
fn test_python_repr_empty_structures() {
    assert_eq!(try_parse_python_repr("{}").unwrap(), json!({}));
    assert_eq!(try_parse_python_repr("[]").unwrap(), json!([]));
}

#[test]
fn test_python_repr_nested_booleans() {
    let input = "{'outer': {'inner': True}, 'list': [False, None, True]}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["outer"]["inner"], true);
    assert_eq!(parsed["list"][0], false);
    assert!(parsed["list"][1].is_null());
    assert_eq!(parsed["list"][2], true);
}

#[test]
#[allow(clippy::approx_constant)]
fn test_python_repr_float_values() {
    let input = "{'pi': 3.14, 'neg': -1.5, 'exp': 1e10}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["pi"], 3.14);
    assert_eq!(parsed["neg"], -1.5);
}

#[test]
fn test_python_repr_mixed_quote_styles() {
    // Mix of single and double quoted strings in Python output
    let input = r#"{'single': 'value', 'double': "value2", 'apostrophe': "it's"}"#;
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["single"], "value");
    assert_eq!(parsed["double"], "value2");
    assert_eq!(parsed["apostrophe"], "it's");
}

#[test]
fn test_python_repr_unicode_content() {
    let input = "{'emoji': '\\u2764', 'text': 'caf\\u00e9'}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["emoji"], "\u{2764}");
    assert_eq!(parsed["text"], "caf\u{00e9}");
}

#[test]
fn test_python_repr_normalize_content_integration() {
    // Full pipeline: Python repr string → normalized content
    let content = JsonValue::String(
        "{'status': 'success', 'content': [{'json': {'city': 'NYC'}}]}".to_string(),
    );
    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "json");
    assert_eq!(arr[0]["data"]["status"], "success");
}

#[test]
fn test_python_repr_graceful_fallback_for_unsupported_escapes() {
    // Python-only escapes like \x produce invalid JSON → returns None → text fallback
    let input = "{'hex': '\\x41'}";
    // \x is not valid JSON escape, so parse may fail → returns None
    // This is acceptable: content falls back to plain text display
    let result = try_parse_python_repr(input);
    // Don't assert success or failure - just verify no panic
    let _ = result;
}

#[test]
fn test_python_repr_boolean_at_start_and_end() {
    let input = "[True, False, None]";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed[0], true);
    assert_eq!(parsed[1], false);
    assert!(parsed[2].is_null());
}

#[test]
fn test_python_repr_adjacent_booleans() {
    // Booleans separated only by comma/space (no alphanumeric between)
    let input = "{'a': True, 'b': False, 'c': None, 'd': True}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["a"], true);
    assert_eq!(parsed["b"], false);
    assert!(parsed["c"].is_null());
    assert_eq!(parsed["d"], true);
}

#[test]
fn test_python_repr_real_world_autogen_tool_result() {
    // AutoGen tool results can also produce Python repr
    let input = "{'name': 'get_weather', 'call_id': 'call_abc123', 'content': 'Sunny, 25C', 'is_error': False}";
    let parsed = try_parse_python_repr(input).unwrap();
    assert_eq!(parsed["name"], "get_weather");
    assert_eq!(parsed["call_id"], "call_abc123");
    assert_eq!(parsed["content"], "Sunny, 25C");
    assert_eq!(parsed["is_error"], false);
}

#[test]
fn regression_langchain_kwargs_in_array() {
    // Regression test: LangChain serialized messages with kwargs wrapper
    // Common pattern when LangChain messages are stored in OTEL attributes
    let content = json!([
        {
            "kwargs": {
                "content": "System prompt here",
                "type": "system"
            }
        },
        {
            "kwargs": {
                "content": "User question",
                "type": "human"
            }
        }
    ]);

    let result = normalize_content(Some(&content));
    let arr = result.as_array().unwrap();

    assert_eq!(arr.len(), 2, "Should unwrap both kwargs wrappers");
    // Both should be normalized (kwargs unwrapped)
    for block in arr {
        assert!(
            block.get("type").is_some(),
            "Each block should have a type after kwargs unwrapping"
        );
    }
}

// ========== Media fallback tests ==========

#[test]
fn test_media_fallback_mime_type_file() {
    let block = json!({
        "type": "file",
        "mime_type": "application/pdf",
        "data": "#!B64!#::f65fabcd",
        "name": "task-document"
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "document");
    assert_eq!(result["media_type"], "application/pdf");
    assert_eq!(result["source"], "file");
    assert_eq!(result["data"], "#!B64!#::f65fabcd");
    assert_eq!(result["name"], "task-document");
}

#[test]
fn test_media_fallback_mime_type_image() {
    let block = json!({
        "type": "image",
        "mime_type": "image/png",
        "data": "iVBORw0KGgoAAAA..."
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["media_type"], "image/png");
    assert_eq!(result["source"], "base64");
}

#[test]
fn test_media_fallback_nested_image_double() {
    let block = json!({
        "type": "image",
        "image": {
            "image": {
                "url": "https://example.com/photo.png"
            }
        }
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "url");
    assert_eq!(result["data"], "https://example.com/photo.png");
}

#[test]
fn test_media_fallback_nested_image_single() {
    let block = json!({
        "type": "image",
        "image": {
            "url": "https://example.com/photo.png"
        }
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "url");
    assert_eq!(result["data"], "https://example.com/photo.png");
}

#[test]
fn test_media_fallback_nested_data() {
    let block = json!({
        "type": "audio",
        "audio": {
            "data": "AAAA..."
        }
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "audio");
    assert_eq!(result["source"], "base64");
    assert_eq!(result["data"], "AAAA...");
}

#[test]
fn test_media_fallback_no_false_positive() {
    // text blocks are handled by earlier handlers; media fallback should not match
    let block = json!({"type": "text", "text": "hello"});
    let result = try_media_fallback(&block);
    assert!(result.is_none());
}

#[test]
fn test_media_fallback_file_reference() {
    let block = json!({
        "type": "image",
        "mime_type": "image/jpeg",
        "data": "#!B64!#::abc123hash"
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["source"], "file");
}

#[test]
fn test_media_fallback_mime_type_without_type_field() {
    let block = json!({
        "mime_type": "image/png",
        "data": "iVBORw0KGgoAAAA..."
    });
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["media_type"], "image/png");
    assert_eq!(result["source"], "base64");
}

// ========== Raw string in content array tests ==========

#[test]
fn test_normalize_content_block_raw_string() {
    let block = json!("Describe the image contents in detail.");
    let result = normalize_content_block(&block).unwrap();
    assert_eq!(result["type"], "text");
    assert_eq!(result["text"], "Describe the image contents in detail.");
}

#[test]
fn test_normalize_content_block_empty_string() {
    let block = json!("");
    let result = normalize_content_block(&block);
    assert!(result.is_none(), "Empty strings should be filtered");
}

#[test]
fn test_media_fallback_bare_file_reference() {
    let block = json!({"data": "#!B64!#::ea5c033d17c29a42a315b46b3c261350"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "file");
    assert_eq!(result["source"], "file");
    assert_eq!(result["data"], "#!B64!#::ea5c033d17c29a42a315b46b3c261350");
}

#[test]
fn test_normalize_content_mixed_array() {
    // AutoGen MultiModalMessage: ["text instruction", {"data": "#!B64!#::hash"}]
    let content = json!([
        "Describe the image contents in detail.",
        {"data": "#!B64!#::ea5c033d17c29a42a315b46b3c261350"}
    ]);
    let result = normalize_content(Some(&content));
    let blocks = result.as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "Describe the image contents in detail.");
    assert_eq!(blocks[1]["type"], "file");
    assert_eq!(blocks[1]["source"], "file");
}

#[test]
fn test_media_fallback_bare_data_no_file_ref() {
    // Regular data field without #!B64!# prefix should NOT match
    let block = json!({"data": "some-random-string"});
    let result = try_media_fallback(&block);
    assert!(
        result.is_none(),
        "Non-file-reference data should not match media fallback"
    );
}
