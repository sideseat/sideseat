//! SideML universal AI message-format normalizer.
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
//! use sideseat_domain::sideml::{normalize, ChatRole, FinishReason};
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

#[cfg(test)]
mod aliases_oracle;
pub mod carrier;
pub(crate) mod content;
pub(crate) mod provenance;
pub mod tools;
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
    BlockEntry, ExtractedTools, FeedMetadata, FeedOptions, FeedResult, RequestContextRows,
    apply_time_window, calls_a_thread_answers, deduplicate_names, deduplicate_tools,
    extract_tools_from_rows, process_feed, process_feed_cached, process_request_span, process_span,
    process_span_cached, process_spans, process_spans_cached,
};

pub use tools::extract_tool_name;

pub use normalize::{SideMLMessage, to_sideml, to_sideml_batch, to_sideml_with_context};

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    pub use super::provenance::PositionPath;

    pub fn message_structure_keys_legacy() -> &'static [&'static str] {
        super::message_structure_keys_legacy()
    }

    pub fn provider_content_fields_legacy() -> &'static [&'static str] {
        super::content::provider_content_fields_legacy()
    }

    pub fn normalize_content_block(block: &serde_json::Value) -> Option<serde_json::Value> {
        super::content::normalize_content_block(block)
    }

    /// Where the declared content chain and the retired Rust one answer a value differently, at any of the
    /// three ways into the chain: a message's content block, a value a tool returned, and a singleton result
    /// object. Compared as serialised text, so member order counts as it does in a stored result.
    ///
    /// The answer says whether the two differ only in **member order**: equal as JSON values, rendered
    /// differently. That is a difference a stored nested result would show, so it is reported, and kept apart
    /// so a caller can hold it to a stated list.
    pub fn content_chain_disagreement(value: &serde_json::Value) -> Option<(bool, String)> {
        let render = |answer: &Option<serde_json::Value>| {
            answer
                .as_ref()
                .map(|value| serde_json::to_string(value).expect("a value serialises"))
        };
        let pairs = [
            (
                "message content block",
                super::content::normalize_content_block(value),
                super::content::legacy_normalize_block(value, true),
            ),
            (
                "returned value",
                super::content::normalize_returned_value_block(value),
                super::content::legacy_normalize_block(value, false),
            ),
            (
                "singleton result",
                super::content::current_try_normalize_provider_format(value),
                super::content::legacy_try_normalize_provider_format(value),
            ),
        ];
        // Long strings shortened in the report only - a base64 payload would hide the member that differs.
        fn abbreviated(text: &str) -> String {
            let mut out = String::new();
            let mut in_string = false;
            let mut run = 0_usize;
            let mut escaped = false;
            for c in text.chars() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        in_string = false;
                        run = 0;
                    }
                    run += 1;
                    if run == 40 {
                        out.push('…');
                    }
                    if run >= 40 && c != '"' {
                        continue;
                    }
                } else if c == '"' {
                    in_string = true;
                    run = 0;
                }
                out.push(c);
            }
            out
        }
        pairs
            .into_iter()
            .find_map(|(path, declared_value, retired_value)| {
                let (declared, retired) = (render(&declared_value), render(&retired_value));
                (declared != retired).then(|| {
                    let order_only = declared_value == retired_value;
                    let report = format!(
                        "{path}:\n    declared {}\n    retired  {}\n    for      {}",
                        abbreviated(declared.as_deref().unwrap_or("nothing")),
                        abbreviated(retired.as_deref().unwrap_or("nothing")),
                        abbreviated(&value.to_string())
                    );
                    (order_only, report)
                })
            })
    }

    /// The Python-literal and constructor-repr readers the content chain applies, so a test oracle that
    /// searches raw telemetry decodes every rendering the engine can read.
    pub fn parse_python_rendering(text: &str) -> Option<serde_json::Value> {
        super::content::try_parse_python_repr(text)
            .or_else(|| super::content::try_parse_python_literal(text))
            .or_else(|| super::content::try_parse_python_constructor_repr(text))
    }
}

// ============================================================================
// PUBLIC API - Normalization Functions
// ============================================================================

use serde_json::{Value as JsonValue, json};

// Message-structure keys that indicate a value is a proper message wrapper,
// not plain structured output data.
/// The retired list, for the equivalence oracle: the declared vocabulary must mean this and no more.
#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(feature = "test-support", allow(dead_code))]
pub(crate) fn message_structure_keys_legacy() -> &'static [&'static str] {
    MESSAGE_STRUCTURE_KEYS
}

#[cfg(any(test, feature = "test-support"))]
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
/// Whether this value is bare data rather than something already message-shaped.
///
/// A generic carrier holds either. A **scalar** is bare data: `output.value = "the answer"` is the answer,
/// and unwrapped, normalisation looks for `role` on a string, finds nothing, and produces a message with no
/// blocks - the answer gone with no error anywhere. An **array** is bare data unless its elements carry a
/// `role`, which is what tells a list of *messages* (expanded upstream, so wrapping it would bury the
/// conversation) from a list of content blocks or plain values.
pub(crate) fn is_plain_data_or_content(value: &JsonValue) -> bool {
    let is_scalar = value.is_string() || value.is_number() || value.is_boolean();
    let is_content_list = value.as_array().is_some_and(|items| {
        !items.is_empty()
            && !items
                .iter()
                .any(|item| item.get("role").is_some() || item.get("messages").is_some())
    });
    is_scalar || is_content_list || is_plain_data_value(value)
}

#[doc(hidden)]
pub fn is_plain_data_value(val: &JsonValue) -> bool {
    let Some(obj) = val.as_object() else {
        return false;
    };
    if obj.is_empty() {
        return false;
    }
    // Declared: which members mean a value is message-shaped rather than bare data.
    !crate::rules::ruleset()
        .message_members
        .any_means_message_shaped(obj.keys())
}

/// Whether a value is bare data, as the retired key list decided it. The equivalence oracle.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn is_plain_data_value_legacy(val: &JsonValue) -> bool {
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
/// use sideseat_domain::sideml::{normalize, ChatRole};
///
/// let raw = serde_json::json!({
///     "role": "user",
///     "content": [{"type": "text", "text": "Hello!"}]
/// });
/// let message = normalize(&raw);
/// assert_eq!(message.role, ChatRole::User);
/// ```
/// The message inside a choice envelope, with the envelope's finish reason.
///
/// The conventions' `gen_ai.choice` event carries `index`, `finish_reason` and a `message` holding the
/// model's turn. Read as it stands, the message member was the content, and a whole message is not a content
/// block, so the answer rendered as raw JSON. Only an envelope that is not itself message-shaped is opened.
fn unwrap_choice_envelope(raw: JsonValue) -> JsonValue {
    // A role may already be on the envelope: the event's name implies one, and it is assigned before this. It is
    // no envelope if it holds content or tool calls itself - under any member the assets say holds them, other
    // than the `message` an envelope is made of.
    let members = &crate::rules::ruleset().message_members;
    let is_envelope = raw.as_object().is_none_or(|object| {
        !members
            .content_in_order()
            .filter(|member| *member != "message")
            .any(|member| object.contains_key(member))
            && !members.any_holds_tool_calls(object.keys())
    });
    let Some(inner) = raw
        .get("message")
        .and_then(JsonValue::as_object)
        .filter(|inner| inner.contains_key("content") || inner.contains_key("tool_calls"))
    else {
        return raw;
    };
    if !is_envelope {
        return raw;
    }
    let mut message = inner.clone();
    if let Some(reason) = raw.get("finish_reason")
        && !message.contains_key("finish_reason")
    {
        message.insert("finish_reason".to_string(), reason.clone());
    }
    JsonValue::Object(message)
}

pub fn normalize(raw: &JsonValue) -> ChatMessage {
    // Unflatten dotted keys first (e.g., "tool_calls.0.function.name" -> nested)
    let raw = unwrap_choice_envelope(unflatten::unflatten_dotted_keys(raw));

    // Infer role: explicit role > tool calls, under SideSeat's member or a declared spelling of it > user.
    let members = &crate::rules::ruleset().message_members;
    let role_str = raw.get("role").and_then(|r| r.as_str()).unwrap_or_else(|| {
        if std::iter::once("tool_calls")
            .chain(members.aliases_of("tool_calls"))
            .any(|member| raw.get(member).is_some())
        {
            "assistant"
        } else {
            "user"
        }
    });

    // `tool_call_id` names the call a message answers, and in this shape only an answer carries it. An
    // instrumentation that flattens Converse's tool results into the user turn they travel in keeps the
    // id but reports the turn's role, which read the result as something the user said.
    let role_str = if role_str == "user"
        && raw
            .get("tool_call_id")
            .and_then(JsonValue::as_str)
            .is_some_and(|id| !id.is_empty())
    {
        "tool"
    } else {
        role_str
    };

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

    // Which member holds the content is declared (`rules/vocabulary/message-members.json`), in order - the first the
    // value *has*, not the first that holds something: a `content` of `null` beside a populated `parts` is a
    // message whose content is null, which is what the retired chain said.
    // Sparse array placeholder filtering happens universally in `normalize_content`.
    let raw_content = crate::rules::ruleset()
        .message_members
        .content_in_order()
        .find_map(|member| raw.get(member))
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

    // Convert tool_calls to ContentBlock::ToolUse. A message may state one call twice - as a content
    // block and again in its call list, which is how a LangChain message serialises a provider's
    // `tool_use` content - and one call id within one message is one call, so a listed call whose id
    // the content already holds is not a second.
    if let Some(tc_array) = tools::normalize_tool_calls(&raw).and_then(|tc| tc.as_array().cloned())
    {
        for tc in tc_array {
            if let Some(name) = tc.get("name").and_then(|n| n.as_str()) {
                let id = tc.get("id").and_then(|i| i.as_str()).map(String::from);
                if id.as_deref().is_some_and(|id| {
                    content_vec.iter().any(|block| {
                        matches!(block, ContentBlock::ToolUse { id: Some(existing), .. } if existing == id)
                    })
                }) {
                    continue;
                }
                let input = tc
                    .get("arguments")
                    .map(|a| match a.as_str() {
                        Some(s) => match serde_json::from_str::<JsonValue>(s) {
                            // Arguments JSON-encoded twice - a JSON string whose text is the object - are
                            // that object: a serialiser that encodes an already encoded payload says
                            // nothing more by it.
                            Ok(JsonValue::String(inner)) => {
                                serde_json::from_str::<JsonValue>(&inner)
                                    .ok()
                                    .filter(|v| v.is_object() || v.is_array())
                                    .unwrap_or(JsonValue::String(inner))
                            }
                            Ok(parsed) => parsed,
                            Err(_) => json!(s),
                        },
                        None => a.clone(),
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

    remove_blank_text_beside_visible_content(&mut content_vec);

    // Parse finish reason, under SideSeat's member or a declared spelling of it.
    let finish_reason = std::iter::once("finish_reason")
        .chain(members.aliases_of("finish_reason"))
        .find_map(|member| raw.get(member))
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
    let stop = std::iter::once("stop")
        .chain(members.aliases_of("stop"))
        .find_map(|member| raw.get(member))
        .and_then(|s| {
            if let Some(arr) = s.as_array() {
                Some(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                )
            } else {
                s.as_str().map(|s| vec![s.to_string()])
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

fn remove_blank_text_beside_visible_content(content: &mut Vec<ContentBlock>) {
    let has_visible_sibling = content
        .iter()
        .any(|block| !matches!(block, ContentBlock::Text { text } if text.trim().is_empty()));
    if has_visible_sibling {
        content.retain(
            |block| !matches!(block, ContentBlock::Text { text } if text.trim().is_empty()),
        );
    }
}

#[cfg(test)]
fn has_meaningful_data_for_oracle(val: &JsonValue) -> bool {
    has_meaningful_data(val)
}

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

/// The context blocks a message carries beside its content - grounding, citations, sources - under the members
/// the assets declare (`holds_context`, `holds_context_parts`), in their declared order.
fn extract_citation_contexts(raw: &JsonValue) -> Vec<ContentBlock> {
    use crate::rules::members::ContextRead;
    let context = |data: JsonValue, kind: &str| ContentBlock::Context {
        data,
        context_type: Some(kind.to_string()),
    };
    let mut blocks = Vec::new();
    for (member, read) in crate::rules::ruleset().message_members.context() {
        match read {
            ContextRead::Whole(kind) => {
                if let Some(data) = raw.get(member).filter(|v| has_meaningful_data(v)) {
                    blocks.push(context(data.clone(), kind));
                }
            }
            ContextRead::Parts { parts, rest } => {
                let Some(object) = raw.get(member).and_then(JsonValue::as_object) else {
                    continue;
                };
                for (part, kind) in parts {
                    if let Some(data) = object.get(part).filter(|v| has_meaningful_data(v)) {
                        blocks.push(context(data.clone(), kind));
                    }
                }
                let other: serde_json::Map<String, JsonValue> = object
                    .iter()
                    .filter(|(k, v)| !parts.contains_key(*k) && has_meaningful_data(v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !other.is_empty() {
                    blocks.push(context(JsonValue::Object(other), rest));
                }
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
        tools: tools_vec,
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
