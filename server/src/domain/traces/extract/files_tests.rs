use super::*;
use serde_json::json;

fn make_base64_image(size: usize) -> String {
    let data = vec![0u8; size];
    format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&data))
}

fn make_raw_base64(size: usize) -> String {
    let data = vec![0u8; size];
    BASE64_STANDARD.encode(&data)
}

#[test]
fn test_extract_data_url() {
    let mut messages = json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/png",
            "data": make_base64_image(2048)
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, Some("image/png".to_string()));
    assert_eq!(result.files[0].size, 2048);

    // Verify replacement
    let data = messages["source"]["data"].as_str().unwrap();
    assert!(data.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_extract_raw_base64() {
    let mut messages = json!({
        "type": "image",
        "source": {
            "bytes": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert!(result.files[0].media_type.is_none());

    let data = messages["source"]["bytes"].as_str().unwrap();
    assert!(data.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_skip_small_files() {
    let mut messages = json!({
        "source": {
            "data": make_base64_image(512) // Below threshold
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_skip_protected_fields() {
    let mut messages = json!({
        "text": make_raw_base64(2048),
        "content": make_raw_base64(2048),
        "thinking": make_raw_base64(2048)
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_skip_urls() {
    let mut messages = json!({
        "url": "https://example.com/image.png",
        "data": "http://example.com/file"
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
}

#[test]
fn test_deduplicate_same_content() {
    let base64_data = make_base64_image(2048);
    let mut messages = json!({
        "images": [
            { "data": base64_data.clone() },
            { "data": base64_data.clone() },
            { "data": base64_data.clone() }
        ]
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    // Only one file should be extracted (deduplicated)
    assert_eq!(result.files.len(), 1);

    // All three should have the same hash reference
    let arr = messages["images"].as_array().unwrap();
    let hash1 = arr[0]["data"].as_str().unwrap();
    let hash2 = arr[1]["data"].as_str().unwrap();
    let hash3 = arr[2]["data"].as_str().unwrap();
    assert_eq!(hash1, hash2);
    assert_eq!(hash2, hash3);
}

#[test]
fn test_nested_json_string() {
    let inner_json = serde_json::to_string(&json!({
        "type": "image",
        "data": make_base64_image(2048)
    }))
    .unwrap();

    let mut messages = json!({
        "attributes": inner_json
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    // The inner JSON should be modified and re-serialized
    let attrs_str = messages["attributes"].as_str().unwrap();
    let inner: JsonValue = serde_json::from_str(attrs_str).unwrap();
    assert!(inner["data"].as_str().unwrap().starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_skip_placeholders() {
    let mut messages = json!({
        "data": "<replaced>",
        "bytes": "<binary>",
        "base64": ""
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
}

#[test]
fn test_already_extracted() {
    let mut messages = json!({
        "data": "#!B64!#::abc123def456"
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_is_file_uri() {
    assert!(is_file_uri("#!B64!#::abc123"));
    assert!(!is_file_uri("data:image/png;base64,abc"));
    assert!(!is_file_uri("https://example.com"));
}

#[test]
fn test_openai_image_url_format() {
    // OpenAI uses image_url.url for data URLs
    let mut messages = json!({
        "type": "image_url",
        "image_url": {
            "url": make_base64_image(2048)
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let url = messages["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_bedrock_format() {
    // Bedrock uses source.bytes
    let mut messages = json!({
        "image": {
            "format": "jpeg",
            "source": {
                "bytes": make_raw_base64(2048)
            }
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let bytes = messages["image"]["source"]["bytes"].as_str().unwrap();
    assert!(bytes.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_gemini_format() {
    // Gemini uses inline_data.data
    let mut messages = json!({
        "inline_data": {
            "mime_type": "image/jpeg",
            "data": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let data = messages["inline_data"]["data"].as_str().unwrap();
    assert!(data.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_multiple_different_files() {
    let mut messages = json!({
        "images": [
            { "data": make_base64_image(2048) },
            { "data": make_base64_image(4096) }  // Different size = different content
        ]
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 2);
}

#[test]
fn test_raw_span_nested_stringified_json_with_bytes() {
    // Simulates the exact raw_span structure from OTLP:
    // attributes contain stringified JSON with base64 in bytes field
    let inner_content = json!([{
        "type": "task-document",
        "source": {
            "bytes": make_raw_base64(2048)
        }
    }]);
    let stringified = serde_json::to_string(&inner_content).unwrap();

    let mut raw_span = json!({
        "trace_id": "abc123",
        "attributes": {
            "gen_ai.content.input": stringified
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(result.modified, "Should have modified the raw_span");
    assert_eq!(result.files.len(), 1, "Should have extracted 1 file");

    // Verify the nested JSON was modified
    let attrs = raw_span["attributes"]["gen_ai.content.input"]
        .as_str()
        .unwrap();
    let inner: JsonValue = serde_json::from_str(attrs).unwrap();
    let bytes = inner[0]["source"]["bytes"].as_str().unwrap();
    assert!(
        bytes.starts_with(FILE_URI_PREFIX),
        "bytes should be replaced with #!B64!#:: URI, got: {}",
        bytes
    );
}

#[test]
fn test_nested_json_in_protected_field() {
    // Tests that nested JSON is processed even when inside a protected field
    // This is the case for events[].attributes.content in raw_span
    let inner_content = json!([{
        "image": {
            "format": "jpeg",
            "source": {
                "bytes": make_raw_base64(2048)
            }
        }
    }]);
    let stringified = serde_json::to_string(&inner_content).unwrap();

    // "content" is a protected field, but nested JSON should still be processed
    let mut raw_span = json!({
        "events": [{
            "name": "gen_ai.choice",
            "attributes": {
                "content": stringified  // Protected field containing nested JSON
            }
        }]
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        result.modified,
        "Should have modified nested JSON inside protected field"
    );
    assert_eq!(result.files.len(), 1, "Should have extracted 1 file");

    // Verify the nested JSON was modified
    let content = raw_span["events"][0]["attributes"]["content"]
        .as_str()
        .unwrap();
    let inner: JsonValue = serde_json::from_str(content).unwrap();
    let bytes = inner[0]["image"]["source"]["bytes"].as_str().unwrap();
    assert!(
        bytes.starts_with(FILE_URI_PREFIX),
        "bytes should be replaced with #!B64!#:: URI, got: {}",
        bytes
    );
}

#[test]
fn test_protected_field_plain_text_not_extracted() {
    // Plain text in protected fields should NOT be processed even if it looks like base64
    // This is important for user-generated content
    let mut messages = json!({
        "text": make_raw_base64(2048),  // Text that happens to be valid base64
        "content": make_raw_base64(2048),  // Content that is just a string
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(
        !result.modified,
        "Plain text in protected fields should not be modified"
    );
    assert!(
        result.files.is_empty(),
        "No files should be extracted from protected plain text"
    );
}

// ========================================================================
// Edge Case Tests
// ========================================================================

#[test]
fn test_base64_with_newlines() {
    // Some encoders add newlines every 76 characters
    let raw = make_raw_base64(2048);
    let with_newlines: String = raw
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 76 == 0 {
                vec!['\n', c]
            } else {
                vec![c]
            }
        })
        .collect();

    let mut messages = json!({
        "bytes": with_newlines
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract base64 with newlines");
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_base64_url_safe_variant() {
    // URL-safe base64 uses - and _ instead of + and /
    // Create data that would have + and / in standard base64
    let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    let url_safe = BASE64_URL_SAFE.encode(&data);

    // Verify it contains URL-safe chars
    assert!(
        url_safe.contains('-') || url_safe.contains('_'),
        "Test data should contain URL-safe chars"
    );

    let mut messages = json!({
        "bytes": url_safe
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract URL-safe base64");
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].size, 2048);
}

#[test]
fn test_double_nested_json() {
    // JSON inside JSON inside JSON
    let innermost = json!({
        "bytes": make_raw_base64(2048)
    });
    let inner = serde_json::to_string(&innermost).unwrap();
    let outer = serde_json::to_string(&json!({
        "nested": inner
    }))
    .unwrap();

    let mut messages = json!({
        "deeply_nested": outer
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract from double-nested JSON");
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_long_text_starting_with_brace() {
    // Text that starts with { but isn't valid JSON shouldn't crash
    let fake_json = format!(
        "{{This is not JSON but starts with brace: {}",
        make_raw_base64(2048)
    );

    let mut messages = json!({
        "data": fake_json
    });

    // Should not panic, should not extract (not valid JSON, not valid base64)
    let result = extract_and_replace_files(&mut messages);
    // The data field doesn't contain valid base64 because of the prefix
    assert!(!result.modified);
}

#[test]
fn test_tool_result_with_image() {
    // Tool results can contain embedded images
    let mut messages = json!({
        "toolResult": {
            "toolUseId": "tool123",
            "status": "success",
            "content": [{
                "image": {
                    "format": "png",
                    "source": {
                        "bytes": make_raw_base64(2048)
                    }
                }
            }]
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract from tool result");
    assert_eq!(result.files.len(), 1);

    let bytes = messages["toolResult"]["content"][0]["image"]["source"]["bytes"]
        .as_str()
        .unwrap();
    assert!(bytes.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_array_with_mixed_content() {
    // Array with both extractable and non-extractable items
    let mut messages = json!([
        {"type": "text", "text": "Hello world"},
        {"type": "image", "bytes": make_raw_base64(2048)},
        {"type": "text", "text": "More text"},
        {"type": "document", "bytes": make_raw_base64(4096)}
    ]);

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 2, "Should extract 2 files from array");
}

#[test]
fn test_empty_nested_json() {
    // Empty JSON strings shouldn't cause issues
    let mut messages = json!({
        "empty_obj": "{}",
        "empty_arr": "[]",
        "data": make_raw_base64(2048)
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_very_large_base64_still_works() {
    // 1MB file should still be extracted
    let large_data = vec![0u8; 1024 * 1024]; // 1MB
    let large_base64 = BASE64_STANDARD.encode(&large_data);

    let mut messages = json!({
        "bytes": large_base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].size, 1024 * 1024);
}

#[test]
fn test_sideseat_uri_not_re_extracted() {
    // Already extracted content should not be processed again
    let mut messages = json!({
        "bytes": "#!B64!#::abc123def456abc123def456abc123def456abc123def456abc123def456abc1"
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_data_url_with_whitespace() {
    // Some data URLs have whitespace in them
    let data = vec![0u8; 2048];
    let base64_with_spaces: String = BASE64_STANDARD
        .encode(&data)
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 76 == 0 {
                vec![' ', c]
            } else {
                vec![c]
            }
        })
        .collect();

    let data_url = format!("data:image/png;base64,{}", base64_with_spaces);

    let mut messages = json!({
        "url": data_url
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract data URL with whitespace");
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_multiple_images_same_content_deduplicated() {
    // Same content appearing multiple times in different locations
    let base64 = make_raw_base64(2048);
    let stringified_inner = serde_json::to_string(&json!({
        "bytes": base64.clone()
    }))
    .unwrap();

    let mut messages = json!({
        "image1": { "bytes": base64.clone() },
        "image2": { "bytes": base64.clone() },
        "nested": stringified_inner
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    // Should only have 1 file (deduplicated)
    assert_eq!(result.files.len(), 1, "Same content should be deduplicated");
}

// ========================================================================
// Magic Bytes Detection Tests
// ========================================================================

#[test]
fn test_magic_bytes_jpeg() {
    // JPEG magic bytes: FF D8 FF
    let mut jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg_data.extend(vec![0u8; 2048 - 4]);
    let base64 = BASE64_STANDARD.encode(&jpeg_data);

    let mut messages = json!({
        "bytes": base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, Some("image/jpeg".to_string()));
}

#[test]
fn test_magic_bytes_png() {
    // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
    let mut png_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png_data.extend(vec![0u8; 2048 - 8]);
    let base64 = BASE64_STANDARD.encode(&png_data);

    let mut messages = json!({
        "bytes": base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, Some("image/png".to_string()));
}

#[test]
fn test_magic_bytes_gif() {
    // GIF magic bytes: GIF89a
    let mut gif_data = b"GIF89a".to_vec();
    gif_data.extend(vec![0u8; 2048 - 6]);
    let base64 = BASE64_STANDARD.encode(&gif_data);

    let mut messages = json!({
        "bytes": base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, Some("image/gif".to_string()));
}

#[test]
fn test_magic_bytes_pdf() {
    // PDF magic bytes: %PDF
    let mut pdf_data = b"%PDF-1.7".to_vec();
    pdf_data.extend(vec![0u8; 2048 - 8]);
    let base64 = BASE64_STANDARD.encode(&pdf_data);

    let mut messages = json!({
        "bytes": base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(
        result.files[0].media_type,
        Some("application/pdf".to_string())
    );
}

#[test]
fn test_magic_bytes_unknown() {
    // Random data with no recognizable magic bytes
    let unknown_data: Vec<u8> = (0..2048).map(|i| ((i * 0x12) % 256) as u8).collect();
    let base64 = BASE64_STANDARD.encode(&unknown_data);

    let mut messages = json!({
        "bytes": base64
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, None); // Unknown format
}

// ========================================================================
// Placeholder Detection Tests
// ========================================================================

#[test]
fn test_placeholder_variants() {
    let placeholders = vec![
        "<replaced>",
        "<binary>",
        "<truncated>",
        "<omitted>",
        "<redacted>",
        "<image>",
        "[binary]",
        "[replaced]",
        "[truncated]",
        "...",
        "\u{2026}", // Unicode ellipsis
    ];

    for placeholder in placeholders {
        let mut messages = json!({
            "data": placeholder
        });

        let result = extract_and_replace_files(&mut messages);
        assert!(!result.modified, "Should skip placeholder: {}", placeholder);
    }
}

#[test]
fn test_generic_angle_bracket_placeholder() {
    let mut messages = json!({
        "data": "<base64 data omitted for brevity>"
    });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        !result.modified,
        "Should skip generic angle bracket placeholder"
    );
}

#[test]
fn test_generic_square_bracket_placeholder() {
    let mut messages = json!({
        "data": "[image content removed]"
    });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        !result.modified,
        "Should skip generic square bracket placeholder"
    );
}

// ========================================================================
// Additional Protected Fields Tests
// ========================================================================

#[test]
fn test_all_protected_fields() {
    let protected_fields = vec![
        "text",
        "content",
        "message",
        "name",
        "description",
        "thinking",
        "reasoning",
        "title",
        "prompt",
        "system",
    ];

    for field in protected_fields {
        let mut messages = json!({
            field: make_raw_base64(2048)
        });

        let result = extract_and_replace_files(&mut messages);
        assert!(
            !result.modified,
            "Should not extract from protected field: {}",
            field
        );
    }
}

// ========================================================================
// Additional Extractable Fields Tests
// ========================================================================

#[test]
fn test_additional_extractable_fields() {
    // Test the new extractable fields we added
    let extractable_fields = vec![
        "data",
        "bytes",
        "base64",
        "b64",
        "url",
        "image_data",
        "audio_data",
        "file_data",
    ];

    for field in extractable_fields {
        let data_url = make_base64_image(2048);
        let mut messages = json!({
            field: data_url
        });

        let result = extract_and_replace_files(&mut messages);
        assert!(result.modified, "Should extract from field: {}", field);
        assert_eq!(result.files.len(), 1);
    }
}

// ========================================================================
// CRLF and Windows Line Endings Tests
// ========================================================================

#[test]
fn test_base64_with_crlf() {
    // Windows-style line endings (CRLF)
    let raw = make_raw_base64(2048);
    let with_crlf: String = raw
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 76 == 0 {
                vec!['\r', '\n', c]
            } else {
                vec![c]
            }
        })
        .collect();

    let mut messages = json!({
        "bytes": with_crlf
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract base64 with CRLF");
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_base64_with_tabs() {
    // Some formatters might use tabs
    let raw = make_raw_base64(2048);
    let with_tabs: String = raw
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 64 == 0 {
                vec!['\t', c]
            } else {
                vec![c]
            }
        })
        .collect();

    let mut messages = json!({
        "bytes": with_tabs
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified, "Should extract base64 with tabs");
    assert_eq!(result.files.len(), 1);
}

// ========================================================================
// Data URL Edge Cases
// ========================================================================

#[test]
fn test_data_url_empty_media_type() {
    // data:;base64,{data} - no media type specified
    let data = vec![0xFF, 0xD8, 0xFF, 0xE0]; // JPEG magic
    let mut jpeg_data = data.clone();
    jpeg_data.extend(vec![0u8; 2048 - 4]);
    let base64 = BASE64_STANDARD.encode(&jpeg_data);

    let data_url = format!("data:;base64,{}", base64);

    let mut messages = json!({
        "url": data_url
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
    // Should detect JPEG from magic bytes since no media type in URL
    assert_eq!(result.files[0].media_type, Some("image/jpeg".to_string()));
}

#[test]
fn test_data_url_not_base64() {
    // data:text/plain,Hello%20World - not base64 encoded, should be skipped
    let mut messages = json!({
        "url": "data:text/plain,Hello%20World"
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified, "Should skip non-base64 data URLs");
}

// ========================================================================
// Framework-Specific Tests
// ========================================================================

#[test]
fn test_openai_audio_format() {
    // OpenAI uses input_audio.data
    let mut messages = json!({
        "type": "input_audio",
        "input_audio": {
            "data": make_raw_base64(2048),
            "format": "wav"
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let data = messages["input_audio"]["data"].as_str().unwrap();
    assert!(data.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_bedrock_document_format() {
    // Bedrock uses document.source.bytes
    let mut messages = json!({
        "document": {
            "format": "pdf",
            "name": "document.pdf",
            "source": {
                "bytes": make_raw_base64(2048)
            }
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let bytes = messages["document"]["source"]["bytes"].as_str().unwrap();
    assert!(bytes.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_bedrock_video_format() {
    // Bedrock uses video.source.bytes
    let mut messages = json!({
        "video": {
            "format": "mp4",
            "source": {
                "bytes": make_raw_base64(2048)
            }
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let bytes = messages["video"]["source"]["bytes"].as_str().unwrap();
    assert!(bytes.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_anthropic_image_format() {
    // Anthropic uses source.type="base64" with source.data
    let mut messages = json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/jpeg",
            "data": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let data = messages["source"]["data"].as_str().unwrap();
    assert!(data.starts_with(FILE_URI_PREFIX));
}

// ========================================================================
// Dotted Key Tests (OTLP flat attributes)
// ========================================================================

#[test]
fn test_dotted_key_extractable_data() {
    // OI dotted attribute: llm.input_messages.0.message.contents.1.message_content.image.source.data
    let mut raw_span = json!({
        "attributes": {
            "llm.input_messages.0.message.contents.1.message_content.image.source.data": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        result.modified,
        "Should extract from dotted key ending in 'data'"
    );
    assert_eq!(result.files.len(), 1);
    let val = raw_span["attributes"]["llm.input_messages.0.message.contents.1.message_content.image.source.data"]
        .as_str().unwrap();
    assert!(val.starts_with(FILE_URI_PREFIX));
}

#[test]
fn test_dotted_key_extractable_bytes() {
    let mut raw_span = json!({
        "attributes": {
            "gen_ai.content.input.0.image.source.bytes": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        result.modified,
        "Should extract from dotted key ending in 'bytes'"
    );
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_dotted_key_extractable_url_data_url() {
    let mut raw_span = json!({
        "attributes": {
            "llm.input_messages.0.message.contents.0.message_content.image.image.url": make_base64_image(2048)
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        result.modified,
        "Should extract data URL from dotted key ending in 'url'"
    );
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_dotted_key_protected_text() {
    // Dotted key ending in protected field should NOT extract
    let mut raw_span = json!({
        "attributes": {
            "llm.input_messages.0.message.content": make_raw_base64(2048),
            "llm.input_messages.0.message.text": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        !result.modified,
        "Should not extract from dotted keys ending in protected fields"
    );
}

#[test]
fn test_dotted_key_non_extractable() {
    // Dotted key ending in non-extractable, non-protected field
    let mut raw_span = json!({
        "attributes": {
            "llm.input_messages.0.message.role": make_raw_base64(2048)
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        !result.modified,
        "Should not extract from non-extractable dotted key"
    );
}

#[test]
fn test_output_value_nested_base64_in_raw_span() {
    // Exact structure from LangGraph Prompt span's output.value:
    // raw_span.attributes["output.value"] = JSON string containing
    // LangChain messages with content[].source.data = raw base64
    let output_value = serde_json::to_string(&json!([
        {
            "content": "System message text",
            "type": "system"
        },
        {
            "content": "User message text",
            "type": "human"
        },
        {
            "content": [
                {"type": "text", "text": "generated_image.png (426.9 KB)"},
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": make_raw_base64(2048)
                    }
                }
            ],
            "type": "tool",
            "name": "read_image",
            "tool_call_id": "tooluse_abc123"
        },
        {
            "content": [
                {"type": "text", "text": "another_image.png (412.2 KB)"},
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": make_raw_base64(4096)
                    }
                }
            ],
            "type": "tool",
            "name": "read_image",
            "tool_call_id": "tooluse_def456"
        }
    ]))
    .unwrap();

    let mut raw_span = json!({
        "trace_id": "abc123",
        "attributes": {
            "output.value": output_value,
            "output.mime_type": "application/json"
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(
        result.modified,
        "Should extract base64 from output.value nested JSON"
    );
    assert_eq!(result.files.len(), 2, "Should extract 2 image files");

    // Verify the output.value was modified
    let ov_str = raw_span["attributes"]["output.value"].as_str().unwrap();
    let parsed: JsonValue = serde_json::from_str(ov_str).unwrap();
    let data1 = parsed[2]["content"][1]["source"]["data"].as_str().unwrap();
    let data2 = parsed[3]["content"][1]["source"]["data"].as_str().unwrap();
    assert!(
        data1.starts_with(FILE_URI_PREFIX),
        "First image should be replaced, got: {}",
        &data1[..40]
    );
    assert!(
        data2.starts_with(FILE_URI_PREFIX),
        "Second image should be replaced, got: {}",
        &data2[..40]
    );
}

#[test]
fn test_dedup_nested_json_still_replaces() {
    // Regression test: when dotted-key attributes have the SAME base64 as
    // nested JSON in output.value, the nested JSON must still be serialized
    // back even though no NEW files are added (dedup skips the file push).
    let image_b64 = make_raw_base64(2048);

    let output_value = serde_json::to_string(&json!([{
        "content": [{
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": image_b64.clone()
            }
        }],
        "type": "tool"
    }]))
    .unwrap();

    // Raw span has BOTH:
    // 1. Dotted-key attribute with the same base64 (processed first)
    // 2. output.value with nested JSON containing the same base64
    let mut raw_span = json!({
        "attributes": {
            "llm.input_messages.0.message.contents.0.message_content.image.source.data": image_b64,
            "output.value": output_value
        }
    });

    let result = extract_and_replace_files(&mut raw_span);

    assert!(result.modified, "Should be modified");
    // Only 1 unique file (deduplicated)
    assert_eq!(result.files.len(), 1, "Should have 1 unique file");

    // The dotted key should have a hash ref
    let dotted = raw_span["attributes"]["llm.input_messages.0.message.contents.0.message_content.image.source.data"]
        .as_str().unwrap();
    assert!(
        dotted.starts_with(FILE_URI_PREFIX),
        "Dotted key should be replaced"
    );

    // The output.value nested JSON must ALSO have the hash ref
    let ov_str = raw_span["attributes"]["output.value"].as_str().unwrap();
    let parsed: JsonValue = serde_json::from_str(ov_str).unwrap();
    let data = parsed[0]["content"][0]["source"]["data"].as_str().unwrap();
    assert!(
        data.starts_with(FILE_URI_PREFIX),
        "output.value nested base64 must also be replaced, got: {}",
        &data[..40.min(data.len())]
    );
}

#[test]
fn test_already_extracted_with_mime() {
    let mut messages = json!({
        "data": "#!B64!#image/jpeg::abc123def456"
    });
    let result = extract_and_replace_files(&mut messages);
    assert!(
        !result.modified,
        "Should skip already-extracted URIs with MIME"
    );
    assert!(result.files.is_empty());
}

// ========================================================================
// Embedded Data URL Tests
// ========================================================================

#[test]
fn test_embedded_data_url_in_python_repr() {
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend(vec![0u8; 2048 - 8]);
    let b64 = BASE64_STANDARD.encode(&png);
    let python_repr = format!(
        "[ContentBlock(content_type='image', body='data:image/png;base64,{}')]",
        b64
    );

    let mut messages = json!({
        "output": python_repr
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(
        result.modified,
        "Should extract embedded data URL from Python repr"
    );
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].media_type, Some("image/png".to_string()));

    let output = messages["output"].as_str().unwrap();
    assert!(output.contains(FILE_URI_PREFIX));
    assert!(!output.contains(";base64,"));
    assert!(
        output.starts_with("[ContentBlock"),
        "Should preserve surrounding text"
    );
}

#[test]
fn test_multiple_embedded_data_urls() {
    let data1 = vec![0u8; 2048];
    let data2 = vec![1u8; 4096];
    let b64_1 = BASE64_STANDARD.encode(&data1);
    let b64_2 = BASE64_STANDARD.encode(&data2);
    let text = format!(
        "First: data:image/png;base64,{} Second: data:image/jpeg;base64,{}",
        b64_1, b64_2
    );

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 2, "Should extract 2 different files");

    let output = messages["output"].as_str().unwrap();
    assert!(output.starts_with("First: "));
    assert!(output.contains(" Second: "));
    assert!(!output.contains(";base64,"));
}

#[test]
fn test_small_embedded_data_url_skipped() {
    let small_data = vec![0u8; 512];
    let b64 = BASE64_STANDARD.encode(&small_data);
    let text = format!("Image: data:image/png;base64,{}", b64);

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified, "Should skip small embedded data URL");
    assert!(result.files.is_empty());
}

#[test]
fn test_embedded_data_url_no_mime() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("File: data:;base64,{}", b64);

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);
}

#[test]
fn test_embedded_data_url_at_start() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("data:image/png;base64,{} and more", b64);

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);

    let output = messages["output"].as_str().unwrap();
    assert!(output.starts_with(FILE_URI_PREFIX));
    assert!(output.ends_with(" and more"));
}

#[test]
fn test_embedded_data_url_at_end() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("Before data:image/png;base64,{}", b64);

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);

    let output = messages["output"].as_str().unwrap();
    assert!(output.starts_with("Before "));
    assert!(output.contains(FILE_URI_PREFIX));
}

#[test]
fn test_false_data_prefix_without_base64_marker() {
    let mut messages = json!({
        "output": "data:text/plain,Hello World (not base64)"
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(!result.modified, "Should skip data URL without ;base64,");
}

#[test]
fn test_dedup_same_embedded_data_url_twice() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!(
        "A: data:image/png;base64,{} B: data:image/png;base64,{}",
        b64, b64
    );

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1, "Same content should be deduplicated");

    let output = messages["output"].as_str().unwrap();
    let count = output.matches(FILE_URI_PREFIX).count();
    assert_eq!(count, 2, "Both occurrences should be replaced");
}

#[test]
fn test_protected_field_with_embedded_data_url() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("Here is an image: data:image/png;base64,{}", b64);

    let mut messages = json!({
        "content": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(
        result.modified,
        "Should extract embedded data URLs even from protected fields"
    );

    let content = messages["content"].as_str().unwrap();
    assert!(content.contains(FILE_URI_PREFIX));
    assert!(!content.contains(";base64,"));
}

#[test]
fn test_valid_json_still_processed_structurally() {
    let mut messages = json!({
        "attributes": serde_json::to_string(&json!({
            "source": {
                "data": make_base64_image(2048)
            }
        })).unwrap()
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(result.modified);
    assert_eq!(result.files.len(), 1);

    let attrs = messages["attributes"].as_str().unwrap();
    let inner: JsonValue = serde_json::from_str(attrs).unwrap();
    assert!(
        inner["source"]["data"]
            .as_str()
            .unwrap()
            .starts_with(FILE_URI_PREFIX)
    );
}

#[test]
fn test_metadata_prefix_not_matched_as_data_url() {
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("metadata:image/png;base64,{}", b64);

    let mut messages = json!({
        "output": text
    });

    let result = extract_and_replace_files(&mut messages);

    assert!(
        !result.modified,
        "Should not match 'metadata:' as a data URL"
    );
}

// ========================================================================
// MIME Embedding in URI Tests
// ========================================================================

#[test]
fn test_data_url_embeds_mime_in_uri() {
    let mut messages = json!({
        "source": {
            "data": make_base64_image(2048)
        }
    });
    let result = extract_and_replace_files(&mut messages);
    assert!(result.modified);
    let data = messages["source"]["data"].as_str().unwrap();
    // data URL was data:image/png;base64,... so URI should embed image/png
    assert!(
        data.starts_with("#!B64!#image/png::"),
        "URI should embed MIME type, got: {}",
        data
    );
}

#[test]
fn test_raw_base64_no_mime_in_uri() {
    let mut messages = json!({
        "bytes": make_raw_base64(2048)
    });
    let result = extract_and_replace_files(&mut messages);
    assert!(result.modified);
    let data = messages["bytes"].as_str().unwrap();
    // Raw base64 of all zeros has no recognizable magic bytes
    assert!(
        data.starts_with("#!B64!#::"),
        "URI should have no MIME for unknown data, got: {}",
        data
    );
}

#[test]
fn test_raw_base64_jpeg_magic_embeds_mime() {
    let mut jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg_data.extend(vec![0u8; 2048 - 4]);
    let base64 = BASE64_STANDARD.encode(&jpeg_data);
    let mut messages = json!({
        "bytes": base64
    });
    let result = extract_and_replace_files(&mut messages);
    assert!(result.modified);
    let data = messages["bytes"].as_str().unwrap();
    assert!(
        data.starts_with("#!B64!#image/jpeg::"),
        "URI should embed image/jpeg from magic bytes, got: {}",
        data
    );
}

// ========================================================================
// Regression Tests: Lazy Decode + Size Estimation
// ========================================================================

#[test]
fn test_size_estimation_matches_actual_decode() {
    // estimate_decoded_size must match actual decoded size for standard padded base64.
    // The new code uses estimation instead of full decode, so any discrepancy
    // would cause files to be incorrectly included/excluded at size boundaries.
    for size in [1024, 1025, 2048, 4096, 1024 * 1024] {
        let data = vec![0u8; size];
        let b64 = BASE64_STANDARD.encode(&data);
        let estimated = estimate_decoded_size(b64.as_bytes());
        assert_eq!(
            estimated, size,
            "Size estimate mismatch for {size} bytes: estimated={estimated}"
        );
    }
}

#[test]
fn test_size_estimation_unpadded_base64() {
    // URL-safe base64 without padding — estimate must be close to actual
    for size in [1024, 1025, 2048, 4096] {
        let data = vec![0u8; size];
        let b64 = BASE64_URL_SAFE.encode(&data);
        // URL_SAFE includes padding by default
        let estimated = estimate_decoded_size(b64.as_bytes());
        assert_eq!(
            estimated, size,
            "URL-safe size estimate mismatch for {size} bytes"
        );
    }
}

#[test]
fn test_exact_min_size_boundary() {
    // File whose decoded size is exactly FILES_MIN_SIZE_BYTES.
    // Use data URL format to bypass the raw base64 minimum string length guard (1400 chars).
    let data = vec![0u8; FILES_MIN_SIZE_BYTES];
    let b64 = BASE64_STANDARD.encode(&data);
    let data_url = format!("data:image/png;base64,{}", b64);
    let mut messages = json!({ "text_with_url": data_url });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        result.modified,
        "File at exact min size boundary should be extracted"
    );
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].size, FILES_MIN_SIZE_BYTES);
}

#[test]
fn test_one_byte_below_min_size() {
    // Use data URL to test size boundary (raw base64 has a separate string-length guard)
    let data = vec![0u8; FILES_MIN_SIZE_BYTES - 1];
    let b64 = BASE64_STANDARD.encode(&data);
    let data_url = format!("data:image/png;base64,{}", b64);
    let mut messages = json!({ "text_with_url": data_url });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        !result.modified,
        "File one byte below min should not be extracted"
    );
}

#[test]
fn test_exact_max_size_boundary() {
    // File at exactly FILES_MAX_SIZE_BYTES must be extracted.
    let data = vec![0u8; FILES_MAX_SIZE_BYTES];
    let b64 = BASE64_STANDARD.encode(&data);
    let mut messages = json!({ "bytes": b64 });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        result.modified,
        "File at exact max size boundary should be extracted"
    );
    assert_eq!(result.files[0].size, FILES_MAX_SIZE_BYTES);
}

#[test]
fn test_one_byte_above_max_size() {
    let data = vec![0u8; FILES_MAX_SIZE_BYTES + 1];
    let b64 = BASE64_STANDARD.encode(&data);
    let mut messages = json!({ "bytes": b64 });

    let result = extract_and_replace_files(&mut messages);
    assert!(
        !result.modified,
        "File one byte above max should not be extracted"
    );
}

// ========================================================================
// Regression Tests: extract_or_cache Helper
// ========================================================================

#[test]
fn test_cached_extraction_produces_same_uri() {
    // First extraction (no cache) and second (with cache) must produce the
    // same URI. Regression: if extract_or_cache changes hash computation,
    // the URI would differ.
    let cache = FileExtractionCache::new();
    let b64 = make_raw_base64(2048);

    let mut msg1 = json!({ "bytes": b64.clone() });
    let result1 = extract_and_replace_files_cached(&mut msg1, &cache);
    let uri1 = msg1["bytes"].as_str().unwrap().to_string();

    let mut msg2 = json!({ "bytes": b64 });
    let result2 = extract_and_replace_files_cached(&mut msg2, &cache);
    let uri2 = msg2["bytes"].as_str().unwrap().to_string();

    assert_eq!(uri1, uri2, "Cached and uncached URIs must match");
    assert_eq!(result1.files.len(), 1, "First should extract file");
    // Cache hit: file still appears but with empty data
    assert_eq!(result2.files.len(), 1, "Cache hit should still record file");
    assert!(
        result2.files[0].data.is_empty(),
        "Cache hit file should have empty data"
    );
}

#[test]
fn test_cache_hit_file_has_correct_metadata() {
    // Cache hit must preserve hash, media_type, and size from original extraction
    let cache = FileExtractionCache::new();

    let mut jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg_data.extend(vec![0u8; 2048 - 4]);
    let b64 = BASE64_STANDARD.encode(&jpeg_data);

    let mut msg1 = json!({ "bytes": b64.clone() });
    let r1 = extract_and_replace_files_cached(&mut msg1, &cache);

    let mut msg2 = json!({ "bytes": b64 });
    let r2 = extract_and_replace_files_cached(&mut msg2, &cache);

    assert_eq!(r1.files[0].hash, r2.files[0].hash, "Hash must match");
    assert_eq!(
        r1.files[0].media_type, r2.files[0].media_type,
        "Media type must match"
    );
    assert_eq!(r1.files[0].size, r2.files[0].size, "Size must match");
}

#[test]
fn test_cache_different_content_different_hash() {
    // Two different base64 strings must produce different hashes even with cache
    let cache = FileExtractionCache::new();
    let b64_a = make_raw_base64(2048);
    let b64_b = make_raw_base64(4096);

    let mut msg_a = json!({ "bytes": b64_a });
    let mut msg_b = json!({ "bytes": b64_b });
    let ra = extract_and_replace_files_cached(&mut msg_a, &cache);
    let rb = extract_and_replace_files_cached(&mut msg_b, &cache);

    assert_ne!(ra.files[0].hash, rb.files[0].hash);
}

// ========================================================================
// Regression Tests: Embedded Data URL + Cache Interaction
// ========================================================================

#[test]
fn test_embedded_data_url_cached_produces_same_uri() {
    let cache = FileExtractionCache::new();
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("Image: data:image/png;base64,{}", b64);

    let mut msg1 = json!({ "output": text.clone() });
    extract_and_replace_files_cached(&mut msg1, &cache);
    let output1 = msg1["output"].as_str().unwrap().to_string();

    let mut msg2 = json!({ "output": text });
    extract_and_replace_files_cached(&mut msg2, &cache);
    let output2 = msg2["output"].as_str().unwrap().to_string();

    assert_eq!(
        output1, output2,
        "Cached embedded URL must produce same output"
    );
}

// ========================================================================
// Regression Tests: Hash Stability (BLAKE3 of base64 bytes)
// ========================================================================

#[test]
fn test_hash_is_64_hex_chars() {
    // BLAKE3 produces 256-bit (32 byte) hash = 64 hex chars
    let b64 = make_raw_base64(2048);
    let mut messages = json!({ "bytes": b64 });
    let result = extract_and_replace_files(&mut messages);
    assert_eq!(
        result.files[0].hash.len(),
        64,
        "BLAKE3 hash should be 64 hex chars"
    );
    assert!(
        result.files[0].hash.chars().all(|c| c.is_ascii_hexdigit()),
        "Hash should be hex"
    );
}

#[test]
fn test_hash_deterministic_across_calls() {
    // Same content must always produce same hash (no randomness)
    let b64 = make_raw_base64(2048);

    let mut msg1 = json!({ "bytes": b64.clone() });
    let r1 = extract_and_replace_files(&mut msg1);

    let mut msg2 = json!({ "bytes": b64 });
    let r2 = extract_and_replace_files(&mut msg2);

    assert_eq!(r1.files[0].hash, r2.files[0].hash);
}

#[test]
fn test_same_binary_different_base64_encoding_gets_different_hash() {
    // New code hashes the base64 bytes, not decoded bytes.
    // Standard and URL-safe encodings of the same binary will differ.
    // This is a known trade-off for lazy decode.
    let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    let standard = BASE64_STANDARD.encode(&data);
    let url_safe = BASE64_URL_SAFE.encode(&data);

    // Only test if encodings actually differ
    if standard != url_safe {
        let mut msg1 = json!({ "bytes": standard });
        let mut msg2 = json!({ "bytes": url_safe });
        let r1 = extract_and_replace_files(&mut msg1);
        let r2 = extract_and_replace_files(&mut msg2);

        // Document: different encodings = different hashes (trade-off)
        assert_ne!(
            r1.files[0].hash, r2.files[0].hash,
            "Different base64 encodings produce different hashes (expected)"
        );
    }
}

// ========================================================================
// Regression Tests: Data Format Consistency
// ========================================================================

#[test]
fn test_extracted_file_data_is_raw_base64_bytes() {
    // New code stores raw base64 bytes (not decoded), unlike old code.
    // Verify ExtractedFile.data is the base64 representation.
    let binary = vec![0xFFu8; 2048];
    let b64 = BASE64_STANDARD.encode(&binary);

    let mut messages = json!({ "bytes": b64.clone() });
    let result = extract_and_replace_files(&mut messages);

    assert!(!result.files[0].data.is_empty());
    // Data should be base64 bytes (ASCII), not raw binary
    let data_str = std::str::from_utf8(&result.files[0].data).expect("data should be valid UTF-8");
    assert_eq!(data_str, b64, "File data should be raw base64 string bytes");
}

#[test]
fn test_data_url_extracted_data_is_base64_bytes() {
    // Data URL extraction should also store base64 bytes, not decoded
    let binary = vec![0xAAu8; 2048];
    let b64 = BASE64_STANDARD.encode(&binary);
    let data_url = format!("data:image/png;base64,{}", b64);

    let mut messages = json!({ "url": data_url });
    let result = extract_and_replace_files(&mut messages);

    let data_str = std::str::from_utf8(&result.files[0].data).expect("data should be valid UTF-8");
    assert_eq!(data_str, b64, "Data URL file data should be base64 bytes");
}

// ========================================================================
// Regression Tests: Empty and Null Inputs
// ========================================================================

#[test]
fn test_extract_from_null_json() {
    let mut messages = JsonValue::Null;
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_extract_from_empty_object() {
    let mut messages = json!({});
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_extract_from_empty_array() {
    let mut messages = json!([]);
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
    assert!(result.files.is_empty());
}

#[test]
fn test_extract_from_boolean() {
    let mut messages = json!(true);
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
}

#[test]
fn test_extract_from_number() {
    let mut messages = json!(42);
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
}

// ========================================================================
// Regression Tests: record_extracted_file + extract_or_cache
// ========================================================================

#[test]
fn test_extract_or_cache_returns_none_for_non_base64() {
    // String that is not base64 should return None from extract_or_cache
    let mut messages = json!({ "bytes": "this is not base64 at all!" });
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
}

#[test]
fn test_extract_or_cache_returns_none_for_small_base64() {
    // Valid base64 below min size should not be extracted
    let small = BASE64_STANDARD.encode(vec![0u8; 100]);
    let mut messages = json!({ "bytes": small });
    let result = extract_and_replace_files(&mut messages);
    assert!(!result.modified);
}

#[test]
fn test_dedup_across_field_and_embedded() {
    // Same base64 appears as both a raw field value and embedded in a data URL.
    // Both paths hash the base64 bytes (not the full data URL string), so
    // they produce the same BLAKE3 hash and are deduped to a single file.
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let data_url = format!("data:image/png;base64,{}", b64);

    let mut messages = json!({
        "bytes": b64,
        "text_with_url": data_url,
    });

    let result = extract_and_replace_files(&mut messages);
    assert!(result.modified);
    // Same base64 bytes → same BLAKE3 hash → deduped to 1 file
    assert_eq!(result.files.len(), 1);
}

// ========================================================================
// Regression Tests: Whitespace Normalization
// ========================================================================

#[test]
fn test_whitespace_normalization_preserves_hash() {
    // Base64 with and without whitespace should produce the same hash
    // (whitespace is stripped before hashing)
    let raw = make_raw_base64(2048);
    let with_ws: String = raw
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 76 == 0 {
                vec!['\n', c]
            } else {
                vec![c]
            }
        })
        .collect();

    let mut msg_clean = json!({ "bytes": raw });
    let mut msg_ws = json!({ "bytes": with_ws });

    let r_clean = extract_and_replace_files(&mut msg_clean);
    let r_ws = extract_and_replace_files(&mut msg_ws);

    assert_eq!(
        r_clean.files[0].hash, r_ws.files[0].hash,
        "Whitespace-stripped base64 should hash identically"
    );
}
