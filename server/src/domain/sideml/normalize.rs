//! Trace pipeline integration.
//!
//! Converts raw OTEL messages to normalized SideML format with metadata.
//! This module integrates the SideML library with the application's trace
//! processing pipeline.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use super::{ChatMessage, ChatRole, ContentBlock, normalize};
use crate::data::types::{MessageCategory, MessageSourceType};
use crate::domain::traces::{MessageSource, RawMessage};

// ============================================================================
// SIDEML MESSAGE (PIPELINE OUTPUT)
// ============================================================================

/// A message with pipeline context (category, source, timestamp).
///
/// This type combines a normalized [`ChatMessage`] with application-specific
/// metadata for storage and processing in the trace pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideMLMessage {
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

    for raw in &expanded {
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
            source: raw.source.clone(),
            category,
            source_type,
            timestamp,
            sideml,
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

/// Source names that contain message arrays which should be expanded.
///
/// These are framework-specific attribute/event keys that store arrays of messages.
/// At query time, these arrays are expanded into individual messages for proper
/// deduplication and processing.
///
/// Note: Not all array sources should be expanded. Some arrays are meant to be
/// kept together (e.g., Logfire events, context arrays). Only add keys here that
/// represent expandable message sequences.
const MESSAGE_ARRAY_SOURCES: &[&str] = &[
    // OTEL GenAI standard
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    // Vercel AI SDK
    "ai.prompt.messages",
    // MLflow
    "mlflow.spanInputs",
    "mlflow.spanOutputs",
];

/// Check if a source name indicates a message array that should be expanded.
fn is_expandable_message_array_source(source_name: &str) -> bool {
    MESSAGE_ARRAY_SOURCES.contains(&source_name)
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
fn expand_bundled_tool_results(raw_messages: &[RawMessage]) -> Vec<RawMessage> {
    let mut result = Vec::with_capacity(raw_messages.len());

    for raw in raw_messages {
        // Check source to determine expansion type
        let source_name = match &raw.source {
            MessageSource::Event { name, .. } => Some(name.as_str()),
            MessageSource::Attribute { key, .. } => Some(key.as_str()),
        };

        // Handle message array expansion from known sources
        if source_name.is_some_and(is_expandable_message_array_source) {
            expand_message_array(&mut result, raw);
            continue;
        }

        // Handle bundled tool results
        let role = raw.content.get("role").and_then(|r| r.as_str());
        let is_tool_result_event = source_name == Some("gen_ai.tool.result");

        if (role == Some("tool") || is_tool_result_event)
            && expand_bundled_tool_result(&mut result, raw)
        {
            continue;
        }

        // No expansion needed - keep as-is
        result.push(raw.clone());
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
fn expand_message_array(result: &mut Vec<RawMessage>, raw: &RawMessage) {
    // Find an array to expand. Only consider nested "content"/"messages" fields
    // if they ARE arrays. A string "content" field is the message's actual content,
    // not a nested message array.
    let array_to_expand: Option<&Vec<JsonValue>> = raw
        .content
        .as_array()
        .or_else(|| raw.content.get("content").and_then(|c| c.as_array()))
        .or_else(|| raw.content.get("messages").and_then(|m| m.as_array()));

    let Some(arr) = array_to_expand else {
        // Not an expandable array - keep as single message
        result.push(raw.clone());
        return;
    };

    // Check if this array contains message-like objects
    let has_message_like_items = arr.iter().any(is_message_like_object);

    if !has_message_like_items {
        // Array doesn't contain messages (e.g., content blocks array)
        result.push(raw.clone());
        return;
    }

    // Expand array into individual messages
    let mut expanded_count = 0;
    let mut skipped_count = 0;

    for item in arr {
        if is_message_like_object(item) {
            result.push(RawMessage {
                source: raw.source.clone(),
                content: item.clone(),
            });
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
/// Handles multiple formats:
/// - Strands/Bedrock: `{"content": [{"toolResult": {...}}, {"toolResult": {...}}]}`
/// - Direct array: `[{"toolResult": {...}}, {"toolResult": {...}}]`
///
/// Returns true if expansion occurred, false otherwise.
fn expand_bundled_tool_result(result: &mut Vec<RawMessage>, raw: &RawMessage) -> bool {
    // Check if content is nested in "content" field or is a direct array
    let (content_array, is_nested) =
        if let Some(nested) = raw.content.get("content").and_then(|c| c.as_array()) {
            (nested, true)
        } else if let Some(direct) = raw.content.as_array() {
            (direct, false)
        } else {
            return false;
        };

    // Check if this is bundled format: array containing multiple {toolResult: ...} objects
    // Also check for snake_case variant: {tool_result: ...}
    let tool_results: Vec<&JsonValue> = content_array
        .iter()
        .filter(|item| item.get("toolResult").is_some() || item.get("tool_result").is_some())
        .collect();

    // Not bundled or only one toolResult - don't expand
    if tool_results.len() <= 1 {
        return false;
    }

    tracing::trace!(
        count = tool_results.len(),
        "Expanding bundled tool results into separate messages"
    );

    // Split bundled tool results into separate messages
    for item in tool_results {
        // Handle both camelCase (Bedrock) and snake_case variants
        let tr = item.get("toolResult").or_else(|| item.get("tool_result"));

        // Create new content structure
        let new_content = if is_nested {
            // Original had nested "content" field - preserve structure
            let mut new_obj = raw.content.clone();
            new_obj["content"] = json!([item.clone()]);
            // Set tool_call_id from this specific toolResult
            if let Some(tr) = tr {
                let id = tr
                    .get("toolUseId")
                    .or_else(|| tr.get("tool_use_id"))
                    .and_then(|v| v.as_str());
                if let Some(id) = id {
                    new_obj["tool_call_id"] = json!(id);
                }
            }
            new_obj
        } else {
            // Original was a direct array - wrap in object
            let mut new_obj = json!({
                "role": "tool",
                "content": [item.clone()]
            });
            // Set tool_call_id from this specific toolResult
            if let Some(tr) = tr {
                let id = tr
                    .get("toolUseId")
                    .or_else(|| tr.get("tool_use_id"))
                    .and_then(|v| v.as_str());
                if let Some(id) = id {
                    new_obj["tool_call_id"] = json!(id);
                }
            }
            new_obj
        };

        result.push(RawMessage {
            source: raw.source.clone(),
            content: new_content,
        });
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

        // If 0 or 1 tool blocks, pass through unchanged
        if tool_use_count <= 1 && tool_result_count <= 1 {
            result.push(msg);
            continue;
        }

        // Split into individual tool messages, preserving relative order.
        // Non-tool blocks are grouped and emitted at their first occurrence position.
        let mut non_tool_blocks = Vec::new();
        let mut non_tool_emitted = false;

        for block in &msg.sideml.content {
            match block {
                ContentBlock::ToolUse { .. } => {
                    // Emit accumulated non-tool blocks before this tool block
                    if !non_tool_emitted && !non_tool_blocks.is_empty() {
                        emit_non_tool_message(&mut result, &msg, &mut non_tool_blocks);
                        non_tool_emitted = true;
                    }
                    // Create individual message for this tool use
                    let new_sideml = ChatMessage {
                        role: msg.sideml.role,
                        content: vec![block.clone()],
                        tool_use_id: msg.sideml.tool_use_id.clone(),
                        ..Default::default()
                    };
                    result.push(SideMLMessage {
                        source: msg.source.clone(),
                        category: msg.category,
                        source_type: msg.source_type,
                        timestamp: msg.timestamp,
                        sideml: new_sideml,
                    });
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    // Emit accumulated non-tool blocks before this tool block
                    if !non_tool_emitted && !non_tool_blocks.is_empty() {
                        emit_non_tool_message(&mut result, &msg, &mut non_tool_blocks);
                        non_tool_emitted = true;
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
                        source: msg.source.clone(),
                        category: msg.category,
                        source_type: msg.source_type,
                        timestamp: msg.timestamp,
                        sideml: new_sideml,
                    });
                }
                _ => {
                    // Collect non-tool blocks
                    non_tool_blocks.push(block.clone());
                }
            }
        }

        // Emit any remaining non-tool blocks at the end
        if !non_tool_blocks.is_empty() {
            emit_non_tool_message(&mut result, &msg, &mut non_tool_blocks);
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
        source: msg.source.clone(),
        category: msg.category,
        source_type: msg.source_type,
        timestamp: msg.timestamp,
        sideml: new_sideml,
    });
}

// ============================================================================
// ROLE DERIVATION FROM OTEL EVENTS
// ============================================================================

/// Derive role from OTEL event name with span context.
///
/// In tool execution spans, events have different semantics:
/// - `gen_ai.tool.message` = tool INPUT (args passed to tool) → assistant role
///   (prevents merging with tool OUTPUT in ToolResultRegistry)
/// - `gen_ai.choice` = tool OUTPUT (result from tool) → tool role
/// - `gen_ai.assistant.message` = conversation history → assistant role (always)
///
/// In chat spans (non-tool):
/// - `gen_ai.tool.message` = tool OUTPUT (result from tool) → tool role
/// - `gen_ai.choice` = assistant response → assistant role
/// - `gen_ai.assistant.message` = assistant response → assistant role (always)
///
/// This query-time derivation enables bug fixes to apply to historical data without re-ingestion.
///
/// # Returns
///
/// - `Some(ChatRole)` - The derived role for the event
/// - `None` - Unknown event, role cannot be derived from event name
pub(crate) fn role_from_event_name_with_context(
    event_name: &str,
    is_tool_span: bool,
) -> Option<ChatRole> {
    match event_name {
        "gen_ai.system.message" => Some(ChatRole::System),
        "gen_ai.user.message" | "gen_ai.content.prompt" => Some(ChatRole::User),

        // Tool message: semantics depend on span context
        "gen_ai.tool.message" => {
            if is_tool_span {
                // In tool span: gen_ai.tool.message is tool INPUT (invocation args)
                // Use Assistant role to prevent merging with tool OUTPUT in ToolResultRegistry
                // (tool_call role maps to Assistant, so this is semantically consistent)
                Some(ChatRole::Assistant)
            } else {
                // In chat span: this is tool OUTPUT (result from tool call)
                Some(ChatRole::Tool)
            }
        }
        // Tool result is always OUTPUT (from tool call)
        "gen_ai.tool.result" => Some(ChatRole::Tool),

        // Assistant message from conversation history - always assistant role
        // (This is an INPUT event containing prior assistant responses, not tool output)
        "gen_ai.assistant.message" => Some(ChatRole::Assistant),

        // Choice/completion: semantics depend on span context
        "gen_ai.choice" | "gen_ai.content.completion" => {
            if is_tool_span {
                // In tool span: this is tool OUTPUT (result)
                Some(ChatRole::Tool)
            } else {
                // In chat span: this is assistant/LLM response
                Some(ChatRole::Assistant)
            }
        }

        _ => {
            // Log unknown event names at trace level for diagnostics
            // This helps identify new event types that should be supported
            if event_name.starts_with("gen_ai.") {
                tracing::trace!(
                    event_name = event_name,
                    is_tool_span = is_tool_span,
                    "Unknown gen_ai event name, role will be derived from content"
                );
            }
            None
        }
    }
}

/// Special roles that MUST NOT be overridden by event-based role derivation.
///
/// These roles are explicitly set during extraction and carry specific semantic meaning
/// that cannot be inferred from event names alone:
/// - `tool_call`: Tool invocation (args passed to tool) - extraction knows this from span context
/// - `tools`: Tool definitions message - set during extraction
/// - `data`: Conversation history data (e.g., Google ADK) - set during extraction
/// - `context`: Chat context (e.g., LiveKit) - set during extraction
/// - `documents`: Retrieved documents (e.g., OpenInference RAG) - set during extraction
///
/// Note: `tool` is NOT in this list because it CAN be derived from event names
/// (`gen_ai.tool.message` in chat spans, `gen_ai.choice` in tool spans).
const SPECIAL_ROLES: &[&str] = &["tool_call", "tools", "data", "context", "documents"];

/// Derive role from message source with span context.
///
/// For events, derives role from event name with span context, overriding any
/// existing role except for special extraction roles (tool_call, tools, data, context).
fn derive_role_from_source_with_context(raw: &RawMessage, is_tool_span: bool) -> JsonValue {
    match &raw.source {
        MessageSource::Event { name, .. } => {
            // Preserve special roles that can't be derived from event names
            // Note: Normalize to lowercase for case-insensitive comparison
            if let Some(existing) = raw.content.get("role").and_then(|r| r.as_str()) {
                let existing_lower = existing.to_lowercase();
                if SPECIAL_ROLES.contains(&existing_lower.as_str()) {
                    return raw.content.clone();
                }
            }

            // Derive role from event name with span context (overrides any existing role)
            if let Some(role) = role_from_event_name_with_context(name, is_tool_span) {
                let mut content = raw.content.clone();
                content["role"] = json!(role.as_str());
                return content;
            }
            raw.content.clone()
        }
        MessageSource::Attribute { .. } => raw.content.clone(),
    }
}

// ============================================================================
// MESSAGE CATEGORIZATION
// ============================================================================

/// Determine message category based on source and content.
///
/// Categorization rules (in priority order):
/// 1. Event with LLM output name (gen_ai.choice, gen_ai.content.completion)
///    → Use event name (semantic output categorization)
/// 2. Event with special role (tool_call, tool, tools, data, context)
///    → Use role-based categorization
/// 3. Other events
///    → Use event name
/// 4. Attribute with role
///    → Use role-based categorization
/// 5. Attribute without role
///    → Default to user message
fn determine_category(source: &MessageSource, content: &JsonValue) -> MessageCategory {
    let role = content.get("role").and_then(|r| r.as_str());

    match source {
        MessageSource::Event { name, .. } => categorize_event_message(name, role, content),
        MessageSource::Attribute { .. } => categorize_attribute_message(role),
    }
}

/// Check if event represents LLM output (always uses event-based categorization).
fn is_llm_output_event(event_name: &str) -> bool {
    matches!(event_name, "gen_ai.choice" | "gen_ai.content.completion")
}

/// Check if role should use role-based categorization instead of event-based.
///
/// This is different from SPECIAL_ROLES (which controls role derivation):
/// - SPECIAL_ROLES: Roles that MUST NOT be overridden during role derivation
/// - is_special_role: Roles that MUST use role-based categorization
///
/// The `tool` role is included here but not in SPECIAL_ROLES because:
/// - `tool` CAN be derived from event names (so not in SPECIAL_ROLES)
/// - `tool` MUST use role-based categorization (so included here)
///
/// Special roles for categorization:
/// - tool_call: tool invocation (input to tool) → GenAIToolInput
/// - tool: tool result (output from tool) → GenAIToolMessage
/// - tools: tool definitions → GenAIToolDefinitions
/// - data/context: conversation history → GenAIContext
/// - documents: retrieved documents → Retrieval
fn is_special_role(role: &str) -> bool {
    matches!(
        role.to_lowercase().as_str(),
        "tool_call" | "tool" | "tools" | "data" | "context" | "documents"
    )
}

/// Categorize event-sourced message.
fn categorize_event_message(
    event_name: &str,
    role: Option<&str>,
    content: &JsonValue,
) -> MessageCategory {
    // LLM output events always use event-based categorization
    // This ensures tool messages in choice events get GenAIChoice (output)
    // rather than GenAIToolMessage (input)
    if is_llm_output_event(event_name) {
        return category_from_event_name(event_name, content);
    }

    // Special roles override event name, but "tool" needs content inspection
    // to distinguish between INPUT (toolUse) and OUTPUT (toolResult)
    if let Some(role_str) = role
        && is_special_role(role_str)
    {
        return category_from_role_with_content(role_str, content);
    }

    // Default: use event name
    category_from_event_name(event_name, content)
}

/// Categorize attribute-sourced message.
fn categorize_attribute_message(role: Option<&str>) -> MessageCategory {
    role.map(category_from_role)
        .unwrap_or(MessageCategory::GenAIUserMessage)
}

/// Map event name to MessageCategory.
fn category_from_event_name(event_name: &str, raw_message: &JsonValue) -> MessageCategory {
    match event_name {
        "gen_ai.system.message" => MessageCategory::GenAISystemMessage,
        "gen_ai.user.message" => MessageCategory::GenAIUserMessage,
        "gen_ai.assistant.message" => MessageCategory::GenAIAssistantMessage,
        "gen_ai.tool.message" => categorize_tool_message(raw_message),
        "gen_ai.choice" | "gen_ai.content.completion" => MessageCategory::GenAIChoice,
        "gen_ai.content.prompt" => MessageCategory::GenAIUserMessage,
        "exception" => MessageCategory::Exception,
        "log" => MessageCategory::Log,
        n if n.contains("retrieval") || n.contains("search") => MessageCategory::Retrieval,
        n if n.contains("score") || n.contains("observation") => MessageCategory::Observation,
        _ => MessageCategory::Other,
    }
}

/// Categorize tool message as input (tool invocation) or output (tool result).
pub(super) fn categorize_tool_message(raw_message: &JsonValue) -> MessageCategory {
    // Check for tool INPUT indicators (assistant calling tools)
    if raw_message.get("tool_calls").is_some() {
        return MessageCategory::GenAIToolInput;
    }

    // Check content blocks for tool_use (input) vs tool_result (output)
    if let Some(content) = raw_message.get("content")
        && let Some(arr) = content.as_array()
    {
        for block in arr {
            // Tool INPUT indicators in content
            if block.get("toolUse").is_some()
                || block.get("functionCall").is_some()
                || block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            {
                return MessageCategory::GenAIToolInput;
            }
            // Tool OUTPUT indicators in content
            if block.get("toolResult").is_some()
                || block.get("functionResponse").is_some()
                || block.get("type").and_then(|t| t.as_str()) == Some("tool_result")
            {
                return MessageCategory::GenAIToolMessage;
            }
        }
    }

    // Default: tool result/output (role="tool" with content)
    MessageCategory::GenAIToolMessage
}

/// Map a role to MessageCategory, with content inspection for ambiguous roles.
///
/// The "tool" role is ambiguous - it can mean:
/// - Tool INPUT (assistant calling a tool): contains toolUse/tool_calls
/// - Tool OUTPUT (tool result): contains toolResult or plain content
///
/// This function inspects content to distinguish between these cases.
fn category_from_role_with_content(role: &str, content: &JsonValue) -> MessageCategory {
    let role_lower = role.to_lowercase();
    match role_lower.as_str() {
        // Tool role needs content inspection to distinguish INPUT vs OUTPUT
        "tool" => categorize_tool_message(content),
        // Other roles delegate to simple role-based categorization
        _ => category_from_role(role),
    }
}

/// Map a role to the appropriate MessageCategory (without content inspection).
///
/// Use `category_from_role_with_content` when content is available and the role
/// might be "tool" (which needs content inspection for INPUT vs OUTPUT).
fn category_from_role(role: &str) -> MessageCategory {
    let role_lower = role.to_lowercase();
    match role_lower.as_str() {
        // Tool definitions message
        "tools" => MessageCategory::GenAIToolDefinitions,
        // Tool invocation (assistant calling a tool)
        "tool_call" => MessageCategory::GenAIToolInput,
        // Context/data roles (conversation history, chat context)
        "data" | "context" => MessageCategory::GenAIContext,
        // Retrieved documents (RAG results)
        "documents" => MessageCategory::Retrieval,
        // Standard roles (including "tool" which defaults to OUTPUT)
        _ => match ChatRole::from_str_normalized(role) {
            ChatRole::System => MessageCategory::GenAISystemMessage,
            ChatRole::Assistant => MessageCategory::GenAIAssistantMessage,
            ChatRole::Tool => MessageCategory::GenAIToolMessage,
            ChatRole::User => MessageCategory::GenAIUserMessage,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn event_source(name: &str) -> MessageSource {
        MessageSource::Event {
            name: name.to_string(),
            time: Utc::now(),
        }
    }

    fn attribute_source() -> MessageSource {
        MessageSource::Attribute {
            key: "test".to_string(),
            time: Utc::now(),
        }
    }

    #[test]
    fn test_is_llm_output_event() {
        assert!(is_llm_output_event("gen_ai.choice"));
        assert!(is_llm_output_event("gen_ai.content.completion"));
        assert!(!is_llm_output_event("gen_ai.user.message"));
        assert!(!is_llm_output_event("gen_ai.assistant.message"));
    }

    #[test]
    fn test_is_special_role() {
        assert!(is_special_role("tool_call"));
        assert!(is_special_role("TOOL_CALL")); // case insensitive
        assert!(is_special_role("tool"));
        assert!(is_special_role("tools"));
        assert!(is_special_role("data"));
        assert!(is_special_role("context"));
        assert!(!is_special_role("user"));
        assert!(!is_special_role("assistant"));
        assert!(!is_special_role("system"));
    }

    #[test]
    fn test_categorize_attribute_message() {
        assert_eq!(
            categorize_attribute_message(Some("user")),
            MessageCategory::GenAIUserMessage
        );
        assert_eq!(
            categorize_attribute_message(Some("assistant")),
            MessageCategory::GenAIAssistantMessage
        );
        assert_eq!(
            categorize_attribute_message(None),
            MessageCategory::GenAIUserMessage
        );
    }

    #[test]
    fn test_determine_category_llm_output_ignores_role() {
        // Even with tool role, gen_ai.choice should be GenAIChoice
        let content = json!({"role": "tool"});
        let source = event_source("gen_ai.choice");
        assert_eq!(
            determine_category(&source, &content),
            MessageCategory::GenAIChoice
        );
    }

    #[test]
    fn test_determine_category_special_role_overrides_event() {
        let content = json!({"role": "tool_call"});
        let source = event_source("gen_ai.assistant.message");
        assert_eq!(
            determine_category(&source, &content),
            MessageCategory::GenAIToolInput
        );
    }

    #[test]
    fn test_determine_category_standard_role_uses_event() {
        let content = json!({"role": "assistant"});
        let source = event_source("gen_ai.assistant.message");
        assert_eq!(
            determine_category(&source, &content),
            MessageCategory::GenAIAssistantMessage
        );
    }

    #[test]
    fn test_determine_category_attribute_uses_role() {
        let content = json!({"role": "system"});
        let source = attribute_source();
        assert_eq!(
            determine_category(&source, &content),
            MessageCategory::GenAISystemMessage
        );
    }

    #[test]
    fn test_expand_message_array_preserves_message_with_string_content() {
        // Individual messages have string "content" field - should NOT be expanded
        // The key insight: only expand if nested content/messages is an ARRAY
        let raw = RawMessage {
            source: MessageSource::Attribute {
                key: "ai.prompt.messages".to_string(),
                time: Utc::now(),
            },
            content: json!({"role": "system", "content": "You are a helpful assistant."}),
        };

        let mut result = Vec::new();
        expand_message_array(&mut result, &raw);

        assert_eq!(result.len(), 1, "Single message should be preserved");
        assert_eq!(
            result[0].content.get("role").and_then(|r| r.as_str()),
            Some("system")
        );
        assert_eq!(
            result[0].content.get("content").and_then(|c| c.as_str()),
            Some("You are a helpful assistant.")
        );
    }

    #[test]
    fn test_expand_message_array_preserves_message_with_array_content() {
        // Messages can have array content (content blocks) - should NOT expand as messages
        let raw = RawMessage {
            source: MessageSource::Attribute {
                key: "ai.prompt.messages".to_string(),
                time: Utc::now(),
            },
            content: json!({
                "role": "user",
                "content": [{"type": "text", "text": "Hello"}]  // Array of content blocks, not messages
            }),
        };

        let mut result = Vec::new();
        expand_message_array(&mut result, &raw);

        assert_eq!(
            result.len(),
            1,
            "Single message with content blocks should be preserved"
        );
        assert_eq!(
            result[0].content.get("role").and_then(|r| r.as_str()),
            Some("user")
        );
    }

    #[test]
    fn test_expand_message_array_expands_top_level_array() {
        // Top-level array of messages should be expanded
        let raw = RawMessage {
            source: MessageSource::Attribute {
                key: "ai.prompt.messages".to_string(),
                time: Utc::now(),
            },
            content: json!([
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ]),
        };

        let mut result = Vec::new();
        expand_message_array(&mut result, &raw);

        assert_eq!(result.len(), 2, "Should expand to 2 messages");
        assert_eq!(
            result[0].content.get("role").and_then(|r| r.as_str()),
            Some("system")
        );
        assert_eq!(
            result[1].content.get("role").and_then(|r| r.as_str()),
            Some("user")
        );
    }

    #[test]
    fn test_expand_message_array_expands_nested_messages_array() {
        // Nested "messages" array should be expanded
        let raw = RawMessage {
            source: MessageSource::Attribute {
                key: "some.attribute".to_string(),
                time: Utc::now(),
            },
            content: json!({
                "messages": [
                    {"role": "user", "content": "Hello"},
                    {"role": "assistant", "content": "Hi there"}
                ]
            }),
        };

        let mut result = Vec::new();
        expand_message_array(&mut result, &raw);

        assert_eq!(result.len(), 2, "Should expand nested messages array");
    }

    #[test]
    fn test_expand_message_array_gemini_parts_format() {
        // Gemini uses "parts" instead of "content" - should be recognized as message
        let raw = RawMessage {
            source: MessageSource::Attribute {
                key: "gen_ai.prompt".to_string(),
                time: Utc::now(),
            },
            content: json!([
                {"role": "user", "parts": [{"text": "Hello"}]},
                {"role": "model", "parts": [{"text": "Hi"}]}
            ]),
        };

        let mut result = Vec::new();
        expand_message_array(&mut result, &raw);

        assert_eq!(result.len(), 2, "Gemini format should be expanded");
    }

    #[test]
    fn test_to_sideml_vercel_ai_system_and_user_messages() {
        // Full pipeline test: Vercel AI system + user messages should both be preserved
        let raw_messages = vec![
            RawMessage {
                source: MessageSource::Attribute {
                    key: "ai.prompt.messages".to_string(),
                    time: Utc::now(),
                },
                content: json!({"role": "system", "content": "You are a helpful assistant."}),
            },
            RawMessage {
                source: MessageSource::Attribute {
                    key: "ai.prompt.messages".to_string(),
                    time: Utc::now(),
                },
                content: json!({"role": "user", "content": [{"type": "text", "text": "Hello"}]}),
            },
        ];

        let result = to_sideml(&raw_messages);

        assert_eq!(result.len(), 2, "Should have 2 messages");
        assert_eq!(result[0].sideml.role, ChatRole::System);
        assert_eq!(result[1].sideml.role, ChatRole::User);

        // Verify content is normalized
        assert!(!result[0].sideml.content.is_empty());
        assert!(!result[1].sideml.content.is_empty());
    }

    #[test]
    fn test_flatten_tool_blocks_preserves_order() {
        // Test that non-tool blocks appear at their first occurrence position
        let msg = SideMLMessage {
            source: event_source("gen_ai.assistant.message"),
            category: MessageCategory::GenAIAssistantMessage,
            source_type: MessageSourceType::Event,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "Before tools".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: Some("tool_1".to_string()),
                        name: "search".to_string(),
                        input: json!({"q": "test"}),
                    },
                    ContentBlock::ToolUse {
                        id: Some("tool_2".to_string()),
                        name: "fetch".to_string(),
                        input: json!({"url": "http://example.com"}),
                    },
                ],
                ..Default::default()
            },
        };

        let result = flatten_tool_blocks(vec![msg]);

        assert_eq!(result.len(), 3, "Should flatten into 3 messages");
        // Non-tool content should come FIRST (at its original position)
        assert!(matches!(
            result[0].sideml.content.first(),
            Some(ContentBlock::Text { text }) if text == "Before tools"
        ));
        // Tool blocks should follow in order
        assert!(matches!(
            result[1].sideml.content.first(),
            Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_1"
        ));
        assert!(matches!(
            result[2].sideml.content.first(),
            Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_2"
        ));
    }

    #[test]
    fn test_flatten_tool_blocks_text_after_tools() {
        // Test that text after tools stays at the end
        let msg = SideMLMessage {
            source: event_source("gen_ai.assistant.message"),
            category: MessageCategory::GenAIAssistantMessage,
            source_type: MessageSourceType::Event,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: Some("tool_1".to_string()),
                        name: "search".to_string(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: Some("tool_2".to_string()),
                        name: "fetch".to_string(),
                        input: json!({}),
                    },
                    ContentBlock::Text {
                        text: "After tools".to_string(),
                    },
                ],
                ..Default::default()
            },
        };

        let result = flatten_tool_blocks(vec![msg]);

        assert_eq!(result.len(), 3, "Should flatten into 3 messages");
        // Tool blocks should come first (in order)
        assert!(matches!(
            result[0].sideml.content.first(),
            Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_1"
        ));
        assert!(matches!(
            result[1].sideml.content.first(),
            Some(ContentBlock::ToolUse { id: Some(id), .. }) if id == "tool_2"
        ));
        // Text should come LAST (at its original position)
        assert!(matches!(
            result[2].sideml.content.first(),
            Some(ContentBlock::Text { text }) if text == "After tools"
        ));
    }

    #[test]
    fn test_flatten_tool_blocks_single_tool_unchanged() {
        // Messages with 0 or 1 tool blocks should pass through unchanged
        let msg = SideMLMessage {
            source: event_source("gen_ai.assistant.message"),
            category: MessageCategory::GenAIAssistantMessage,
            source_type: MessageSourceType::Event,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "Here's the result".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: Some("tool_1".to_string()),
                        name: "search".to_string(),
                        input: json!({}),
                    },
                ],
                ..Default::default()
            },
        };

        let result = flatten_tool_blocks(vec![msg.clone()]);

        assert_eq!(result.len(), 1, "Single tool should not be flattened");
        assert_eq!(
            result[0].sideml.content.len(),
            2,
            "All content blocks preserved"
        );
    }

    #[test]
    fn test_flatten_tool_blocks_multiple_tool_results() {
        // Test flattening multiple tool results (parallel tool execution)
        let msg = SideMLMessage {
            source: event_source("gen_ai.tool.message"),
            category: MessageCategory::GenAIToolMessage,
            source_type: MessageSourceType::Event,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Tool,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: Some("call_1".to_string()),
                        content: json!({"result": "weather data"}),
                        is_error: false,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: Some("call_2".to_string()),
                        content: json!({"result": "time data"}),
                        is_error: false,
                    },
                ],
                ..Default::default()
            },
        };

        let result = flatten_tool_blocks(vec![msg]);

        assert_eq!(result.len(), 2, "Multiple tool results should be flattened");

        // Each message should have exactly one tool result
        for (i, msg) in result.iter().enumerate() {
            assert_eq!(
                msg.sideml.content.len(),
                1,
                "Message {} should have exactly one content block",
                i
            );
            assert!(
                matches!(msg.sideml.content[0], ContentBlock::ToolResult { .. }),
                "Message {} should contain a ToolResult",
                i
            );
        }

        // Verify tool_use_ids are preserved and propagated to message level
        let tool_ids: Vec<_> = result
            .iter()
            .filter_map(|msg| msg.sideml.tool_use_id.clone())
            .collect();
        assert!(tool_ids.contains(&"call_1".to_string()));
        assert!(tool_ids.contains(&"call_2".to_string()));
    }

    #[test]
    fn test_flatten_tool_blocks_mixed_tool_use_and_result() {
        // Edge case: Message with both ToolUse and ToolResult (unusual but possible)
        let msg = SideMLMessage {
            source: event_source("gen_ai.assistant.message"),
            category: MessageCategory::GenAIAssistantMessage,
            source_type: MessageSourceType::Event,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: Some("call_1".to_string()),
                        name: "search".to_string(),
                        input: json!({}),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: Some("call_0".to_string()),
                        content: json!("previous result"),
                        is_error: false,
                    },
                    ContentBlock::ToolUse {
                        id: Some("call_2".to_string()),
                        name: "fetch".to_string(),
                        input: json!({}),
                    },
                ],
                ..Default::default()
            },
        };

        let result = flatten_tool_blocks(vec![msg]);

        // Should flatten into 3 separate messages (2 tool uses + 1 tool result)
        assert_eq!(
            result.len(),
            3,
            "Mixed tool uses and results should be flattened"
        );

        // Count block types
        let tool_use_count = result
            .iter()
            .filter(|m| matches!(m.sideml.content.first(), Some(ContentBlock::ToolUse { .. })))
            .count();
        let tool_result_count = result
            .iter()
            .filter(|m| {
                matches!(
                    m.sideml.content.first(),
                    Some(ContentBlock::ToolResult { .. })
                )
            })
            .count();

        assert_eq!(tool_use_count, 2, "Should have 2 tool use messages");
        assert_eq!(tool_result_count, 1, "Should have 1 tool result message");
    }

    #[test]
    fn test_flattened_tool_results_get_name_enriched() {
        // Regression test: name enrichment must happen AFTER flattening,
        // otherwise flattened tool results won't get their tool names.
        use crate::domain::traces::RawMessage;

        // Create a tool use message and a bundled tool result message
        let tool_use_msg = RawMessage {
            source: MessageSource::Event {
                name: "gen_ai.assistant.message".to_string(),
                time: Utc::now(),
            },
            content: json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call_weather",
                    "name": "get_weather",
                    "input": {"city": "NYC"}
                }, {
                    "type": "tool_use",
                    "id": "call_time",
                    "name": "get_time",
                    "input": {"timezone": "EST"}
                }]
            }),
        };

        // Bundled tool results - will be flattened
        let tool_results_msg = RawMessage {
            source: MessageSource::Event {
                name: "gen_ai.tool.message".to_string(),
                time: Utc::now(),
            },
            content: json!({
                "role": "tool",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_weather",
                    "content": "Sunny, 72F"
                }, {
                    "type": "tool_result",
                    "tool_use_id": "call_time",
                    "content": "3:00 PM EST"
                }]
            }),
        };

        let result = to_sideml(&[tool_use_msg, tool_results_msg]);

        // Should have 4 messages: 2 tool uses + 2 tool results (flattened)
        assert_eq!(result.len(), 4, "Should flatten into 4 messages");

        // Find the tool result messages and verify they have names
        let tool_results: Vec<_> = result
            .iter()
            .filter(|m| m.sideml.role == ChatRole::Tool)
            .collect();

        assert_eq!(tool_results.len(), 2, "Should have 2 tool result messages");

        // Each tool result should have a name enriched from the tool use
        for tr in &tool_results {
            assert!(
                tr.sideml.name.is_some(),
                "Tool result should have name enriched, got: {:?}",
                tr.sideml
            );
        }

        // Verify correct names
        let names: Vec<_> = tool_results
            .iter()
            .filter_map(|m| m.sideml.name.clone())
            .collect();
        assert!(names.contains(&"get_weather".to_string()));
        assert!(names.contains(&"get_time".to_string()));
    }
}
