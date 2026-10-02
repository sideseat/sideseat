// ========== MIME-bearing file URI tests ==========

#[test]
fn test_parse_data_url_file_uri_with_mime() {
    let (source, data, media_type) = parse_data_url("#!B64!#image/png::hash123");
    assert_eq!(source, "file");
    assert_eq!(data, "#!B64!#image/png::hash123");
    assert_eq!(media_type, Some("image/png".to_string()));
}

#[test]
fn test_parse_data_url_file_uri_without_mime() {
    let (source, data, media_type) = parse_data_url("#!B64!#::hash123");
    assert_eq!(source, "file");
    assert_eq!(data, "#!B64!#::hash123");
    assert_eq!(media_type, None);
}

#[test]
fn test_media_fallback_bare_file_ref_with_mime() {
    let block = json!({"data": "#!B64!#image/jpeg::ea5c033d"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
    assert_eq!(result["media_type"], "image/jpeg");
}

#[test]
fn test_media_fallback_bare_file_ref_with_pdf_mime() {
    let block = json!({"data": "#!B64!#application/pdf::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "document");
    assert_eq!(result["media_type"], "application/pdf");
}

#[test]
fn test_media_fallback_bare_file_ref_with_audio_mime() {
    let block = json!({"data": "#!B64!#audio/mpeg::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "audio");
    assert_eq!(result["media_type"], "audio/mpeg");
}

#[test]
fn test_media_fallback_bare_file_ref_without_mime() {
    // Without MIME, should default to "file" type (no media_type field)
    let block = json!({"data": "#!B64!#::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "file");
    assert_eq!(result["source"], "file");
    // No media_type field present when MIME is unknown
    assert!(
        result.get("media_type").is_none() || result["media_type"].is_null(),
        "media_type should be absent or null when no MIME"
    );
}

#[test]
fn test_normalize_content_mixed_array_with_mime() {
    // AutoGen MultiModalMessage with MIME-bearing URI
    let content = json!([
        "Describe the image contents in detail.",
        {"data": "#!B64!#image/jpeg::ea5c033d"}
    ]);
    let result = normalize_content(Some(&content));
    let blocks = result.as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["media_type"], "image/jpeg");
    assert_eq!(blocks[1]["source"], "file");
}

#[test]
fn test_openai_image_url_with_mime_file_ref() {
    let block = json!({
        "type": "image_url",
        "image_url": {
            "url": "#!B64!#image/png::hash123"
        }
    });
    let result = try_openai_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
    assert_eq!(result["media_type"], "image/png");
}

#[test]
fn test_bedrock_media_with_mime_file_ref() {
    let block = json!({
        "image": {
            "format": "jpeg",
            "source": {
                "bytes": "#!B64!#image/jpeg::hash123"
            }
        }
    });
    let result = try_bedrock_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_gemini_inline_data_with_mime_file_ref() {
    let block = json!({
        "inline_data": {
            "mime_type": "image/webp",
            "data": "#!B64!#image/webp::hash123"
        }
    });
    let result = try_gemini_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_vercel_file_with_mime_file_ref() {
    let block = json!({
        "type": "file",
        "mediaType": "image/png",
        "data": "#!B64!#image/png::hash123"
    });
    let result = try_vercel_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_anthropic_media_with_mime_file_ref() {
    let block = json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/jpeg",
            "data": "#!B64!#image/jpeg::hash123"
        }
    });
    let result = try_anthropic_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn python_constructor_repr_preserves_data_and_exposes_volatile_metadata_to_rules() {
    let parsed = try_parse_python_constructor_repr(
        "ToolResponse(content=[TextBlock(type='text', text='Sunny, 22°C', \
         id='generated', created_at='2026-10-01T15:23:10', finished_at=None)], \
         state=<ToolResultState.SUCCESS: 'success'>, metadata={}, id='response-id')",
    )
    .expect("the supported constructor repr parses");

    assert_eq!(parsed["__python_constructor"], "ToolResponse");
    assert_eq!(parsed["content"][0]["__python_constructor"], "TextBlock");
    assert_eq!(parsed["content"][0]["text"], "Sunny, 22°C");
    assert_eq!(parsed["content"][0]["id"], "generated");
    assert_eq!(parsed["state"], "success");
}

#[test]
fn constructor_repr_inside_a_tool_response_becomes_semantic_content() {
    let repr = "TextBlock(type='text', text='Sunny, 22°C', id='generated', \
                created_at='2026-10-01T15:23:10', finished_at=None)";
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": serde_json::to_string(&vec![repr]).expect("the response serialises"),
    });

    assert_eq!(
        normalize_content_block(&block),
        Some(json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": [{"type": "text", "text": "Sunny, 22°C"}],
            "is_error": false,
        })),
        "generated ids and timestamps are not product content"
    );
}

#[test]
fn constructor_repr_keeps_agentscope_media_content_and_its_name() {
    let repr = "DataBlock(type='data', id='generated', \
                source=URLSource(type='url', url=AnyUrl('https://example.com/chart.png'), \
                media_type='image/png'), name='chart.png', \
                created_at='2026-10-01T15:23:10', finished_at=None)";
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": serde_json::to_string(&vec![repr]).expect("the response serialises"),
    });

    assert_eq!(
        normalize_content_block(&block),
        Some(json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": [{
                "type": "image",
                "media_type": "image/png",
                "source": "url",
                "data": "https://example.com/chart.png",
                "name": "chart.png",
            }],
            "is_error": false,
        }))
    );
}

#[test]
fn an_unknown_constructor_remains_text() {
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": "BusinessResult(value='keep this wording')",
    });
    assert_eq!(
        normalize_content_block(&block).expect("the tool result normalises")["content"],
        json!([{"type": "text", "text": "BusinessResult(value='keep this wording')"}])
    );
}

#[test]
fn canonical_tool_results_preserve_scalar_and_structured_json() {
    for content in [
        json!(0.0),
        json!(7),
        json!(true),
        json!({"value": {"amount": 7}}),
    ] {
        let block = json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": content,
        });
        let normalized = normalize_content_block(&block).expect("the canonical block survives");
        assert_eq!(
            normalized["content"], content,
            "canonical result data is not a provider payload to reinterpret"
        );
    }
}

/// The conventions' binary part carries its payload in `content` beside `mime_type`. It used to fall through
/// to an unknown block, so an image and a PDF a user sent rendered as raw JSON.
#[test]
fn a_semconv_blob_part_is_a_media_block_of_its_mime_type() {
    for (mime, kind) in [("image/jpeg", "image"), ("application/pdf", "document")] {
        let block = json!({"type": "blob", "mime_type": mime, "modality": "image", "content": "AAAA"});
        let normalized = normalize_content_block(&block).expect("the blob normalises");
        assert_eq!(normalized["type"], kind, "{mime}");
        assert_eq!(normalized["media_type"], mime);
        assert_eq!(normalized["data"], "AAAA");
    }
}

/// A tool result under `result` rather than `response` keeps its value and its tool name. Read only as
/// `response`, it became an empty result with no name - invisible wherever no tool span restated it.
#[test]
fn a_tool_call_response_under_result_keeps_its_value() {
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "name": "final_result",
        "result": "Final result processed.",
    });
    let normalized = normalize_content_block(&block).expect("the tool result normalises");
    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["tool_use_id"], "call-1");
    assert_eq!(normalized["name"], "final_result");
    assert_eq!(normalized["content"], "Final result processed.");
}
