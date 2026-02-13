//! File extraction from raw messages
//!
//! Recursively scans JSON for base64 data and extracts files >= 1KB.
//! Replaces extracted data with `#!B64!#[mime]::hash` URIs.
//!
//! ## Supported Formats
//!
//! This module handles base64 extraction from ALL major AI provider formats:
//!
//! | Provider | Field Path | Example |
//! |----------|------------|---------|
//! | OpenAI | `image_url.url` | `data:image/jpeg;base64,...` |
//! | OpenAI Audio | `input_audio.data` | Raw base64 |
//! | Anthropic | `source.data` | Raw base64 with media_type |
//! | Bedrock | `source.bytes` | Raw base64 |
//! | Gemini | `inline_data.data` | Raw base64 with mime_type |
//! | Custom | `data`, `bytes`, `base64` | Various |
//!
//! ## Detection Strategy
//!
//! 1. Scan all strings for embedded `data:mime;base64,...` markers (field-independent)
//! 2. For known extractable fields only:
//!    a. Parse standalone data URLs (`data:mime;base64,...`)
//!    b. Detect raw base64 by charset validation + decode attempt
//!    c. Detect media type from magic bytes when not explicit

use base64::prelude::*;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::core::constants::{FILES_MAX_SIZE_BYTES, FILES_MIN_SIZE_BYTES};
use crate::utils::mime::detect_mime_type;

/// Fields that may contain base64 data to extract.
///
/// These are field names used by various AI providers to store binary content:
/// - `data`: Anthropic, Gemini, OpenAI audio, generic
/// - `bytes`: AWS Bedrock Converse (image, document, video)
/// - `base64`: Legacy/custom formats
/// - `b64`: Shorthand used in some frameworks
/// - `url`: OpenAI image_url (for `data:` URLs only)
/// - `image_data`: Some custom implementations
/// - `audio_data`: Some audio-specific implementations
/// - `file_data`: Some file upload implementations
const EXTRACTABLE_FIELDS: &[&str] = &[
    "data",
    "bytes",
    "base64",
    "b64",
    "url",
    "image_url",
    "image_data",
    "audio_data",
    "file_data",
];

/// Fields that should never be modified (contain user text).
///
/// Even if these fields contain valid base64 strings, they should not be
/// extracted because they represent user-generated content, not binary files.
const PROTECTED_FIELDS: &[&str] = &[
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

/// Check if a key (or its last dotted segment) is an extractable field.
/// Handles OTLP dotted attributes like `llm.input_messages.0.message.contents.1.message_content.image.source.data`
/// where the last segment `data` is extractable.
fn is_extractable_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    EXTRACTABLE_FIELDS.contains(&leaf)
}

/// Check if a key (or its last dotted segment) is a protected field.
fn is_protected_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    PROTECTED_FIELDS.contains(&leaf)
}

/// URI prefix for file references.
///
/// Format: `#!B64!#[mime/type]::hash`
/// - With MIME:    `#!B64!#image/png::abc123`
/// - Without MIME: `#!B64!#::abc123`
pub const FILE_URI_PREFIX: &str = "#!B64!#";

/// An extracted file ready for storage
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    /// SHA-256 hash of the file content (64 hex chars)
    pub hash: String,
    /// Raw file bytes
    pub data: Vec<u8>,
    /// Media type (e.g., "image/jpeg")
    pub media_type: Option<String>,
    /// Size in bytes
    pub size: usize,
}

/// Result of file extraction
#[derive(Debug, Default)]
pub struct ExtractionResult {
    /// Files extracted from the messages
    pub files: Vec<ExtractedFile>,
    /// Whether any modifications were made to the JSON
    pub modified: bool,
}

/// Extract and replace base64 data in raw messages.
///
/// Recursively scans JSON for base64 data >= 1KB in extractable fields.
/// Replaces found data with `#!B64!#[mime]::hash` URIs.
/// Returns extracted files and indicates if messages were modified.
///
/// # Arguments
/// * `messages` - Raw messages JSON (will be modified in place)
///
/// # Returns
/// * `ExtractionResult` with extracted files and modification flag
pub fn extract_and_replace_files(messages: &mut JsonValue) -> ExtractionResult {
    let mut result = ExtractionResult::default();
    let mut seen_hashes = std::collections::HashSet::new();

    result.modified = extract_recursive(messages, None, &mut result.files, &mut seen_hashes);

    result
}

/// Recursively scan and extract base64 from JSON.
/// Returns true if the JSON was modified (any replacements made).
fn extract_recursive(
    json: &mut JsonValue,
    parent_key: Option<&str>,
    files: &mut Vec<ExtractedFile>,
    seen_hashes: &mut std::collections::HashSet<String>,
) -> bool {
    match json {
        JsonValue::String(s) => {
            // Handle nested JSON strings FIRST (before protected field check)
            // Nested JSON may contain extractable fields even inside protected keys
            // e.g., events[].attributes.content = "[{\"image\": {\"bytes\": \"...\"}}]"
            if s.starts_with('{') || s.starts_with('[') {
                if let Ok(mut nested) = serde_json::from_str::<JsonValue>(s) {
                    let modified = extract_recursive(&mut nested, None, files, seen_hashes);
                    if modified {
                        match serde_json::to_string(&nested) {
                            Ok(new_s) => *s = new_s,
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to re-serialize modified nested JSON")
                            }
                        }
                    }
                    return modified;
                }
            }

            // Scan for embedded data URLs in any string, regardless of field name.
            // Data URLs are self-describing (data:mime;base64,...) with zero false-positive risk.
            if let Some(modified_str) = extract_embedded_data_urls(s, files, seen_hashes) {
                *s = modified_str;
                return true;
            }

            // Check protected fields - don't extract base64 from user text fields
            if let Some(key) = parent_key {
                if is_protected_key(key) {
                    return false;
                }
            }

            // Only extract if parent key is extractable
            if let Some(key) = parent_key {
                if !is_extractable_key(key) {
                    return false;
                }
            } else {
                // No parent key context, skip
                return false;
            }

            // Try to extract base64 data
            if let Some(extracted) = try_extract_base64(s) {
                if extracted.size >= FILES_MIN_SIZE_BYTES {
                    let hash = sha256_bytes(&extracted.data);

                    // Build replacement URI before moving media_type
                    let uri = match &extracted.media_type {
                        Some(mt) => format!("{FILE_URI_PREFIX}{mt}::{hash}"),
                        None => format!("{FILE_URI_PREFIX}::{hash}"),
                    };

                    // Deduplicate within same extraction
                    if !seen_hashes.contains(&hash) {
                        seen_hashes.insert(hash.clone());
                        files.push(ExtractedFile {
                            hash,
                            data: extracted.data,
                            media_type: extracted.media_type,
                            size: extracted.size,
                        });
                    }

                    *s = uri;
                    return true;
                }
            }
            false
        }
        JsonValue::Array(arr) => {
            let mut modified = false;
            for item in arr.iter_mut() {
                modified |= extract_recursive(item, None, files, seen_hashes);
            }
            modified
        }
        JsonValue::Object(obj) => {
            let mut modified = false;
            for (key, value) in obj.iter_mut() {
                modified |= extract_recursive(value, Some(key), files, seen_hashes);
            }
            modified
        }
        _ => false,
    }
}

/// Result of base64 extraction attempt
struct ExtractedData {
    data: Vec<u8>,
    media_type: Option<String>,
    size: usize,
}

/// Try to extract base64 data from a string.
///
/// Detection priority:
/// 1. Skip URLs, already-extracted URIs, and placeholders
/// 2. Parse data URLs (most reliable - contains mime type)
/// 3. Detect raw base64 by charset + decode + magic bytes
fn try_extract_base64(s: &str) -> Option<ExtractedData> {
    // Skip URLs (but not data URLs)
    if s.starts_with("http://") || s.starts_with("https://") {
        return None;
    }

    // Skip #!B64!# URIs (already extracted)
    if is_file_uri(s) {
        return None;
    }

    // Skip placeholder values - various formats used by frameworks
    if is_placeholder_value(s) {
        return None;
    }

    // Handle data URL format: data:{media_type};base64,{data}
    if s.starts_with("data:") {
        return parse_data_url(s);
    }

    // For raw base64, require minimum length (1KB decoded ~= 1.37KB encoded)
    if s.len() < 1400 {
        return None;
    }

    // Validate base64 charset before expensive decode
    if !is_valid_base64_charset(s) {
        return None;
    }

    // Clean whitespace (some encoders add line breaks)
    let cleaned: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    // Try to decode
    let data = decode_base64_flexible(&cleaned)?;

    // Check size limits
    if data.len() < FILES_MIN_SIZE_BYTES {
        return None;
    }
    if data.len() > FILES_MAX_SIZE_BYTES {
        tracing::warn!(
            size = data.len(),
            max = FILES_MAX_SIZE_BYTES,
            "Skipping file extraction: exceeds max size"
        );
        return None;
    }

    // Detect media type from magic bytes (for raw base64 without explicit type)
    let media_type = detect_mime_type(&data).map(String::from);

    Some(ExtractedData {
        size: data.len(),
        data,
        media_type,
    })
}

/// Check if value is a placeholder (not actual content).
///
/// Frameworks often replace binary content with placeholder strings during
/// logging or when content is too large to include.
fn is_placeholder_value(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }

    // Common explicit placeholders
    const PLACEHOLDERS: &[&str] = &[
        "<replaced>",
        "<binary>",
        "<truncated>",
        "<omitted>",
        "<redacted>",
        "<image>",
        "<audio>",
        "<video>",
        "<file>",
        "[binary]",
        "[replaced]",
        "[truncated]",
        "[omitted]",
        "[redacted]",
        "[image]",
        "[audio]",
        "[video]",
        "[file]",
        "...",
        "…",
    ];

    let trimmed = s.trim();
    if PLACEHOLDERS.contains(&trimmed) {
        return true;
    }

    // Generic angle bracket placeholders like <...>, <base64 data>, etc.
    if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() < 50 {
        return true;
    }

    // Generic square bracket placeholders
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() < 50 {
        return true;
    }

    false
}

/// Parse a data URL and extract the base64 content.
///
/// Format: `data:[<mediatype>][;base64],<data>`
///
/// Examples:
/// - `data:image/png;base64,iVBORw0KGgo...`
/// - `data:;base64,SGVsbG8=` (no media type)
/// - `data:text/plain,Hello%20World` (not base64 - returns None)
fn parse_data_url(url: &str) -> Option<ExtractedData> {
    let without_prefix = url.strip_prefix("data:")?;

    // Find the base64 marker - only extract base64-encoded data
    let base64_marker = ";base64,";
    let base64_pos = without_prefix.find(base64_marker)?;

    let media_type = &without_prefix[..base64_pos];
    let base64_data = &without_prefix[base64_pos + base64_marker.len()..];

    // Strip whitespace and decode (some encoders add newlines in data URLs)
    let cleaned: String = base64_data
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let data = decode_base64_flexible(&cleaned)?;

    // Check size limits
    if data.len() < FILES_MIN_SIZE_BYTES {
        return None;
    }
    if data.len() > FILES_MAX_SIZE_BYTES {
        tracing::warn!(
            size = data.len(),
            max = FILES_MAX_SIZE_BYTES,
            "Skipping data URL extraction: exceeds max size"
        );
        return None;
    }

    // Determine media type: explicit from URL, or detect from magic bytes
    let detected_type = if media_type.is_empty() {
        detect_mime_type(&data).map(String::from)
    } else {
        Some(media_type.to_string())
    };

    Some(ExtractedData {
        size: data.len(),
        data,
        media_type: detected_type,
    })
}

/// Decode base64 data, trying both standard and URL-safe alphabets
fn decode_base64_flexible(s: &str) -> Option<Vec<u8>> {
    // Try standard base64 first (most common)
    if let Ok(data) = BASE64_STANDARD.decode(s) {
        return Some(data);
    }
    // Try URL-safe base64
    if let Ok(data) = BASE64_URL_SAFE.decode(s) {
        return Some(data);
    }
    // Try URL-safe without padding
    BASE64_URL_SAFE_NO_PAD.decode(s).ok()
}

/// Check if string contains only valid base64 characters.
/// Allows standard base64 (+/), URL-safe base64 (-_), padding (=), and whitespace.
/// Uses byte iteration since all valid base64 characters are ASCII.
fn is_valid_base64_charset(s: &str) -> bool {
    s.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || b == b'+'
            || b == b'/'
            || b == b'-' // URL-safe variant
            || b == b'_' // URL-safe variant
            || b == b'='
            || b.is_ascii_whitespace() // Some encoders add newlines
    })
}

/// Calculate SHA-256 hash of bytes and return as hex string
fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Check if a string is a valid MIME type (e.g., "image/png", "application/octet-stream").
/// Requires exactly one `/` and only valid MIME characters.
fn is_valid_mime_type(s: &str) -> bool {
    let slash_count = s.bytes().filter(|&b| b == b'/').count();
    if slash_count != 1 {
        return false;
    }
    s.bytes().all(|b| {
        b.is_ascii_alphanumeric() || b == b'/' || b == b'.' || b == b'-' || b == b'+' || b == b'_'
    })
}

/// Find the end of base64 data starting at `start` in `s`.
/// For embedded data URLs, whitespace terminates the base64 (unlike standalone base64
/// where whitespace is stripped). Returns the index one past the last base64 character.
fn find_base64_end(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric()
            || b == b'+'
            || b == b'/'
            || b == b'-'
            || b == b'_'
            || b == b'='
        {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// Scan a string for embedded `data:mime;base64,...` patterns and extract them.
///
/// Unlike field-based extraction, this works on ANY string regardless of field name,
/// because data URLs are self-describing markers with zero false-positive risk.
/// Returns `Some(modified_string)` if any data URLs were extracted, `None` otherwise.
fn extract_embedded_data_urls(
    s: &str,
    files: &mut Vec<ExtractedFile>,
    seen_hashes: &mut std::collections::HashSet<String>,
) -> Option<String> {
    if !s.contains(";base64,") || !s.contains("data:") {
        return None;
    }

    let mut result = String::with_capacity(s.len());
    let mut modified = false;
    let mut pos = 0;

    while pos < s.len() {
        match s[pos..].find("data:") {
            Some(offset) => {
                let data_start = pos + offset;

                // Ensure "data:" is not part of a longer word (e.g., "metadata:")
                if data_start > 0 {
                    let prev = s.as_bytes()[data_start - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        result.push_str(&s[pos..data_start + 5]);
                        pos = data_start + 5;
                        continue;
                    }
                }

                let after_prefix = data_start + 5;

                let extracted = s[after_prefix..]
                    .find(";base64,")
                    .and_then(|marker_offset| {
                        let mime = &s[after_prefix..after_prefix + marker_offset];
                        if !mime.is_empty() && !is_valid_mime_type(mime) {
                            return None;
                        }
                        let b64_start = after_prefix + marker_offset + 8;
                        let b64_end = find_base64_end(s, b64_start);
                        let data_url = &s[data_start..b64_end];
                        parse_data_url(data_url).map(|ext| (ext, b64_end))
                    });

                if let Some((extracted, end_pos)) = extracted {
                    let hash = sha256_bytes(&extracted.data);
                    let uri = match &extracted.media_type {
                        Some(mt) => format!("{FILE_URI_PREFIX}{mt}::{hash}"),
                        None => format!("{FILE_URI_PREFIX}::{hash}"),
                    };

                    result.push_str(&s[pos..data_start]);
                    result.push_str(&uri);

                    if !seen_hashes.contains(&hash) {
                        seen_hashes.insert(hash.clone());
                        files.push(ExtractedFile {
                            hash,
                            data: extracted.data,
                            media_type: extracted.media_type,
                            size: extracted.size,
                        });
                    }

                    modified = true;
                    pos = end_pos;
                } else {
                    result.push_str(&s[pos..after_prefix]);
                    pos = after_prefix;
                }
            }
            None => {
                result.push_str(&s[pos..]);
                break;
            }
        }
    }

    if modified { Some(result) } else { None }
}

/// Parsed components of a `#!B64!#` file URI.
#[derive(Debug, Clone, PartialEq)]
pub struct FileUri<'a> {
    pub hash: &'a str,
    pub media_type: Option<&'a str>,
}

/// Parse a sideseat file URI into its components.
///
/// Accepts both formats:
/// - `#!B64!#image/png::abc123` → hash="abc123", media_type=Some("image/png")
/// - `#!B64!#::abc123`          → hash="abc123", media_type=None
pub fn parse_file_uri(uri: &str) -> Option<FileUri<'_>> {
    let rest = uri.strip_prefix(FILE_URI_PREFIX)?;
    let sep = rest.find("::")?;
    let mime_part = &rest[..sep];
    let hash = &rest[sep + 2..];
    if hash.is_empty() {
        return None;
    }
    let media_type = if mime_part.is_empty() {
        None
    } else {
        Some(mime_part)
    };
    Some(FileUri { hash, media_type })
}

/// Check if a string is a sideseat file URI.
pub fn is_file_uri(s: &str) -> bool {
    s.starts_with(FILE_URI_PREFIX) && s.contains("::")
}

#[cfg(test)]
mod tests {
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
            "…", // Unicode ellipsis
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

    // ========================================================================
    // parse_file_uri / is_file_uri Tests
    // ========================================================================

    #[test]
    fn test_parse_file_uri_with_mime() {
        let result = parse_file_uri("#!B64!#image/png::abc123");
        assert_eq!(
            result,
            Some(FileUri {
                hash: "abc123",
                media_type: Some("image/png")
            })
        );
    }

    #[test]
    fn test_parse_file_uri_without_mime() {
        let result = parse_file_uri("#!B64!#::abc123");
        assert_eq!(
            result,
            Some(FileUri {
                hash: "abc123",
                media_type: None
            })
        );
    }

    #[test]
    fn test_parse_file_uri_invalid() {
        assert!(parse_file_uri("not-a-uri").is_none());
        assert!(parse_file_uri("#!B64!#no-separator").is_none());
        assert!(parse_file_uri("").is_none());
        // Empty hash after separator
        assert!(parse_file_uri("#!B64!#::").is_none());
        assert!(parse_file_uri("#!B64!#image/png::").is_none());
    }

    #[test]
    fn test_is_file_uri_with_mime() {
        assert!(is_file_uri("#!B64!#image/jpeg::abc123"));
        assert!(is_file_uri("#!B64!#application/pdf::hash"));
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
}
