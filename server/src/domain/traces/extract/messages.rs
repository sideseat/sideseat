//! Message extraction from spans.
//!
//! Extracts messages from OTEL events and span attributes for various frameworks.

#![allow(clippy::collapsible_if)]

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::trace::v1::Span;
use opentelemetry_proto::tonic::trace::v1::span::Event;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::domain::sideml::is_plain_data_value;
use crate::domain::sideml::tools::tool_definition_quality;
use crate::utils::otlp::extract_attributes;
use crate::utils::time::nanos_to_datetime;

use super::{extract_json, keys};

// ============================================================================
// JSON PARSING HELPERS
// ============================================================================

/// Parse a string as JSON, with logging on parse failure.
///
/// Returns the parsed JSON value on success, or the original string as a JSON string on failure.
/// Logs a trace-level warning when falling back to string representation.
fn parse_json_with_fallback(value: &str, context: &str) -> JsonValue {
    match serde_json::from_str(value) {
        Ok(json) => json,
        Err(e) => {
            tracing::trace!(
                context = context,
                error = %e,
                value_preview = %truncate_for_log(value, 100),
                "JSON parse failed, using string fallback"
            );
            json!(value)
        }
    }
}

/// Truncate a string for logging purposes (UTF-8 safe).
fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a valid UTF-8 char boundary at or before max_len
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Normalize an array that may contain stringified JSON elements.
///
/// OTLP arrays of objects are converted by `extract_attributes()` into arrays
/// where each element is a JSON string. This function parses those strings
/// to get the actual JSON objects.
///
/// Example input: `["{ \"name\": \"foo\" }", "{ \"name\": \"bar\" }"]`
/// Example output: `[{ "name": "foo" }, { "name": "bar" }]`
fn normalize_stringified_array(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(arr) => {
            let normalized: Vec<JsonValue> = arr
                .into_iter()
                .map(|item| {
                    if let JsonValue::String(s) = &item {
                        serde_json::from_str(s).unwrap_or(item)
                    } else {
                        item
                    }
                })
                .collect();
            JsonValue::Array(normalized)
        }
        other => other,
    }
}

// ============================================================================
// RAW MESSAGE TYPES
// ============================================================================

/// Pre-normalized message with source tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    pub source: MessageSource,
    pub content: JsonValue,
}

/// Source of a raw message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    Event { name: String, time: DateTime<Utc> },
    Attribute { key: String, time: DateTime<Utc> },
}

impl RawMessage {
    pub fn from_event(name: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Event {
                name: name.to_string(),
                time,
            },
            content,
        }
    }

    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: MessageSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}

// ============================================================================
// RAW TOOL DEFINITION TYPES
// ============================================================================

/// Pre-normalized tool definition with source tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawToolDefinition {
    pub source: ToolDefinitionSource,
    pub content: JsonValue,
}

/// Source of a raw tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDefinitionSource {
    Attribute { key: String, time: DateTime<Utc> },
}

impl RawToolDefinition {
    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: ToolDefinitionSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}

// ============================================================================
// RAW TOOL NAMES TYPES
// ============================================================================

/// Pre-normalized tool names list with source tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawToolNames {
    pub source: ToolDefinitionSource,
    pub content: JsonValue,
}

impl RawToolNames {
    pub fn from_attr(key: &str, time: DateTime<Utc>, content: JsonValue) -> Self {
        Self {
            source: ToolDefinitionSource::Attribute {
                key: key.to_string(),
                time,
            },
            content,
        }
    }
}

// ============================================================================
// MESSAGE EVENT RECOGNITION
// ============================================================================

/// Check if an event name is a recognized message event.
fn is_message_event(event_name: &str) -> bool {
    matches!(
        event_name,
        keys::EVENT_SYSTEM_MESSAGE
            | keys::EVENT_USER_MESSAGE
            | keys::EVENT_CONTENT_PROMPT
            | keys::EVENT_ASSISTANT_MESSAGE
            | keys::EVENT_CHOICE
            | keys::EVENT_CONTENT_COMPLETION
            | keys::EVENT_TOOL_MESSAGE
            | keys::EVENT_INFERENCE_OPERATION_DETAILS
    )
}

// ============================================================================
// MESSAGE EXTRACTION FROM EVENTS
// ============================================================================

pub(crate) fn extract_messages_from_events(
    messages: &mut Vec<RawMessage>,
    events: &[Event],
    is_tool_span: bool,
) {
    for event in events {
        messages.extend(extract_message_from_event(event, is_tool_span));
    }
}

pub(crate) fn extract_message_from_event(event: &Event, is_tool_span: bool) -> Vec<RawMessage> {
    // Only process known message events
    if !is_message_event(&event.name) {
        return vec![];
    }

    let attrs = extract_attributes(&event.attributes);
    let event_time = nanos_to_datetime(event.time_unix_nano);

    // Strands new convention: gen_ai.client.inference.operation.details contains
    // gen_ai.input.messages and gen_ai.output.messages as event attributes
    if event.name == keys::EVENT_INFERENCE_OPERATION_DETAILS {
        return extract_inference_operation_details_event(&attrs, event_time);
    }

    // Build raw message preserving literal attributes only (no metadata)
    let mut raw = serde_json::Map::new();
    for (key, value) in &attrs {
        // Try to parse JSON values, otherwise keep as string
        let json_val = if value.starts_with('{') || value.starts_with('[') {
            parse_json_with_fallback(value, &format!("event.{}.{}", event.name, key))
        } else {
            json!(value)
        };
        raw.insert(key.clone(), json_val);
    }

    // Role derivation moved to query-time in sideml/pipeline.rs
    // (role_from_event_name_with_context handles tool span semantics)
    // Store raw event data; let query-time pipeline derive role from event name

    let mut messages = vec![RawMessage::from_event(
        &event.name,
        event_time,
        JsonValue::Object(raw.clone()),
    )];

    // Strands: gen_ai.choice events may have "tool.result" attribute with full Bedrock toolResult.
    // Store as-is; splitting bundled results happens at query time in conversation pipeline
    // for ingestion-independence (fixes apply to historical data without re-ingestion).
    // Use EVENT_TOOL_RESULT (not EVENT_TOOL_MESSAGE) to avoid history filtering.
    // Role derived at query-time from event name.
    if event.name == keys::EVENT_CHOICE && !is_tool_span {
        if let Some(tool_result) = raw.get("tool.result") {
            let mut tool_msg = serde_json::Map::new();
            tool_msg.insert("content".to_string(), tool_result.clone());
            // Extract first tool_call_id for message-level identification (will be split at query time)
            if let Some(arr) = tool_result.as_array() {
                for block in arr {
                    if let Some(tr) = block.get("toolResult") {
                        if let Some(id) = tr.get("toolUseId").and_then(|v| v.as_str()) {
                            tool_msg.insert("tool_call_id".to_string(), json!(id));
                            break;
                        }
                    }
                }
            }
            messages.push(RawMessage::from_event(
                keys::EVENT_TOOL_RESULT,
                event_time,
                JsonValue::Object(tool_msg),
            ));
        }
    }

    messages
}

/// Extract messages from Strands gen_ai.client.inference.operation.details event.
/// This event contains gen_ai.input.messages and/or gen_ai.output.messages.
/// Arrays are stored as-is; expansion happens at query time in SideML pipeline
/// for ingestion-independence (fixes apply to historical data without re-ingestion).
fn extract_inference_operation_details_event(
    attrs: &HashMap<String, String>,
    event_time: DateTime<Utc>,
) -> Vec<RawMessage> {
    let mut messages = Vec::new();

    // Extract input messages (request/prompt) - store as-is, expand at query time
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_INPUT_MESSAGES) {
        messages.push(RawMessage::from_event(
            keys::GEN_AI_INPUT_MESSAGES,
            event_time,
            parsed,
        ));
    }

    // Extract output messages (response/completion) - store as-is, expand at query time
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_OUTPUT_MESSAGES) {
        messages.push(RawMessage::from_event(
            keys::GEN_AI_OUTPUT_MESSAGES,
            event_time,
            parsed,
        ));
    }

    messages
}

// ============================================================================
// MESSAGE EXTRACTION FROM ATTRIBUTES
// ============================================================================

/// Function signature for attribute-based message extractors.
type AttrExtractor = fn(
    &mut Vec<RawMessage>,
    &mut Vec<RawToolDefinition>,
    &HashMap<String, String>,
    &str,
    DateTime<Utc>,
) -> bool;

/// Named extractor for logging and debugging.
struct NamedExtractor {
    name: &'static str,
    extractor: AttrExtractor,
}

/// Framework extractors in priority order.
///
/// The order matters: first matching extractor wins. This priority is designed to:
/// 1. Check specific indexed formats first (gen_ai.prompt.0.*, gen_ai.completion.0.*)
/// 2. Check OTEL standard formats (gen_ai.input.messages, gen_ai.output.messages)
/// 3. Check framework-specific formats (OpenInference, Vercel AI, etc.)
/// 4. Fall back to generic I/O formats (input.value, output.value)
///
/// If you need to debug framework detection, enable SIDESEAT_LOG=trace to see
/// which extractor is used for each span.
const EXTRACTORS: &[NamedExtractor] = &[
    NamedExtractor {
        name: "gen_ai_indexed",
        extractor: try_gen_ai_indexed,
    },
    NamedExtractor {
        name: "otel_genai_messages",
        extractor: try_otel_genai_messages,
    },
    NamedExtractor {
        name: "openinference",
        extractor: try_openinference,
    },
    NamedExtractor {
        name: "logfire_events",
        extractor: try_logfire_events,
    },
    NamedExtractor {
        name: "vercel_ai",
        extractor: try_vercel_ai,
    },
    NamedExtractor {
        name: "google_adk",
        extractor: try_google_adk,
    },
    NamedExtractor {
        name: "livekit",
        extractor: try_livekit,
    },
    NamedExtractor {
        name: "mlflow",
        extractor: try_mlflow,
    },
    NamedExtractor {
        name: "traceloop",
        extractor: try_traceloop,
    },
    NamedExtractor {
        name: "pydantic_ai",
        extractor: try_pydantic_ai,
    },
    NamedExtractor {
        name: "langsmith",
        extractor: try_langsmith,
    },
    NamedExtractor {
        name: "langgraph",
        extractor: try_langgraph,
    },
    NamedExtractor {
        name: "autogen",
        extractor: try_autogen,
    },
    NamedExtractor {
        name: "crewai",
        extractor: try_crewai,
    },
    NamedExtractor {
        name: "raw_io",
        extractor: try_raw_io,
    },
];

pub(crate) fn extract_messages_from_attrs(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) {
    // Try extractors in priority order - stop at first match
    for named in EXTRACTORS {
        if (named.extractor)(messages, tool_definitions, attrs, span_name, timestamp) {
            tracing::trace!(
                extractor = named.name,
                span_name = span_name,
                messages_extracted = messages.len(),
                "Framework extractor matched"
            );
            return;
        }
    }

    tracing::trace!(span_name = span_name, "No framework extractor matched");
}

/// Extract tool definitions from any span (runs even on tool spans)
///
/// This is separate from `try_otel_genai_messages` because tool definitions
/// are metadata that should be extracted from tool execution spans too,
/// not just chat spans.
pub(crate) fn extract_tool_definitions(
    attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
) -> (Vec<RawToolDefinition>, Vec<RawToolNames>) {
    let mut tool_definitions = Vec::new();
    let mut tool_names = Vec::new();

    // gen_ai.tool.definitions - full tool schemas (JSON array)
    if let Some(definitions_json) = attrs.get(keys::GEN_AI_TOOL_DEFINITIONS) {
        if let Ok(content) = serde_json::from_str::<JsonValue>(definitions_json) {
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::GEN_AI_TOOL_DEFINITIONS,
                timestamp,
                content,
            ));
        }
    }

    // gen_ai.agent.tools - list of tool names (separate from full definitions)
    if let Some(tools_json) = attrs.get(keys::GEN_AI_AGENT_TOOLS) {
        if let Ok(content) = serde_json::from_str::<JsonValue>(tools_json) {
            tool_names.push(RawToolNames::from_attr(
                keys::GEN_AI_AGENT_TOOLS,
                timestamp,
                content,
            ));
        }
    }

    // CrewAI tool lists from agent/task metadata.
    // These attributes often coexist with event messages, so extraction must
    // happen in this always-on metadata path (not only in fallback attr parser).
    // Guard: only attempt CrewAI extraction when at least one CrewAI-specific
    // attribute is present, to avoid parsing input.value on every non-CrewAI span.
    if attrs.contains_key("crew_key")
        || attrs.contains_key("crew_id")
        || attrs.contains_key("crew_tasks")
        || attrs.contains_key("task_key")
        || attrs.contains_key("crew_agents")
    {
        append_crewai_tool_definitions(
            &mut tool_definitions,
            attrs,
            timestamp,
            &[keys::INPUT_VALUE, "crew_agents", "crew_tasks"],
        );
    }

    // ai.prompt.tools - Vercel AI SDK tool definitions
    // Note: Vercel AI sends this as an OTLP array where each element is a JSON string.
    // After extract_attributes(), we get a JSON array of strings: `["...", "..."]`.
    // We need to parse each string element to get the actual tool objects.
    if let Some(tools_json) = attrs.get(keys::AI_PROMPT_TOOLS) {
        if let Ok(content) = serde_json::from_str::<JsonValue>(tools_json) {
            let normalized = normalize_stringified_array(content);
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::AI_PROMPT_TOOLS,
                timestamp,
                normalized,
            ));
        }
    }

    // llm.tools - OpenInference tool definitions (single JSON attribute)
    if let Some(tools_json) = attrs.get(keys::LLM_TOOLS) {
        if let Ok(content) = serde_json::from_str::<JsonValue>(tools_json) {
            let normalized = normalize_stringified_array(content);
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::LLM_TOOLS,
                timestamp,
                normalized,
            ));
        }
    }

    // llm.tools.N.tool.json_schema - OpenInference indexed tool definitions (LangGraph)
    let tool_indices = extract_indices(attrs, "llm.tools");
    if !tool_indices.is_empty() {
        let mut tools = Vec::new();
        let mut names = Vec::new();
        for idx in tool_indices {
            let json_schema_key = format!("llm.tools.{}.tool.json_schema", idx);
            if let Some(schema_str) = attrs.get(&json_schema_key) {
                if let Ok(schema) = serde_json::from_str::<JsonValue>(schema_str) {
                    // Extract tool name from function definition
                    if let Some(name) = schema
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        names.push(json!(name));
                    }
                    tools.push(schema);
                }
            }
        }
        if !tools.is_empty() {
            tool_definitions.push(RawToolDefinition::from_attr(
                "llm.tools.N.tool.json_schema",
                timestamp,
                JsonValue::Array(tools),
            ));
        }
        if !names.is_empty() {
            tool_names.push(RawToolNames::from_attr(
                "llm.tools.N.tool.json_schema",
                timestamp,
                JsonValue::Array(names),
            ));
        }
    }

    // Assemble tool definition from individual gen_ai.tool.* attributes.
    // Only create if name looks like a valid identifier (starts with alphanumeric or underscore).
    // Filters framework-internal names like "(merged tools)" from Google ADK.
    if let Some(tool_name) = attrs.get(keys::GEN_AI_TOOL_NAME) {
        if tool_name.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            let description = attrs.get(keys::GEN_AI_TOOL_DESCRIPTION);
            let json_schema = attrs
                .get(keys::GEN_AI_TOOL_JSON_SCHEMA)
                .and_then(|s| serde_json::from_str::<JsonValue>(s).ok());

            let mut func = json!({ "name": tool_name });
            if let Some(desc) = description {
                func["description"] = json!(desc);
            }
            if let Some(schema) = json_schema {
                func["parameters"] = schema;
            }

            let content = json!([{
                "type": "function",
                "function": func
            }]);
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::GEN_AI_TOOL_NAME,
                timestamp,
                content,
            ));
        }
    }

    // OpenInference: tool.name + tool.description + tool.parameters (single tool per span)
    if tool_definitions.is_empty() {
        if let Some(tool_name) = attrs.get(keys::OI_TOOL_NAME) {
            let description = attrs.get(keys::OI_TOOL_DESCRIPTION);
            let parameters = attrs
                .get(keys::OI_TOOL_PARAMETERS)
                .and_then(|s| serde_json::from_str::<JsonValue>(s).ok());

            let mut func = json!({ "name": tool_name });
            if let Some(desc) = description {
                func["description"] = json!(desc);
            }
            if let Some(params) = parameters {
                func["parameters"] = params;
            }

            let content = json!([{
                "type": "function",
                "function": func
            }]);
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::OI_TOOL_NAME,
                timestamp,
                content,
            ));
        }
    }

    // response attribute - OpenAI Agents full API response with tools field
    if let Some(response_json) = attrs.get(keys::RESPONSE) {
        if let Ok(response) = serde_json::from_str::<JsonValue>(response_json) {
            if let Some(tools) = response.get("tools").and_then(|t| t.as_array()) {
                if !tools.is_empty() {
                    tool_definitions.push(RawToolDefinition::from_attr(
                        keys::RESPONSE,
                        timestamp,
                        JsonValue::Array(tools.clone()),
                    ));
                }
            }
        }
    }

    // request_data.tools - Logfire Chat Completions / Anthropic Messages
    // Older logfire versions (< 4.20) don't set gen_ai.tool.definitions separately;
    // tools are only inside the request_data JSON payload.
    if tool_definitions.is_empty() {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::REQUEST_DATA) {
            if let Some(tools) = parsed.get("tools").and_then(|t| t.as_array()) {
                if !tools.is_empty() {
                    tool_definitions.push(RawToolDefinition::from_attr(
                        keys::REQUEST_DATA,
                        timestamp,
                        JsonValue::Array(tools.clone()),
                    ));
                }
            }
        }
    }

    (tool_definitions, tool_names)
}

/// Check if span is a tool execution span based on attributes.
/// Tool spans emit message events containing tool INPUT, not OUTPUT - skip them.
pub(crate) fn is_tool_execution_span(attrs: &HashMap<String, String>) -> bool {
    // Primary: gen_ai.operation.name == "execute_tool"
    if attrs
        .get(keys::GEN_AI_OPERATION_NAME)
        .is_some_and(|op| op == "execute_tool")
    {
        return true;
    }

    // OpenInference: openinference.span.kind == "TOOL"
    if attrs
        .get(keys::OPENINFERENCE_SPAN_KIND)
        .is_some_and(|k| k.eq_ignore_ascii_case("tool"))
    {
        return true;
    }

    // Tool execution indicators: has both tool name AND tool call ID
    // (having just tool name could be a chat span referencing tools)
    if attrs.contains_key(keys::GEN_AI_TOOL_NAME) && attrs.contains_key(keys::GEN_AI_TOOL_CALL_ID) {
        return true;
    }

    // ADK tool execution: has tool response attribute
    if attrs.contains_key(keys::GCP_VERTEX_TOOL_RESPONSE) {
        return true;
    }

    // Vercel AI: has ai.toolCall.name and ai.toolCall.id
    if attrs.contains_key(keys::AI_TOOLCALL_NAME) && attrs.contains_key(keys::AI_TOOLCALL_ID) {
        return true;
    }

    false
}

// ============================================================================
// FRAMEWORK-SPECIFIC EXTRACTORS
// ============================================================================

pub(crate) fn try_gen_ai_indexed(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let prompt_indices = extract_indices(attrs, "gen_ai.prompt");
    let completion_indices = extract_indices(attrs, "gen_ai.completion");

    if prompt_indices.is_empty() && completion_indices.is_empty() {
        return false;
    }

    for idx in prompt_indices {
        if let Some(msg) = extract_indexed_message(attrs, "gen_ai.prompt", idx, timestamp) {
            messages.push(msg);
        }
    }

    for idx in completion_indices {
        if let Some(msg) = extract_indexed_message(attrs, "gen_ai.completion", idx, timestamp) {
            messages.push(msg);
        }
    }

    !messages.is_empty()
}

/// OTEL standard GenAI messages (gen_ai.input/output.messages).
pub(crate) fn try_otel_genai_messages(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // gen_ai.system_instructions - system prompt as parts array (PydanticAI v2+)
    if let Some(instructions_json) = attrs.get(keys::GEN_AI_SYSTEM_INSTRUCTIONS) {
        if let Ok(parts) = serde_json::from_str::<JsonValue>(instructions_json) {
            let msg = json!({
                "role": "system",
                "parts": parts
            });
            messages.push(RawMessage::from_attr(
                keys::GEN_AI_SYSTEM_INSTRUCTIONS,
                timestamp,
                msg,
            ));
            found = true;
        }
    }

    // gen_ai.input.messages - store as-is, expand at query time in SideML pipeline
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_INPUT_MESSAGES) {
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_INPUT_MESSAGES,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // gen_ai.output.messages - store as-is, expand at query time in SideML pipeline
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::GEN_AI_OUTPUT_MESSAGES) {
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_OUTPUT_MESSAGES,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // pydantic_ai.all_messages - full conversation history on agent run spans
    // Wrap as context message so it normalizes correctly (array in content field)
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::PYDANTIC_AI_ALL_MESSAGES) {
        let msg = json!({
            "role": "context",
            "type": "conversation_history",
            "content": parsed
        });
        messages.push(RawMessage::from_attr(
            keys::PYDANTIC_AI_ALL_MESSAGES,
            timestamp,
            msg,
        ));
        found = true;
    }

    // gen_ai.tool.call.arguments - tool call input
    if let Some(args_json) = attrs.get(keys::GEN_AI_TOOL_CALL_ARGUMENTS) {
        let args = serde_json::from_str::<JsonValue>(args_json).unwrap_or(json!(args_json));
        let msg = json!({
            "role": "tool_call",
            "content": args
        });
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_TOOL_CALL_ARGUMENTS,
            timestamp,
            msg,
        ));
        found = true;
    }

    // gen_ai.tool.call.result - tool call output
    if let Some(result_json) = attrs.get(keys::GEN_AI_TOOL_CALL_RESULT) {
        let result = serde_json::from_str::<JsonValue>(result_json).unwrap_or(json!(result_json));
        let msg = json!({
            "role": "tool",
            "content": result
        });
        messages.push(RawMessage::from_attr(
            keys::GEN_AI_TOOL_CALL_RESULT,
            timestamp,
            msg,
        ));
        found = true;
    }

    // Tool definitions are now extracted by extract_tool_definitions() which runs on all spans
    // including tool execution spans. This avoids duplication and ensures tool defs are always captured.

    found
}

pub(crate) fn try_openinference(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // LLM input/output messages
    let input_indices = extract_indices(attrs, "llm.input_messages");
    let output_indices = extract_indices(attrs, "llm.output_messages");

    for idx in input_indices {
        if let Some(msg) =
            extract_openinference_message(attrs, "llm.input_messages", idx, timestamp)
        {
            messages.push(msg);
            found = true;
        }
    }

    for idx in output_indices {
        if let Some(msg) =
            extract_openinference_message(attrs, "llm.output_messages", idx, timestamp)
        {
            messages.push(msg);
            found = true;
        }
    }

    // llm.tools is extracted by extract_tool_definitions() which runs on all spans

    // retrieval.documents.N.* - Retrieved documents for RAG spans
    let retrieval_indices = extract_indices(attrs, "retrieval.documents");
    if !retrieval_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "retrieval.documents",
            &retrieval_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    // reranker.input_documents.N.* and reranker.output_documents.N.*
    let reranker_input_indices = extract_indices(attrs, "reranker.input_documents");
    let reranker_output_indices = extract_indices(attrs, "reranker.output_documents");

    if !reranker_input_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "reranker.input_documents",
            &reranker_input_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    if !reranker_output_indices.is_empty() {
        if let Some(msg) = extract_openinference_documents(
            attrs,
            "reranker.output_documents",
            &reranker_output_indices,
            timestamp,
        ) {
            messages.push(msg);
            found = true;
        }
    }

    // reranker.query - Reranker query string
    if let Some(query) = attrs.get(keys::RERANKER_QUERY) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("user"));
        msg.insert("content".to_string(), json!(query));
        msg.insert("_source".to_string(), json!("reranker.query"));
        messages.push(RawMessage::from_attr(
            keys::RERANKER_QUERY,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // embedding.text - Input text for embedding spans
    if let Some(text) = attrs.get(keys::EMBEDDING_TEXT) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("user"));
        msg.insert("content".to_string(), json!(text));
        msg.insert("_source".to_string(), json!("embedding.text"));
        messages.push(RawMessage::from_attr(
            keys::EMBEDDING_TEXT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    if found {
        enrich_oi_multimodal_from_input_value(messages, attrs);
    }

    found
}

/// Enrich OpenInference multimodal messages with richer content from `input.value`.
///
/// OI dotted attributes lose file blocks and use `__REDACTED__` URLs.
/// `input.value` contains the complete LangChain-serialized content with all blocks.
/// For user messages with multimodal `contents.*` dotted keys, replace the dotted-key
/// content with the richer content array from `input.value`.
fn enrich_oi_multimodal_from_input_value(
    messages: &mut [RawMessage],
    attrs: &HashMap<String, String>,
) {
    // Fast path: check if any extracted message has multimodal contents.* dotted keys
    let has_multimodal = messages.iter().any(|m| {
        if let Some(obj) = m.content.as_object() {
            obj.keys().any(|k| k.starts_with("contents."))
        } else {
            false
        }
    });
    if !has_multimodal {
        return;
    }

    // Parse input.value
    let input_json = match attrs.get(keys::INPUT_VALUE) {
        Some(v) => v,
        None => return,
    };
    let parsed = match serde_json::from_str::<JsonValue>(input_json) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Navigate to message array: {"messages": [[m1,m2]]} or {"messages": [m1,m2]} or [m1,m2]
    let msg_array = find_input_value_messages(&parsed);
    let msg_array = match msg_array {
        Some(arr) if !arr.is_empty() => arr,
        _ => return,
    };

    // LangChain format guard: verify messages have LangChain structure
    let is_langchain = msg_array.iter().any(|m| {
        m.get("id").and_then(|v| v.as_array()).is_some()
            || m.get("lc").is_some()
            || m.get("kwargs").is_some()
    });
    if !is_langchain {
        return;
    }

    // Build index map: OI message index → input.value content
    // OI source keys look like "llm.input_messages.N.message"
    for msg in messages.iter_mut() {
        let oi_index = match extract_oi_message_index(msg) {
            Some(idx) => idx,
            None => continue,
        };

        // Check this message has multimodal contents.* keys
        let has_contents = msg
            .content
            .as_object()
            .is_some_and(|obj| obj.keys().any(|k| k.starts_with("contents.")));
        if !has_contents {
            continue;
        }

        // Get corresponding input.value message
        let iv_msg = match msg_array.get(oi_index) {
            Some(m) => m,
            None => continue,
        };

        // Extract content via LangChain format
        let iv_content = match extract_langchain_content(iv_msg) {
            Some(c) => c,
            None => continue,
        };

        // Only replace if input.value has array content (multimodal).
        // input.value is strictly higher quality than OI dotted keys:
        // real URLs instead of __REDACTED__, all block types preserved.
        if !iv_content.is_array() {
            continue;
        }

        // Replace: remove all contents.* keys, set content = array
        if let Some(obj) = msg.content.as_object_mut() {
            let keys_to_remove: Vec<String> = obj
                .keys()
                .filter(|k| k.starts_with("contents."))
                .cloned()
                .collect();
            for key in keys_to_remove {
                obj.remove(&key);
            }
            obj.insert("content".to_string(), iv_content);
        }
    }
}

/// Find message array in input.value JSON.
/// Handles: {"messages": [[m1,m2]]}, {"messages": [m1,m2]}, [m1,m2]
fn find_input_value_messages(parsed: &JsonValue) -> Option<&Vec<JsonValue>> {
    // {"messages": ...}
    if let Some(msgs) = parsed.get("messages") {
        if let Some(arr) = msgs.as_array() {
            // Nested: {"messages": [[m1,m2]]}
            if arr.len() == 1 {
                if let Some(inner) = arr[0].as_array() {
                    return Some(inner);
                }
            }
            // Direct: {"messages": [m1,m2]}
            return Some(arr);
        }
    }
    // Direct array: [m1,m2]
    parsed.as_array()
}

/// Extract the OI message index from a RawMessage source key.
/// Source keys look like "llm.input_messages.N.message" → returns N.
fn extract_oi_message_index(msg: &RawMessage) -> Option<usize> {
    let key = match &msg.source {
        MessageSource::Attribute { key, .. } => key,
        _ => return None,
    };
    // Pattern: "llm.input_messages.N.message" or "llm.input_messages.N"
    if !key.starts_with("llm.input_messages.") {
        return None;
    }
    let rest = key.strip_prefix("llm.input_messages.")?;
    let idx_str = rest.split('.').next()?;
    idx_str.parse().ok()
}

pub(crate) fn try_logfire_events(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Try "events" attribute
    if let Some(events_json) = attrs.get(keys::EVENTS) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(events_json) {
            found |= extract_logfire_event_array(messages, parsed, keys::EVENTS, timestamp);
        }
    }

    // Also check "prompt" attribute for input
    if let Some(prompt_json) = attrs.get(keys::PROMPT) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(prompt_json) {
            for msg in parsed {
                if let JsonValue::Object(raw) = msg {
                    messages.push(RawMessage::from_attr(
                        keys::PROMPT,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
        }
    }

    // Also check "all_messages_events" attribute for output
    if let Some(all_msgs_json) = attrs.get(keys::ALL_MESSAGES_EVENTS) {
        if let Ok(parsed) = serde_json::from_str::<Vec<JsonValue>>(all_msgs_json) {
            for msg in parsed {
                if let JsonValue::Object(raw) = msg {
                    messages.push(RawMessage::from_attr(
                        keys::ALL_MESSAGES_EVENTS,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
        }
    }

    // Logfire Chat Completions / Anthropic Messages: request_data/response_data.
    // Stored as-is — structural extraction happens at query time
    // via MESSAGE_ARRAY_SOURCES expansion in normalize.rs.
    if !found {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::REQUEST_DATA) {
            if parsed
                .get("messages")
                .and_then(|m| m.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                messages.push(RawMessage::from_attr(keys::REQUEST_DATA, timestamp, parsed));
                found = true;
            }
        }
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::RESPONSE_DATA) {
            // Non-streaming: {message: {role, ...}, usage: {...}}
            let has_message = parsed.get("message").is_some_and(|m| m.is_object());
            // Streaming: {combined_chunk_content: "...", chunk_count: N}
            let has_streaming = parsed
                .get("combined_chunk_content")
                .and_then(|c| c.as_str())
                .is_some_and(|s| !s.is_empty());
            if has_message || has_streaming {
                messages.push(RawMessage::from_attr(
                    keys::RESPONSE_DATA,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    found
}

/// Extract messages from Logfire event array.
///
/// Logfire embeds OTEL-style events in an attribute as a JSON array.
/// Each event has "event.name" that can be used for query-time role derivation.
/// We preserve the raw content including event.name for query-time processing.
fn extract_logfire_event_array(
    messages: &mut Vec<RawMessage>,
    events: Vec<JsonValue>,
    _source_key: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // First pass: extract recognized message events
    for event in &events {
        let Some(raw) = event.as_object() else {
            continue;
        };

        let event_name = raw.get("event.name").and_then(|e| e.as_str()).unwrap_or("");

        if !is_message_event(event_name) {
            continue;
        }

        messages.push(RawMessage::from_event(event_name, timestamp, event.clone()));
        found = true;
    }

    // Second pass: collect unrecognized events with structured data.
    // Logfire emits multimodal content blocks (input_text, input_image,
    // input_file, etc.) as gen_ai.unknown events. The actual content is in
    // the `data` object, while `content` is a human-readable summary.
    // Group consecutive same-role blocks into a single synthetic message.
    let mut pending_blocks: Vec<JsonValue> = Vec::new();
    let mut pending_role: Option<&str> = None;

    for event in &events {
        let Some(raw) = event.as_object() else {
            continue;
        };
        let event_name = raw.get("event.name").and_then(|e| e.as_str()).unwrap_or("");
        if is_message_event(event_name) {
            continue;
        }

        let Some(data) = raw.get("data").filter(|d| d.is_object()) else {
            continue;
        };
        let Some(data_type) = data.get("type").and_then(|t| t.as_str()) else {
            continue;
        };

        let role = if data_type.starts_with("input_") {
            "user"
        } else if data_type.starts_with("output_") {
            "assistant"
        } else {
            continue;
        };

        // Flush if role changed
        if let Some(pr) = pending_role {
            if pr != role {
                let event_name = if pr == "user" {
                    keys::EVENT_USER_MESSAGE
                } else {
                    keys::EVENT_ASSISTANT_MESSAGE
                };
                let blocks = std::mem::take(&mut pending_blocks);
                let msg = json!({"role": pr, "content": JsonValue::Array(blocks)});
                messages.push(RawMessage::from_event(event_name, timestamp, msg));
            }
        }
        pending_role = Some(role);
        pending_blocks.push(data.clone());
        found = true;
    }

    // Final flush
    if let Some(role) = pending_role {
        if !pending_blocks.is_empty() {
            let event_name = if role == "user" {
                keys::EVENT_USER_MESSAGE
            } else {
                keys::EVENT_ASSISTANT_MESSAGE
            };
            let blocks = std::mem::take(&mut pending_blocks);
            let msg = json!({"role": role, "content": JsonValue::Array(blocks)});
            messages.push(RawMessage::from_event(event_name, timestamp, msg));
        }
    }

    found
}

pub(crate) fn try_vercel_ai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Input messages - try ai.prompt.messages first, then ai.prompt
    let prompt_json = attrs
        .get(keys::AI_PROMPT_MESSAGES)
        .or_else(|| attrs.get(keys::AI_PROMPT));

    if let Some(prompt_json) = prompt_json {
        match serde_json::from_str::<Vec<JsonValue>>(prompt_json) {
            Ok(prompt_msgs) => {
                tracing::debug!(
                    prompt_count = prompt_msgs.len(),
                    "try_vercel_ai parsed prompt messages"
                );
                for msg_val in prompt_msgs {
                    let raw = match msg_val {
                        JsonValue::Object(map) => map,
                        _ => continue,
                    };
                    messages.push(RawMessage::from_attr(
                        keys::AI_PROMPT_MESSAGES,
                        timestamp,
                        JsonValue::Object(raw),
                    ));
                    found = true;
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    prompt_json_preview = %prompt_json.chars().take(100).collect::<String>(),
                    "try_vercel_ai failed to parse prompt messages"
                );
            }
        }
    }

    // Tool definitions are extracted by extract_tool_definitions() which runs on all spans

    // Tool call input
    if let Some(tool_args) = attrs.get(keys::AI_TOOLCALL_ARGS) {
        let tool_name = attrs.get(keys::AI_TOOLCALL_NAME).map(|s| s.as_str());
        let tool_id = attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
        let args_val = serde_json::from_str::<JsonValue>(tool_args).unwrap_or(json!(tool_args));
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool_call"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), args_val);
        messages.push(RawMessage::from_attr(
            keys::AI_TOOLCALL_ARGS,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // Response - collect ai.response.* attributes with fallback to ai.result.*, then output.value
    let mut raw = serde_json::Map::new();

    // Check if this looks like a Vercel span before using output.value fallback
    // Only fall back to output.value if we have other Vercel-specific indicators
    // Including span name pattern (ai.* spans like ai.generateText)
    let has_vercel_indicators = attrs.contains_key(keys::AI_PROMPT_MESSAGES)
        || attrs.contains_key(keys::AI_PROMPT)
        || attrs.contains_key("ai.response.text")
        || attrs.contains_key(keys::AI_RESULT_TEXT)
        || attrs.contains_key("ai.response.toolCalls")
        || attrs.contains_key(keys::AI_RESULT_TOOL_CALLS)
        || attrs.contains_key(keys::AI_TOOLCALL_NAME)
        || span_name.starts_with("ai.");

    // Text content - try new attribute first, then legacy
    // Only fall back to output.value if we have evidence this is a Vercel span
    let text = attrs
        .get("ai.response.text")
        .or_else(|| attrs.get(keys::AI_RESULT_TEXT))
        .or_else(|| {
            if has_vercel_indicators {
                attrs.get(keys::OUTPUT_VALUE)
            } else {
                None
            }
        });
    if let Some(t) = text {
        raw.insert("content".to_string(), json!(t));
    }

    // Tool calls - try new attribute first, then legacy
    let tool_calls = attrs
        .get("ai.response.toolCalls")
        .or_else(|| attrs.get(keys::AI_RESULT_TOOL_CALLS));
    if let Some(tc) = tool_calls {
        let parsed = serde_json::from_str::<JsonValue>(tc).unwrap_or(json!(tc));
        raw.insert("tool_calls".to_string(), parsed); // Use snake_case for SideML
    }

    // Structured object - try new attribute first, then legacy
    let object = attrs
        .get("ai.response.object")
        .or_else(|| attrs.get(keys::AI_RESULT_OBJECT));
    if let Some(obj) = object {
        let parsed = parse_json_with_fallback(obj, "ai.result.object");
        raw.insert("object".to_string(), parsed);
    }

    // Collect any other ai.response.* attributes (but not text/toolCalls/object)
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix("ai.response.") {
            if suffix != "text" && suffix != "toolCalls" && suffix != "object" {
                let json_val = if value.starts_with('{') || value.starts_with('[') {
                    parse_json_with_fallback(value, &format!("ai.response.{}", suffix))
                } else {
                    json!(value)
                };
                raw.insert(suffix.to_string(), json_val);
            }
        }
    }

    if !raw.is_empty() {
        raw.insert("role".to_string(), json!("assistant"));
        messages.push(RawMessage::from_attr(
            "ai.response",
            timestamp,
            JsonValue::Object(raw),
        ));
        found = true;
    }

    // Tool call result
    if let Some(tool_result) = attrs.get(keys::AI_TOOLCALL_RESULT) {
        let tool_name = attrs.get(keys::AI_TOOLCALL_NAME).map(|s| s.as_str());
        let tool_id = attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
        let result_val =
            serde_json::from_str::<JsonValue>(tool_result).unwrap_or(json!(tool_result));
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), result_val);
        messages.push(RawMessage::from_attr(
            keys::AI_TOOLCALL_RESULT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    found
}

pub(crate) fn try_google_adk(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // LLM spans - detect by presence of request/response attributes (not span name)
    // Handle empty "{}" - fall back to tool_call_args when request is empty
    if let Some(request_json) = attrs.get(keys::GCP_VERTEX_LLM_REQUEST) {
        if let Ok(request) = serde_json::from_str::<JsonValue>(request_json) {
            // Check if request is meaningful (not empty object)
            let is_empty = matches!(&request, JsonValue::Object(m) if m.is_empty());
            if !is_empty {
                found |=
                    extract_adk_request_messages(messages, tool_definitions, &request, timestamp);
            }
        }
    }

    // Fall back to tool_call_args when llm_request is empty or missing
    if !found {
        if let Some(tool_args_json) = attrs.get(keys::GCP_VERTEX_TOOL_CALL_ARGS) {
            if let Ok(tool_args) = serde_json::from_str::<JsonValue>(tool_args_json) {
                let msg = json!({
                    "role": "tool_call",
                    "content": tool_args
                });
                messages.push(RawMessage::from_attr(
                    keys::GCP_VERTEX_TOOL_CALL_ARGS,
                    timestamp,
                    msg,
                ));
                found = true;
            }
        }
    }

    // Response - try llm_response first
    if let Some(response_json) = attrs.get(keys::GCP_VERTEX_LLM_RESPONSE) {
        if let Ok(response) = serde_json::from_str::<JsonValue>(response_json) {
            // Check if response is meaningful (not empty object)
            let is_empty = matches!(&response, JsonValue::Object(m) if m.is_empty());
            if !is_empty {
                found |= extract_adk_response_message(messages, &response, timestamp);
            }
        }
    }

    // Tool spans - detect by presence of tool response attribute (not span name)
    if let Some(response_json) = attrs.get(keys::GCP_VERTEX_TOOL_RESPONSE) {
        if let Ok(response) = serde_json::from_str::<JsonValue>(response_json) {
            let msg = json!({
                "role": "tool",
                "content": response
            });
            messages.push(RawMessage::from_attr(
                keys::GCP_VERTEX_TOOL_RESPONSE,
                timestamp,
                msg,
            ));
            found = true;
        }
    }

    // gcp.vertex.agent.data - data sent to agent (conversation history from trace_send_data)
    if let Some(data_json) = attrs.get(keys::GCP_VERTEX_DATA) {
        if let Ok(data) = serde_json::from_str::<JsonValue>(data_json) {
            // Check if data is meaningful (not empty)
            let is_empty = matches!(&data, JsonValue::Object(m) if m.is_empty())
                || matches!(&data, JsonValue::Array(a) if a.is_empty());
            if !is_empty {
                let msg = json!({
                    "role": "data",
                    "type": "conversation_history",
                    "content": data
                });
                messages.push(RawMessage::from_attr(keys::GCP_VERTEX_DATA, timestamp, msg));
                found = true;
            }
        }
    }

    found
}

/// Extract messages from ADK LLM request.
/// Format: {model, config: {system_instruction, tools}, contents: [{parts, role}, ...]}
/// Also handles Vertex AI native format: {systemInstruction: {parts: [...]}, contents: [...]}
fn extract_adk_request_messages(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    request: &JsonValue,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Extract system instruction - try multiple formats:
    // 1. Vertex AI native: systemInstruction.parts (camelCase, object with parts)
    // 2. Vertex AI native: systemInstruction as string
    // 3. ADK format: config.system_instruction (snake_case, string)
    let system_msg = request
        .get("systemInstruction")
        .and_then(|si| {
            // Try as object with parts first
            if let Some(parts) = si.get("parts") {
                Some(json!({
                    "role": "system",
                    "content": parts
                }))
            } else {
                // Fall back to string
                si.as_str()
                    .filter(|s| !s.is_empty())
                    .map(|text| json!({"role": "system", "content": text}))
            }
        })
        .or_else(|| {
            // ADK format: config.system_instruction as string
            request
                .get("config")
                .and_then(|c| c.get("system_instruction"))
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(|text| {
                    json!({
                        "role": "system",
                        "content": text
                    })
                })
        });

    if let Some(msg) = system_msg {
        messages.push(RawMessage::from_attr(
            keys::GCP_VERTEX_LLM_REQUEST,
            timestamp,
            msg,
        ));
        found = true;
    }

    // Extract tools - try multiple formats:
    // 1. Vertex AI native: top-level tools array
    // 2. ADK format: config.tools
    let tools = request
        .get("tools")
        .and_then(|t| t.as_array())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            request
                .get("config")
                .and_then(|c| c.get("tools"))
                .and_then(|t| t.as_array())
                .filter(|t| !t.is_empty())
        });

    if let Some(tools) = tools {
        // Unwrap function_declarations (snake_case ADK) or functionDeclarations (camelCase Vertex)
        let mut flattened = Vec::new();
        for tool_group in tools {
            let decls = tool_group
                .get("function_declarations")
                .or_else(|| tool_group.get("functionDeclarations"))
                .and_then(|fd| fd.as_array());
            if let Some(decls) = decls {
                flattened.extend(decls.iter().cloned());
            } else {
                flattened.push(tool_group.clone());
            }
        }
        if !flattened.is_empty() {
            tool_definitions.push(RawToolDefinition::from_attr(
                keys::GCP_VERTEX_LLM_REQUEST,
                timestamp,
                JsonValue::Array(flattened),
            ));
        }
        found = true;
    }

    // Extract messages from contents array
    if let Some(contents) = request.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            // Convert Gemini format: {parts, role} -> {role, content: parts}
            if let (Some(role), Some(parts)) = (
                content.get("role").and_then(|r| r.as_str()),
                content.get("parts"),
            ) {
                let msg = json!({
                    "role": role,
                    "content": parts
                });
                messages.push(RawMessage::from_attr(
                    keys::GCP_VERTEX_LLM_REQUEST,
                    timestamp,
                    msg,
                ));
                found = true;
            }
        }
    }

    found
}

/// Extract message from ADK LLM response.
/// Format: {model_version, content: {parts, role}, finish_reason, usage_metadata}
fn extract_adk_response_message(
    messages: &mut Vec<RawMessage>,
    response: &JsonValue,
    timestamp: DateTime<Utc>,
) -> bool {
    let Some(content) = response.get("content") else {
        return false;
    };

    let (Some(role), Some(parts)) = (
        content.get("role").and_then(|r| r.as_str()),
        content.get("parts"),
    ) else {
        return false;
    };

    let finish_reason = response
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .map(|s| s.to_lowercase());

    let mut msg = json!({
        "role": if role == "model" { "assistant" } else { role },
        "content": parts
    });
    if let Some(fr) = finish_reason {
        msg["finish_reason"] = json!(fr);
    }
    messages.push(RawMessage::from_attr(
        keys::GCP_VERTEX_LLM_RESPONSE,
        timestamp,
        msg,
    ));
    true
}

/// LiveKit message extraction
pub(crate) fn try_livekit(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // lk.instructions - system instructions
    if let Some(instructions) = attrs.get(keys::LK_INSTRUCTIONS) {
        if !instructions.is_empty() {
            let msg = json!({
                "role": "system",
                "content": instructions
            });
            messages.push(RawMessage::from_attr(keys::LK_INSTRUCTIONS, timestamp, msg));
            found = true;
        }
    }

    // lk.function_tools - tool definitions
    if let Some(content) = extract_json::<JsonValue>(attrs, keys::LK_FUNCTION_TOOLS) {
        tool_definitions.push(RawToolDefinition::from_attr(
            keys::LK_FUNCTION_TOOLS,
            timestamp,
            content,
        ));
        found = true;
    }

    // lk.chat_ctx - chat context (full conversation history)
    if let Some(ctx) = extract_json::<JsonValue>(attrs, keys::LK_CHAT_CTX) {
        let msg = json!({
            "role": "context",
            "type": "chat_history",
            "content": ctx
        });
        messages.push(RawMessage::from_attr(keys::LK_CHAT_CTX, timestamp, msg));
        found = true;
    }

    // lk.user_input or lk.input_text - user input
    let input = attrs
        .get(keys::LK_USER_INPUT)
        .or_else(|| attrs.get(keys::LK_INPUT_TEXT));
    if let Some(text) = input {
        if !text.is_empty() {
            let msg = json!({
                "role": "user",
                "content": text
            });
            let key = if attrs.contains_key(keys::LK_USER_INPUT) {
                keys::LK_USER_INPUT
            } else {
                keys::LK_INPUT_TEXT
            };
            messages.push(RawMessage::from_attr(key, timestamp, msg));
            found = true;
        }
    }

    // lk.function_tool.arguments - tool call input
    if let Some(args) = attrs.get(keys::LK_FUNCTION_TOOL_ARGS) {
        let tool_name = attrs.get(keys::LK_FUNCTION_TOOL_NAME);
        let tool_id = attrs.get(keys::LK_FUNCTION_TOOL_ID);
        let args_val = serde_json::from_str::<JsonValue>(args).unwrap_or(json!(args));

        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool_call"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), args_val);
        messages.push(RawMessage::from_attr(
            keys::LK_FUNCTION_TOOL_ARGS,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // lk.function_tool.output - tool result
    if let Some(output) = attrs.get(keys::LK_FUNCTION_TOOL_OUTPUT) {
        let tool_name = attrs.get(keys::LK_FUNCTION_TOOL_NAME);
        let tool_id = attrs.get(keys::LK_FUNCTION_TOOL_ID);
        let is_error = attrs
            .get(keys::LK_FUNCTION_TOOL_IS_ERROR)
            .is_some_and(|v| v == "true");
        let output_val = serde_json::from_str::<JsonValue>(output).unwrap_or(json!(output));

        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("tool"));
        if let Some(name) = tool_name {
            msg.insert("name".to_string(), json!(name));
        }
        if let Some(id) = tool_id {
            msg.insert("tool_call_id".to_string(), json!(id));
        }
        msg.insert("content".to_string(), output_val);
        if is_error {
            msg.insert("is_error".to_string(), json!(true));
        }
        messages.push(RawMessage::from_attr(
            keys::LK_FUNCTION_TOOL_OUTPUT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    }

    // lk.response.text - assistant response text
    if let Some(text) = attrs.get(keys::LK_RESPONSE_TEXT) {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("assistant"));
        msg.insert("content".to_string(), json!(text));

        // lk.response.function_calls - tool calls in response
        if let Some(calls) = extract_json::<JsonValue>(attrs, keys::LK_RESPONSE_FUNCTION_CALLS) {
            msg.insert("tool_calls".to_string(), calls);
        }

        messages.push(RawMessage::from_attr(
            keys::LK_RESPONSE_TEXT,
            timestamp,
            JsonValue::Object(msg),
        ));
        found = true;
    } else if let Some(calls) = extract_json::<JsonValue>(attrs, keys::LK_RESPONSE_FUNCTION_CALLS) {
        // Response with only function calls (no text)
        let msg = json!({
            "role": "assistant",
            "tool_calls": calls
        });
        messages.push(RawMessage::from_attr(
            keys::LK_RESPONSE_FUNCTION_CALLS,
            timestamp,
            msg,
        ));
        found = true;
    }

    found
}

/// MLflow message extraction
pub(crate) fn try_mlflow(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // mlflow.spanInputs - JSON string for input
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::MLFLOW_SPAN_INPUTS) {
        messages.push(RawMessage::from_attr(
            keys::MLFLOW_SPAN_INPUTS,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // mlflow.spanOutputs - JSON string for output
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::MLFLOW_SPAN_OUTPUTS) {
        messages.push(RawMessage::from_attr(
            keys::MLFLOW_SPAN_OUTPUTS,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // mlflow.chat.tools - JSON array of tool definitions
    if let Some(content) = extract_json::<JsonValue>(attrs, keys::MLFLOW_CHAT_TOOLS) {
        tool_definitions.push(RawToolDefinition::from_attr(
            keys::MLFLOW_CHAT_TOOLS,
            timestamp,
            content,
        ));
        found = true;
    }

    found
}

/// TraceLoop message extraction
pub(crate) fn try_traceloop(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // traceloop.entity.input - JSON string
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::TRACELOOP_ENTITY_INPUT) {
        messages.push(RawMessage::from_attr(
            keys::TRACELOOP_ENTITY_INPUT,
            timestamp,
            parsed,
        ));
        found = true;
    }

    // traceloop.entity.output - JSON string
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::TRACELOOP_ENTITY_OUTPUT) {
        messages.push(RawMessage::from_attr(
            keys::TRACELOOP_ENTITY_OUTPUT,
            timestamp,
            parsed,
        ));
        found = true;
    }

    found
}

/// Pydantic AI (via Logfire) message extraction
pub(crate) fn try_pydantic_ai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // tool_arguments - tool call input
    if let Some(args) = attrs.get(keys::TOOL_ARGUMENTS) {
        let content = serde_json::from_str::<JsonValue>(args).unwrap_or(json!(args));
        let msg = json!({
            "role": "tool_call",
            "content": content
        });
        messages.push(RawMessage::from_attr(keys::TOOL_ARGUMENTS, timestamp, msg));
        found = true;
    }

    // tool_response - tool call output
    if let Some(response) = attrs.get(keys::TOOL_RESPONSE) {
        let content = serde_json::from_str::<JsonValue>(response).unwrap_or(json!(response));
        let msg = json!({
            "role": "tool",
            "content": content
        });
        messages.push(RawMessage::from_attr(keys::TOOL_RESPONSE, timestamp, msg));
        found = true;
    }

    found
}

pub(crate) fn try_langsmith(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // LangSmith detection: must have langsmith.* attributes
    let is_langsmith = attrs.contains_key(keys::LANGSMITH_SPAN_KIND)
        || attrs.contains_key(keys::LANGSMITH_TRACE_SESSION_ID)
        || attrs.contains_key(keys::LANGSMITH_TRACE_NAME)
        || attrs.keys().any(|k| k.starts_with("langsmith."));

    if !is_langsmith {
        return false;
    }

    let mut found = false;

    // LangSmith uses gen_ai.prompt for full input JSON (messages array)
    if let Some(prompt_json) = attrs.get(keys::GEN_AI_PROMPT) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(prompt_json) {
            // Extract messages array from prompt
            if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_array()) {
                for msg in msgs {
                    if msg.get("role").is_some() && msg.get("content").is_some() {
                        messages.push(RawMessage::from_attr(
                            keys::GEN_AI_PROMPT,
                            timestamp,
                            msg.clone(),
                        ));
                        found = true;
                    }
                }
            } else if parsed.get("role").is_some() && parsed.get("content").is_some() {
                // Single message format
                messages.push(RawMessage::from_attr(
                    keys::GEN_AI_PROMPT,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    // LangSmith uses gen_ai.completion for full output JSON
    if let Some(completion_json) = attrs.get(keys::GEN_AI_COMPLETION) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(completion_json) {
            // Extract message from choices array (OpenAI format)
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    if let Some(msg) = choice.get("message") {
                        if msg.get("role").is_some() || msg.get("content").is_some() {
                            let mut output_msg = msg.clone();
                            // Add finish_reason if present
                            if let Some(fr) = choice.get("finish_reason") {
                                output_msg["finish_reason"] = fr.clone();
                            }
                            messages.push(RawMessage::from_attr(
                                keys::GEN_AI_COMPLETION,
                                timestamp,
                                output_msg,
                            ));
                            found = true;
                        }
                    }
                }
            } else if parsed.get("role").is_some() || parsed.get("content").is_some() {
                // Direct message format
                messages.push(RawMessage::from_attr(
                    keys::GEN_AI_COMPLETION,
                    timestamp,
                    parsed,
                ));
                found = true;
            }
        }
    }

    found
}

pub(crate) fn try_langgraph(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // LangGraph detection: must have langgraph.* attributes or langgraph metadata
    let is_langgraph = attrs.contains_key(keys::LANGGRAPH_NODE)
        || attrs.contains_key(keys::LANGGRAPH_CHECKPOINT_NS)
        || attrs.contains_key(keys::LANGGRAPH_THREAD_ID)
        || attrs
            .get(keys::METADATA)
            .is_some_and(|m| m.contains("langgraph_"));

    if !is_langgraph {
        return false;
    }

    let mut found = false;

    // Try to extract from input.value (node inputs, may contain messages)
    if let Some(input_json) = attrs.get(keys::INPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(input_json) {
            if extract_langgraph_messages(messages, &parsed, keys::INPUT_VALUE, timestamp) {
                found = true;
            }
        }
    }

    // Try to extract from output.value (node outputs, may contain messages)
    if let Some(output_json) = attrs.get(keys::OUTPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(output_json) {
            if extract_langgraph_messages(messages, &parsed, keys::OUTPUT_VALUE, timestamp) {
                found = true;
            }
        }
    }

    // Try to extract from message attribute (single message)
    if let Some(msg_json) = attrs.get(keys::MESSAGE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(msg_json) {
            if let Some(normalized) = normalize_langchain_message(&parsed) {
                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, normalized));
                found = true;
            }
        }
    }

    found
}

/// Extract messages from LangGraph state (handles nested messages in dicts/lists)
fn extract_langgraph_messages(
    messages: &mut Vec<RawMessage>,
    value: &JsonValue,
    source_key: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Check if value is a LangChain message type
    if let Some(normalized) = normalize_langchain_message(value) {
        messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
        return true;
    }

    // Check for messages array in state dict
    if let Some(obj) = value.as_object() {
        // Look for "messages" key (common LangGraph pattern)
        if let Some(msgs_value) = obj.get("messages") {
            if let Some(msgs_array) = msgs_value.as_array() {
                for msg in msgs_array {
                    if let Some(normalized) = normalize_langchain_message(msg) {
                        messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                        found = true;
                    }
                }
            }
        }

        // Also check for direct message values in object
        for (_key, val) in obj {
            if let Some(normalized) = normalize_langchain_message(val) {
                messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                found = true;
            }
        }
    }

    // Check if value is an array of messages
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(normalized) = normalize_langchain_message(item) {
                messages.push(RawMessage::from_attr(source_key, timestamp, normalized));
                found = true;
            }
        }
    }

    found
}

/// Normalize LangChain message types to standard SideML format
fn normalize_langchain_message(msg: &JsonValue) -> Option<JsonValue> {
    // Check for LangChain message type discriminator
    let msg_type = msg.get("type").and_then(|t| t.as_str());

    // LangChain messages have lc_type or type field
    let lc_type = msg
        .get("lc")
        .and_then(|lc| lc.get("type"))
        .and_then(|t| t.as_str());

    // Also check kwargs.type for serialized LangChain messages
    let kwargs_type = msg
        .get("kwargs")
        .and_then(|k| k.get("type"))
        .and_then(|t| t.as_str());

    let effective_type = msg_type.or(lc_type).or(kwargs_type);

    match effective_type {
        Some("human") | Some("HumanMessage") => {
            let content = extract_langchain_content(msg)?;
            Some(json!({
                "role": "user",
                "content": content
            }))
        }
        Some("ai") | Some("AIMessage") => {
            let content = extract_langchain_content(msg)?;
            let mut result = json!({
                "role": "assistant",
                "content": content
            });

            // Extract tool_calls if present
            if let Some(tool_calls) = msg
                .get("tool_calls")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("tool_calls")))
            {
                if tool_calls.is_array() && !tool_calls.as_array().unwrap().is_empty() {
                    result["tool_calls"] = tool_calls.clone();
                }
            }

            // Extract additional_kwargs for function calls
            if let Some(additional) = msg
                .get("additional_kwargs")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("additional_kwargs")))
            {
                if let Some(fc) = additional.get("function_call") {
                    result["function_call"] = fc.clone();
                }
                if let Some(tc) = additional.get("tool_calls") {
                    if result.get("tool_calls").is_none() {
                        result["tool_calls"] = tc.clone();
                    }
                }
            }

            Some(result)
        }
        Some("system") | Some("SystemMessage") => {
            let content = extract_langchain_content(msg)?;
            Some(json!({
                "role": "system",
                "content": content
            }))
        }
        Some("tool") | Some("ToolMessage") => {
            let content = extract_langchain_content(msg)?;
            let mut result = json!({
                "role": "tool",
                "content": content
            });

            // Extract tool_call_id
            if let Some(tool_call_id) = msg
                .get("tool_call_id")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("tool_call_id")))
                .and_then(|v| v.as_str())
            {
                result["tool_call_id"] = json!(tool_call_id);
            }

            // Extract name
            if let Some(name) = msg
                .get("name")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("name")))
                .and_then(|v| v.as_str())
            {
                result["name"] = json!(name);
            }

            Some(result)
        }
        Some("function") | Some("FunctionMessage") => {
            let content = extract_langchain_content(msg)?;
            let name = msg
                .get("name")
                .or_else(|| msg.get("kwargs").and_then(|k| k.get("name")))
                .and_then(|v| v.as_str())
                .unwrap_or("function");

            Some(json!({
                "role": "function",
                "name": name,
                "content": content
            }))
        }
        // Check for standard role/content format (already normalized)
        _ => {
            if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
                if msg.get("content").is_some() {
                    return Some(msg.clone());
                }
                // Has role but missing content - try to extract
                if let Some(content) = extract_langchain_content(msg) {
                    return Some(json!({
                        "role": role,
                        "content": content
                    }));
                }
            }
            None
        }
    }
}

/// Extract content from LangChain message (handles various formats)
fn extract_langchain_content(msg: &JsonValue) -> Option<JsonValue> {
    // Direct content field
    if let Some(content) = msg.get("content") {
        return Some(content.clone());
    }

    // Content in kwargs (serialized LangChain format)
    if let Some(kwargs) = msg.get("kwargs") {
        if let Some(content) = kwargs.get("content") {
            return Some(content.clone());
        }
    }

    // Text field (some LangChain versions)
    if let Some(text) = msg.get("text") {
        return Some(text.clone());
    }

    None
}

pub(crate) fn try_autogen(
    messages: &mut Vec<RawMessage>,
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let mut found = false;

    // Claim AutoGen OpenInference spans (agent/chain spans with cancellation_token
    // or output_task_messages). These aggregate data from child spans — actual messages
    // come from child `autogen process` spans. The input.value contains Python repr()
    // garbage, so we claim without extracting.
    if let Some(input_val) = attrs.get(keys::INPUT_VALUE) {
        if input_val.contains("cancellation_token") || input_val.contains("output_task_messages") {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(input_val) {
                if parsed.get("cancellation_token").is_some()
                    || parsed.get("output_task_messages").is_some()
                {
                    found = true;
                }
            }
        }
    }

    // Try to extract from message attribute (AutoGen message types)
    // OpenInference AutoGen uses formats like:
    // - {"messages":[{type:"TextMessage",...},...], "output_task_messages":true}
    // - {"message":{type:"ToolCallRequestEvent",...}}
    if let Some(json_str) = attrs.get(keys::MESSAGE) {
        if json_str != "No Message" && json_str != "{}" {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(json_str) {
                // Check if this is a typed AutoGen message directly
                let normalized = normalize_autogen_message(&parsed);
                if !normalized.is_empty() {
                    for n in normalized {
                        messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                    }
                    found = true;
                } else if is_autogen_skip_type(&parsed) {
                    found = true;
                } else if let Some(msgs_array) = parsed.get("messages").and_then(|m| m.as_array()) {
                    for msg in msgs_array {
                        let normalized = normalize_autogen_message(msg);
                        if !normalized.is_empty() {
                            for n in normalized {
                                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                            }
                            found = true;
                        } else if is_autogen_skip_type(msg) {
                            found = true;
                        } else if msg.get("content").is_some() {
                            messages.push(RawMessage::from_attr(
                                keys::MESSAGE,
                                timestamp,
                                msg.clone(),
                            ));
                            found = true;
                        }
                    }
                } else if let Some(nested_msg) = parsed.get("message") {
                    let normalized = normalize_autogen_message(nested_msg);
                    if !normalized.is_empty() {
                        for n in normalized {
                            messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                        }
                        found = true;
                    } else if is_autogen_skip_type(nested_msg) {
                        found = true;
                    } else if nested_msg.get("content").is_some() {
                        messages.push(RawMessage::from_attr(
                            keys::MESSAGE,
                            timestamp,
                            nested_msg.clone(),
                        ));
                        found = true;
                    }
                } else if let Some(response) = parsed.get("response") {
                    // Handle response wrapper: {"response": {"chat_message": {...}, "inner_messages": [...]}}
                    if let Some(chat_msg) = response.get("chat_message") {
                        for n in normalize_autogen_message(chat_msg) {
                            messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                            found = true;
                        }
                    }
                    if let Some(inner) = response.get("inner_messages").and_then(|m| m.as_array()) {
                        for msg in inner {
                            for n in normalize_autogen_message(msg) {
                                messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, n));
                                found = true;
                            }
                        }
                    }
                } else if parsed.is_object()
                    && (parsed.get("content").is_some() || parsed.get("role").is_some())
                {
                    messages.push(RawMessage::from_attr(keys::MESSAGE, timestamp, parsed));
                    found = true;
                }
            }
        }
    }

    // Try to extract LLMCallEvent from autogen logging (may be in body or attributes)
    // Format: {"type": "LLMCall", "messages": [...], "response": {...}, "prompt_tokens": N, ...}
    for key in ["body", "log.body", "autogen.event"] {
        if let Some(json_str) = attrs.get(key) {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(json_str) {
                let event_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

                match event_type {
                    "LLMCall" | "LLMStreamEnd" => {
                        // Extract input messages (standard SideML format: role/content)
                        if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_array()) {
                            for msg in msgs {
                                let normalized = normalize_autogen_message(msg);
                                if !normalized.is_empty() {
                                    for n in normalized {
                                        messages.push(RawMessage::from_attr(key, timestamp, n));
                                    }
                                    found = true;
                                } else if msg.get("role").is_some() || msg.get("content").is_some()
                                {
                                    messages.push(RawMessage::from_attr(
                                        key,
                                        timestamp,
                                        msg.clone(),
                                    ));
                                    found = true;
                                }
                            }
                        }

                        // Extract response as assistant message
                        if let Some(response) = parsed.get("response") {
                            if let Some(normalized) = normalize_autogen_response(response) {
                                messages.push(RawMessage::from_attr(key, timestamp, normalized));
                                found = true;
                            }
                        }

                        // Extract tools as tool_definitions
                        if let Some(tools) = parsed.get("tools").and_then(|t| t.as_array()) {
                            if !tools.is_empty() {
                                tool_definitions.push(RawToolDefinition::from_attr(
                                    key,
                                    timestamp,
                                    json!(tools),
                                ));
                                found = true;
                            }
                        }
                    }
                    "ToolCall" => {
                        let tool_name = parsed
                            .get("tool_name")
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown");
                        let arguments = parsed.get("arguments").cloned().unwrap_or(json!({}));
                        let result = parsed.get("result").cloned().unwrap_or(json!(null));

                        let tool_msg = json!({
                            "role": "tool",
                            "name": tool_name,
                            "content": result,
                            "tool_call": {
                                "name": tool_name,
                                "arguments": arguments
                            }
                        });
                        messages.push(RawMessage::from_attr(key, timestamp, tool_msg));
                        found = true;
                    }
                    _ => {}
                }
            }
        }
    }

    found
}

/// AutoGen message types that should be silently skipped (not extracted as messages).
/// ToolCallSummaryMessage concatenates tool results as Python repr() — duplicate noise.
fn is_autogen_skip_type(msg: &JsonValue) -> bool {
    msg.get("type").and_then(|t| t.as_str()) == Some("ToolCallSummaryMessage")
}

/// Determine role from AutoGen source field: "user" → "user", anything else → "assistant"
fn autogen_role_from_source(source: Option<&str>) -> &'static str {
    match source {
        Some("user") => "user",
        _ => "assistant",
    }
}

/// Convert AutoGen tool call array [{id, name, arguments}] to OpenAI-compatible format.
fn normalize_autogen_tool_calls(tool_calls: &[JsonValue]) -> Vec<JsonValue> {
    tool_calls
        .iter()
        .filter_map(|tc| {
            let id = tc.get("id").and_then(|i| i.as_str())?;
            let name = tc.get("name").and_then(|n| n.as_str())?;
            let args = tc.get("arguments").cloned().unwrap_or(json!({}));

            let parsed_args = if let Some(args_str) = args.as_str() {
                serde_json::from_str(args_str).unwrap_or(json!(args_str))
            } else {
                args
            };

            Some(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": parsed_args
                }
            }))
        })
        .collect()
}

/// Convert AutoGen tool result array [{content, name, call_id}] to individual tool messages.
/// Processes ALL items (not just first). Falls back to parent's call_id when item lacks one.
fn normalize_autogen_tool_results(items: &[JsonValue], parent: &JsonValue) -> Vec<JsonValue> {
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|n| n.as_str());
            let call_id = item
                .get("call_id")
                .and_then(|c| c.as_str())
                .or_else(|| parent.get("call_id").and_then(|c| c.as_str()));
            let inner_content = item.get("content").cloned().unwrap_or(json!(""));

            // Skip items with no useful content
            if name.is_none() && call_id.is_none() && inner_content == json!("") {
                return None;
            }

            let mut result = json!({"role": "tool", "content": inner_content});
            if let Some(n) = name {
                result["name"] = json!(n);
            }
            if let Some(id) = call_id {
                result["tool_call_id"] = json!(id);
            }
            Some(result)
        })
        .collect()
}

/// Infer AutoGen message type when the `type` field is missing.
/// Uses content structure heuristics to determine the message kind.
fn infer_autogen_message_type(msg: &JsonValue) -> Vec<JsonValue> {
    // Already in SideML format (has role) — preserve as-is
    if msg.get("role").is_some() {
        return vec![msg.clone()];
    }

    let content = match msg.get("content") {
        Some(c) => c,
        None => return vec![],
    };

    // Array content: could be tool calls or tool results
    if let Some(arr) = content.as_array() {
        if arr.is_empty() {
            return vec![];
        }
        let first = &arr[0];

        // Tool call request: [{id, name, arguments}]
        if first.get("arguments").is_some() {
            let normalized_calls = normalize_autogen_tool_calls(arr);
            if normalized_calls.is_empty() {
                return vec![];
            }
            let source = msg.get("source").and_then(|s| s.as_str());
            let mut result = json!({
                "role": "assistant",
                "tool_calls": normalized_calls
            });
            if let Some(src) = source {
                result["name"] = json!(src);
            }
            return vec![result];
        }

        // Tool execution: [{call_id, ...}] or [{content, name}] without arguments
        if first.get("call_id").is_some()
            || (first.get("content").is_some()
                && first.get("name").is_some()
                && first.get("arguments").is_none())
        {
            let results = normalize_autogen_tool_results(arr, msg);
            if results.is_empty() {
                return vec![];
            }
            return results;
        }

        return vec![];
    }

    // String content: TextMessage equivalent
    if let Some(text) = content.as_str() {
        if text.is_empty() {
            return vec![];
        }
        let source = msg.get("source").and_then(|s| s.as_str());
        let role = autogen_role_from_source(source);
        let mut result = json!({
            "role": role,
            "content": text
        });
        if let Some(src) = source {
            if src != "user" {
                result["name"] = json!(src);
            }
        }
        return vec![result];
    }

    vec![]
}

/// Normalize AutoGen message types to standard SideML format.
///
/// Returns a Vec because some message types (ToolCallExecutionEvent,
/// FunctionExecutionResultMessage) contain arrays of results that expand
/// to multiple individual messages.
fn normalize_autogen_message(msg: &JsonValue) -> Vec<JsonValue> {
    let msg_type = match msg.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return infer_autogen_message_type(msg),
    };

    match msg_type {
        "SystemMessage" => match msg.get("content") {
            Some(content) => vec![json!({"role": "system", "content": content})],
            None => vec![],
        },
        "UserMessage" => match msg.get("content") {
            Some(content) => {
                let mut result = json!({"role": "user", "content": content});
                if let Some(source) = msg.get("source") {
                    result["name"] = source.clone();
                }
                vec![result]
            }
            None => vec![],
        },
        "AssistantMessage" => {
            match msg.get("content") {
                Some(content) => {
                    let mut result = json!({"role": "assistant"});
                    if let Some(source) = msg.get("source") {
                        result["name"] = source.clone();
                    }
                    // Prepend thought as thinking content block so SideML can process it
                    let has_thought = msg
                        .get("thought")
                        .map(|t| !t.is_null() && t.as_str().is_none_or(|s| !s.is_empty()))
                        .unwrap_or(false);
                    if has_thought {
                        let thinking_block = json!({
                            "type": "thinking",
                            "text": msg["thought"],
                            "signature": null
                        });
                        result["content"] = json!([thinking_block, content]);
                    } else {
                        result["content"] = content.clone();
                    }
                    vec![result]
                }
                None => vec![],
            }
        }
        "TextMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "MultiModalMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "StopMessage" => match msg.get("content") {
            Some(content) => {
                let source = msg.get("source").and_then(|s| s.as_str());
                let role = autogen_role_from_source(source);
                let mut result = json!({"role": role, "content": content});
                if let Some(src) = source {
                    if src != "user" {
                        result["name"] = json!(src);
                    }
                }
                vec![result]
            }
            None => vec![],
        },
        "HandoffMessage" => match msg.get("content") {
            Some(content) => {
                let mut result = json!({"role": "assistant", "content": content});
                if let Some(source) = msg.get("source").and_then(|s| s.as_str()) {
                    result["name"] = json!(source);
                }
                vec![result]
            }
            None => vec![],
        },
        "ThoughtEvent" => match msg.get("content") {
            Some(content) => {
                if content.as_str().is_some_and(|s| s.is_empty()) {
                    return vec![];
                }
                vec![json!({
                    "role": "assistant",
                    "content": [{
                        "type": "thinking",
                        "text": content,
                        "signature": null
                    }]
                })]
            }
            None => vec![],
        },
        "ToolCallRequestEvent" => {
            let content = match msg.get("content") {
                Some(c) => c,
                None => return vec![],
            };
            let source = msg.get("source").and_then(|s| s.as_str());

            if let Some(tool_calls) = content.as_array() {
                let normalized_calls = normalize_autogen_tool_calls(tool_calls);
                if !normalized_calls.is_empty() {
                    let mut result = json!({
                        "role": "assistant",
                        "tool_calls": normalized_calls
                    });
                    if let Some(src) = source {
                        result["name"] = json!(src);
                    }
                    return vec![result];
                }
            }
            vec![]
        }
        "ToolCallExecutionEvent" => match msg.get("content").and_then(|c| c.as_array()) {
            Some(arr) if !arr.is_empty() => normalize_autogen_tool_results(arr, msg),
            _ => vec![],
        },
        // ToolCallSummaryMessage is an internal AutoGen mechanism that concatenates
        // str(result) for each tool call. Individual tool results are already captured
        // as proper tool_result entries via ToolCallExecutionEvent/FunctionExecutionResultMessage.
        // Skipping avoids duplicate raw Python repr text in the conversation display.
        "ToolCallSummaryMessage" => vec![],
        "FunctionExecutionResultMessage" => match msg.get("content").and_then(|c| c.as_array()) {
            Some(arr) if !arr.is_empty() => normalize_autogen_tool_results(arr, msg),
            _ => vec![],
        },
        _ => {
            if msg.get("role").is_some() || msg.get("content").is_some() {
                vec![msg.clone()]
            } else {
                vec![]
            }
        }
    }
}

/// Normalize AutoGen LLM response to assistant message
fn normalize_autogen_response(response: &JsonValue) -> Option<JsonValue> {
    // Response may have content directly or in choices
    let content = response
        .get("content")
        .or_else(|| {
            response
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|choice| choice.get("message"))
                .and_then(|msg| msg.get("content"))
        })
        .cloned()?;

    let mut result = json!({
        "role": "assistant",
        "content": content
    });

    // Check for tool_calls in response
    if let Some(tool_calls) = response.get("tool_calls").or_else(|| {
        response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("tool_calls"))
    }) {
        if let Some(arr) = tool_calls.as_array() {
            if !arr.is_empty() {
                result["tool_calls"] = tool_calls.clone();
            }
        }
    }

    Some(result)
}

// ============================================================================
// FRAMEWORK: CREWAI
// ============================================================================

/// Append CrewAI tool definitions from selected metadata attributes.
///
/// `attribute_keys` can include any CrewAI attributes carrying agent/task tool lists,
/// for example `crew_agents` and `crew_tasks`.
fn append_crewai_tool_definitions(
    tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
    attribute_keys: &[&str],
) -> bool {
    let mut found = false;

    for key in attribute_keys {
        if let Some(raw_json) = attrs.get(*key) {
            let maybe_tools = if *key == keys::INPUT_VALUE {
                extract_crewai_tools_from_input_value(raw_json)
            } else {
                extract_crewai_tools_from_json(raw_json)
            };
            if let Some(tools) = maybe_tools {
                tool_definitions.push(RawToolDefinition::from_attr(
                    key,
                    timestamp,
                    JsonValue::Array(tools),
                ));
                found = true;
            }
        }
    }

    found
}

/// Extract CrewAI tool definitions from a JSON array attribute.
///
/// CrewAI emits tool names in agent/task metadata with multiple shapes:
/// - `[{ "tools_names": ["a", "b"] }]`
/// - `[{ "tools": ["a", "b"] }]`
/// - `[{ "tools": [{ "name": "a" }, ...] }]`
/// - `["a", "b"]` (fallback)
///
/// Returns OpenAI-compatible tool definitions:
/// `[{ "type": "function", "function": { "name": "a" } }, ...]`.
fn extract_crewai_tools_from_json(raw_json: &str) -> Option<Vec<JsonValue>> {
    let parsed = serde_json::from_str::<JsonValue>(raw_json).ok()?;
    let entries = parsed.as_array()?;

    let mut best_by_name: HashMap<String, (i32, JsonValue)> = HashMap::new();
    let mut order = Vec::new();

    for entry in entries {
        if let Some(tool) = crewai_tool_from_json_value(entry) {
            upsert_crewai_tool(tool, &mut best_by_name, &mut order);
        }

        for key in ["tools_names", "tools"] {
            if let Some(arr) = entry.get(key).and_then(|v| v.as_array()) {
                for tool in arr {
                    if let Some(parsed_tool) = crewai_tool_from_json_value(tool) {
                        upsert_crewai_tool(parsed_tool, &mut best_by_name, &mut order);
                    }
                }
            }
        }
    }

    finalize_crewai_tools(best_by_name, order)
}

/// Extract CrewAI tool definitions from input.value payload.
///
/// CrewAI often embeds richer tool metadata in `input.value`:
/// - `{"tools": ["name='x' description=\"Tool Arguments: {...}\" ..."]}`
/// - `{"tool": "CrewStructuredTool(name='x', description='Tool Arguments: {...}')"}`
///
/// Returns OpenAI-compatible definitions with best-effort `description` and `parameters`.
fn extract_crewai_tools_from_input_value(raw_json: &str) -> Option<Vec<JsonValue>> {
    let parsed = serde_json::from_str::<JsonValue>(raw_json).ok()?;
    let obj = parsed.as_object()?;

    let mut best_by_name: HashMap<String, (i32, JsonValue)> = HashMap::new();
    let mut order = Vec::new();

    if let Some(tools_value) = obj.get("tools") {
        match tools_value {
            JsonValue::Array(arr) => {
                for item in arr {
                    if let Some(tool) = crewai_tool_from_json_value(item) {
                        upsert_crewai_tool(tool, &mut best_by_name, &mut order);
                    }
                }
            }
            _ => {
                if let Some(tool) = crewai_tool_from_json_value(tools_value) {
                    upsert_crewai_tool(tool, &mut best_by_name, &mut order);
                }
            }
        }
    }

    if let Some(tool_value) = obj.get("tool")
        && let Some(tool) = crewai_tool_from_json_value(tool_value)
    {
        upsert_crewai_tool(tool, &mut best_by_name, &mut order);
    }

    finalize_crewai_tools(best_by_name, order)
}

/// Parse a CrewAI tool repr string into OpenAI-style function definition.
fn parse_crewai_tool_repr(tool_repr: &str) -> Option<JsonValue> {
    let fallback_name = extract_repr_field(tool_repr, "name")
        .or_else(|| extract_labeled_value(tool_repr, "Tool Name:"))
        .or_else(|| {
            let trimmed = tool_repr.trim();
            if !trimmed.is_empty() && !trimmed.contains('=') && !trimmed.contains(' ') {
                Some(trimmed.to_string())
            } else {
                None
            }
        })?;

    // Prefer the explicit description field when present; otherwise parse the
    // whole repr to catch labels in non-standard shapes.
    let details = extract_crewai_description_field(tool_repr).unwrap_or_else(|| tool_repr.into());
    let (name, description, parameters) = parse_crewai_tool_details(&details, &fallback_name);

    let mut function = json!({ "name": name });
    if let Some(desc) = description {
        function["description"] = json!(desc);
    }
    if let Some(params) = parameters {
        function["parameters"] = params;
    }

    Some(json!({
        "type": "function",
        "function": function
    }))
}

/// Parse structured details embedded inside CrewAI `description=...`.
fn parse_crewai_tool_details(
    details: &str,
    fallback_name: &str,
) -> (String, Option<String>, Option<JsonValue>) {
    // Some CrewAI payloads escape newlines (`\\n`) inside repr strings.
    // Normalize to real newlines so label extraction is stable.
    let normalized = details.replace("\\n", "\n");

    let name = extract_labeled_value(&normalized, "Tool Name:")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback_name.to_string());

    let description =
        extract_labeled_value(&normalized, "Tool Description:").filter(|v| !v.is_empty());

    let parameters = extract_labeled_python_dict(&normalized, "Tool Arguments:")
        .and_then(parse_python_repr_json)
        .and_then(|value| crewai_args_to_json_schema(&value));

    (name, description, parameters)
}

/// Extract `field='...'` or `field="..."` from repr-like strings.
fn extract_repr_field(input: &str, field: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        let prefix = format!("{field}={quote}");
        if let Some(start) = input.find(&prefix) {
            let rest = &input[start + prefix.len()..];
            let mut escaped = false;
            for (idx, ch) in rest.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == quote {
                    return Some(rest[..idx].to_string());
                }
            }
        }
    }
    None
}

/// Extract CrewAI description field where single-quoted values can contain
/// unescaped single quotes (Python dict repr inside the description).
fn extract_crewai_description_field(input: &str) -> Option<String> {
    if let Some(start) = input.find("description='") {
        let rest = &input[start + "description='".len()..];
        // CrewStructuredTool(...) form
        if let Some(end) = rest.rfind("')") {
            return Some(rest[..end].to_string());
        }
        // Fallback forms with additional repr fields after description
        for needle in [
            "' env_vars=",
            "', env_vars=",
            "' args_schema=",
            "', args_schema=",
        ] {
            if let Some(end) = rest.find(needle) {
                return Some(rest[..end].to_string());
            }
        }
        // As a last resort, consume until end
        return Some(rest.to_string());
    }

    extract_repr_field(input, "description")
}

/// Extract one-line value from `Label: value` pattern.
fn extract_labeled_value(input: &str, label: &str) -> Option<String> {
    input.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(label)
            .map(|v| v.trim().to_string())
    })
}

/// Extract balanced Python dict text after a label.
fn extract_labeled_python_dict<'a>(input: &'a str, label: &str) -> Option<&'a str> {
    let idx = input.find(label)?;
    let rest = &input[idx + label.len()..];
    extract_balanced_braces(rest)
}

/// Extract first balanced `{...}` block, honoring quoted strings.
fn extract_balanced_braces(input: &str) -> Option<&str> {
    let mut start: Option<usize> = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                in_string = true;
                quote = ch;
            }
            '{' => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        let s = start?;
                        return Some(&input[s..=idx]);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

/// Parse Python repr JSON-like strings (`{'k': True, 'v': None}`) into JSON.
fn parse_python_repr_json(s: &str) -> Option<JsonValue> {
    if !s.starts_with('{') && !s.starts_with('[') {
        return None;
    }

    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 16);
    let mut i = 0;
    let mut in_string = false;
    let mut quote_char = 0u8;

    while i < len {
        let b = bytes[i];

        if in_string {
            if b == quote_char {
                out.push('"');
                in_string = false;
                i += 1;
            } else if b == b'"' && quote_char == b'\'' {
                out.push_str("\\\"");
                i += 1;
            } else if b == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                match next {
                    b'\'' => {
                        out.push('\'');
                        i += 2;
                    }
                    b'"' => {
                        out.push_str("\\\"");
                        i += 2;
                    }
                    b'\\' | b'/' | b'n' | b't' | b'r' | b'b' | b'f' => {
                        out.push('\\');
                        out.push(next as char);
                        i += 2;
                    }
                    b'u' => {
                        out.push('\\');
                        out.push('u');
                        i += 2;
                    }
                    _ => {
                        out.push('\\');
                        i += 1;
                    }
                }
            } else {
                let ch = s[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        } else {
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
                    let ch = s[i..].chars().next()?;
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }

    serde_json::from_str(&out).ok()
}

#[inline]
fn matches_python_literal(bytes: &[u8], i: usize, literal: &[u8]) -> bool {
    let end = i + literal.len();
    if end > bytes.len() || bytes[i..end] != *literal {
        return false;
    }
    if end < bytes.len() {
        let after = bytes[end];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    if i > 0 {
        let before = bytes[i - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    true
}

fn crewai_args_to_json_schema(value: &JsonValue) -> Option<JsonValue> {
    let args = value.as_object()?;
    let mut properties = serde_json::Map::new();

    for (name, meta) in args {
        let mut prop = serde_json::Map::new();
        match meta {
            JsonValue::Object(m) => {
                if let Some(type_name) = m.get("type").and_then(|v| v.as_str()) {
                    prop.insert("type".to_string(), json!(map_crewai_type(type_name)));
                }
                if let Some(desc) = m.get("description").and_then(|v| v.as_str())
                    && !desc.trim().is_empty()
                {
                    prop.insert("description".to_string(), json!(desc));
                }
            }
            JsonValue::String(type_name) => {
                prop.insert("type".to_string(), json!(map_crewai_type(type_name)));
            }
            _ => {}
        }
        if prop.is_empty() {
            prop.insert("type".to_string(), json!("string"));
        }
        properties.insert(name.clone(), JsonValue::Object(prop));
    }

    Some(json!({
        "type": "object",
        "properties": properties
    }))
}

fn map_crewai_type(type_name: &str) -> &'static str {
    let t = type_name.trim();
    if t.eq_ignore_ascii_case("str") || t.eq_ignore_ascii_case("string") {
        "string"
    } else if t.eq_ignore_ascii_case("int") || t.eq_ignore_ascii_case("integer") {
        "integer"
    } else if t.eq_ignore_ascii_case("float")
        || t.eq_ignore_ascii_case("double")
        || t.eq_ignore_ascii_case("number")
    {
        "number"
    } else if t.eq_ignore_ascii_case("bool") || t.eq_ignore_ascii_case("boolean") {
        "boolean"
    } else if t.eq_ignore_ascii_case("list") || t.eq_ignore_ascii_case("array") {
        "array"
    } else if t.eq_ignore_ascii_case("dict") || t.eq_ignore_ascii_case("object") {
        "object"
    } else {
        "string"
    }
}

fn crewai_tool_from_json_value(value: &JsonValue) -> Option<JsonValue> {
    if let Some(s) = value.as_str() {
        if s.contains("name=") || s.contains("Tool Name:") || s.contains("CrewStructuredTool(") {
            return parse_crewai_tool_repr(s);
        }

        let name = s.trim();
        if name.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "function",
            "function": { "name": name }
        }));
    }

    let obj = value.as_object()?;

    // Already OpenAI-style function definition.
    if let Some(function) = obj.get("function")
        && function.get("name").and_then(|n| n.as_str()).is_some()
    {
        let mut tool = json!({
            "type": "function",
            "function": function.clone()
        });
        if let Some(strict) = obj.get("strict") {
            tool["strict"] = strict.clone();
        }
        return Some(tool);
    }

    let fallback_name = obj.get("name").and_then(|n| n.as_str())?.trim();
    if fallback_name.is_empty() {
        return None;
    }

    let mut function = json!({ "name": fallback_name });

    if let Some(desc) = obj.get("description").and_then(|d| d.as_str()) {
        let (name, parsed_desc, parsed_params) = parse_crewai_tool_details(desc, fallback_name);
        function["name"] = json!(name);
        if let Some(parsed_desc) = parsed_desc {
            function["description"] = json!(parsed_desc);
        } else if !desc.trim().is_empty() {
            function["description"] = json!(desc.trim());
        }
        if let Some(parsed_params) = parsed_params {
            function["parameters"] = parsed_params;
        }
    }

    if function.get("parameters").is_none()
        && let Some(params_value) = obj
            .get("parameters")
            .or_else(|| obj.get("input_schema"))
            .or_else(|| obj.get("args"))
            .or_else(|| obj.get("arguments"))
            .or_else(|| obj.get("tool_args"))
            .or_else(|| obj.get("tool_arguments"))
        && let Some(params) = normalize_crewai_parameters(params_value)
    {
        function["parameters"] = params;
    }

    Some(json!({
        "type": "function",
        "function": function
    }))
}

fn normalize_crewai_parameters(value: &JsonValue) -> Option<JsonValue> {
    if value.is_null() {
        return None;
    }

    if let Some(s) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(s) {
            return normalize_crewai_parameters(&parsed);
        }
        if let Some(parsed) = parse_python_repr_json(s) {
            return normalize_crewai_parameters(&parsed);
        }
    }

    if value
        .get("type")
        .is_some_and(|t| t.is_string() || t.is_object() || t.is_array())
        || value.get("properties").is_some()
    {
        return Some(value.clone());
    }

    crewai_args_to_json_schema(value)
}

fn extract_crewai_tool_name(tool: &JsonValue) -> Option<String> {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| tool.get("name").and_then(|n| n.as_str()))
        .or_else(|| tool.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

fn upsert_crewai_tool(
    tool: JsonValue,
    best_by_name: &mut HashMap<String, (i32, JsonValue)>,
    order: &mut Vec<String>,
) -> bool {
    let Some(name) = extract_crewai_tool_name(&tool) else {
        return false;
    };
    let quality = tool_definition_quality(&tool);
    if !best_by_name.contains_key(&name) {
        order.push(name.clone());
    }
    match best_by_name.get(&name) {
        Some((best_quality, _)) if *best_quality >= quality => false,
        _ => {
            best_by_name.insert(name, (quality, tool));
            true
        }
    }
}

fn finalize_crewai_tools(
    mut best_by_name: HashMap<String, (i32, JsonValue)>,
    order: Vec<String>,
) -> Option<Vec<JsonValue>> {
    if best_by_name.is_empty() {
        return None;
    }

    let mut tools = Vec::with_capacity(best_by_name.len());
    for name in order {
        if let Some((_, tool)) = best_by_name.remove(&name) {
            tools.push(tool);
        }
    }

    if tools.is_empty() { None } else { Some(tools) }
}

/// Check if a JSON object looks like a chat message (has role + content or tool_calls).
fn is_chat_message(msg: &JsonValue) -> bool {
    msg.get("role").is_some()
        && (msg.get("content").is_some()
            || msg.get("tool_calls").is_some()
            || msg.get("toolCalls").is_some())
}

pub(crate) fn try_crewai(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // CrewAI detection: must have CrewAI-specific attributes
    let is_crewai = attrs.contains_key("crew_key")
        || attrs.contains_key("crew_id")
        || attrs.contains_key("crew_tasks")
        || attrs.contains_key("task_key");

    if !is_crewai {
        return false;
    }

    let mut found = false;

    // Tool definitions are extracted by extract_tool_definitions() which runs on all spans.

    // crew_tasks attribute (task definitions)
    if let Some(tasks_json) = attrs.get("crew_tasks") {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(tasks_json) {
            messages.push(RawMessage::from_attr("crew_tasks", timestamp, parsed));
            found = true;
        }
    }

    // output.value - try to extract messages array, fall back to raw output
    if let Some(output_json) = attrs.get(keys::OUTPUT_VALUE) {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(output_json) {
            let mut extracted_messages = false;

            // Try to extract messages array from the output
            if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_array()) {
                for msg in msgs.iter().filter(|m| is_chat_message(m)) {
                    messages.push(RawMessage::from_attr(
                        keys::OUTPUT_VALUE,
                        timestamp,
                        msg.clone(),
                    ));
                    extracted_messages = true;
                }
            }

            // Also check tasks_output for messages
            if let Some(tasks) = parsed.get("tasks_output").and_then(|t| t.as_array()) {
                for task in tasks {
                    if let Some(msgs) = task.get("messages").and_then(|m| m.as_array()) {
                        for msg in msgs.iter().filter(|m| is_chat_message(m)) {
                            messages.push(RawMessage::from_attr(
                                keys::OUTPUT_VALUE,
                                timestamp,
                                msg.clone(),
                            ));
                            extracted_messages = true;
                        }
                    }
                }
            }

            // Fall back to raw output if no messages array found
            if !extracted_messages {
                messages.push(RawMessage::from_attr(keys::OUTPUT_VALUE, timestamp, parsed));
            }
            found = true;
        }
    }

    found
}

/// Wrap a plain data object (structured output) with message structure.
/// If the value is a plain data object without message-structure keys,
/// wraps it as `{"role": <role>, "content": <value>}` so normalize() can process it.
/// Non-plain-data values (already message-shaped, arrays, strings) pass through unchanged.
fn wrap_plain_data(value: JsonValue, role: &str) -> JsonValue {
    if is_plain_data_value(&value) {
        json!({"role": role, "content": value})
    } else {
        value
    }
}

pub(crate) fn try_raw_io(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    _: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    // Note: system_prompt is extracted in extract_messages_for_span (mod.rs)

    // input.value - preserve raw JSON, wrap plain data as user message
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::INPUT_VALUE) {
        let wrapped = wrap_plain_data(parsed, "user");
        messages.push(RawMessage::from_attr(keys::INPUT_VALUE, timestamp, wrapped));
    }

    // raw_input (Logfire fallback) - preserve raw array
    if attrs.get(keys::INPUT_VALUE).is_none() {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::RAW_INPUT) {
            let wrapped = wrap_plain_data(parsed, "user");
            messages.push(RawMessage::from_attr(keys::RAW_INPUT, timestamp, wrapped));
        }
    }

    // output.value - preserve raw JSON, wrap plain data as assistant message
    if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::OUTPUT_VALUE) {
        let wrapped = wrap_plain_data(parsed, "assistant");
        messages.push(RawMessage::from_attr(
            keys::OUTPUT_VALUE,
            timestamp,
            wrapped,
        ));
    }

    // response (Logfire fallback) - preserve raw JSON
    if attrs.get(keys::OUTPUT_VALUE).is_none() {
        if let Some(parsed) = extract_json::<JsonValue>(attrs, keys::RESPONSE) {
            let wrapped = wrap_plain_data(parsed, "assistant");
            messages.push(RawMessage::from_attr(keys::RESPONSE, timestamp, wrapped));
        }
    }

    !messages.is_empty()
}

// ============================================================================
// INDEXED MESSAGE EXTRACTION
// ============================================================================

fn extract_indices(attrs: &HashMap<String, String>, prefix: &str) -> BTreeSet<usize> {
    attrs
        .keys()
        .filter_map(|k| {
            k.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.split('.').next())
                .and_then(|idx| idx.parse().ok())
        })
        .collect()
}

fn extract_indexed_message(
    attrs: &HashMap<String, String>,
    prefix: &str,
    idx: usize,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let msg_prefix = format!("{}.{}", prefix, idx);

    // Check for content - either direct (gen_ai.prompt.0.content)
    // or nested (gen_ai.prompt.0.content.0.text)
    let content_prefix = format!("{}.content", msg_prefix);
    let has_content = attrs.contains_key(&content_prefix)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", content_prefix)));

    if !has_content {
        return None;
    }

    // Collect all raw attributes with this prefix (literal, no metadata)
    let mut raw = serde_json::Map::new();
    let attr_prefix = format!("{}.", msg_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&attr_prefix) {
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("indexed.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    Some(RawMessage::from_attr(
        &msg_prefix,
        timestamp,
        JsonValue::Object(raw),
    ))
}

fn extract_openinference_message(
    attrs: &HashMap<String, String>,
    prefix: &str,
    idx: usize,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let item_prefix = format!("{}.{}", prefix, idx);
    let msg_prefix = format!("{}.message", item_prefix);

    // Check role exists
    attrs.get(&format!("{}.role", msg_prefix))?;

    // Check if message has meaningful content:
    // - Direct content (llm.input_messages.0.message.content)
    // - Nested content blocks (llm.input_messages.0.message.contents.0.*)
    // - Tool calls (llm.input_messages.0.message.tool_calls.*)
    // - Tool call ID (for tool result messages)
    // - Function call (legacy format)
    let content_key = format!("{}.content", msg_prefix);
    let contents_prefix = format!("{}.contents", msg_prefix);
    let tool_calls_prefix = format!("{}.tool_calls", msg_prefix);
    let tool_call_id_key = format!("{}.tool_call_id", msg_prefix);
    let function_call_prefix = format!("{}.function_call", msg_prefix);

    let has_content = attrs.contains_key(&content_key)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", contents_prefix)))
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", tool_calls_prefix)))
        || attrs.contains_key(&tool_call_id_key)
        || attrs
            .keys()
            .any(|k| k.starts_with(&format!("{}.", function_call_prefix)));

    if !has_content {
        return None;
    }

    // Collect all raw attributes (literal, no metadata)
    let mut raw = serde_json::Map::new();

    // Collect message.* attributes including nested content blocks
    let attr_prefix = format!("{}.", msg_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&attr_prefix) {
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("openinference.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    // Also collect ALL item-level attributes (not just message.*)
    let item_attr_prefix = format!("{}.", item_prefix);
    for (key, value) in attrs {
        if let Some(suffix) = key.strip_prefix(&item_attr_prefix) {
            // Skip message.* as we already collected those above
            if suffix.starts_with("message.") {
                continue;
            }
            let json_val = if value.starts_with('{') || value.starts_with('[') {
                parse_json_with_fallback(value, &format!("openinference.{}", key))
            } else {
                json!(value)
            };
            raw.insert(suffix.to_string(), json_val);
        }
    }

    Some(RawMessage::from_attr(
        &msg_prefix,
        timestamp,
        JsonValue::Object(raw),
    ))
}

/// Extract OpenInference documents (retrieval.documents.N.* or reranker.*.documents.N.*)
fn extract_openinference_documents(
    attrs: &HashMap<String, String>,
    prefix: &str,
    indices: &BTreeSet<usize>,
    timestamp: DateTime<Utc>,
) -> Option<RawMessage> {
    let mut documents = Vec::new();

    for &idx in indices {
        let doc_prefix = format!("{}.{}.document", prefix, idx);
        let mut doc = serde_json::Map::new();

        // Extract document.id
        if let Some(id) = attrs.get(&format!("{}.id", doc_prefix)) {
            doc.insert("id".to_string(), json!(id));
        }

        // Extract document.content
        if let Some(content) = attrs.get(&format!("{}.content", doc_prefix)) {
            doc.insert("content".to_string(), json!(content));
        }

        // Extract document.score
        if let Some(score) = attrs.get(&format!("{}.score", doc_prefix)) {
            if let Ok(score_f64) = score.parse::<f64>() {
                doc.insert("score".to_string(), json!(score_f64));
            } else {
                doc.insert("score".to_string(), json!(score));
            }
        }

        // Extract document.metadata (JSON)
        if let Some(metadata) = attrs.get(&format!("{}.metadata", doc_prefix)) {
            if let Ok(parsed) = serde_json::from_str::<JsonValue>(metadata) {
                doc.insert("metadata".to_string(), parsed);
            } else {
                doc.insert("metadata".to_string(), json!(metadata));
            }
        }

        if !doc.is_empty() {
            documents.push(JsonValue::Object(doc));
        }
    }

    if documents.is_empty() {
        return None;
    }

    let mut msg = serde_json::Map::new();
    msg.insert("role".to_string(), json!("documents"));
    msg.insert("content".to_string(), JsonValue::Array(documents));
    msg.insert("_source".to_string(), json!(prefix));

    Some(RawMessage::from_attr(
        prefix,
        timestamp,
        JsonValue::Object(msg),
    ))
}

// ============================================================================
// SPAN MESSAGE EXTRACTION ORCHESTRATION
// ============================================================================

/// Extract messages and tool definitions for a single span.
pub(super) fn extract_messages_for_span(
    otlp_span: &Span,
    span_attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
) -> (Vec<RawMessage>, Vec<RawToolDefinition>, Vec<RawToolNames>) {
    let is_tool_span = is_tool_execution_span(span_attrs);

    // Debug: Check for VercelAISDK attributes
    let has_ai_prompt = span_attrs.contains_key(keys::AI_PROMPT_MESSAGES)
        || span_attrs.contains_key(keys::AI_PROMPT);
    if has_ai_prompt {
        tracing::debug!(
            span_name = %otlp_span.name,
            is_tool_span,
            has_ai_prompt_messages = span_attrs.contains_key(keys::AI_PROMPT_MESSAGES),
            has_ai_prompt = span_attrs.contains_key(keys::AI_PROMPT),
            "VercelAISDK span detected with prompt attributes"
        );
    }

    let mut raw_messages = Vec::new();
    let mut tool_definitions = Vec::new();
    let mut tool_names = Vec::new();

    // Always extract system_prompt if present (comes before conversation)
    if let Some(system_prompt) = span_attrs.get(keys::SYSTEM_PROMPT) {
        if !system_prompt.is_empty() {
            let mut raw = serde_json::Map::new();
            raw.insert("role".to_string(), json!("system"));
            raw.insert("content".to_string(), json!(system_prompt));
            raw_messages.push(RawMessage::from_attr(
                keys::SYSTEM_PROMPT,
                timestamp,
                JsonValue::Object(raw),
            ));
        }
    }

    extract_messages_from_events(&mut raw_messages, &otlp_span.events, is_tool_span);

    // Enrich tool span messages with metadata from span attributes
    // Check event name (not role) since role is now derived at query-time
    if is_tool_span {
        let tool_name = span_attrs.get(keys::GEN_AI_TOOL_NAME);
        let tool_call_id = span_attrs.get(keys::GEN_AI_TOOL_CALL_ID);

        for msg in &mut raw_messages {
            // Get event name from message source
            let event_name = match &msg.source {
                MessageSource::Event { name, .. } => Some(name.as_str()),
                MessageSource::Attribute { .. } => None,
            };

            match event_name {
                // Tool input events (gen_ai.tool.message in tool span)
                // Raw role preserved; semantic role derived at query-time in SideML
                Some(keys::EVENT_TOOL_MESSAGE) => {
                    // Set role only if not already set (preserve raw data)
                    if msg.content.get("role").is_none() {
                        msg.content["role"] = json!("tool_call");
                    }
                    // Map "id" to "tool_call_id"
                    if msg.content.get("tool_call_id").is_none() {
                        let id = msg
                            .content
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .or_else(|| tool_call_id.cloned());
                        if let Some(id) = id {
                            msg.content["tool_call_id"] = json!(id);
                        }
                    }
                    // Add tool name
                    if msg.content.get("name").is_none() {
                        if let Some(name) = tool_name {
                            msg.content["name"] = json!(name);
                        }
                    }
                }
                // Tool output events (gen_ai.choice in tool span)
                // Role is derived at query-time as "tool" by role_from_event_name_with_context
                Some(keys::EVENT_CHOICE) | Some(keys::EVENT_CONTENT_COMPLETION) => {
                    // Add tool_call_id for correlation with tool call
                    // (extract_tool_use_id in sideml/tools.rs looks for tool_call_id)
                    if msg.content.get("tool_call_id").is_none() {
                        let id = msg
                            .content
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .or_else(|| tool_call_id.cloned());
                        if let Some(id) = id {
                            msg.content["tool_call_id"] = json!(id);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Vercel AI toolCall spans: extract from ai.toolCall.* attributes
    // These spans have no events, only attributes. Extract tool call input and result.
    if is_tool_span && raw_messages.is_empty() {
        if let Some(tool_args) = span_attrs.get(keys::AI_TOOLCALL_ARGS) {
            let tool_name = span_attrs.get(keys::AI_TOOLCALL_NAME).map(|s| s.as_str());
            let tool_id = span_attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
            let args_val = serde_json::from_str::<JsonValue>(tool_args).unwrap_or(json!(tool_args));
            let mut msg = serde_json::Map::new();
            msg.insert("role".to_string(), json!("tool_call"));
            if let Some(name) = tool_name {
                msg.insert("name".to_string(), json!(name));
            }
            if let Some(id) = tool_id {
                msg.insert("tool_call_id".to_string(), json!(id));
            }
            msg.insert("content".to_string(), args_val);
            raw_messages.push(RawMessage::from_attr(
                keys::AI_TOOLCALL_ARGS,
                timestamp,
                JsonValue::Object(msg),
            ));
        }

        if let Some(tool_result) = span_attrs.get(keys::AI_TOOLCALL_RESULT) {
            let tool_id = span_attrs.get(keys::AI_TOOLCALL_ID).map(|s| s.as_str());
            let result_val =
                serde_json::from_str::<JsonValue>(tool_result).unwrap_or(json!(tool_result));
            let mut msg = serde_json::Map::new();
            msg.insert("role".to_string(), json!("tool"));
            if let Some(id) = tool_id {
                msg.insert("tool_call_id".to_string(), json!(id));
            }
            msg.insert("content".to_string(), result_val);
            raw_messages.push(RawMessage::from_attr(
                keys::AI_TOOLCALL_RESULT,
                timestamp,
                JsonValue::Object(msg),
            ));
        }
    }

    // Always extract tool definitions and tool names from any span (they're metadata, not conversation)
    let (defs, names) = extract_tool_definitions(span_attrs, timestamp);
    tool_definitions.extend(defs);
    tool_names.extend(names);

    // Skip attribute extraction for tool spans - their input.value/output.value contain
    // tool params/results, not conversation messages
    let should_extract_attrs = !is_tool_span && should_fallback_to_attributes(&raw_messages);

    // Debug: Log extraction decision
    if span_attrs.contains_key(keys::AI_PROMPT_MESSAGES) || span_attrs.contains_key(keys::AI_PROMPT)
    {
        tracing::debug!(
            span_name = %otlp_span.name,
            is_tool_span,
            messages_before_attrs = raw_messages.len(),
            should_extract_attrs,
            "VercelAISDK extraction decision"
        );
    }

    if should_extract_attrs {
        extract_messages_from_attrs(
            &mut raw_messages,
            &mut tool_definitions,
            span_attrs,
            &otlp_span.name,
            timestamp,
        );
    }

    // Debug: Log final message count
    if span_attrs.contains_key(keys::AI_PROMPT_MESSAGES) || span_attrs.contains_key(keys::AI_PROMPT)
    {
        tracing::debug!(
            span_name = %otlp_span.name,
            final_messages = raw_messages.len(),
            "VercelAISDK extraction complete"
        );
    }

    // Debug: Log AutoGen extraction results
    if otlp_span.name.starts_with("autogen") && span_attrs.contains_key("message") {
        tracing::trace!(
            span_name = %otlp_span.name,
            is_tool_span,
            should_extract_attrs,
            event_count = otlp_span.events.len(),
            raw_messages_count = raw_messages.len(),
            has_message_attr = true,
            "AutoGen span extraction result"
        );
    }

    (raw_messages, tool_definitions, tool_names)
}

/// Check if messages contain only non-conversation content (system prompts) or are empty.
///
/// Returns true if attribute extraction fallback should be attempted.
/// This happens when:
/// - No messages exist, OR
/// - All messages are attribute-sourced system prompts (no event-sourced content)
///
/// Event-sourced messages indicate the span has OTEL GenAI events with conversation
/// content, so we shouldn't also extract from attributes (which would duplicate).
fn should_fallback_to_attributes(messages: &[RawMessage]) -> bool {
    // If we have any event-sourced messages, don't fall back to attributes
    // (events contain the authoritative conversation data)
    let has_event_messages = messages
        .iter()
        .any(|m| matches!(&m.source, MessageSource::Event { .. }));

    if has_event_messages {
        return false;
    }

    // No events - check if we only have system prompts from attributes
    // If so, fall back to attributes for conversation messages
    messages.is_empty()
        || messages
            .iter()
            .all(|m| m.content.get("role").and_then(|r| r.as_str()) == Some("system"))
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
