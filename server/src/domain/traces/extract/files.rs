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
use moka::sync::Cache;
use serde_json::Value as JsonValue;

use crate::core::constants::{
    FILE_EXTRACTION_CACHE_IDLE_SECS, FILE_EXTRACTION_CACHE_MAX_ENTRIES, FILES_MAX_SIZE_BYTES,
    FILES_MIN_SIZE_BYTES,
};
#[cfg(test)]
use crate::utils::file_uri::FILE_URI_PREFIX;
use crate::utils::file_uri::{build_file_uri, is_file_uri};
use crate::utils::mime::{detect_mime_type, is_valid_mime_type};
use crate::utils::string::is_placeholder_value;

// ============================================================================
// FILE EXTRACTION CACHE
// ============================================================================

/// Cached result of a previous base64 extraction.
///
/// Stores the BLAKE3 hash, media type, and estimated decoded size so that
/// repeated encounters of the same base64 string can skip the
/// charset validation + BLAKE3 hash computation entirely.
#[derive(Clone)]
struct CachedFileResult {
    hash: String,
    media_type: Option<String>,
    size: usize,
}

/// Cross-batch cache for base64 file extraction.
///
/// Keyed on full BLAKE3 hash of the raw string (32 bytes, zero collision risk).
/// On cache hit, charset validation, normalization, and BLAKE3 are skipped.
/// Bounded by TinyLFU eviction to ~10K entries.
///
/// In real workloads, the same images appear repeatedly across spans
/// (history accumulation, agent context duplication). Observed: 91 total
/// extractions for only 3 unique files — 97% redundant work.
pub struct FileExtractionCache {
    inner: Cache<blake3::Hash, CachedFileResult>,
}

impl FileExtractionCache {
    pub fn new() -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(FILE_EXTRACTION_CACHE_MAX_ENTRIES)
                .time_to_idle(std::time::Duration::from_secs(
                    FILE_EXTRACTION_CACHE_IDLE_SECS,
                ))
                .build(),
        }
    }

    fn cache_key(s: &str) -> blake3::Hash {
        blake3::hash(s.as_bytes())
    }

    fn get(&self, key: &blake3::Hash) -> Option<CachedFileResult> {
        self.inner.get(key)
    }

    fn insert(&self, key: blake3::Hash, value: CachedFileResult) {
        self.inner.insert(key, value);
    }
}

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

/// An extracted file ready for storage
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    /// BLAKE3 hash of the normalized base64 content (64 hex chars)
    pub hash: String,
    /// Raw base64 bytes (not decoded) for cache misses, empty for cache hits
    pub data: Vec<u8>,
    /// Media type (e.g., "image/jpeg")
    pub media_type: Option<String>,
    /// Estimated decoded size in bytes
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

    result.modified = extract_recursive(messages, None, &mut result.files, &mut seen_hashes, None);

    result
}

/// Extract and replace base64 data with cross-batch caching.
///
/// Same as `extract_and_replace_files` but uses a shared cache to skip
/// charset validation + BLAKE3 hash for previously seen base64 strings.
/// Cache-hit files have `data: Vec::new()` (empty) since the actual bytes
/// were already captured by a previous extraction.
pub fn extract_and_replace_files_cached(
    messages: &mut JsonValue,
    cache: &FileExtractionCache,
) -> ExtractionResult {
    let mut result = ExtractionResult::default();
    let mut seen_hashes = std::collections::HashSet::new();

    result.modified = extract_recursive(
        messages,
        None,
        &mut result.files,
        &mut seen_hashes,
        Some(cache),
    );

    result
}

/// Recursively scan and extract base64 from JSON.
/// Returns true if the JSON was modified (any replacements made).
fn extract_recursive(
    json: &mut JsonValue,
    parent_key: Option<&str>,
    files: &mut Vec<ExtractedFile>,
    seen_hashes: &mut std::collections::HashSet<String>,
    cache: Option<&FileExtractionCache>,
) -> bool {
    match json {
        JsonValue::String(s) => {
            // Handle nested JSON strings FIRST (before protected field check)
            // Nested JSON may contain extractable fields even inside protected keys
            // e.g., events[].attributes.content = "[{\"image\": {\"bytes\": \"...\"}}]"
            if s.starts_with('{') || s.starts_with('[') {
                if let Ok(mut nested) = serde_json::from_str::<JsonValue>(s) {
                    let modified = extract_recursive(&mut nested, None, files, seen_hashes, cache);
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
            if let Some(modified_str) = extract_embedded_data_urls(s, files, seen_hashes, cache) {
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

            // Try cache lookup or fresh extraction
            if let Some(uri) = extract_or_cache(s, files, seen_hashes, cache, try_extract_base64) {
                *s = uri;
                return true;
            }
            false
        }
        JsonValue::Array(arr) => {
            let mut modified = false;
            for item in arr.iter_mut() {
                modified |= extract_recursive(item, None, files, seen_hashes, cache);
            }
            modified
        }
        JsonValue::Object(obj) => {
            let mut modified = false;
            for (key, value) in obj.iter_mut() {
                modified |= extract_recursive(value, Some(key), files, seen_hashes, cache);
            }
            modified
        }
        _ => false,
    }
}

/// Record an extracted file, deduplicating by hash within the same extraction.
/// Returns the file URI for replacement.
fn record_extracted_file(
    hash: String,
    data: Vec<u8>,
    media_type: Option<String>,
    size: usize,
    files: &mut Vec<ExtractedFile>,
    seen_hashes: &mut std::collections::HashSet<String>,
) -> String {
    let uri = build_file_uri(&hash, media_type.as_deref());
    if !seen_hashes.contains(&hash) {
        seen_hashes.insert(hash.clone());
        files.push(ExtractedFile {
            hash,
            data,
            media_type,
            size,
        });
    }
    uri
}

/// Try cache lookup, or extract + populate cache. Returns `Some(uri)` on success.
fn extract_or_cache(
    s: &str,
    files: &mut Vec<ExtractedFile>,
    seen_hashes: &mut std::collections::HashSet<String>,
    cache: Option<&FileExtractionCache>,
    extract_fn: impl FnOnce(&str) -> Option<ExtractedData>,
) -> Option<String> {
    let cache_key = cache.map(|_| FileExtractionCache::cache_key(s));

    // Cache hit: skip charset validation + BLAKE3
    if let Some(cached) = cache_key.and_then(|key| cache.unwrap().get(&key)) {
        let uri = record_extracted_file(
            cached.hash,
            Vec::new(),
            cached.media_type,
            cached.size,
            files,
            seen_hashes,
        );
        return Some(uri);
    }

    // Cache miss: extract, hash, and populate cache
    let extracted = extract_fn(s)?;
    if extracted.size < FILES_MIN_SIZE_BYTES {
        return None;
    }
    let hash = blake3_base64(&extracted.data);
    if let (Some(cache), Some(key)) = (cache, cache_key) {
        cache.insert(
            key,
            CachedFileResult {
                hash: hash.clone(),
                media_type: extracted.media_type.clone(),
                size: extracted.size,
            },
        );
    }
    let uri = record_extracted_file(
        hash,
        extracted.data,
        extracted.media_type,
        extracted.size,
        files,
        seen_hashes,
    );
    Some(uri)
}

/// Result of base64 extraction attempt
struct ExtractedData {
    data: Vec<u8>,
    media_type: Option<String>,
    size: usize,
}

/// Strip whitespace from base64 string, returning raw bytes.
fn strip_base64_whitespace(s: &str) -> Vec<u8> {
    if s.bytes().any(|b| b.is_ascii_whitespace()) {
        s.bytes().filter(|b| !b.is_ascii_whitespace()).collect()
    } else {
        s.as_bytes().to_vec()
    }
}

/// Check estimated decoded size is within extraction bounds.
/// Returns `Some(size)` if valid, `None` if too small or too large.
fn check_size_bounds(b64: &[u8], context: &str) -> Option<usize> {
    let size = estimate_decoded_size(b64);
    if size < FILES_MIN_SIZE_BYTES {
        return None;
    }
    if size > FILES_MAX_SIZE_BYTES {
        tracing::warn!(
            size,
            max = FILES_MAX_SIZE_BYTES,
            "Skipping {context}: exceeds max size"
        );
        return None;
    }
    Some(size)
}

/// Try to extract base64 data from a string.
///
/// Detection priority:
/// 1. Skip URLs, already-extracted URIs, and placeholders
/// 2. Parse data URLs (most reliable - contains mime type)
/// 3. Detect raw base64 by charset validation + size estimate + partial decode for magic bytes
///
/// Does NOT fully decode the base64 — returns normalized base64 bytes.
/// Full decode is deferred to persist time.
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

    // Single-pass: validate charset and detect whitespace simultaneously.
    let has_whitespace = check_base64_charset(s)?;

    // Normalize: strip whitespace if present, then check size bounds
    let normalized: Vec<u8> = if has_whitespace {
        s.bytes().filter(|b| !b.is_ascii_whitespace()).collect()
    } else {
        s.as_bytes().to_vec()
    };
    let estimated_size = check_size_bounds(&normalized, "file extraction")?;

    // Detect media type from first 16 base64 chars (partial decode of 12 bytes)
    let media_type = detect_media_type_from_b64_prefix(&normalized);

    Some(ExtractedData {
        size: estimated_size,
        data: normalized,
        media_type,
    })
}

/// Parse a data URL and extract the base64 content.
///
/// Format: `data:[<mediatype>][;base64],<data>`
///
/// Does NOT fully decode — returns normalized base64 bytes.
/// Full decode is deferred to persist time.
fn parse_data_url(url: &str) -> Option<ExtractedData> {
    let without_prefix = url.strip_prefix("data:")?;

    // Find the base64 marker - only extract base64-encoded data
    let base64_marker = ";base64,";
    let base64_pos = without_prefix.find(base64_marker)?;

    let media_type = &without_prefix[..base64_pos];
    let base64_data = &without_prefix[base64_pos + base64_marker.len()..];

    let normalized = strip_base64_whitespace(base64_data);
    let estimated_size = check_size_bounds(&normalized, "data URL extraction")?;

    // Determine media type: explicit from URL, or detect from magic bytes via partial decode
    let detected_type = if media_type.is_empty() {
        detect_media_type_from_b64_prefix(&normalized)
    } else {
        Some(media_type.to_string())
    };

    Some(ExtractedData {
        size: estimated_size,
        data: normalized,
        media_type: detected_type,
    })
}

/// Single-pass charset validation + whitespace detection.
///
/// Returns `Some(has_whitespace)` if the string contains only valid base64 characters,
/// `None` if any invalid character is found. Replaces the previous two-pass approach
/// of separate charset check + whitespace strip.
fn check_base64_charset(s: &str) -> Option<bool> {
    let mut has_whitespace = false;
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || b == b'+'
            || b == b'/'
            || b == b'-'
            || b == b'_'
            || b == b'='
        {
            continue;
        } else if b.is_ascii_whitespace() {
            has_whitespace = true;
        } else {
            return None;
        }
    }
    Some(has_whitespace)
}

/// BLAKE3 hash of raw base64 bytes, returned as 64-char hex string.
fn blake3_base64(b64: &[u8]) -> String {
    blake3::hash(b64).to_hex().to_string()
}

/// Estimate decoded size from base64 bytes (handles both padded and unpadded).
fn estimate_decoded_size(b64: &[u8]) -> usize {
    let len = b64.len();
    if len == 0 {
        return 0;
    }
    let padding = b64.iter().rev().take(2).filter(|&&b| b == b'=').count();
    (len * 3) / 4 - padding
}

/// Detect media type by partially decoding the first 16 base64 chars (12 raw bytes).
fn detect_media_type_from_b64_prefix(b64: &[u8]) -> Option<String> {
    let mut prefix = [0u8; 16];
    let mut count = 0;
    for &b in b64 {
        if count >= 16 {
            break;
        }
        if !b.is_ascii_whitespace() {
            prefix[count] = b;
            count += 1;
        }
    }
    if count < 16 {
        return None;
    }
    let decoded = BASE64_STANDARD
        .decode(&prefix[..16])
        .or_else(|_| BASE64_URL_SAFE.decode(&prefix[..16]))
        .ok()?;
    detect_mime_type(&decoded).map(String::from)
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
    cache: Option<&FileExtractionCache>,
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

                // Find the data URL boundaries (cheap: just scanning chars)
                let url_bounds = s[after_prefix..]
                    .find(";base64,")
                    .and_then(|marker_offset| {
                        let mime = &s[after_prefix..after_prefix + marker_offset];
                        if !mime.is_empty() && !is_valid_mime_type(mime) {
                            return None;
                        }
                        let b64_start = after_prefix + marker_offset + 8;
                        let b64_end = find_base64_end(s, b64_start);
                        Some((data_start, b64_end))
                    });

                if let Some((url_start, url_end)) = url_bounds {
                    let data_url = &s[url_start..url_end];

                    if let Some(uri) =
                        extract_or_cache(data_url, files, seen_hashes, cache, parse_data_url)
                    {
                        result.push_str(&s[pos..url_start]);
                        result.push_str(&uri);
                        modified = true;
                        pos = url_end;
                    } else {
                        result.push_str(&s[pos..after_prefix]);
                        pos = after_prefix;
                    }
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

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
