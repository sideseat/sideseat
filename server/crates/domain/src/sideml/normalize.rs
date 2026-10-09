//! SideML trace-pipeline integration.
//!
//! Converts raw OTEL messages to normalized SideML format with metadata.
//! This module integrates the SideML library with the application's trace
//! processing pipeline.

mod categorization;
#[cfg(test)]
mod expansion_oracle;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use super::provenance::PositionPath;
use super::{ChatMessage, ChatRole, ContentBlock, normalize};
use crate::observations::{MessageSource, RawMessage};
use sideseat_ports::types::{MessageCategory, MessageSourceType};

#[cfg(test)]
pub(super) use categorization::categorize_tool_message;
use categorization::determine_category;
#[cfg(test)]
pub(crate) use categorization::role_from_event_name_with_context_legacy;
#[cfg(test)]
use categorization::{categorize_attribute_message, is_llm_output_event, is_special_role};

// ============================================================================
// SIDEML MESSAGE (PIPELINE OUTPUT)
// ============================================================================

/// A message with pipeline context (category, source, timestamp).
///
/// This type combines a normalized [`ChatMessage`] with application-specific
/// metadata for storage and processing in the trace pipeline.
/// A raw message with the path it occupies in its stored payload.
///
/// A pair rather than a field on `RawMessage`: that type is the *stored* shape, and the path is
/// derived at query time, so it has no business being serialised alongside it.
type Observed = (RawMessage, PositionPath);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideMLMessage {
    /// Where this message sat in the payload it came from - see [`PositionPath`].
    ///
    /// Carried rather than re-derived: expansion and tool splitting turn one stored payload into
    /// several messages, and without this the only thing left to tell two of them apart is their
    /// content, which is identical whenever a model asks for the same thing twice.
    ///
    /// **Not serialised.** It is derived at query time from a payload the caller cannot see, and the comment on
    /// `Observed` above already says it has no business travelling with a stored shape - `skip` makes that
    /// true rather than merely intended, and is what let `PositionPath` drop serde entirely.
    #[serde(skip)]
    pub position: PositionPath,
    /// The message source (event or attribute)
    pub source: MessageSource,
    /// Message category (user, assistant, tool, etc.)
    pub category: MessageCategory,
    /// Source type (Event or Attribute)
    pub source_type: MessageSourceType,
    /// Message timestamp
    pub timestamp: DateTime<Utc>,
    /// The normalized SideML message
    pub sideml: ChatMessage,
    /// The stored message was a rendering (`RawMessage::rendering`); not serialised, like `position`.
    #[serde(skip)]
    pub rendering: bool,
    /// The side of the span its reading declared, where it declared one (`RawMessage::direction`); not
    /// serialised, like `rendering`.
    #[serde(skip)]
    pub direction: Option<crate::rules::schema::ReadingDirection>,
}

// ============================================================================
// CONVERSION FUNCTIONS
// ============================================================================

/// Convert raw messages to SideML format with pipeline metadata.
///
/// Normalizes raw messages to unified SideML format with category detection.
/// Also correlates tool results with their tool calls to set the tool name.
///
/// Bundled tool results (multiple toolResult objects in one message) are split
/// into separate messages at query time for proper deduplication. This ensures
/// fixes apply to historical data without re-ingestion.
///
/// For backward compatibility, assumes non-tool span context.
/// Use `to_sideml_with_context` for full control over span context.
pub fn to_sideml(raw_messages: &[RawMessage]) -> Vec<SideMLMessage> {
    to_sideml_with_context(raw_messages, false)
}

/// Convert raw messages to SideML format with span context.
///
/// The `is_tool_span` parameter enables query-time role derivation:
/// - In tool spans: `gen_ai.choice` → tool output, `gen_ai.tool.message` → tool input
/// - In chat spans: `gen_ai.choice` → assistant response, `gen_ai.tool.message` → tool result
///
/// This approach allows role derivation fixes to apply to historical data without re-ingestion.
pub fn to_sideml_with_context(
    raw_messages: &[RawMessage],
    is_tool_span: bool,
) -> Vec<SideMLMessage> {
    // Pre-process: split bundled tool results into separate messages
    let expanded = expand_bundled_tool_results(raw_messages);

    // First pass: normalize all messages and build tool_use_id -> name map
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut messages: Vec<SideMLMessage> = Vec::with_capacity(expanded.len());

    for (raw, position) in &expanded {
        // Derive role from event name at query time, considering span context
        let content_with_role = derive_role_from_source_with_context(raw, is_tool_span);

        // Normalize to SideML format
        let sideml = normalize(&content_with_role);

        // Collect tool_use_id -> name mappings from tool_use content blocks
        for block in &sideml.content {
            if let ContentBlock::ToolUse {
                id: Some(id), name, ..
            } = block
            {
                tool_names.insert(id.clone(), name.clone());
            }
        }

        // Determine category based on source and derived role
        // IMPORTANT: Use content_with_role (not raw.content) to see the derived role
        let category = determine_category(&raw.source, &content_with_role);

        // Determine source type and time
        let (source_type, timestamp) = match &raw.source {
            MessageSource::Event { time, .. } => (MessageSourceType::Event, *time),
            MessageSource::Attribute { time, .. } => (MessageSourceType::Attribute, *time),
        };

        messages.push(SideMLMessage {
            position: position.clone(),
            source: raw.source.clone(),
            category,
            source_type,
            timestamp,
            sideml,
            rendering: raw.rendering,
            direction: raw.direction,
        });
    }

    // Second pass: flatten bundled tool messages into individual messages
    // This ensures each message has at most one tool ID, simplifying deduplication.
    // IMPORTANT: Must happen BEFORE name enrichment so flattened messages get enriched.
    let mut messages = flatten_tool_blocks(messages);

    // Third pass: enrich tool role messages with tool name from their tool_use_id
    for msg in &mut messages {
        if msg.sideml.role == ChatRole::Tool
            && msg.sideml.name.is_none()
            && let Some(tool_use_id) = &msg.sideml.tool_use_id
            && let Some(name) = tool_names.get(tool_use_id)
        {
            msg.sideml.name = Some(name.clone());
        }
    }

    messages
}

/// Convert a batch of raw messages to SideML format.
///
/// Takes a reference to avoid cloning the raw messages.
pub fn to_sideml_batch(raw_messages: &[Vec<RawMessage>]) -> Vec<Vec<SideMLMessage>> {
    raw_messages
        .iter()
        .map(|messages| to_sideml(messages))
        .collect()
}

// ============================================================================
// MESSAGE EXPANSION (Query-Time)
// ============================================================================

/// Whether this carrier holds an array of messages that expands into one observation each.
///
/// Declared per key rather than per family, because `ai.prompt` is a request while `ai.prompt.messages` is
/// the array inside it, and expanding the wrong one turns one message into fragments. Not every array is
/// expandable: a content-block list, a tool manifest and a context array all stay whole.
///
/// There is no residue list behind it: every key the retired one named is in an asset, and a list that
/// outlived its entries is a second answer waiting to disagree with the first.
fn is_expandable_message_array_source(source_name: &str) -> bool {
    crate::sideml::carrier::holds_expandable_message_array(source_name)
}

/// Expand bundled messages into individual messages at query time.
///
/// This handles two types of expansion:
///
/// 1. **Message arrays**: Arrays of messages from known sources are expanded
///    into individual RawMessages. See `MESSAGE_ARRAY_SOURCES` for the list.
///
/// 2. **Bundled tool results** (`gen_ai.tool.result`, role="tool"):
///    Multiple `toolResult` objects in a single message are split so each
///    can be properly deduplicated by its `tool_use_id`.
///
/// This happens at query time (not ingestion) for ingestion-independence:
/// fixes apply to historical data without re-ingestion.
fn expand_bundled_tool_results(raw_messages: &[RawMessage]) -> Vec<Observed> {
    let mut result = Vec::with_capacity(raw_messages.len());

    for (index, raw) in raw_messages.iter().enumerate() {
        let path = PositionPath::root(index);
        // Check source to determine expansion type
        let source_name = match &raw.source {
            MessageSource::Event { name, .. } => Some(name.as_str()),
            MessageSource::Attribute { key, .. } => Some(key.as_str()),
        };

        // Handle message array expansion from known sources
        if source_name.is_some_and(is_expandable_message_array_source) {
            expand_message_array(&mut result, raw, &path);
            continue;
        }

        // Handle bundled tool results
        let role = raw.content.get("role").and_then(|r| r.as_str());
        let is_tool_result_event = source_name == Some("gen_ai.tool.result");

        if (role == Some("tool") || is_tool_result_event)
            && expand_bundled_tool_result(&mut result, raw, &path)
        {
            continue;
        }

        // No expansion needed - keep as-is
        result.push((raw.clone(), path));
    }

    result
}

/// Expand a message array into individual RawMessages.
///
/// If content is an array of messages (with role/content), expands each.
/// If content is a single message object, keeps as-is.
///
/// Supports multiple nesting formats:
/// - Top-level array: `[{role, content}, ...]`
/// - Nested in "content": `{content: [{role, content}, ...]}`
/// - Nested in "messages": `{messages: [{role, content}, ...]}`
/// - Nested in "message": `{message: {role, content}, ...}` (singular — unwrapped)
/// - A streamed reply's combined text, under a member declared `holds_streamed_reply` (synthesised as an
///   assistant message)
fn expand_message_array(result: &mut Vec<Observed>, raw: &RawMessage, path: &PositionPath) {
    // Find an array to expand. Only consider nested "content"/"messages" fields
    // if they ARE arrays. A string "content" field is the message's actual content,
    // not a nested message array.
    // The member the array came from is part of the path, so an observation can say it was the third
    // entry of `messages` rather than merely "the third thing here".
    let (array_to_expand, array_key): (Option<&Vec<JsonValue>>, Option<&str>) =
        if let Some(direct) = raw.content.as_array() {
            (Some(direct), None)
        } else if let Some(nested) = raw.content.get("content").and_then(|c| c.as_array()) {
            (Some(nested), Some("content"))
        } else if let Some(messages) = raw.content.get("messages").and_then(|m| m.as_array()) {
            (Some(messages), Some("messages"))
        } else {
            (None, None)
        };
    let array_path = match array_key {
        Some(key) => path.child_key(key),
        None => path.clone(),
    };

    let Some(arr) = array_to_expand else {
        // Single nested message: {message: {role, content, ...}, usage: ...}
        if let Some(msg) = raw
            .content
            .get("message")
            .filter(|m| is_message_like_object(m))
        {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: msg.clone(),
                    rendering: raw.rendering,
                    direction: raw.direction,
                },
                path.child_key("message"),
            ));
        // A streamed reply's combined text, under a member the assets declare.
        } else if let Some((member, text)) = crate::rules::ruleset()
            .message_members
            .streamed_reply()
            .find_map(|member| {
                raw.content
                    .get(member)
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|text| (member, text))
            })
        {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: json!({"role": "assistant", "content": text}),
                    rendering: raw.rendering,
                    direction: raw.direction,
                },
                path.child_key(member),
            ));
        } else {
            // Not an expandable structure — keep as-is
            result.push((raw.clone(), path.clone()));
        }
        return;
    };

    // Check if this array contains message-like objects
    let has_message_like_items = arr.iter().any(is_message_like_object);

    if !has_message_like_items {
        // Array doesn't contain messages (e.g., content blocks array)
        result.push((raw.clone(), path.clone()));
        return;
    }

    // A system prompt a request carries beside its message array rather than in it, under a member the assets
    // declare: synthesised as the system message it is, before the array's own messages.
    if let Some((member, system)) = crate::rules::ruleset()
        .message_members
        .detached_system()
        .find_map(|member| raw.content.get(member).map(|value| (member, value)))
    {
        let system_text = if let Some(s) = system.as_str() {
            // Simple string: "system": "You are helpful."
            Some(s.to_string())
        } else if let Some(arr) = system.as_array() {
            // Array of blocks: "system": [{"type": "text", "text": "..."}]
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        } else {
            None
        };
        if let Some(text) = system_text {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: json!({"role": "system", "content": text}),
                    rendering: raw.rendering,
                    direction: raw.direction,
                },
                path.child_key(member),
            ));
        }
    }

    // `opentelemetry-util-genai` represents a streamed Gemini answer as one output message per
    // chunk. That is transport framing, not a conversation with the assistant taking several
    // turns. It is distinguishable from multiple candidates: every intermediate chunk is
    // text-only and unfinished, while the final chunk alone carries a finish reason.
    //
    // A span covering several model responses - a client that runs the tool loop itself streams
    // one response per round - holds several such streams back to back, each ended by the one
    // chunk with a finish reason. Each stream is combined on its own; finished messages between
    // them, such as a round's function call, stay as they are.
    //
    // Do this at query time so captures already stored with that shape are repaired too. Keep
    // the first item's position because the combined observation starts where the stream starts.
    let source_name = match &raw.source {
        MessageSource::Event { name, .. } => name.as_str(),
        MessageSource::Attribute { key, .. } => key.as_str(),
    };
    if source_name == "gen_ai.output.messages" {
        let runs = streamed_runs(arr);
        if runs.iter().any(|run| run.combined.is_some()) {
            for run in runs {
                match run.combined {
                    Some(combined) => result.push((
                        RawMessage {
                            source: raw.source.clone(),
                            content: combined,
                            rendering: raw.rendering,
                            direction: raw.direction,
                        },
                        array_path.child_index(run.start),
                    )),
                    None => {
                        let messages = arr.iter().enumerate().take(run.end).skip(run.start);
                        for (position, message) in messages {
                            if is_message_like_object(message) {
                                result.push((
                                    RawMessage {
                                        source: raw.source.clone(),
                                        content: message.clone(),
                                        rendering: raw.rendering,
                                        direction: raw.direction,
                                    },
                                    array_path.child_index(position),
                                ));
                            }
                        }
                    }
                }
            }
            return;
        }
    }

    // Expand array into individual messages
    let mut expanded_count = 0;
    let mut skipped_count = 0;

    for (position, item) in arr.iter().enumerate() {
        if is_message_like_object(item) {
            result.push((
                RawMessage {
                    source: raw.source.clone(),
                    content: item.clone(),
                    rendering: raw.rendering,
                    direction: raw.direction,
                },
                array_path.child_index(position),
            ));
            expanded_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    if skipped_count > 0 {
        tracing::trace!(
            expanded = expanded_count,
            skipped = skipped_count,
            "Expanded message array with some non-message items skipped"
        );
    }
}

/// One response of an output array: the messages `start..end`, the last of which is the first to
/// carry a finish reason (or the array's end), and their combination when they are chunk framing.
struct StreamedRun {
    start: usize,
    end: usize,
    combined: Option<JsonValue>,
}

fn streamed_runs(messages: &[JsonValue]) -> Vec<StreamedRun> {
    let mut runs = Vec::new();
    let mut start = 0;
    for (index, message) in messages.iter().enumerate() {
        let finished = message
            .get("finish_reason")
            .and_then(JsonValue::as_str)
            .is_some_and(|finish| !finish.is_empty());
        if finished || index + 1 == messages.len() {
            let end = index + 1;
            runs.push(StreamedRun {
                start,
                end,
                combined: coalesce_streamed_output_messages(&messages[start..end]),
            });
            start = end;
        }
    }
    runs
}

/// Combine a text-only output stream into the single assistant turn it represents.
///
/// Returns `None` unless the shape proves this is chunk framing:
/// - at least two assistant messages;
/// - every part is text;
/// - all but the final message are unfinished;
/// - the final message has a non-empty finish reason.
///
/// Multiple candidates therefore stay separate, as does any multimodal or tool-call stream whose
/// concatenation would require provider-specific semantics.
fn coalesce_streamed_output_messages(messages: &[JsonValue]) -> Option<JsonValue> {
    if messages.len() < 2 {
        return None;
    }

    let mut text = String::new();
    for (index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(JsonValue::as_str) != Some("assistant") {
            return None;
        }

        let parts = message.get("parts").and_then(JsonValue::as_array)?;
        if parts.is_empty() {
            return None;
        }
        for part in parts {
            if part.get("type").and_then(JsonValue::as_str) != Some("text") {
                return None;
            }
            text.push_str(part.get("content").and_then(JsonValue::as_str)?);
        }

        let finish = message
            .get("finish_reason")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let is_final = index + 1 == messages.len();
        if (is_final && finish.is_empty()) || (!is_final && !finish.is_empty()) {
            return None;
        }
    }

    let mut combined = messages.last()?.clone();
    combined["parts"] = json!([{"content": text, "type": "text"}]);
    Some(combined)
}

/// Check if a JSON value looks like a message object.
///
/// Message-like objects have:
/// - `role` field (OpenAI, Anthropic, Vercel AI, etc.)
/// - OR `parts` field (Gemini format)
///
/// Note: We don't check for just `content` because that's too broad -
/// content blocks also have `content` but aren't messages.
fn is_message_like_object(value: &JsonValue) -> bool {
    value.is_object()
        && (value.get("role").is_some() || value.get("parts").and_then(|p| p.as_array()).is_some())
}

/// Expand bundled tool results into separate messages.
///
/// An item of the content list - nested under `content`, or the list itself - is one result when it holds a
/// member declared `holds_bundled_tool_result`, and its call id is under one declared `holds_result_call_id`.
///
/// Returns true if expansion occurred, false otherwise.
fn expand_bundled_tool_result(
    result: &mut Vec<Observed>,
    raw: &RawMessage,
    path: &PositionPath,
) -> bool {
    // Check if content is nested in "content" field or is a direct array
    let (content_array, is_nested) =
        if let Some(nested) = raw.content.get("content").and_then(|c| c.as_array()) {
            (nested, true)
        } else if let Some(direct) = raw.content.as_array() {
            (direct, false)
        } else {
            return false;
        };

    let members = &crate::rules::ruleset().message_members;
    fn result_of<'v>(
        members: &crate::rules::members::MemberPlan,
        item: &'v JsonValue,
    ) -> Option<&'v JsonValue> {
        members
            .bundled_tool_result()
            .find_map(|member| item.get(member))
    }
    let call_id_of = |result: &JsonValue| -> Option<String> {
        members
            .result_call_id_in_order()
            .find_map(|member| result.get(member))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let tool_results: Vec<&JsonValue> = content_array
        .iter()
        .filter(|item| result_of(members, item).is_some())
        .collect();

    // Not bundled or only one toolResult - don't expand
    if tool_results.len() <= 1 {
        return false;
    }

    tracing::trace!(
        count = tool_results.len(),
        "Expanding bundled tool results into separate messages"
    );

    // Split bundled tool results into separate messages. Each result keeps the position it held in
    // the bundle, so two results of two identical calls are two observations rather than one.
    let bundle_path = if is_nested {
        path.child_key("content")
    } else {
        path.clone()
    };
    for (position, &item) in tool_results.iter().enumerate() {
        let tr = result_of(members, item);

        // Create new content structure
        let new_content = if is_nested {
            // Original had nested "content" field - preserve structure
            let mut new_obj = raw.content.clone();
            new_obj["content"] = json!([item.clone()]);
            // The call id of this one result.
            if let Some(id) = tr.and_then(call_id_of) {
                new_obj["tool_call_id"] = json!(id);
            }
            new_obj
        } else {
            // Original was a direct array - wrap in object
            let mut new_obj = json!({
                "role": "tool",
                "content": [item.clone()]
            });
            if let Some(id) = tr.and_then(call_id_of) {
                new_obj["tool_call_id"] = json!(id);
            }
            new_obj
        };

        result.push((
            RawMessage {
                source: raw.source.clone(),
                content: new_content,
                rendering: raw.rendering,
                direction: raw.direction,
            },
            bundle_path.child_index(position),
        ));
    }

    true
}

// ============================================================================
// TOOL BLOCK FLATTENING (Query-Time)
// ============================================================================

/// Flatten messages with multiple tool calls/results into individual messages.
///
/// This ensures each message has at most one tool ID, simplifying deduplication.
/// The transformation is:
///
/// - `[ToolUse(A), ToolUse(B), Text]` → `[ToolUse(A)]`, `[ToolUse(B)]`, `[Text]`
/// - `[ToolResult(A), ToolResult(B)]` → `[ToolResult(A)]`, `[ToolResult(B)]`
///
/// Non-tool content (Text, Image, etc.) is grouped into a separate message.
/// Messages with 0-1 tool blocks pass through unchanged.
///
/// This happens at query time for ingestion-independence: fixes apply to
/// historical data without re-ingestion.
fn flatten_tool_blocks(messages: Vec<SideMLMessage>) -> Vec<SideMLMessage> {
    let mut result = Vec::with_capacity(messages.len() * 2);

    for msg in messages {
        // Single-pass count of tool blocks
        let (tool_use_count, tool_result_count) =
            msg.sideml
                .content
                .iter()
                .fold((0, 0), |(uses, results), b| match b {
                    ContentBlock::ToolUse { .. } => (uses + 1, results),
                    ContentBlock::ToolResult { .. } => (uses, results + 1),
                    _ => (uses, results),
                });

        // One tool block at most, or there is nothing to split. Counted **together**: as two separate
        // comparisons, `[ToolUse, ToolResult]` passed through with two tool ids on one message, which is
        // precisely what this function promises never to emit.
        if tool_use_count + tool_result_count <= 1 {
            result.push(msg);
            continue;
        }

        // Split into individual tool messages, preserving relative order.
        // Non-tool blocks are grouped and emitted at their first occurrence position.
        let mut non_tool_blocks = Vec::new();
        // Where the accumulated group starts, which is the position it is emitted at. A group used to take
        // the *parent's* position, so a response shaped `[Text, ToolUse, ToolUse, Text]` produced two groups
        // with the same path - and flatten restarts its block index per message, so both texts landed on
        // `parent.0`. Identical text in one atomic emission is then one identity at one ordinal, and dedup
        // drops the second: content loss, in the one place position exists to prevent it.
        let mut non_tool_start: Option<usize> = None;

        // Enumerated: a split tool message takes the position of the block it was made from, which is
        // what tells two identical calls of one response apart once each is its own message.
        for (block_position, block) in msg.sideml.content.iter().enumerate() {
            match block {
                ContentBlock::ToolUse { .. } => {
                    // Emit accumulated non-tool blocks before this tool block
                    if !non_tool_blocks.is_empty() {
                        emit_non_tool_message(
                            &mut result,
                            &msg,
                            &mut non_tool_blocks,
                            non_tool_start.take(),
                        );
                    }
                    // Create individual message for this tool use
                    let new_sideml = ChatMessage {
                        role: msg.sideml.role,
                        content: vec![block.clone()],
                        tool_use_id: msg.sideml.tool_use_id.clone(),
                        ..Default::default()
                    };
                    result.push(SideMLMessage {
                        position: msg.position.child_index(block_position),
                        source: msg.source.clone(),
                        category: msg.category,
                        source_type: msg.source_type,
                        timestamp: msg.timestamp,
                        sideml: new_sideml,
                        rendering: msg.rendering,
                        direction: msg.direction,
                    });
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    // Emit accumulated non-tool blocks before this tool block
                    if !non_tool_blocks.is_empty() {
                        emit_non_tool_message(
                            &mut result,
                            &msg,
                            &mut non_tool_blocks,
                            non_tool_start.take(),
                        );
                    }
                    // Create individual message for this tool result
                    // Use block's tool_use_id if available, else message-level
                    let block_id = tool_use_id
                        .clone()
                        .or_else(|| msg.sideml.tool_use_id.clone());
                    let new_sideml = ChatMessage {
                        role: msg.sideml.role,
                        content: vec![block.clone()],
                        tool_use_id: block_id,
                        ..Default::default()
                    };
                    result.push(SideMLMessage {
                        position: msg.position.child_index(block_position),
                        source: msg.source.clone(),
                        category: msg.category,
                        source_type: msg.source_type,
                        timestamp: msg.timestamp,
                        sideml: new_sideml,
                        rendering: msg.rendering,
                        direction: msg.direction,
                    });
                }
                _ => {
                    // Collect non-tool blocks, remembering where this group began.
                    non_tool_start.get_or_insert(block_position);
                    non_tool_blocks.push(block.clone());
                }
            }
        }

        // Emit any remaining non-tool blocks at the end
        if !non_tool_blocks.is_empty() {
            emit_non_tool_message(
                &mut result,
                &msg,
                &mut non_tool_blocks,
                non_tool_start.take(),
            );
        }
    }

    result
}

/// Helper to emit non-tool blocks as a single message.
#[inline]
fn emit_non_tool_message(
    result: &mut Vec<SideMLMessage>,
    msg: &SideMLMessage,
    non_tool_blocks: &mut Vec<ContentBlock>,
    first_block: Option<usize>,
) {
    let new_sideml = ChatMessage {
        role: msg.sideml.role,
        content: std::mem::take(non_tool_blocks),
        tool_use_id: msg.sideml.tool_use_id.clone(),
        tool_choice: msg.sideml.tool_choice.clone(),
        finish_reason: msg.sideml.finish_reason,
        ..Default::default()
    };
    result.push(SideMLMessage {
        // The position of the group's **first** block. It stands for several blocks, so no single index
        // names all of them - but the first one distinguishes this group from the next, which the parent's
        // position does not: two groups of one message then share a path, and with the block index
        // restarting per message their blocks collide outright.
        position: match first_block {
            Some(first) => msg.position.child_index(first),
            None => msg.position.clone(),
        },
        source: msg.source.clone(),
        category: msg.category,
        source_type: msg.source_type,
        timestamp: msg.timestamp,
        sideml: new_sideml,
        rendering: msg.rendering,
        direction: msg.direction,
    });
}

// ============================================================================
// ROLE DERIVATION FROM OTEL EVENTS
// ============================================================================

/// The role a message's **source name** implies, where the name decides it.
///
/// Declared (`event_roles` in the assets), not a table here: which name means which role is a fact about a
/// telemetry vocabulary, and one of the entries belongs to a single CLI rather than to the conventions - so
/// it lives in that dialect's own asset. Two names mean the opposite thing on a tool execution span, which
/// is why the span's kind is part of the question.
///
/// Resolved to `ChatRole` at compile time, so nothing here dispatches on a string.
///
/// Derived at query time, so a correction applies to stored spans without re-ingestion.
///
/// # Returns
///
/// - `Some(ChatRole)` - the role the name declares
/// - `None` - the name declares none, and the role is derived from the content
pub(crate) fn role_from_event_name_with_context(
    event_name: &str,
    is_tool_span: bool,
) -> Option<ChatRole> {
    let Some(declared) = crate::rules::ruleset().event_roles.get(event_name) else {
        // Not a name any asset speaks for. Logged only for the conventions' namespace, where an unrecognised
        // name is more likely to be a spelling this server should know than a producer's own invention.
        if event_name.starts_with("gen_ai.") {
            tracing::trace!(
                event_name = event_name,
                is_tool_span = is_tool_span,
                "no declared role for this source name, role will be derived from content"
            );
        }
        return None;
    };
    // Three states, one spelling each: a role of its own, silence, or - by absence - the same role as elsewhere.
    // Resolved on the declaration, so this and a test asking the same question cannot answer differently.
    declared.role_on(is_tool_span)
}

/// The role a **tagged** source name implies - a name this engine assigned with `tag_as`.
///
/// A tagged emission is an *attribute* whose key the engine chose, which is why it consults the same
/// declarations while a producer's own attribute of the same name does not: the role of a name is a
/// statement about this engine's vocabulary there, not about the producer's.
///
/// This is the gap the event-role move first left open, and it was reachable. A dialect tags a bundled tool
/// result `gen_ai.tool.result`; a bundle of **one** is not split (splitting exists to separate results that
/// would otherwise share an identity, and one needs no separating), so it reached role derivation as an
/// attribute, matched nothing, and was normalised as a **user** message - the model's question, showing the
/// tool's answer. `a_tagged_source_name_takes_its_declared_role` is that shape.
fn role_from_tagged_source(key: &str, is_tool_span: bool) -> Option<ChatRole> {
    crate::rules::ruleset()
        .tagged_source_names
        .contains(key)
        .then(|| role_from_event_name_with_context(key, is_tool_span))
        .flatten()
}

/// Derive role from message source with span context.
///
/// For events, derives role from event name with span context, overriding any
/// existing role except for special extraction roles (tool_call, tools, data, context).
fn derive_role_from_source_with_context(raw: &RawMessage, is_tool_span: bool) -> JsonValue {
    match &raw.source {
        MessageSource::Event { name, .. } => {
            // Preserve special roles that can't be derived from event names
            // Note: Normalize to lowercase for case-insensitive comparison
            // **Declared**: which stated roles are not replaced by the role an event name derives. Roles
            // nothing derives from a name - a tool invocation, a tool-definitions message, framework state -
            // where deriving overwrites the more specific fact with a guess.
            if let Some(existing) = raw.content.get("role").and_then(|r| r.as_str())
                && crate::rules::ruleset()
                    .role_authority
                    .survives_event_derivation(existing)
            {
                return raw.content.clone();
            }

            // Derive role from event name with span context (overrides any existing role)
            if let Some(role) = role_from_event_name_with_context(name, is_tool_span) {
                let mut content = raw.content.clone();
                content["role"] = json!(role.as_str());
                return content;
            }
            raw.content.clone()
        }
        MessageSource::Attribute { key, .. } => {
            // Only a name this engine assigned, and only where the content has already said something
            // this pipeline can *read*. A role it recognises, or one of the special roles that survive
            // derivation, is more specific than the name the reading was tagged with. An unrecognised string
            // is not: it normalises to `user` further down, so treating it as authoritative turned a tool's
            // answer into the user's question on the strength of a value nothing understands.
            let stated = raw
                .content
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("");
            // **Declared**, and no longer "whatever the folding table recognises". The two are the same set
            // today, and that was the hazard: adding an alias for folding granted it authority here, and neither
            // change named the authority it moved.
            if crate::rules::ruleset()
                .role_authority
                .outranks_a_tag(stated)
            {
                return raw.content.clone();
            }
            match role_from_tagged_source(key, is_tool_span) {
                Some(role) => {
                    let mut content = raw.content.clone();
                    content["role"] = json!(role.as_str());
                    content
                }
                None => raw.content.clone(),
            }
        }
    }
}

#[cfg(test)]
#[path = "normalize_tests.rs"]
mod tests;
