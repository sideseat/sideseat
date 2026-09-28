
#[test]
fn test_embedded_data_url_cached_produces_same_uri() {
    let cache = FileExtractionCache::new();
    let data = vec![0u8; 2048];
    let b64 = BASE64_STANDARD.encode(&data);
    let text = format!("Image: data:image/png;base64,{}", b64);

    let mut msg1 = json!({ "output": text });
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

    let mut msg1 = json!({ "bytes": b64 });
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

    let mut messages = json!({ "bytes": b64 });
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
