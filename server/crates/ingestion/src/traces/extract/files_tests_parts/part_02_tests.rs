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
                "data": image_b64
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

    let mut msg1 = json!({ "bytes": b64 });
    let result1 = extract_and_replace_files_cached(&mut msg1, &cache);
    let uri1 = msg1["bytes"].as_str().unwrap().to_string();

    let mut msg2 = json!({ "bytes": b64 });
    let result2 = extract_and_replace_files_cached(&mut msg2, &cache);
    let uri2 = msg2["bytes"].as_str().unwrap().to_string();

    assert_eq!(uri1, uri2, "Cached and uncached URIs must match");
    assert_eq!(result1.files.len(), 1, "First should extract file");
    assert_eq!(result2.files.len(), 1, "Cache hit should still record file");
    // A hit carries the bytes, and this assertion used to require the opposite. Empty data was read
    // downstream as "already in storage", which the cache cannot know: it is global and keyed on the
    // source text, so a hit in one project says only that some project once hashed these bytes. What
    // the cache saves is the charset validation and the BLAKE3, not the copy.
    assert_eq!(
        result2.files[0].data, result1.files[0].data,
        "a cache hit must carry the same bytes as the extraction it stands in for"
    );
    assert!(
        !result2.files[0].data.is_empty(),
        "a cache hit must not claim the bytes are already stored"
    );
}

#[test]
fn test_cache_hit_file_has_correct_metadata() {
    // Cache hit must preserve hash, media_type, and size from original extraction
    let cache = FileExtractionCache::new();

    let mut jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg_data.extend(vec![0u8; 2048 - 4]);
    let b64 = BASE64_STANDARD.encode(&jpeg_data);

    let mut msg1 = json!({ "bytes": b64 });
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

/// A reference embedded in prose is found whatever punctuation follows it.
///
/// Splitting on whitespace missed these: `...::abc123.` parsed as a hash of `abc123.` and
/// `...::abc123",` did not parse at all - so a forged reference could hide behind a full stop.
#[test]
fn an_embedded_reference_is_found_next_to_punctuation() {
    let uri = sideseat_core::utils::file_uri::build_file_uri("abc123", Some("image/png"));
    for (text, why) in [
        (format!("see {uri}"), "plain"),
        (format!("see {uri}."), "sentence end"),
        (format!("see {uri}, and more"), "comma"),
        (format!("{{\"u\":\"{uri}\"}}"), "quoted inside text"),
        (format!("{uri} {uri}"), "twice"),
    ] {
        let mut found = Vec::new();
        collect_file_references(&json!({ "text": text.clone() }), &mut found);
        assert!(
            found.contains(&uri),
            "{why}: expected to find {uri} in {text:?}, found {found:?}"
        );
    }

    // And twice really is twice, so a repeated reference is reconciled for each occurrence.
    let mut twice = Vec::new();
    collect_file_references(&json!({ "text": format!("{uri} {uri}") }), &mut twice);
    assert_eq!(twice.len(), 2, "both occurrences are reported: {twice:?}");
}

/// A reference that arrives already formed is *found*, not silently trusted.
///
/// Extraction skips `#!B64!#` strings, which is right - they are already references - but nothing
/// reported them, so a client could send one for content it never uploaded and the row would be
/// committed pointing at nothing. `collect_file_references` is what lets the caller subtract the
/// references extraction just created and reconcile the rest against storage.
#[test]
fn an_incoming_reference_is_collected_and_ours_are_distinguishable() {
    let forged =
        sideseat_core::utils::file_uri::build_file_uri("never-uploaded", Some("image/png"));
    let data = vec![3u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);

    let mut msg = json!({
        "forged": forged.clone(),
        "bytes": b64,
        "nested": [{ "also_forged": forged.clone() }],
    });
    let result = extract_and_replace_files(&mut msg);

    let mut found = Vec::new();
    collect_file_references(&msg, &mut found);

    assert_eq!(result.files.len(), 1, "the real payload was extracted");
    let ours = sideseat_core::utils::file_uri::build_file_uri(
        &result.files[0].hash,
        result.files[0].media_type.as_deref(),
    );
    assert!(
        found.contains(&ours),
        "the reference extraction created is present: {found:?}"
    );
    assert_eq!(
        found.iter().filter(|u| **u == forged).count(),
        2,
        "both forged references are found, including the nested one: {found:?}"
    );
}

/// A cache hit on a `data:` URL must carry the *payload* bytes, not the URL.
///
/// The payload starts after `data:image/png;base64,`. Reconstructing the bytes from the source string
/// handed the whole URL to persistence, which base64-decodes it - so every cached data-URL file failed
/// to store while its reference was committed. The existing cache test compared only the generated
/// URIs, which are identical either way, so it could not see this.
#[test]
fn cached_data_url_carries_the_payload_not_the_url() {
    let cache = FileExtractionCache::new();
    let data = vec![7u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("Image: data:image/png;base64,{}", b64);

    let mut first = json!({ "output": text.clone() });
    let miss = extract_and_replace_files_cached(&mut first, &cache);
    let mut second = json!({ "output": text });
    let hit = extract_and_replace_files_cached(&mut second, &cache);

    assert_eq!(miss.files.len(), 1);
    assert_eq!(hit.files.len(), 1, "a cache hit still records the file");
    assert_eq!(
        hit.files[0].data, miss.files[0].data,
        "a hit must carry the same payload bytes the extraction produced"
    );
    assert_eq!(
        BASE64_STANDARD.decode(&hit.files[0].data).unwrap(),
        data,
        "the cached payload must decode back to the original bytes"
    );
}
