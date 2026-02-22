//! Content block normalization.
//!
//! Normalizes content blocks from various AI provider formats to unified SideML format.
//! Handles: OpenAI, Anthropic, AWS Bedrock/Strands, Google Gemini, and compatible providers.

use serde_json::{Value as JsonValue, json};

use super::types::ChatRole;
use crate::utils::file_uri as files;

/// FNV-1a hash constants (32-bit).
///
/// FNV-1a is a simple, non-cryptographic hash that's deterministic across
/// processes and platforms. Used for generating synthetic IDs.
const FNV_OFFSET_BASIS: u32 = 2166136261;
const FNV_PRIME: u32 = 16777619;

/// Compute a stable FNV-1a hash of a byte slice.
///
/// This hash is deterministic across process restarts and platforms,
/// unlike `DefaultHasher` which uses random seeding.
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in data {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Compute a short hash string (8 hex chars) for a JSON value.
///
/// Used for generating synthetic IDs for providers that don't supply them (e.g., Gemini).
/// This hash is **deterministic across process restarts and platforms**, making it
/// suitable for correlating tool calls and results across server restarts.
fn compute_short_hash(value: &JsonValue) -> String {
    // Serialize to JSON string (deterministic ordering from serde_json)
    let json_str = serde_json::to_string(value).unwrap_or_default();
    let hash = fnv1a_hash(json_str.as_bytes());
    format!("{:08x}", hash)
}

/// Try to parse a Python repr string as JSON.
///
/// Python SDKs (e.g., OpenAI Agents) sometimes serialize tool results using Python's
/// `str()` instead of `json.dumps()`, producing single-quoted dicts:
///   `{'status': 'success', 'content': [{'json': {...}}]}`
///
/// Single-pass conversion handles:
/// - Single-quoted strings → double-quoted (with inner `"` escaped)
/// - Double-quoted strings → pass through (with inner `'` preserved)
/// - `True`/`False`/`None` outside strings → `true`/`false`/`null`
/// - Escape sequences within strings (Python `\'` → literal `'`)
///
/// Only attempts conversion for strings starting with `{` or `[`.
/// Returns `None` if conversion produces invalid JSON (graceful fallback to text).
fn try_parse_python_repr(s: &str) -> Option<JsonValue> {
    if !s.starts_with('{') && !s.starts_with('[') {
        return None;
    }

    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 32);
    let mut i = 0;
    // false = outside any string, true = inside a string
    let mut in_string = false;
    // The opening quote character of the current string (b'\'' or b'"')
    let mut quote_char = 0u8;

    while i < len {
        let b = bytes[i];

        if in_string {
            if b == quote_char {
                // Closing quote → always emit JSON double quote
                out.push('"');
                in_string = false;
                i += 1;
            } else if b == b'"' && quote_char == b'\'' {
                // Literal double quote inside a single-quoted Python string
                // Must be escaped for JSON
                out.push_str("\\\"");
                i += 1;
            } else if b == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                match next {
                    // Python \' → literal single quote (safe in JSON double-quoted string)
                    b'\'' => {
                        out.push('\'');
                        i += 2;
                    }
                    // Python \" → literal double quote (must be escaped in JSON)
                    b'"' => {
                        out.push_str("\\\"");
                        i += 2;
                    }
                    // Common escape sequences valid in both Python and JSON
                    b'\\' | b'/' | b'n' | b't' | b'r' | b'b' | b'f' => {
                        out.push('\\');
                        out.push(next as char);
                        i += 2;
                    }
                    // Unicode escape: \uXXXX (valid in both Python and JSON)
                    b'u' => {
                        out.push('\\');
                        out.push('u');
                        i += 2;
                    }
                    // Other Python escapes (\x, \N, \0, etc.) → pass through
                    // May cause JSON parse failure → graceful fallback to text
                    _ => {
                        out.push('\\');
                        i += 1;
                    }
                }
            } else {
                // Regular character inside string (handles multi-byte UTF-8)
                let ch = s[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        } else {
            // Outside any string
            match b {
                b'\'' | b'"' => {
                    out.push('"');
                    in_string = true;
                    quote_char = b;
                    i += 1;
                }
                b'T' if matches_python_literal(bytes, i, b"True") => {
                    out.push_str("true");
                    i += 4;
                }
                b'F' if matches_python_literal(bytes, i, b"False") => {
                    out.push_str("false");
                    i += 5;
                }
                b'N' if matches_python_literal(bytes, i, b"None") => {
                    out.push_str("null");
                    i += 4;
                }
                _ => {
                    // Structure chars, whitespace, numbers (all ASCII outside strings)
                    let ch = s[i..].chars().next()?;
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }

    serde_json::from_str(&out).ok()
}

/// Check if a Python literal (`True`, `False`, `None`) appears at byte position `i`
/// with word boundaries on both sides.
///
/// Word boundary = not preceded/followed by alphanumeric or underscore.
/// This prevents replacing inside identifiers like `Trueness` or `_None`.
#[inline]
fn matches_python_literal(bytes: &[u8], i: usize, literal: &[u8]) -> bool {
    let end = i + literal.len();
    if end > bytes.len() || bytes[i..end] != *literal {
        return false;
    }
    // Check boundary after literal
    if end < bytes.len() {
        let after = bytes[end];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    // Check boundary before literal (non-ASCII bytes are always valid boundaries)
    if i > 0 {
        let before = bytes[i - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    true
}

/// Normalize content to Vec<ContentBlock> format.
///
/// This function handles:
/// - Double-encoded JSON strings (common from some SDKs)
/// - Sparse arrays with placeholder empty objects (from unflatten)
/// - Nested content wrappers (message_content, reasoning_content)
/// - Provider-specific formats (OpenAI, Anthropic, Bedrock, Gemini, Vercel)
pub fn normalize_content(content: Option<&JsonValue>) -> JsonValue {
    match content {
        None => json!([]),
        Some(JsonValue::String(s)) if s.is_empty() => json!([]),
        Some(JsonValue::String(s)) => {
            // Try to parse as JSON first (handles double-encoded content from some SDKs)
            // This is common when content is stored as a JSON string in attributes
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(s) {
                // Recursively normalize the parsed JSON
                return normalize_content(Some(&parsed));
            }
            // Try Python repr format (common from Python SDKs like OpenAI Agents)
            // Python's str() uses single quotes: {'key': 'value', 'flag': True}
            if let Some(parsed) = try_parse_python_repr(s) {
                return normalize_content(Some(&parsed));
            }
            // If not valid JSON or Python repr, treat as plain text
            json!([{"type": "text", "text": s}])
        }
        Some(JsonValue::Array(arr)) => {
            // Filter sparse array placeholders (empty objects from unflatten)
            // ONLY when the array has a mix of empty and non-empty elements.
            // An array with only empty objects (e.g., [{}]) could be valid structured output.
            let has_non_empty = arr.iter().any(|v| !is_sparse_array_placeholder(v));
            let has_empty = arr.iter().any(is_sparse_array_placeholder);
            let should_filter_placeholders = has_non_empty && has_empty;

            let blocks: Vec<JsonValue> = arr
                .iter()
                .filter(|v| !should_filter_placeholders || !is_sparse_array_placeholder(v))
                .filter_map(normalize_content_block)
                .collect();
            json!(blocks)
        }
        Some(obj @ JsonValue::Object(_)) => {
            // Single content block (skip if it's a sparse array placeholder)
            if is_sparse_array_placeholder(obj) {
                return json!([]);
            }
            match normalize_content_block(obj) {
                Some(block) => json!([block]),
                None => json!([]),
            }
        }
        _ => json!([]),
    }
}

/// Check if a value is a sparse array placeholder created by unflatten.
///
/// When unflatten processes indexed attributes with gaps (e.g., `contents.1` exists
/// but `contents.0` doesn't), it creates empty `{}` objects as placeholders.
/// These should be filtered out during normalization.
///
/// Criteria for placeholder detection:
/// - Must be an object
/// - Must be completely empty (no keys)
/// - OR only contains other empty placeholders (recursively)
///
/// Note: This is intentionally strict. Objects with ANY keys are kept, even if
/// those keys have null/empty values, to avoid filtering legitimate structured output.
fn is_sparse_array_placeholder(value: &JsonValue) -> bool {
    match value {
        JsonValue::Object(obj) => {
            if obj.is_empty() {
                return true;
            }
            // Check for objects that only contain placeholders (recursive sparse arrays)
            // e.g., {"contents": [{}, {}]} where all array elements are placeholders
            obj.values().all(|v| match v {
                JsonValue::Array(arr) => arr.iter().all(is_sparse_array_placeholder),
                JsonValue::Object(_) => is_sparse_array_placeholder(v),
                _ => false,
            })
        }
        _ => false,
    }
}

/// Normalize a single content block from any provider format to canonical SideML format.
///
/// Handles all known provider formats:
/// - **SideML**: Already-normalized blocks pass through unchanged
/// - OpenAI: `{"type": "text", "text": "..."}`, `{"type": "image_url", ...}`, etc.
/// - Anthropic: `{"type": "text|image|tool_use|tool_result", ...}`
/// - Bedrock/Strands: `{"text": "..."}`, `{"toolUse": {...}}`, `{"toolResult": {...}}`
/// - Gemini: `{"inline_data": {...}}`, `{"functionCall": {...}}`, `{"functionResponse": {...}}`
/// - Vercel AI: `{"type": "tool-call|tool-result|json|text", ...}`
///
/// Unknown formats are handled based on structure:
/// - Plain JSON objects without type → `{"type": "json", "data": ...}` (structured output)
/// - Objects with unrecognized type → `{"type": "unknown", "raw": ...}` (preserved for debugging)
pub fn normalize_content_block(block: &JsonValue) -> Option<JsonValue> {
    // Handle raw strings in mixed arrays (e.g., AutoGen MultiModalMessage ["text", {image}])
    if let Some(s) = block.as_str() {
        return if s.is_empty() {
            None
        } else {
            Some(json!({"type": "text", "text": s}))
        };
    }

    // First check if block is already in SideML format (idempotent operation)
    try_sideml_passthrough(block)
        // OpenInference nested message_content wrapper
        .or_else(|| try_openinference_message_content(block))
        // Then try provider-specific formats
        .or_else(|| try_openai_format(block))
        .or_else(|| try_anthropic_format(block))
        .or_else(|| try_bedrock_format(block))
        .or_else(|| try_gemini_format(block))
        .or_else(|| try_vercel_format(block))
        // Universal media patterns (mime_type fields, nested self-named media)
        .or_else(|| try_media_fallback(block))
        // Finally, handle unknown formats
        .or_else(|| try_unknown_fallback(block))
}

/// Passthrough for already-normalized SideML content blocks.
///
/// This ensures normalization is idempotent - calling it multiple times
/// on the same data produces the same result.
///
/// We check BOTH type name AND structure because some provider formats
/// (Mistral, Anthropic) share type names with SideML but have different structures.
fn try_sideml_passthrough(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;

    // Check structure based on type
    let is_valid_sideml = match block_type {
        // Text: must have "text" string field (not just type)
        "text" => block.get("text").is_some_and(|t| t.is_string()),

        // Image/audio/document/video/file: must have "source" and "data" fields
        "image" | "audio" | "document" | "video" | "file" => {
            block.get("source").is_some() && block.get("data").is_some()
        }

        // Thinking: must have "text" field (not "thinking" array like Mistral)
        "thinking" => block.get("text").is_some() && block.get("thinking").is_none(),

        // Redacted thinking: must have "data" field
        "redacted_thinking" => block.get("data").is_some(),

        // Tool use: must have "name" field
        "tool_use" => block.get("name").is_some(),

        // Tool result: must have "tool_use_id" or "content" field
        "tool_result" => block.get("tool_use_id").is_some() || block.get("content").is_some(),

        // JSON: must have "data" field (not "value" like Vercel)
        "json" => block.get("data").is_some(),

        // Refusal: must have "message" field (not "refusal" like OpenAI)
        "refusal" => block.get("message").is_some(),

        // These are SideML-only types with no provider variants
        "unknown" | "context" | "tool_definitions" => true,

        // Unknown type - not SideML
        _ => false,
    };

    if is_valid_sideml {
        Some(block.clone())
    } else {
        None
    }
}

/// Try to normalize as a known provider content block format (without unknown fallback).
///
/// Used for tool result content objects to distinguish:
/// - Provider content blocks: `{"text": "hello"}` → normalize to SideML
/// - Structured data: `{"temp": 72}` → keep as-is (returns None)
fn try_normalize_provider_format(block: &JsonValue) -> Option<JsonValue> {
    try_openai_format(block)
        .or_else(|| try_anthropic_format(block))
        .or_else(|| try_bedrock_format(block))
        .or_else(|| try_gemini_format(block))
        .or_else(|| try_vercel_format(block))
        .or_else(|| try_media_fallback(block))
    // No unknown fallback - returns None if no provider format matches
}

/// Extract the "type" field from a content block as a string.
#[inline]
fn get_block_type(block: &JsonValue) -> Option<&str> {
    block.get("type").and_then(|t| t.as_str())
}

/// Normalize tool result content to SideML format.
///
/// Converts provider-specific formats to unified SideML:
/// - Array of blocks → normalize each block, deduplicate identical blocks
/// - String → keep as-is (simple text result)
/// - Object matching provider format → normalize to SideML
/// - Object (structured data) → keep as-is (e.g., {"temp": 72})
/// - Other primitives → wrap in {"type": "json", "data": ...}
///
/// Deduplication handles cases where the same data appears in multiple formats,
/// e.g., Vercel AI SDK sends both raw data and `{type: "json", value: ...}` wrapper.
fn normalize_tool_result_content(content: Option<JsonValue>) -> JsonValue {
    match content {
        None => json!(null),
        Some(JsonValue::Array(arr)) => {
            // Normalize each block in the array to SideML format
            let normalized: Vec<JsonValue> =
                arr.iter().filter_map(normalize_content_block).collect();
            if normalized.is_empty() {
                json!(null)
            } else {
                // Deduplicate identical blocks (same data in different formats)
                json!(deduplicate_content_blocks(normalized))
            }
        }
        Some(JsonValue::String(s)) => json!(s), // Keep string as-is
        Some(obj @ JsonValue::Object(_)) => {
            // Try to normalize if it matches a known provider content block format
            // E.g., {"text": "hello"} (Bedrock) → {"type": "text", "text": "hello"}
            // Keep structured data as-is: {"temp": 72} → {"temp": 72}
            try_normalize_provider_format(&obj).unwrap_or(obj)
        }
        Some(other) => {
            // Wrap primitives (bool, number, null) in json block
            json!([{"type": "json", "data": other}])
        }
    }
}

/// Deduplicate identical content blocks.
///
/// Vercel AI SDK (and possibly others) may send the same data in multiple formats:
/// - Raw structured data: `{status: "success", content: [...]}`
/// - Wrapped format: `{type: "json", value: {status: "success", content: [...]}}`
///
/// After normalization, these become identical `{type: "json", data: ...}` blocks.
/// This function removes duplicates while preserving order (keeps first occurrence).
fn deduplicate_content_blocks(blocks: Vec<JsonValue>) -> Vec<JsonValue> {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(blocks.len());

    for block in blocks {
        // Use JSON serialization as identity (deterministic ordering from serde_json)
        let key = serde_json::to_string(&block).unwrap_or_default();
        if seen.insert(key) {
            result.push(block);
        }
    }

    result
}

// ========== Provider-specific content format handlers ==========

/// Type-tagged format (OpenAI and compatible providers).
/// Handles: text, image_url, input_audio, audio, refusal, output_json, thinking, etc.
fn try_openai_format(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        "text" | "input_text" | "output_text" => {
            let text = block.get("text")?.as_str()?;
            Some(json!({"type": "text", "text": text}))
        }
        "image_url" | "input_image" => {
            let image_obj = block.get("image_url")?;
            let url = image_obj
                .get("url")
                .or(Some(image_obj))
                .and_then(|u| u.as_str())?;
            let (source, data, media_type) = parse_data_url(url);
            // Preserve detail field (affects token usage: "auto", "low", "high")
            let detail = image_obj.get("detail").and_then(|d| d.as_str());
            let mut result =
                json!({"type": "image", "media_type": media_type, "source": source, "data": data});
            if let Some(d) = detail {
                result["detail"] = json!(d);
            }
            Some(result)
        }
        // Audio blocks: input_audio, audio (same pattern)
        "input_audio" | "audio" => {
            let audio_field = if block_type == "input_audio" {
                "input_audio"
            } else {
                "audio"
            };
            let audio = block.get(audio_field)?;
            let data = audio.get("data")?.as_str()?;
            let format = audio.get("format").and_then(|f| f.as_str());

            // Check if data was replaced with #!B64!# file reference
            let source_type = if files::is_file_uri(data) {
                "file"
            } else {
                "base64"
            };

            Some(json!({
                "type": "audio",
                "media_type": build_media_type(format, "audio"),
                "source": source_type,
                "data": data
            }))
        }
        "input_file" => {
            if let Some(data_str) = block.get("file_data").and_then(|d| d.as_str()) {
                let (source, data, media_type) = parse_data_url(data_str);
                let content_type = media_type
                    .as_deref()
                    .map(mime_to_content_type)
                    .unwrap_or("file");
                let mut result = json!({
                    "type": content_type,
                    "source": source,
                    "data": data,
                    "media_type": media_type
                });
                if let Some(name) = block.get("filename").and_then(|n| n.as_str()) {
                    result["name"] = json!(name);
                }
                Some(result)
            } else if let Some(url) = block.get("file_url").and_then(|u| u.as_str()) {
                Some(json!({"type": "file", "source": "url", "data": url}))
            } else {
                block
                    .get("file_id")
                    .and_then(|id| id.as_str())
                    .map(|file_id| json!({"type": "file", "source": "file_id", "data": file_id}))
            }
        }
        "refusal" => {
            // Handle both {"type": "refusal", "message": "..."} and {"type": "refusal", "refusal": "..."}
            let message = block
                .get("message")
                .or_else(|| block.get("refusal"))
                .and_then(|m| m.as_str())?;
            Some(json!({"type": "refusal", "message": message}))
        }
        "output_json" | "json_object" => {
            let json_data = block.get("json").cloned().unwrap_or(json!({}));
            Some(json!({"type": "json", "data": json_data}))
        }
        // Claude extended thinking / Mistral reasoning / PydanticAI
        "thinking" => {
            // Extract thinking content from various provider formats
            let text = extract_thinking_text(block);
            let signature = block.get("signature").and_then(|s| s.as_str());
            Some(json!({
                "type": "thinking",
                "text": text,
                "signature": signature
            }))
        }
        // Claude redacted thinking (when thinking is not exposed)
        "redacted_thinking" => {
            let data = block.get("data").and_then(|d| d.as_str()).unwrap_or("");
            Some(json!({
                "type": "redacted_thinking",
                "data": data
            }))
        }
        _ => None,
    }
}

/// Anthropic format: {"type": "text|image|document|tool_use|tool_result", ...}
fn try_anthropic_format(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        // Media blocks: image, document (same pattern with source object)
        "image" | "document" => try_anthropic_media(block, block_type),
        "tool_use" => Some(json!({
            "type": "tool_use",
            "id": block.get("id"),
            "name": block.get("name"),
            "input": block.get("input").cloned().unwrap_or(json!({}))
        })),
        "tool_result" => {
            let raw_content = block.get("content").cloned();
            let normalized_content = normalize_tool_result_content(raw_content);
            Some(json!({
                "type": "tool_result",
                "tool_use_id": block.get("tool_use_id"),
                "content": normalized_content,
                "is_error": block.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false)
            }))
        }
        _ => None, // "text" handled by try_openai_format
    }
}

/// Extract Anthropic media block (image, document).
fn try_anthropic_media(block: &JsonValue, block_type: &str) -> Option<JsonValue> {
    let source = block.get("source")?;
    let source_type = source.get("type")?.as_str()?;
    let (src, data) = match source_type {
        "base64" => {
            let data = source.get("data")?.as_str()?;
            // Check if data was replaced with #!B64!# file reference
            if files::is_file_uri(data) {
                ("file", data)
            } else {
                ("base64", data)
            }
        }
        "url" => ("url", source.get("url")?.as_str()?),
        _ => return None,
    };
    Some(json!({
        "type": block_type,
        "media_type": source.get("media_type").and_then(|m| m.as_str()),
        "source": src,
        "data": data
    }))
}

/// Bedrock/Strands format: {"text": "..."}, {"image": {...}}, {"toolUse": {...}}, {"reasoningContent": {...}}
fn try_bedrock_format(block: &JsonValue) -> Option<JsonValue> {
    // Text block: STRICT match - must be exactly {"text": "..."} with no other fields
    // This prevents structured output like {"text": "summary", "confidence": 0.9} from
    // being incorrectly parsed as just a text block (which would lose the confidence field)
    if let Some(obj) = block.as_object()
        && obj.len() == 1
        && let Some(text) = obj.get("text").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "text", "text": text}));
    }

    // Bedrock extended thinking (reasoningContent)
    // Format: {"reasoningContent": {"reasoningText": {"text": "...", "signature": "..."}}}
    if let Some(reasoning) = block.get("reasoningContent") {
        // Primary format: reasoningText with text content
        if let Some(reasoning_text) = reasoning.get("reasoningText") {
            let text = reasoning_text
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let signature = reasoning_text.get("signature").and_then(|s| s.as_str());
            return Some(json!({
                "type": "thinking",
                "text": text,
                "signature": signature
            }));
        }
        // Redacted thinking variant
        if let Some(redacted) = reasoning.get("redactedContent") {
            let data = redacted.get("data").and_then(|d| d.as_str()).unwrap_or("");
            return Some(json!({
                "type": "redacted_thinking",
                "data": data
            }));
        }
        // Future-proofing: unknown reasoningContent variant
        // Preserve raw content so we don't silently lose data
        return Some(json!({
            "type": "unknown",
            "raw": block.clone()
        }));
    }

    // Media blocks: image, document, video (all follow same pattern)
    if let Some(result) = try_bedrock_media(block, "image", "image") {
        return Some(result);
    }
    if let Some(result) = try_bedrock_media(block, "document", "application") {
        return Some(result);
    }
    if let Some(result) = try_bedrock_media(block, "video", "video") {
        return Some(result);
    }
    // Tool use (Strands/Bedrock native)
    if let Some(tool_use) = block.get("toolUse") {
        return Some(json!({
            "type": "tool_use",
            "id": tool_use.get("toolUseId"),
            "name": tool_use.get("name"),
            "input": tool_use.get("input").cloned().unwrap_or(json!({}))
        }));
    }
    // Tool result
    if let Some(tool_result) = block.get("toolResult") {
        let raw_content = tool_result.get("content").cloned();
        let normalized_content = normalize_tool_result_content(raw_content);
        return Some(json!({
            "type": "tool_result",
            "tool_use_id": tool_result.get("toolUseId"),
            "content": normalized_content,
            "is_error": tool_result.get("status").and_then(|s| s.as_str()) == Some("error")
        }));
    }
    None
}

/// Extract Bedrock media block (image, document, video).
fn try_bedrock_media(block: &JsonValue, field: &str, mime_prefix: &str) -> Option<JsonValue> {
    let media = block.get(field)?;
    let format = media.get("format").and_then(|f| f.as_str());
    let source = media.get("source")?;
    let data = source.get("bytes")?.as_str()?;

    // Check if data was replaced with #!B64!# file reference
    let source_type = if files::is_file_uri(data) {
        "file"
    } else {
        "base64"
    };

    let mut result = json!({
        "type": field,
        "media_type": build_media_type(format, mime_prefix),
        "source": source_type,
        "data": data
    });
    // Document can have a name field
    if field == "document"
        && let Some(name) = media.get("name").and_then(|n| n.as_str())
    {
        result["name"] = json!(name);
    }
    Some(result)
}

/// Gemini format: {"text": "..."}, {"inline_data": {...}}, {"file_data": {...}}, {"functionCall": {...}}, {"thinking": "..."}
fn try_gemini_format(block: &JsonValue) -> Option<JsonValue> {
    // Gemini thinking part: {"thinking": "..."}
    // Similar to text parts {"text": "..."} but for extended thinking
    // STRICT match: must be exactly {"thinking": "..."} with no other fields
    if let Some(obj) = block.as_object()
        && obj.len() == 1
        && let Some(thinking) = obj.get("thinking").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "thinking", "text": thinking, "signature": null}));
    }

    // Gemini/ADK thought block: {"text": "...", "thought": true}
    // ADK sends thinking content as text blocks with a thought flag
    if let Some(obj) = block.as_object()
        && obj
            .get("thought")
            .is_some_and(|t| t.as_bool().unwrap_or(false))
        && let Some(text) = obj.get("text").and_then(|t| t.as_str())
    {
        return Some(json!({"type": "thinking", "text": text, "signature": null}));
    }

    // inline_data (base64)
    if let Some(inline) = block.get("inline_data") {
        let mime = inline.get("mime_type")?.as_str()?;
        let data = inline.get("data")?.as_str()?;
        let block_type = mime_to_content_type(mime);

        // Check if data was replaced with #!B64!# file reference
        let source_type = if files::is_file_uri(data) {
            "file"
        } else {
            "base64"
        };

        return Some(json!({
            "type": block_type,
            "media_type": mime,
            "source": source_type,
            "data": data
        }));
    }
    // file_data (URL)
    if let Some(file) = block.get("file_data") {
        let mime = file.get("mime_type")?.as_str()?;
        let uri = file.get("file_uri")?.as_str()?;
        let block_type = mime_to_content_type(mime);
        return Some(json!({
            "type": block_type,
            "media_type": mime,
            "source": "url",
            "data": uri
        }));
    }
    // functionCall / function_call (Gemini tool use - both camelCase and snake_case)
    // Gemini doesn't provide tool call IDs, so we generate synthetic IDs based on
    // function name + args hash to prevent deduplication collisions when the same
    // function is called multiple times with different arguments.
    if let Some(fc) = block
        .get("functionCall")
        .or_else(|| block.get("function_call"))
    {
        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let args = fc.get("args").cloned().unwrap_or(json!({}));
        let args_hash = compute_short_hash(&args);
        let synthetic_id = format!("gemini_{name}_call_{args_hash}");

        return Some(json!({
            "type": "tool_use",
            "id": synthetic_id,
            "name": name,
            "input": args
        }));
    }
    // functionResponse / function_response (Gemini tool result - both cases)
    // Gemini doesn't provide tool call IDs, so we generate synthetic IDs based on
    // function name + response hash to prevent deduplication collisions when the same
    // function returns different results.
    if let Some(fr) = block
        .get("functionResponse")
        .or_else(|| block.get("function_response"))
    {
        let name = fr.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let raw_content = fr.get("response").cloned();
        let response_hash = match &raw_content {
            Some(content) => compute_short_hash(content),
            None => compute_short_hash(&json!(null)),
        };
        let synthetic_id = format!("gemini_{name}_result_{response_hash}");

        let normalized_content = normalize_tool_result_content(raw_content);
        return Some(json!({
            "type": "tool_result",
            "tool_use_id": synthetic_id,
            "content": normalized_content,
            "is_error": false
        }));
    }
    None
}

/// Vercel AI SDK formats:
///
/// 1. Typed blocks: `{"type": "tool-call"|"tool-result"|"json"|"text", ...}`
/// 2. Aggregated response: `{"content": "...", "finishReason": "stop", "role": "assistant"}`
///
/// Vercel AI uses hyphenated type names and camelCase field names.
fn try_vercel_format(block: &JsonValue) -> Option<JsonValue> {
    // Try typed block format first
    if let Some(block_type) = block.get("type").and_then(|t| t.as_str()) {
        return match block_type {
            // Tool call: {"type": "tool-call", "toolCallId": "...", "toolName": "...", "input": {...}}
            "tool-call" => {
                let id = block.get("toolCallId").and_then(|v| v.as_str());
                let name = block.get("toolName").and_then(|v| v.as_str())?;
                let input = block.get("input").cloned().unwrap_or(json!({}));
                // Also check for "args" field (alternative format)
                let input = if input == json!({}) {
                    block.get("args").cloned().unwrap_or(json!({}))
                } else {
                    input
                };
                Some(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }))
            }
            // Tool result: {"type": "tool-result", "toolCallId": "...", "result": {...}}
            "tool-result" => {
                let tool_use_id = block.get("toolCallId").and_then(|v| v.as_str());
                let raw_content = block.get("result").or_else(|| block.get("output")).cloned();
                let normalized_content = normalize_tool_result_content(raw_content);
                let is_error = block
                    .get("isError")
                    .or_else(|| block.get("is_error"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Some(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": normalized_content,
                    "is_error": is_error
                }))
            }
            // JSON content block: {"type": "json", "value": {...}}
            "json" => {
                let data = block.get("value").cloned().unwrap_or(json!({}));
                Some(json!({"type": "json", "data": data}))
            }
            // Text content block: {"type": "text", "value": "..."}
            "text" if block.get("value").is_some() => {
                let text = block.get("value").and_then(|v| v.as_str())?;
                Some(json!({"type": "text", "text": text}))
            }
            // File content block: {"type": "file", "mediaType": "image/jpeg", "data": "..."}
            // Also handles "mimeType" variant (older API versions)
            "file" => {
                let media_type = block
                    .get("mediaType")
                    .or_else(|| block.get("mimeType"))
                    .and_then(|m| m.as_str())?;
                let data = block.get("data")?.as_str()?;

                // Determine content type from MIME type
                let content_type = mime_to_content_type(media_type);

                // Determine source type (file reference or base64)
                let source_type = if files::is_file_uri(data) {
                    "file"
                } else {
                    "base64"
                };

                Some(json!({
                    "type": content_type,
                    "media_type": media_type,
                    "source": source_type,
                    "data": data
                }))
            }
            _ => None,
        };
    }

    // Try aggregated response format: {"content": "...", "finishReason": "stop", "role": "assistant"}
    let obj = block.as_object()?;
    let content = obj.get("content").and_then(|v| v.as_str())?;
    let has_vercel_fields = obj.contains_key("finishReason")
        || obj.contains_key("providerMetadata")
        || obj.get("role").and_then(|v| v.as_str()) == Some("assistant");

    if has_vercel_fields {
        return Some(json!({"type": "text", "text": content}));
    }

    None
}

/// Universal media fallback for patterns not handled by provider-specific handlers.
///
/// Handles two patterns:
/// 1. Blocks with `mime_type` + `data` fields (LangChain Python, various SDKs)
/// 2. Nested self-named media from dotted-key unflattening (OpenInference)
///    e.g. `{"type": "image", "image": {"image": {"url": "..."}}}`
fn try_media_fallback(block: &JsonValue) -> Option<JsonValue> {
    // Pattern 1: mime_type + data (no "type" field required)
    if let Some(mime) = block.get("mime_type").and_then(|m| m.as_str())
        && let Some(data) = block.get("data").and_then(|d| d.as_str())
    {
        let content_type = mime_to_content_type(mime);
        let source = infer_source_type(data);
        let mut result = json!({
            "type": content_type,
            "media_type": mime,
            "source": source,
            "data": data
        });
        if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
            result["name"] = json!(name);
        }
        return Some(result);
    }

    // Pattern 2: Bare file reference {"data": "#!B64!#[mime]::hash"} (e.g., AutoGen Image)
    // Use embedded MIME from URI to set proper content type (image/audio/video/document)
    if let Some(data) = block.get("data").and_then(|d| d.as_str())
        && files::is_file_uri(data)
    {
        let parsed = files::parse_file_uri(data);
        let mime = parsed.as_ref().and_then(|f| f.media_type);
        let content_type = mime.map(mime_to_content_type).unwrap_or("file");
        let mut result = json!({
            "type": content_type,
            "source": "file",
            "data": data
        });
        if let Some(mt) = mime {
            result["media_type"] = json!(mt);
        }
        return Some(result);
    }

    // Pattern 3: Self-named nested media (requires "type" field)
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        "image" | "audio" | "video" | "document" | "file" => {
            let media = block.get(block_type)?;
            // Try URL: media.url or media.<type>.url
            let url = media
                .get("url")
                .and_then(|u| u.as_str())
                .or_else(|| media.get(block_type)?.get("url")?.as_str());
            if let Some(url) = url {
                let (source, data, media_type) = parse_data_url(url);
                let mut result = json!({
                    "type": block_type,
                    "source": source,
                    "data": data
                });
                if let Some(mt) = media_type {
                    result["media_type"] = json!(mt);
                }
                return Some(result);
            }
            // Try data: media.data or media.<type>.data
            let data = media
                .get("data")
                .and_then(|d| d.as_str())
                .or_else(|| media.get(block_type)?.get("data")?.as_str());
            if let Some(data) = data {
                return Some(json!({
                    "type": block_type,
                    "source": infer_source_type(data),
                    "data": data
                }));
            }
        }
        _ => {}
    }

    None
}

/// Infer the source type from data content.
fn infer_source_type(data: &str) -> &'static str {
    if files::is_file_uri(data) {
        "file"
    } else if data.starts_with("http://") || data.starts_with("https://") {
        "url"
    } else {
        "base64"
    }
}

/// Provider-specific field names that indicate a content block structure.
/// If ANY of these exist at the top level but didn't match in provider handlers,
/// it's likely a malformed content block → unknown (conservative approach).
///
/// This catches cases like `{"text": "hello", "extra": "field"}` which looks like
/// a Bedrock text block with extra fields - better to flag as unknown than assume
/// it's structured output.
const PROVIDER_CONTENT_FIELDS: &[&str] = &[
    // Bedrock/Strands
    "text",
    "image",
    "document",
    "video",
    "toolUse",
    "toolResult",
    "reasoningContent", // Bedrock extended thinking
    // Gemini
    "inline_data",
    "file_data",
    "functionCall",
    "functionResponse",
    "function_call",
    "function_response",
    "thinking", // Gemini thinking part
    // Anthropic (when combined with other fields)
    "source",
    // Vercel AI SDK (camelCase)
    "toolCallId",
    "toolName",
];

/// Universal content wrapper unwrapping.
///
/// Various frameworks wrap content blocks in different wrapper objects:
/// - OpenInference: `{ "message_content": { "type": "text", "text": "..." } }`
/// - OpenInference: `{ "reasoning_content": { "text": "...", "signature": "..." } }`
/// - LangChain/LangGraph: `{ "kwargs": { "content": "..." } }`
/// - Some SDKs: `{ "value": { "type": "text", "text": "..." } }`
///
/// This function handles all known wrapper patterns universally.
fn try_openinference_message_content(block: &JsonValue) -> Option<JsonValue> {
    // Known content wrapper field names
    const CONTENT_WRAPPER_FIELDS: &[&str] = &[
        "message_content", // OpenInference
        "value",           // Some SDKs wrap content in a value field
    ];

    // Check for standard content wrappers
    for field in CONTENT_WRAPPER_FIELDS {
        if let Some(inner) = block.get(*field) {
            // Recursively normalize the inner content
            return normalize_content_block(inner);
        }
    }

    // Check for reasoning_content wrapper (extended thinking - special handling)
    if let Some(inner) = block.get("reasoning_content") {
        // Extract thinking text from the reasoning content
        let text = inner
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        let signature = inner.get("signature").and_then(|s| s.as_str());
        return Some(json!({
            "type": "thinking",
            "text": text,
            "signature": signature
        }));
    }

    // Check for LangChain kwargs wrapper (common in LangChain message serialization)
    // Pattern: { "kwargs": { "content": "...", "type": "..." } }
    if let Some(kwargs) = block.get("kwargs")
        && (kwargs.get("content").is_some() || kwargs.get("type").is_some())
    {
        return normalize_content_block(kwargs);
    }

    None
}

/// Fallback for unrecognized content block formats.
///
/// Classification logic:
/// 1. **Non-object (array, primitive)** → unknown (preserve raw data)
/// 2. **Has string `type` field** → unknown (unrecognized or malformed content block)
/// 3. **Has provider field** → unknown (malformed provider-specific block)
/// 4. **Plain JSON object** → json (structured output)
fn try_unknown_fallback(block: &JsonValue) -> Option<JsonValue> {
    // Handle non-objects (arrays, primitives) - preserve as unknown
    // This prevents nested arrays from being silently dropped
    let Some(obj) = block.as_object() else {
        return Some(json!({"type": "unknown", "raw": block.clone()}));
    };

    // Has string `type` field - this is a content block format that didn't match any handler
    // Either malformed (known type with wrong structure) or future (unknown type)
    // Note: non-string `type` field (e.g., `{"type": 123}`) is treated as structured output
    if obj.get("type").and_then(|t| t.as_str()).is_some() {
        return Some(json!({"type": "unknown", "raw": block.clone()}));
    }

    // Has provider-specific field but didn't match in earlier handlers
    // This is a malformed content block - preserve as unknown
    for field in PROVIDER_CONTENT_FIELDS {
        if obj.contains_key(*field) {
            return Some(json!({"type": "unknown", "raw": block.clone()}));
        }
    }

    // Plain JSON object without type or provider fields
    // This is structured output (Pydantic models, json_mode responses, etc.)
    Some(json!({"type": "json", "data": block.clone()}))
}

/// Convert content blocks to unified tool_result format when associated with a tool call.
///
/// Behavior:
/// - Multiple tool_results: Keep as-is (LLM span with multiple tool results)
/// - Single tool_result with siblings: Merge siblings INTO the tool_result's content
/// - Single tool_result alone: Keep as-is
/// - No tool_result: Create one wrapping all content blocks
///
/// This ensures consistent nested format: [tool_result with [all content inside]]
///
/// # Important: Role-based conversion only
///
/// Conversion is triggered by ROLE, not by `tool_use_id` presence. While `tool_use_id`
/// can appear on both tool results AND assistant messages (tool calls), we only want
/// to wrap content in tool_result for actual tool role messages.
pub fn convert_to_tool_result(
    content: &JsonValue,
    role: &str,
    tool_use_id: &Option<String>,
) -> JsonValue {
    // Only convert for tool role - NOT based on tool_use_id presence alone
    // (tool_use_id can appear on assistant messages with tool calls too)
    if !ChatRole::is_tool_role(role) {
        return content.clone();
    }

    let Some(arr) = content.as_array() else {
        return content.clone();
    };

    // If content has tool_use blocks, this is an assistant message with tool calls - don't convert
    let has_tool_use = arr.iter().any(|b| get_block_type(b) == Some("tool_use"));
    if has_tool_use {
        return content.clone();
    }

    // Partition into tool_result blocks and other blocks
    let (tool_results, others): (Vec<_>, Vec<_>) = arr
        .iter()
        .partition(|b| get_block_type(b) == Some("tool_result"));

    match (tool_results.len(), others.len()) {
        // No tool_result: create one wrapping all blocks
        (0, _) if !arr.is_empty() => {
            let inner_content = create_inner_content(arr);
            json!([create_tool_result(tool_use_id, inner_content)])
        }

        // Single tool_result, no siblings: keep as-is
        (1, 0) => content.clone(),

        // Single tool_result WITH siblings: merge siblings into its content
        (1, _) => {
            let tool_result = tool_results[0];
            let merged_content = merge_into_tool_result(tool_result, &others);
            json!([merged_content])
        }

        // Multiple tool_results: keep as-is (each is complete)
        (_, _) => content.clone(),
    }
}

/// Create inner content for tool_result from content blocks.
/// Single text block becomes string, single json/unknown becomes raw data, multiple become array.
fn create_inner_content(blocks: &[JsonValue]) -> JsonValue {
    // Track if single block was json/unknown for optimization
    let single_raw_data = if blocks.len() == 1 {
        match get_block_type(&blocks[0]) {
            Some("unknown") => blocks[0].get("raw").cloned(),
            Some("json") => blocks[0].get("data").cloned(),
            _ => None,
        }
    } else {
        None
    };

    // Extract actual content from wrapper types
    let inner: Vec<JsonValue> = blocks
        .iter()
        .filter_map(|block| {
            match get_block_type(block) {
                // For unknown/json, extract the raw data directly
                Some("unknown") => block.get("raw").cloned(),
                Some("json") => block.get("data").cloned(),
                _ => Some(block.clone()),
            }
        })
        .collect();

    // Single block optimizations for cleaner output
    if inner.len() == 1 {
        // Single text block: use string content
        if let Some(text) = inner[0].get("text").and_then(|t| t.as_str()) {
            return json!(text);
        }
        // Single json/unknown block: use raw data directly (tracked from original block type)
        if let Some(raw) = single_raw_data {
            return raw;
        }
    }

    json!(inner)
}

/// Merge sibling blocks into an existing tool_result's content.
fn merge_into_tool_result(tool_result: &JsonValue, siblings: &[&JsonValue]) -> JsonValue {
    let current_content = tool_result.get("content").cloned().unwrap_or(json!(null));

    // Convert current content to array
    let mut content_arr = match current_content {
        JsonValue::Array(arr) => arr,
        JsonValue::String(s) => vec![json!({"type": "text", "text": s})],
        JsonValue::Null => vec![],
        other => vec![json!({"type": "json", "data": other})],
    };

    // Append sibling blocks
    for block in siblings {
        content_arr.push((*block).clone());
    }

    // Recreate tool_result with merged content
    json!({
        "type": "tool_result",
        "tool_use_id": tool_result.get("tool_use_id").cloned(),
        "content": json!(content_arr),
        "is_error": tool_result.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false)
    })
}

/// Create a new tool_result block.
fn create_tool_result(tool_use_id: &Option<String>, content: JsonValue) -> JsonValue {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id.clone(),
        "content": content,
        "is_error": false
    })
}

// ========== Helper functions ==========

/// Parse data URL or regular URL, extracting media_type from data URLs.
///
/// Returns (source_type, data, media_type):
/// - For `#!B64!#mime::hash` file references: ("file", uri, Some("mime"))
/// - For `#!B64!#::hash` file references: ("file", uri, None)
/// - For data URLs: ("base64", base64_data, Some("image/png"))
/// - For regular URLs: ("url", url, None)
fn parse_data_url(url: &str) -> (&'static str, String, Option<String>) {
    // Handle #!B64!# file references (content-addressed storage)
    if let Some(parsed) = files::parse_file_uri(url) {
        return ("file", url.to_string(), parsed.media_type.map(String::from));
    }
    if url.starts_with("data:") {
        // Format: data:<media_type>;base64,<data>
        // Example: data:image/png;base64,ABC123...
        if let Some(comma_idx) = url.find(',') {
            let prefix = &url[5..comma_idx]; // Skip "data:"
            let media_type = prefix
                .split(';')
                .next()
                .filter(|s| !s.is_empty())
                .map(String::from);
            return ("base64", url[comma_idx + 1..].to_string(), media_type);
        }
    }
    ("url", url.to_string(), None)
}

/// Map MIME type to content block type.
fn mime_to_content_type(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime == "application/pdf" {
        "document"
    } else {
        "file"
    }
}

/// Build media type from format and prefix (e.g., "png" + "image" -> "image/png").
fn build_media_type(format: Option<&str>, prefix: &str) -> Option<String> {
    format.map(|f| format!("{}/{}", prefix, f))
}

/// Extract thinking text from various provider formats.
/// Handles: Anthropic (thinking), Mistral (thinking array), PydanticAI (content), Legacy (text)
fn extract_thinking_text(block: &JsonValue) -> String {
    // 1. Try "thinking" field (Anthropic, Mistral)
    if let Some(thinking) = block.get("thinking") {
        // Mistral nested array: {"thinking": [{"type": "text", "text": "..."}]}
        if let Some(arr) = thinking.as_array() {
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if !texts.is_empty() {
                return texts.join("\n\n");
            }
        }
        // Anthropic plain string
        if let Some(s) = thinking.as_str() {
            return s.to_string();
        }
    }

    // 2. Try "text" field (SideML format, some providers)
    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
        return text.to_string();
    }

    // 3. Try "content" field (PydanticAI format)
    if let Some(content) = block.get("content").and_then(|c| c.as_str()) {
        return content.to_string();
    }

    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_gemini_function_call_format() {
        let block = json!({
            "functionCall": {
                "name": "search",
                "args": {"query": "test"}
            }
        });

        let result = try_gemini_format(&block);
        assert!(result.is_some());
        let normalized = result.unwrap();

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
}
