use super::*;
use base64::prelude::*;
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
            { "data": base64_data },
            { "data": base64_data },
            { "data": base64_data }
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
        "bytes": base64
    }))
    .unwrap();

    let mut messages = json!({
        "image1": { "bytes": base64 },
        "image2": { "bytes": base64 },
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
    let mut jpeg_data = data;
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
