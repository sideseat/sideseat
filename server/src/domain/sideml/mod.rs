//! SideML - Universal AI Message Format Normalizer
//!
//! A library for normalizing chat messages from various AI providers to a unified format.
//!
//! # Supported Providers
//!
//! - **OpenAI** (GPT-4, o1, etc.) - text, images, audio, tool calls, structured output
//! - **Anthropic** (Claude) - text, images, documents, tool use, thinking blocks
//! - **AWS Bedrock/Strands** - native format with toolUse/toolResult
//! - **Google Gemini** - inline_data, file_data, functionCall/Response
//! - **Other providers** using OpenAI-compatible formats (Mistral, Cohere, etc.)
//!
//! # Core Types
//!
//! - [`ChatMessage`] - Normalized message with role and content blocks
//! - [`ChatRole`] - Message role (system, user, assistant, tool)
//! - [`ContentBlock`] - Content block variants (text, image, audio, tool_use, tool_result, etc.)
//! - [`FinishReason`] - Completion reason (stop, length, tool_use, content_filter)
//!
//! # Feed Processing
//!
//! - [`FeedOptions`] - Filter options for message collections
//! - [`process_spans`] - Process raw span rows into normalized messages
//! - [`process_feed`] - Process spans with session grouping
//!
//! # Example
//!
//! ```
//! use sideseat_server::domain::sideml::{normalize, ChatRole, FinishReason};
//!
//! let raw = serde_json::json!({
//!     "role": "assistant",
//!     "content": "Hello!",
//!     "finish_reason": "end_turn"
//! });
//!
//! let message = normalize(&raw);
//! assert_eq!(message.role, ChatRole::Assistant);
//! assert_eq!(message.finish_reason, Some(FinishReason::Stop));
//! ```

// ============================================================================
// INTERNAL MODULES
// ============================================================================

pub(crate) mod content;
pub(crate) mod tools;
mod types;
mod unflatten;

// ============================================================================
// PUBLIC MODULES
// ============================================================================

/// Message normalization pipeline.
///
/// Converts raw OTEL messages to normalized SideML format with metadata.
pub mod normalize;

/// Feed pipeline for message processing.
///
/// Clean, single-responsibility modules:
/// - Parse raw messages to SideML format
/// - Merge complementary tool results
/// - Compute birth times for ordering
/// - Filter, deduplicate, sort, enrich
pub mod feed;

// ============================================================================
// PUBLIC API - Types
// ============================================================================

pub use types::{
    CacheControl, ChatMessage, ChatRole, ContentBlock, FinishReason, JsonSchemaDetails,
    ResponseFormat, ToolChoice,
};

pub use feed::{
    BlockEntry, ExtractedTools, FeedMetadata, FeedOptions, FeedResult, deduplicate_names,
    deduplicate_tools, extract_tools_from_rows, process_feed, process_spans,
};

pub use tools::extract_tool_name;

pub use normalize::{SideMLMessage, to_sideml, to_sideml_batch, to_sideml_with_context};

// ============================================================================
// PUBLIC API - Normalization Functions
// ============================================================================

use serde_json::{Value as JsonValue, json};

// Message-structure keys that indicate a value is a proper message wrapper,
// not plain structured output data.
const MESSAGE_STRUCTURE_KEYS: &[&str] = &[
    "role",
    "content",
    "contents",
    "message",
    "parts",
    "text",
    "object",
    "arguments",
    "tool_calls",
    "toolCalls",
    "finish_reason",
    "finishReason",
    "type",
    "generations",
    "choices",
    // Provider content-block keys
    "toolUse",
    "toolResult",
    "functionCall",
    "functionResponse",
    "inline_data",
    "reasoningContent",
];

/// Check if a JSON value is a plain data object (structured output).
///
/// Returns true for non-empty objects that lack any message-structure keys.
/// Used to detect structured output like `{"name": "Jane", "age": 28}` that
/// needs wrapping before normalization.
pub(crate) fn is_plain_data_value(val: &JsonValue) -> bool {
    let Some(obj) = val.as_object() else {
        return false;
    };
    if obj.is_empty() {
        return false;
    }
    !MESSAGE_STRUCTURE_KEYS
        .iter()
        .any(|key| obj.contains_key(*key))
}

/// Normalize a raw message to unified SideML format.
///
/// This is the main entry point for message normalization. It handles:
/// - Content blocks (text, images, tool_use, tool_result, etc.)
/// - Tool calls (nested OpenAI format -> flat format)
/// - Tool definitions (all providers -> OpenAI format)
/// - Role names (human -> user, ai -> assistant, etc.)
/// - Finish reasons (end_turn -> stop, etc.)
///
/// # Example
///
/// ```
/// use sideseat_server::domain::sideml::{normalize, ChatRole};
///
/// let raw = serde_json::json!({
///     "role": "user",
///     "content": [{"type": "text", "text": "Hello!"}]
/// });
/// let message = normalize(&raw);
/// assert_eq!(message.role, ChatRole::User);
/// ```
pub fn normalize(raw: &JsonValue) -> ChatMessage {
    // Unflatten dotted keys first (e.g., "tool_calls.0.function.name" -> nested)
    let raw = unflatten::unflatten_dotted_keys(raw);

    // Infer role: explicit role > tool_calls presence > default to user
    let role_str = raw.get("role").and_then(|r| r.as_str()).unwrap_or_else(|| {
        if raw.get("tool_calls").is_some() || raw.get("toolCalls").is_some() {
            "assistant"
        } else {
            "user"
        }
    });

    // Handle special "tools" role for tool definitions
    if ChatRole::is_tools_definition_role(role_str) {
        return normalize_tools_message(&raw);
    }

    // Handle "tool_call" role for tool invocations
    if role_str == "tool_call" {
        return normalize_tool_call_message(&raw);
    }

    // Handle "data" and "context" roles for conversation context
    if role_str == "data" || role_str == "context" {
        return normalize_context_message(&raw, role_str);
    }

    let role = ChatRole::from_str_normalized(role_str);
    let tool_use_id = tools::extract_tool_use_id(&raw, role_str);

    // Extract content from multiple possible fields (framework-specific)
    // Note: Sparse array placeholder filtering happens universally in normalize_content()
    let raw_content = raw
        .get("content") // Standard: OpenAI, Anthropic, most frameworks
        .or_else(|| raw.get("contents")) // OpenInference nested format (plural)
        .or_else(|| raw.get("message")) // Some frameworks wrap in message field
        .or_else(|| raw.get("parts")) // Gemini format
        .or_else(|| raw.get("text")) // Simple text-only format
        .or_else(|| raw.get("object")) // Structured output
        .or_else(|| raw.get("arguments")) // Tool call arguments
        // Defense-in-depth: if no known field matched but the whole value is
        // a plain data object (structured output), pass it to normalize_content
        .or_else(|| {
            if is_plain_data_value(&raw) {
                Some(&raw)
            } else {
                None
            }
        });
    let normalized_content = content::normalize_content(raw_content);
    let normalized_content =
        content::convert_to_tool_result(&normalized_content, role_str, &tool_use_id);

    // Convert content JsonValue array to Vec<ContentBlock>
    let content_json_vec: Vec<JsonValue> =
        normalized_content.as_array().cloned().unwrap_or_default();
    let mut content_vec: Vec<ContentBlock> = content_json_vec
        .into_iter()
        .filter_map(
            |v| match serde_json::from_value::<ContentBlock>(v.clone()) {
                Ok(block) => Some(block),
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        block = ?v,
                        "Failed to deserialize content block, dropping"
                    );
                    None
                }
            },
        )
        .collect();

    // Handle message-level refusal field (OpenAI)
    if let Some(refusal) = raw.get("refusal").and_then(|r| r.as_str())
        && !refusal.is_empty()
    {
        content_vec.push(ContentBlock::Refusal {
            message: refusal.to_string(),
        });
    }

    // Convert tool_calls to ContentBlock::ToolUse
    if let Some(tc_array) = tools::normalize_tool_calls(&raw).and_then(|tc| tc.as_array().cloned())
    {
        for tc in tc_array {
            if let Some(name) = tc.get("name").and_then(|n| n.as_str()) {
                let id = tc.get("id").and_then(|i| i.as_str()).map(String::from);
                let input = tc
                    .get("arguments")
                    .map(|a| {
                        if let Some(s) = a.as_str() {
                            serde_json::from_str(s).unwrap_or_else(|_| json!(s))
                        } else {
                            a.clone()
                        }
                    })
                    .unwrap_or(json!({}));

                content_vec.push(ContentBlock::ToolUse {
                    id,
                    name: name.to_string(),
                    input,
                });
            }
        }
    }

    // Extract citation/grounding metadata
    content_vec.extend(extract_citation_contexts(&raw));

    // API error extraction
    if let Some(error) = raw
        .get("error")
        .filter(|e| e.is_object() && has_meaningful_data(e))
    {
        content_vec.push(ContentBlock::Context {
            data: error.clone(),
            context_type: Some("api_error".to_string()),
        });
    }

    // Parse finish reason (snake_case or camelCase)
    let finish_reason = raw
        .get("finish_reason")
        .or_else(|| raw.get("finishReason"))
        .and_then(|fr| fr.as_str())
        .and_then(|fr_str| match FinishReason::from_str_normalized(fr_str) {
            Some(reason) => Some(reason),
            None => {
                tracing::trace!(
                    finish_reason = fr_str,
                    "Unknown finish_reason value, ignoring"
                );
                None
            }
        });

    let tool_choice = parse_tool_choice(&raw);
    let response_format = raw
        .get("response_format")
        .and_then(|rf| serde_json::from_value(rf.clone()).ok());
    let cache_control = raw
        .get("cache_control")
        .and_then(|cc| serde_json::from_value(cc.clone()).ok());
    let stop = raw
        .get("stop")
        .or_else(|| raw.get("stop_sequences"))
        .and_then(|s| {
            if let Some(arr) = s.as_array() {
                Some(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                )
            } else if let Some(s) = s.as_str() {
                Some(vec![s.to_string()])
            } else {
                None
            }
        });

    ChatMessage {
        role,
        name: raw.get("name").and_then(|n| n.as_str()).map(String::from),
        content: content_vec,
        tool_use_id,
        finish_reason,
        index: raw.get("index").and_then(|i| i.as_i64()).map(|i| i as i32),
        tool_choice,
        response_format,
        model: raw.get("model").and_then(|m| m.as_str()).map(String::from),
        cache_control,
        stop,
        parallel_tool_calls: raw.get("parallel_tool_calls").and_then(|p| p.as_bool()),
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

const CITATION_CONTEXT_FIELDS: &[(&str, &str)] = &[
    ("groundingMetadata", "grounding"),
    ("citationMetadata", "citations"),
    ("data_sources", "data_sources"),
    ("search_results", "search_results"),
    ("citations", "citations"),
    ("attributions", "attributions"),
];

fn has_meaningful_data(val: &JsonValue) -> bool {
    match val {
        JsonValue::Null => false,
        JsonValue::Bool(_) => false,
        JsonValue::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        JsonValue::String(s) => !s.trim().is_empty(),
        JsonValue::Array(arr) => arr.iter().any(has_meaningful_data),
        JsonValue::Object(obj) => obj.values().any(has_meaningful_data),
    }
}

fn extract_citation_contexts(raw: &JsonValue) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    for (field, context_type) in CITATION_CONTEXT_FIELDS {
        if let Some(data) = raw.get(*field).filter(|v| has_meaningful_data(v)) {
            blocks.push(ContentBlock::Context {
                data: data.clone(),
                context_type: Some(context_type.to_string()),
            });
        }
    }

    if let Some(context) = raw.get("context").filter(|v| v.is_object()) {
        if let Some(citations) = context.get("citations").filter(|v| has_meaningful_data(v)) {
            blocks.push(ContentBlock::Context {
                data: citations.clone(),
                context_type: Some("citations".to_string()),
            });
        }
        if let Some(obj) = context.as_object() {
            let other: serde_json::Map<String, JsonValue> = obj
                .iter()
                .filter(|(k, v)| *k != "citations" && has_meaningful_data(v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !other.is_empty() {
                blocks.push(ContentBlock::Context {
                    data: JsonValue::Object(other),
                    context_type: Some("azure_context".to_string()),
                });
            }
        }
    }

    blocks
}

fn parse_tool_choice(raw: &JsonValue) -> Option<ToolChoice> {
    let tc = raw.get("tool_choice")?;
    if let Some(s) = tc.as_str() {
        match s {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        }
    } else if tc.is_object() {
        tc.get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|name| ToolChoice::Function {
                name: name.to_string(),
            })
    } else {
        None
    }
}

// ============================================================================
// SPECIAL ROLE HANDLERS
// ============================================================================

fn normalize_tools_message(raw: &JsonValue) -> ChatMessage {
    let tools_content = raw.get("content").cloned();
    let tools_vec: Vec<JsonValue> = tools_content
        .map(|t| {
            let normalized = tools::normalize_tools(&t);
            normalized.as_array().cloned().unwrap_or_default()
        })
        .unwrap_or_default();

    let tool_choice_value = raw.get("tool_choice").cloned();
    let tool_choice = parse_tool_choice(raw);

    let content = vec![ContentBlock::ToolDefinitions {
        tools: tools_vec.clone(),
        tool_choice: tool_choice_value,
    }];

    ChatMessage {
        role: ChatRole::System,
        content,
        tool_choice,
        ..Default::default()
    }
}

fn normalize_tool_call_message(raw: &JsonValue) -> ChatMessage {
    let name = raw
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("unknown")
        .to_string();

    let tool_call_id = raw
        .get("tool_call_id")
        .and_then(|id| id.as_str())
        .map(String::from);

    let input = raw
        .get("content")
        .cloned()
        .unwrap_or(JsonValue::Object(serde_json::Map::new()));

    let content = vec![ContentBlock::ToolUse {
        id: tool_call_id,
        name,
        input,
    }];

    ChatMessage {
        role: ChatRole::Assistant,
        content,
        finish_reason: Some(FinishReason::ToolUse),
        ..Default::default()
    }
}

fn normalize_context_message(raw: &JsonValue, role_str: &str) -> ChatMessage {
    let context_type = raw
        .get("type")
        .and_then(|t| t.as_str())
        .map(String::from)
        .or_else(|| match role_str {
            "data" => Some("conversation_history".to_string()),
            "context" => Some("chat_context".to_string()),
            _ => None,
        });

    let data = raw.get("content").cloned().unwrap_or(JsonValue::Null);
    let content = vec![ContentBlock::Context { data, context_type }];

    ChatMessage {
        role: ChatRole::User,
        content,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
