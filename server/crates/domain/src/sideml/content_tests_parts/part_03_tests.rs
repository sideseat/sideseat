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
