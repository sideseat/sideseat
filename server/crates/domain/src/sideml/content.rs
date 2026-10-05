//! Content-block normalization.
//!
//! Normalizes content blocks from various AI provider formats to unified SideML format.
//! Handles: OpenAI, Anthropic, AWS Bedrock/Strands, Google Gemini, and compatible providers.

use serde_json::{Value as JsonValue, json};

use super::types::ChatRole;
use sideseat_core::utils::file_uri as files;

mod canonical;
mod provider_formats;
mod python_repr;
mod tool_result;

use provider_formats::{
    try_anthropic_format, try_bedrock_format, try_gemini_format, try_openai_format,
};
#[cfg(test)]
use provider_formats::{try_gemini_function_format, try_vercel_format};
use python_repr::{try_normalize_python_constructor_content, try_parse_python_repr};
pub(crate) use python_repr::{try_parse_python_constructor_repr, try_parse_python_literal};
pub use tool_result::convert_to_tool_result;

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
pub(crate) fn compute_short_hash(value: &JsonValue) -> String {
    // Serialize to JSON string (deterministic ordering from serde_json)
    let json_str = serde_json::to_string(value).unwrap_or_default();
    let hash = fnv1a_hash(json_str.as_bytes());
    format!("{:08x}", hash)
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
            // A string that parses as a number, a boolean or null - a tool returning `395.0` - is the text
            // it was: decoding it left a value with no block to hold it, and the content vanished.
            if let Ok(
                parsed @ (JsonValue::Object(_) | JsonValue::Array(_) | JsonValue::String(_)),
            ) = serde_json::from_str::<JsonValue>(s)
            {
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

            let mut spliced = Vec::with_capacity(arr.len());
            splice_into(
                arr.iter()
                    .filter(|v| !should_filter_placeholders || !is_sparse_array_placeholder(v)),
                &mut spliced,
            );
            let blocks: Vec<JsonValue> = spliced
                .into_iter()
                .filter_map(normalize_content_block)
                .map(withheld_thinking_as_redacted)
                .filter(is_renderable_block)
                .collect();
            json!(blocks)
        }
        Some(obj @ JsonValue::Object(_)) => {
            // Single content block (skip if it's a sparse array placeholder)
            if is_sparse_array_placeholder(obj) {
                return json!([]);
            }
            if let Some(members) = crate::rules::ruleset().content_blocks.splice(obj) {
                return normalize_content(Some(&JsonValue::Array(members.clone())));
            }
            match normalize_content_block(obj).map(withheld_thinking_as_redacted) {
                Some(block) if is_renderable_block(&block) => json!([block]),
                _ => json!([]),
            }
        }
        Some(scalar @ (JsonValue::Number(_) | JsonValue::Bool(_))) => {
            json!([{"type": "text", "text": scalar.to_string()}])
        }
        _ => json!([]),
    }
}

/// A message's content blocks, with every block a declared splice recognises replaced by its members, in order.
///
/// Recursive, and bounded: a member is always strictly inside the block that held it.
fn splice_into<'b>(blocks: impl Iterator<Item = &'b JsonValue>, out: &mut Vec<&'b JsonValue>) {
    for block in blocks {
        match crate::rules::ruleset().content_blocks.splice(block) {
            Some(members) => splice_into(members.iter(), out),
            None => out.push(block),
        }
    }
}

/// A `thinking` block whose text was withheld but whose signature survived, as `redacted_thinking`.
///
/// Current Claude models think by default and omit the reasoning text unless the caller asks for a
/// summary, returning only the signature a later request replays. As a `thinking` block that renders
/// as an empty reasoning bubble, indistinguishable from a parsing failure. It is hidden reasoning, which
/// is exactly what `redacted_thinking` means; the signature becomes its opaque payload.
fn withheld_thinking_as_redacted(block: JsonValue) -> JsonValue {
    let is_withheld = block.get("type").and_then(JsonValue::as_str) == Some("thinking")
        && block
            .get("text")
            .and_then(JsonValue::as_str)
            .is_none_or(|text| text.trim().is_empty());
    let signature = block
        .get("signature")
        .and_then(JsonValue::as_str)
        .filter(|signature| !signature.trim().is_empty());
    match signature {
        Some(signature) if is_withheld => json!({"type": "redacted_thinking", "data": signature}),
        _ => block,
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

    // Rebuild text blocks rather than cloning them. A typed SideML text block has exactly its text; producer
    // metadata beside it is not part of the block and otherwise survives when the block is nested inside a
    // tool result, where no typed deserialisation strips unknown members.
    if block_type == "text" {
        let text = block.get("text")?.as_str()?;
        return Some(json!({"type": "text", "text": text}));
    }
    if block_type == "tool_result"
        && (block.get("tool_use_id").is_some() || block.get("content").is_some())
    {
        let mut result = serde_json::Map::new();
        result.insert("type".to_string(), json!("tool_result"));
        if let Some(id) = block.get("tool_use_id") {
            result.insert("tool_use_id".to_string(), id.clone());
        }
        if let Some(name) = block.get("name").and_then(JsonValue::as_str) {
            result.insert("name".to_string(), json!(name));
        }
        result.insert(
            "content".to_string(),
            canonical::normalize_tool_result_content(block.get("content")),
        );
        result.insert(
            "is_error".to_string(),
            json!(
                block
                    .get("is_error")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
            ),
        );
        return Some(JsonValue::Object(result));
    }

    // A streamed tool call accumulates its input as the JSON text the deltas carried. The block's input is
    // the structured value, so the text is decoded; left as a string, the same call read from a completed
    // response elsewhere on the span digests differently and survives as a second call.
    if block_type == "tool_use"
        && let (Some(name), Some(JsonValue::String(text))) = (block.get("name"), block.get("input"))
        && let Ok(input @ (JsonValue::Object(_) | JsonValue::Array(_))) =
            serde_json::from_str::<JsonValue>(text)
    {
        let mut call = serde_json::Map::new();
        call.insert("type".to_string(), json!("tool_use"));
        if let Some(id) = block.get("id") {
            call.insert("id".to_string(), id.clone());
        }
        call.insert("name".to_string(), name.clone());
        call.insert("input".to_string(), input);
        return Some(JsonValue::Object(call));
    }

    // Check structure based on type
    let is_valid_sideml = match block_type {
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
mod renderable_block_tests;
