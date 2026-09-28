//! Content-block normalization.
//!
//! Normalizes content blocks from various AI provider formats to unified SideML format.
//! Handles: OpenAI, Anthropic, AWS Bedrock/Strands, Google Gemini, and compatible providers.

use serde_json::{Value as JsonValue, json};

use super::types::ChatRole;
use sideseat_core::utils::file_uri as files;

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
                .filter(is_renderable_block)
                .collect();
            json!(blocks)
        }
        Some(obj @ JsonValue::Object(_)) => {
            // Single content block (skip if it's a sparse array placeholder)
            if is_sparse_array_placeholder(obj) {
                return json!([]);
            }
            match normalize_content_block(obj) {
                Some(block) if is_renderable_block(&block) => json!([block]),
                _ => json!([]),
            }
        }
        _ => json!([]),
    }
}

/// Whether a normalized block carries anything a reader could see.
///
/// Deliberately per block type rather than a blanket "has text" rule: an empty `tool_result`
/// still records that a tool ran and must survive, while an empty `thinking` block renders as
/// a blank reasoning bubble that is indistinguishable from a parsing failure.
///
/// The case this exists for: semconv 1.37 reasoning parts arrive as
/// `{"type":"reasoning","content":""}` from models that return summarised or encrypted
/// reasoning. With no text and no signature there is nothing to show and nothing to prove
/// reasoning happened - reasoning token counts come from `gen_ai.usage.*` attributes, not from
/// this block, so dropping it loses no accounting. A block that DOES carry a signature or
/// redacted payload is kept: that is replay state a multi-turn request needs.
fn is_renderable_block(block: &JsonValue) -> bool {
    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match block_type {
        "thinking" => {
            let has_text = block
                .get("text")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.trim().is_empty());
            let has_signature = block
                .get("signature")
                .and_then(|s| s.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            has_text || has_signature
        }
        "redacted_thinking" => block
            .get("data")
            .and_then(|d| d.as_str())
            .is_some_and(|d| !d.trim().is_empty()),
        // Everything else is kept as-is. An empty text block can be a legitimate model
        // response, and an empty tool_result records a completed invocation.
        _ => true,
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
    normalize_block(block, true)
}

/// The same chain **without** the message-envelope cases, for a value a tool returned.
///
/// An element of a tool's returned array is a returned value, exactly as a singleton returned object is - not a
/// message's content block. Sending one through the message chain consulted the envelope cases too, and a
/// returned `{"value": …}` then had its own member stripped: the same defect that made `MessageEnvelope` a
/// position of its own, one caller further along. It needs the *whole* rest of the chain, fallbacks included,
/// which is why this is the same function with one step withheld rather than the provider chain.
pub(crate) fn normalize_returned_value_block(block: &JsonValue) -> Option<JsonValue> {
    normalize_block(block, false)
}

fn normalize_block(block: &JsonValue, consult_envelopes: bool) -> Option<JsonValue> {
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
        // Declared shapes, at the two positions the chain's order makes load-bearing.
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::BeforeProviderFormats,
            )
        })
        // Envelopes around a *message's* content block, which the tool-result chains must not consult;
        // see `rules/vocabulary/content-blocks-wrappers.json`.
        .or_else(|| {
            consult_envelopes.then(|| {
                crate::rules::ruleset()
                    .content_blocks
                    .normalize(block, crate::rules::schema::ChainPosition::MessageEnvelope)
            })?
        })
        // Then try provider-specific formats
        .or_else(|| try_openai_format(block))
        .or_else(|| try_anthropic_format(block))
        .or_else(|| try_bedrock_format(block))
        .or_else(|| try_gemini_format(block))
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::AfterProviderFormats,
            )
        })
        // One dialect's blocks are declared at the position above; see `rules/vocabulary/content-blocks-vercel.json`.
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
    // Both declared positions, in the same order the full chain uses them. Consulting only the *after* one
    // meant a tool result written as a bare object skipped every `before_provider_formats` case, while the
    // same result inside an array ran the whole chain - so the first such declaration would have behaved
    // differently according to whether the producer wrapped it.
    crate::rules::ruleset()
        .content_blocks
        .normalize(
            block,
            crate::rules::schema::ChainPosition::BeforeProviderFormats,
        )
        .or_else(|| try_openai_format(block))
        .or_else(|| try_anthropic_format(block))
        .or_else(|| try_bedrock_format(block))
        .or_else(|| try_gemini_format(block))
        .or_else(|| {
            crate::rules::ruleset().content_blocks.normalize(
                block,
                crate::rules::schema::ChainPosition::AfterProviderFormats,
            )
        })
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
pub(crate) fn normalize_tool_result_content(content: Option<JsonValue>) -> JsonValue {
    match content {
        None => json!(null),
        Some(JsonValue::Array(arr)) => {
            // Each block with the source it came from: what makes two normalised blocks the *same datum*
            // is that the carrier wrote it twice in two encodings, and only the sources say so.
            // Envelope-free, as the singleton object below is: an element of a returned array is a returned
            // value, not a message's content block.
            let normalized: Vec<(JsonValue, JsonValue)> = arr
                .iter()
                .filter_map(|src| normalize_returned_value_block(src).map(|out| (out, src.clone())))
                .collect();
            if normalized.is_empty() {
                json!(null)
            } else {
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

/// Collapse blocks that are **one datum written twice**, and keep blocks that are two occurrences.
///
/// Vercel AI SDK (and possibly others) writes the same tool result in two encodings within one array:
/// - Raw structured data: `{status: "success", content: [...]}`
/// - Wrapped format: `{type: "json", value: {status: "success", content: [...]}}`
///
/// Normalisation makes those identical, which is what this collapses. What it must *not* collapse is a tool that
/// genuinely returned the same part twice: `[{"text": "retry"}, {"text": "retry"}]` is two ordered occurrences,
/// and a carrier's position is the evidence of that.
///
/// **The test is containment, not difference.** It used to be "the sources differ", and that does not hold: two
/// *genuine* occurrences can be written differently - `{"text":"retry"}` beside `{"type":"text","text":"retry"}`
/// is two positions a producer wrote, and the second was discarded because its JSON did not match the first's.
/// Different encodings are not evidence of one datum.
///
/// What *is* evidence is that one source **envelopes** the other: `{type:"json", value: X}` contains `X`, which
/// is the relation between a wrapping and the thing wrapped. Two independently written encodings of one value do
/// not contain each other, so the two cases separate without any declaration - and a producer that invents a new
/// wrapper is covered by the same relation rather than needing a new one.
fn deduplicate_content_blocks(blocks: Vec<(JsonValue, JsonValue)>) -> Vec<JsonValue> {
    use std::collections::HashMap;

    // normalised form -> the sources already kept for it.
    let mut seen: HashMap<String, Vec<JsonValue>> = HashMap::new();
    let mut result = Vec::with_capacity(blocks.len());

    for (block, source) in blocks {
        // JSON serialisation as identity (deterministic ordering from serde_json).
        let key = serde_json::to_string(&block).unwrap_or_default();
        let sources = seen.entry(key).or_default();
        // Dropped only where one source is an envelope of a source already kept, in either direction: the
        // wrapping may come first or second, and both orders are the same datum written twice.
        let one_datum = sources
            .iter()
            .any(|kept| envelopes(kept, &source) || envelopes(&source, kept));
        if !one_datum {
            sources.push(source);
            result.push(block);
        }
    }

    result
}

/// Whether `outer` is a wrapping **around** `inner`: one of its member values *is* `inner`.
///
/// Direct members only, which is what a wrapper is - `{type:"json", value: X}` states `X` at one level. A deep
/// search would call a tool result that happens to quote an earlier one an envelope of it, which is a different
/// claim entirely.
fn envelopes(outer: &JsonValue, inner: &JsonValue) -> bool {
    match outer {
        JsonValue::Object(members) => members.values().any(|value| value == inner),
        JsonValue::Array(items) => items.iter().any(|item| item == inner),
        _ => false,
    }
}

// ========== Provider-specific content format handlers ==========

/// Type-tagged format (OpenAI and compatible providers).
/// Handles: text, image_url, input_audio, audio, refusal, output_json, thinking, etc.
fn try_openai_format(block: &JsonValue) -> Option<JsonValue> {
    let block_type = block.get("type")?.as_str()?;
    match block_type {
        "text" | "input_text" | "output_text" => {
            // Standard: "text" field (OpenAI, Anthropic, etc.)
            // Agent Framework uses "content" field instead of "text"
            let text = block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|t| t.as_str())?;
            Some(json!({"type": "text", "text": text}))
        }
        // Agent Framework binary data: {"type": "data", "uri": "#!B64!#...", "media_type": "...", "filename"?: "..."}
        "data" => {
            let uri = block.get("uri").and_then(|u| u.as_str())?;
            let media_type = block.get("media_type").and_then(|m| m.as_str());
            let content_type = mime_to_content_type(media_type.unwrap_or(""));
            let mut result = json!({
                "type": content_type,
                "media_type": media_type,
                "source": if files::is_file_uri(uri) { "file" } else { "url" },
                "data": uri
            });
            if let Some(name) = block.get("filename").and_then(|n| n.as_str()) {
                result["name"] = json!(name);
            }
            Some(result)
        }
        // Agent Framework tool call: {"type": "tool_call", "id": "...", "name": "...", "arguments": {...}}
        // arguments may be a JSON-encoded string or an object
        "tool_call" => {
            let name = block.get("name").and_then(|n| n.as_str())?;
            let id = block.get("id").and_then(|i| i.as_str());
            let input = block
                .get("arguments")
                .map(|a| {
                    if let Some(s) = a.as_str() {
                        serde_json::from_str::<JsonValue>(s).unwrap_or_else(|_| json!(s))
                    } else {
                        a.clone()
                    }
                })
                .unwrap_or(json!({}));
            Some(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }))
        }
        // Agent Framework tool result: {"type": "tool_call_response", "id": "...", "response": "..."}
        // response is json.dumps(result) — may be a JSON string of an object, array, or plain string
        "tool_call_response" => {
            let id = block.get("id").and_then(|i| i.as_str());
            let response = block.get("response");
            let content = if let Some(s) = response.and_then(|r| r.as_str()) {
                match serde_json::from_str::<JsonValue>(s) {
                    // Object or array → render as json block
                    Ok(v @ (JsonValue::Object(_) | JsonValue::Array(_))) => {
                        json!([{"type": "json", "data": v}])
                    }
                    // JSON-encoded string (e.g. json.dumps("text result")) → unwrap to text
                    Ok(JsonValue::String(text)) => json!([{"type": "text", "text": text}]),
                    // Non-parseable or scalar → plain text
                    _ => json!([{"type": "text", "text": s}]),
                }
            } else if let Some(v) = response {
                json!([{"type": "json", "data": v}])
            } else {
                json!([])
            };
            Some(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": content,
                "is_error": false
            }))
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
        // semconv 1.37 reasoning part. Without this it fell through to the unknown
        // passthrough and rendered as {"type":"unknown"} instead of a thinking block.
        "reasoning" => {
            let text = block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|t| t.as_str())?;
            Some(json!({"type": "thinking", "text": text, "signature": null}))
        }
        // semconv 1.37 binary part: same shape as Agent Framework's "data", but the
        // payload lives under `data` rather than `uri`.
        "blob" => {
            let raw = block
                .get("data")
                .or_else(|| block.get("uri"))
                .and_then(|u| u.as_str())?;
            let media_type = block
                .get("media_type")
                .or_else(|| block.get("mime_type"))
                .and_then(|m| m.as_str())
                .unwrap_or("application/octet-stream");
            // parse_data_url returns the media type only when the payload carried one;
            // fall back to the block's own field.
            let (source, data, parsed_media) = parse_data_url(raw);
            Some(json!({
                "type": "document",
                "media_type": parsed_media.as_deref().unwrap_or(media_type),
                "source": source,
                "data": data
            }))
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
/// Also handles Strands JS SDK camelCase variants: {"type": "toolUse|toolResult", ...}
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
        // Strands JS SDK flat camelCase format: {"type": "toolUse", "toolUseId": "...", "name": "...", "input": {...}}
        "toolUse" => Some(json!({
            "type": "tool_use",
            "id": block.get("toolUseId"),
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
        // Strands JS SDK flat camelCase format: {"type": "toolResult", "toolUseId": "...", "content": [...], "status": "..."}
        "toolResult" => {
            let raw_content = block.get("content").cloned();
            let normalized_content = normalize_tool_result_content(raw_content);
            Some(json!({
                "type": "tool_result",
                "tool_use_id": block.get("toolUseId"),
                "content": normalized_content,
                "is_error": block.get("status").and_then(|s| s.as_str()) == Some("error")
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
        let normalized_content = normalize_tool_result_content(raw_content);
        // No `tool_use_id`: Gemini supplies none, and deriving one from the RESPONSE (as this
        // did) produced an id no call ever had, so every ADK tool result was permanently
        // dangling - correlating by it found nothing. The name is carried instead and the
        // feed's correlation stage ties the result to its call, which is the only place both
        // halves are visible at once.
        return Some(json!({
            "type": "tool_result",
            "name": name,
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
/// Retired: declared in `rules/vocabulary/content-blocks-vercel.json`, at the `after_provider_formats` position. Kept as
/// the equivalence oracle - `the_declared_dialect_blocks_match_the_reader_they_replace` runs both over every
/// form it recognised and every shape where it declined.
#[cfg(test)]
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
/// The retired list, for the equivalence oracle: the declared vocabulary must mean this and no more.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn provider_content_fields_legacy() -> &'static [&'static str] {
    PROVIDER_CONTENT_FIELDS
}

#[cfg(any(test, feature = "test-support"))]
const PROVIDER_CONTENT_FIELDS: &[&str] = &[
    // Bedrock/Strands
    "text",
    "image",
    "document",
    "video",
    "toolUse",
    "toolResult",
    "reasoningContent", // Bedrock extended thinking
    "reasoning",        // Strands TypeScript extended thinking
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
/// Retired: declared in `rules/vocabulary/content-blocks-wrappers.json`, at the `before_provider_formats` position. Kept
/// as the equivalence oracle - `the_declared_wrappers_match_the_reader_they_replace` runs both over every
/// wrapper it unwrapped and every shape where it declined.
#[cfg(test)]
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
    // Also handles Strands TypeScript "reasoning" key (same structure)
    for reasoning_key in &["reasoning_content", "reasoning"] {
        if let Some(inner) = block.get(*reasoning_key) {
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

    // A member that means "content block", declared in `rules/vocabulary/message-members.json`, on a block no case
    // recognised: malformed rather than plain data, and worth seeing as such. The policy is the only part left
    // here - which members say so is the producers' business.
    if crate::rules::ruleset()
        .message_members
        .any_means_content_block(obj.keys())
    {
        return Some(json!({"type": "unknown", "raw": block.clone()}));
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
    // The one decoder, so this and `data_source_kind` cannot disagree about what a value is - they used to,
    // in opposite directions.
    let (kind, media_type) = decode_media_source(url);
    let media_type = media_type.map(String::from);
    // A data URL's *payload* is what follows the comma; every other kind carries the value as it stands.
    let value = match kind {
        "base64" if url.starts_with("data:") => url
            .find(',')
            .map_or_else(|| url.to_string(), |comma| url[comma + 1..].to_string()),
        _ => url.to_string(),
    };
    (kind, value, media_type)
}

/// Map MIME type to content block type.
/// What a media member's value **is**, and the media type it carries where it carries one.
///
/// Derived from the value, not declared: a producer writes the same member either way, and which it is is a
/// fact about the value. One decoder, because there were two with opposite mistakes - `data_source_kind`
/// answered `base64` for anything that was not a file reference, so an ordinary `https://` URL was labelled as
/// inline bytes; `parse_data_url` answered `url` for anything that was not a file reference or a data URL, so
/// raw base64 was labelled a URL. Every caller now gets the same four-way answer.
///
/// Total by construction, with the residue stated: a value that names no scheme is the bytes themselves, which
/// is what a producer writing a media member without a URL means.
pub(crate) fn decode_media_source(data: &str) -> (&'static str, Option<&str>) {
    if let Some(parsed) = files::parse_file_uri(data) {
        return ("file", parsed.media_type);
    }
    if data.starts_with("data:")
        && let Some(comma) = data.find(',')
    {
        let media_type = data[5..comma]
            .split(';')
            .next()
            .filter(|part| !part.is_empty());
        return ("base64", media_type);
    }
    // A scheme means a fetch, not bytes. Checked as `scheme://` rather than by a list of schemes, because which
    // ones a producer uses is not this function's business.
    if data.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    }) {
        return ("url", None);
    }
    ("base64", None)
}

pub(crate) fn mime_to_content_type(mime: &str) -> &'static str {
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
#[path = "content_tests.rs"]
mod tests;

#[cfg(test)]
mod renderable_block_tests {
    use super::*;

    /// The captured shape that motivated the predicate: agent-framework via Bedrock's
    /// OpenAI-compatible endpoint emits a reasoning part with no text and no signature, which
    /// reached the feed as a blank reasoning bubble.
    #[test]
    fn empty_reasoning_part_is_dropped() {
        let content = json!([{"type": "reasoning", "content": ""}]);
        assert_eq!(normalize_content(Some(&content)), json!([]));
    }

    #[test]
    fn whitespace_only_reasoning_is_dropped_in_every_source_shape() {
        for shape in [
            json!([{"type": "reasoning", "content": "   "}]),
            json!([{"type": "reasoning", "text": "\n\t"}]),
            json!([{"type": "thinking", "text": ""}]),
            json!([{"thinking": ""}]),
        ] {
            let out = normalize_content(Some(&shape));
            let blocks = out.as_array().expect("array");
            assert!(
                !blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking")),
                "a blank thinking block survived for {shape}"
            );
        }
    }

    #[test]
    fn visible_reasoning_is_kept() {
        let content = json!([{"type": "reasoning", "content": "Let me check the units."}]);
        let out = normalize_content(Some(&content));
        assert_eq!(out[0]["type"], "thinking");
        assert_eq!(out[0]["text"], "Let me check the units.");
    }

    /// Signature-only reasoning is replay state a multi-turn request needs, so it must survive
    /// even with no visible text.
    #[test]
    fn signature_only_thinking_is_kept() {
        let content = json!([{"type": "thinking", "text": "", "signature": "abc123"}]);
        let out = normalize_content(Some(&content));
        assert_eq!(out.as_array().expect("array").len(), 1);
        assert_eq!(out[0]["type"], "thinking");
    }

    #[test]
    fn redacted_thinking_is_kept_when_it_carries_data() {
        assert!(is_renderable_block(
            &json!({"type": "redacted_thinking", "data": "opaque"})
        ));
        assert!(!is_renderable_block(
            &json!({"type": "redacted_thinking", "data": ""})
        ));
    }

    /// An empty tool result still records that the tool ran. A blanket "has text" rule would
    /// have removed it.
    #[test]
    fn empty_tool_result_is_kept() {
        assert!(is_renderable_block(
            &json!({"type": "tool_result", "tool_use_id": "1", "content": []})
        ));
    }

    /// An empty assistant text block is a legitimate response (e.g. a tool-only turn).
    #[test]
    fn empty_text_block_is_kept() {
        assert!(is_renderable_block(&json!({"type": "text", "text": ""})));
    }

    /// One decoder for what a media value **is**, where there were two with opposite mistakes.
    ///
    /// `data_source_kind` answered `base64` for anything that was not a file reference, so an ordinary
    /// `https://` URL was labelled as inline bytes. `parse_data_url` answered `url` for anything that was not a
    /// file reference or a data URL, so raw base64 was labelled a URL. Each was wrong about exactly what the
    /// other got right.
    #[test]
    fn a_media_value_is_decoded_the_same_way_by_every_caller() {
        // A stored reference, which also carries the media type the bytes were stored under.
        assert_eq!(
            decode_media_source("#!B64!#application/pdf::abc123"),
            ("file", Some("application/pdf"))
        );
        // A data URL: the payload is base64 and the prefix states its type.
        assert_eq!(
            decode_media_source("data:image/png;base64,AAAA"),
            ("base64", Some("image/png"))
        );
        // A URL is a fetch, not bytes - this is what was called `base64`.
        assert_eq!(
            decode_media_source("https://example.com/a.png"),
            ("url", None)
        );
        assert_eq!(decode_media_source("s3://bucket/key"), ("url", None));
        // And a bare value is the bytes themselves - this is what was called `url`.
        assert_eq!(decode_media_source("iVBORw0KGgoAAAANSU"), ("base64", None));
        // A scheme-looking prefix that is not one: `://` is the test, so a base64 payload containing a colon is
        // still bytes.
        assert_eq!(decode_media_source("abc:def"), ("base64", None));

        // `parse_data_url` agrees, and returns the data URL's *payload* rather than the whole URL.
        assert_eq!(
            parse_data_url("data:image/png;base64,AAAA"),
            ("base64", "AAAA".to_string(), Some("image/png".to_string()))
        );
        assert_eq!(
            parse_data_url("https://example.com/a.png"),
            ("url", "https://example.com/a.png".to_string(), None)
        );
        assert_eq!(
            parse_data_url("iVBORw0KGgoAAAANSU"),
            ("base64", "iVBORw0KGgoAAAANSU".to_string(), None)
        );
    }

    /// Two positions collapse only when one source **envelopes** the other.
    ///
    /// The test used to be "the sources differ", and that does not hold: two *genuine* occurrences can be written
    /// differently. `[{"text":"retry"}, {"type":"text","text":"retry"}]` contains two positions a
    /// producer wrote, where the second was discarded because its JSON did not match the first's. Different
    /// encodings are not evidence of one datum.
    ///
    /// What is evidence is containment: `{type:"json", value: X}` holds `X`, which is the relation between a
    /// wrapping and the thing wrapped. Two independently written encodings of one value do not contain each
    /// other, so the cases separate with no declaration - and a producer inventing a new wrapper is covered by
    /// the same relation.
    #[test]
    fn two_positions_collapse_only_when_one_envelopes_the_other() {
        let parts = |content: serde_json::Value| {
            normalize_tool_result_content(Some(content))
                .as_array()
                .map_or(0, Vec::len)
        };

        // Two encodings in two positions are both retained.
        assert_eq!(
            parts(json!([{"text": "retry"}, {"type": "text", "text": "retry"}])),
            2,
            "two differently-written positions are two occurrences - a producer wrote both"
        );
        // A genuine repeat, identically written: also two, which was already right.
        assert_eq!(
            parts(json!([{"text": "retry"}, {"text": "retry"}])),
            2,
            "and an identical repeat is still two"
        );

        // The envelope relation, in both orders - the wrapping may come first or second.
        let datum = json!({"status": "success", "rows": [1, 2]});
        assert_eq!(
            parts(json!([datum, {"type": "json", "value": datum}])),
            1,
            "a value beside its own wrapping is one datum written twice, which is what this exists for"
        );
        assert_eq!(
            parts(json!([{"type": "json", "value": datum}, datum])),
            1,
            "and the order does not matter"
        );

        // Two *different* data, each wrapped, stay two: containment is about one pair, not about the shape.
        assert_eq!(
            parts(json!([
                {"type": "json", "value": {"a": 1}},
                {"type": "json", "value": {"b": 2}}
            ])),
            2
        );
    }
}
