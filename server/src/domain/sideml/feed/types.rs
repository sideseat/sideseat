//! Feed pipeline types.
//!
//! Core types for the SideML feed processing pipeline.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as JsonValue;

use super::super::types::{ChatRole, ContentBlock, FinishReason};
use super::{GENAI_INPUT_EVENTS, GENAI_OUTPUT_EVENTS, obs_type, source_type};
use crate::data::types::MessageCategory;

// ============================================================================
// FEED OPTIONS
// ============================================================================

/// Options for feed processing.
#[derive(Debug, Clone, Default)]
pub struct FeedOptions {
    /// Filter by specific role (e.g., "user", "assistant", "system", "tool").
    pub role: Option<String>,
}

impl FeedOptions {
    /// Create a new options with default settings (no filtering).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set role filter to include only messages with the specified role.
    #[must_use]
    pub fn with_role(mut self, role: Option<String>) -> Self {
        self.role = role;
        self
    }
}

// ============================================================================
// BLOCK ENTRY
// ============================================================================

/// A single flattened content block with comprehensive metadata.
///
/// This is the output of the feed pipeline. Each block contains exactly ONE
/// ContentBlock plus all relevant metadata for rendering and deduplication.
///
/// # Type-Safe Helpers
///
/// Use the helper methods instead of string comparisons:
/// - `is_generation_span()` instead of `observation_type.as_deref() == Some("generation")`
/// - `is_tool_span()` instead of `observation_type.as_deref() == Some("tool")`
/// - `is_agent_span()` instead of `observation_type.as_deref() == Some("agent")`
/// - `is_accumulator_span()` for span/agent/chain types
/// - `is_tool_use()`, `is_tool_result()`, `is_text()` for content type checks
/// - `is_root_span()` for hierarchy checks
#[derive(Debug, Clone, Serialize)]
pub struct BlockEntry {
    // Content
    /// Block type name ("text", "tool_use", "tool_result", etc.)
    pub entry_type: String,
    /// Single content block
    pub content: ContentBlock,
    /// Message role
    pub role: ChatRole,

    // Position
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Position in source messages
    pub message_index: i32,
    /// Position within message content array
    pub entry_index: i32,

    // Hierarchy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// Full path from root: [root, ..., parent, span_id]
    pub span_path: Vec<String>,

    // Timing
    pub timestamp: DateTime<Utc>,

    // Span context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_type: Option<String>,

    // Generation context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    // Message context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,

    // Tool context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    // Metrics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,

    // Status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<String>,
    pub is_error: bool,

    // Source info
    /// "event" or "attribute"
    pub source_type: String,
    /// Event name if from event source (e.g., "gen_ai.user.message")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    /// Attribute key if from attribute source (e.g., "llm.output_messages", "input.value")
    /// Used to determine if block is input TO or output FROM the span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_attribute: Option<String>,
    /// Message category for semantic filtering
    pub category: MessageCategory,

    // For future deduplication (hash as string for JSON safety)
    pub content_hash: String,
    pub is_semantic: bool,

    // Classification flags (computed during pipeline, not serialized)
    /// True if this block should use span_end for effective timestamp.
    ///
    /// This determines TIMESTAMP STRATEGY, not protection from history marking.
    /// - `true`: Use span_end (completion events like gen_ai.choice, tool results from tool spans)
    /// - `false`: Use event_time (intermediate events, input, tool_use)
    ///
    /// NOTE: This is separate from `is_protected()` in history.rs which determines
    /// whether a block can be marked as history.
    #[serde(skip_serializing)]
    pub uses_span_end: bool,

    /// True if this block is historical context (not current turn).
    ///
    /// History blocks are filtered during deduplication. See `history.rs` for
    /// the full eight-phase detection algorithm (phases 2-7 plus 4b).
    #[serde(skip_serializing)]
    pub is_history: bool,
}

impl BlockEntry {
    // ========================================================================
    // OBSERVATION TYPE HELPERS
    // ========================================================================

    /// Check if this block is from a generation span (LLM call).
    #[inline]
    pub fn is_generation_span(&self) -> bool {
        self.observation_type.as_deref() == Some(obs_type::GENERATION)
    }

    /// Check if this block is from a tool execution span.
    #[inline]
    pub fn is_tool_span(&self) -> bool {
        self.observation_type.as_deref() == Some(obs_type::TOOL)
    }

    /// Check if this block is from an agent span.
    #[inline]
    pub fn is_agent_span(&self) -> bool {
        self.observation_type.as_deref() == Some(obs_type::AGENT)
    }

    /// Check if this block is from an accumulator span (span/agent/chain).
    ///
    /// Accumulator spans collect and pass through messages without
    /// generating new content.
    #[inline]
    pub fn is_accumulator_span(&self) -> bool {
        matches!(
            self.observation_type.as_deref(),
            Some(obs_type::SPAN) | Some(obs_type::AGENT) | Some(obs_type::CHAIN)
        )
    }

    // ========================================================================
    // CONTENT TYPE HELPERS
    // ========================================================================

    /// Check if this block contains a ToolUse.
    #[inline]
    pub fn is_tool_use(&self) -> bool {
        matches!(self.content, ContentBlock::ToolUse { .. })
    }

    /// Check if this block contains a ToolResult.
    #[inline]
    pub fn is_tool_result(&self) -> bool {
        matches!(self.content, ContentBlock::ToolResult { .. })
    }

    /// Check if this block contains Text.
    #[inline]
    pub fn is_text(&self) -> bool {
        matches!(self.content, ContentBlock::Text { .. })
    }

    /// Check if this block contains Thinking.
    #[inline]
    pub fn is_thinking(&self) -> bool {
        matches!(self.content, ContentBlock::Thinking { .. })
    }

    /// Check if this block contains a Json content block (structured output).
    #[inline]
    pub fn is_json_block(&self) -> bool {
        matches!(self.content, ContentBlock::Json { .. })
    }

    // ========================================================================
    // HIERARCHY HELPERS
    // ========================================================================

    /// Check if this block is from a root span (no parent).
    #[inline]
    pub fn is_root_span(&self) -> bool {
        self.parent_span_id.is_none()
    }

    // ========================================================================
    // SOURCE HELPERS
    // ========================================================================

    /// Check if this block came from an OTEL event.
    #[inline]
    pub fn is_from_event(&self) -> bool {
        self.source_type == source_type::EVENT
    }

    // ========================================================================
    // EVENT CLASSIFICATION HELPERS
    // ========================================================================

    /// Check if this block's event is a GenAI output event (gen_ai.choice, etc.).
    ///
    /// Output events represent LLM completions.
    #[inline]
    pub fn is_output_event(&self) -> bool {
        self.event_name
            .as_ref()
            .is_some_and(|name| GENAI_OUTPUT_EVENTS.contains(&name.as_str()))
    }

    /// Check if this block's event is a GenAI input event (user.message, etc.).
    ///
    /// Input events represent context/history passed to the LLM.
    #[inline]
    pub fn is_input_event(&self) -> bool {
        self.event_name
            .as_ref()
            .is_some_and(|name| GENAI_INPUT_EVENTS.contains(&name.as_str()))
    }

    // ========================================================================
    // SOURCE LOCATION HELPERS (Universal Input/Output Classification)
    // ========================================================================

    /// Check if this block came from INPUT attributes (context TO the span).
    ///
    /// Recognized input sources (universal across frameworks):
    /// - `llm.input_messages.*` - OpenInference
    /// - `gen_ai.input.*`, `gen_ai.prompt.*` - OTEL GenAI
    /// - `input.value` - Generic
    /// - `gcp.vertex.agent.llm_request`, `gcp.vertex.agent.data` - ADK/Vertex
    /// - `ai.prompt` - Vercel AI SDK
    /// - `lk.input_text`, `lk.user_input`, `lk.instructions`, `lk.chat_ctx` - LiveKit
    /// - `mlflow.spanInputs` - MLflow
    /// - `traceloop.entity.input` - TraceLoop
    /// - `pydantic_ai.all_messages` - Pydantic AI
    /// - `request_data` - Logfire
    /// - Input events (gen_ai.user.message, etc.)
    #[inline]
    pub fn is_input_source(&self) -> bool {
        self.source_attribute.as_ref().is_some_and(|attr| {
            // Standard OTel / OpenInference
            attr.starts_with("llm.input_messages")
                || attr.starts_with("gen_ai.input.")
                || attr.starts_with("gen_ai.prompt.")
                || attr == "input.value"
                // ADK / Vertex
                || attr == "gcp.vertex.agent.llm_request"
                || attr == "gcp.vertex.agent.data"
                // Vercel AI SDK
                || attr == "ai.prompt"
                // LiveKit
                || attr == "lk.input_text"
                || attr == "lk.user_input"
                || attr == "lk.instructions"
                || attr == "lk.chat_ctx"
                // MLflow
                || attr == "mlflow.spanInputs"
                // TraceLoop
                || attr == "traceloop.entity.input"
                // Pydantic AI
                || attr == "pydantic_ai.all_messages"
                // Logfire (instrument_openai, instrument_anthropic)
                || attr == "request_data"
        }) || self.is_input_event()
    }

    /// Check if this block came from OUTPUT attributes (results FROM the span).
    ///
    /// Recognized output sources (universal across frameworks):
    /// - `llm.output_messages.*` - OpenInference
    /// - `gen_ai.output.*`, `gen_ai.completion.*` - OTEL GenAI
    /// - `output.value` - Generic
    /// - `gcp.vertex.agent.llm_response` - ADK/Vertex
    /// - `ai.result.*` - Vercel AI SDK
    /// - `lk.response.*` - LiveKit
    /// - `mlflow.spanOutputs` - MLflow
    /// - `traceloop.entity.output` - TraceLoop
    /// - `response_data` - Logfire
    /// - Output events (gen_ai.choice, etc.)
    #[inline]
    pub fn is_output_source(&self) -> bool {
        self.source_attribute.as_ref().is_some_and(|attr| {
            // Standard OTel / OpenInference
            attr.starts_with("llm.output_messages")
                || attr.starts_with("gen_ai.output.")
                || attr.starts_with("gen_ai.completion.")
                || attr == "output.value"
                // ADK / Vertex
                || attr == "gcp.vertex.agent.llm_response"
                // Vercel AI SDK
                || attr.starts_with("ai.result.")
                // LiveKit
                || attr.starts_with("lk.response.")
                // MLflow
                || attr == "mlflow.spanOutputs"
                // TraceLoop
                || attr == "traceloop.entity.output"
                // Logfire (instrument_openai, instrument_anthropic)
                || attr == "response_data"
        }) || self.is_output_event()
    }

    /// Check if this block has the GenAIChoice category.
    #[inline]
    pub fn is_choice_category(&self) -> bool {
        self.category == MessageCategory::GenAIChoice
    }

    // ========================================================================
    // PROTECTION STATUS
    // ========================================================================

    /// Check if this block is protected from history marking.
    ///
    /// Protected blocks represent actual LLM output and should NEVER be
    /// filtered as history. A block is protected if it has ANY of:
    /// - `gen_ai.choice` or `gen_ai.content.completion` event
    /// - `GenAIChoice` category
    /// - Explicit `finish_reason`
    ///
    /// This is used by history detection to ensure real LLM output is preserved.
    #[inline]
    pub fn is_protected(&self) -> bool {
        self.is_output_event() || self.is_choice_category() || self.finish_reason.is_some()
    }
}

// ============================================================================
// FEED RESULT
// ============================================================================

/// Result of processing spans through the feed pipeline.
#[derive(Debug)]
pub struct FeedResult {
    pub messages: Vec<BlockEntry>,
    pub tool_definitions: Vec<JsonValue>,
    pub tool_names: Vec<String>,
    pub metadata: FeedMetadata,
}

/// Tool definitions and names extracted from span rows.
///
/// Separated from the feed pipeline so handlers can scope tool extraction
/// independently (e.g., to a single trace when session-loading).
#[derive(Debug, Default)]
pub struct ExtractedTools {
    pub tool_definitions: Vec<JsonValue>,
    pub tool_names: Vec<String>,
}

/// Metadata about the processed feed.
#[derive(Debug, Clone, Serialize)]
pub struct FeedMetadata {
    pub block_count: usize,
    pub span_count: usize,
    pub total_tokens: i64,
    pub total_cost: f64,
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sideml::types::ContentBlock;
    use chrono::Utc;

    fn make_test_block() -> BlockEntry {
        BlockEntry {
            entry_type: "text".to_string(),
            content: ContentBlock::Text {
                text: "test".to_string(),
            },
            role: ChatRole::User,
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: None,
            span_path: vec!["span1".to_string()],
            timestamp: Utc::now(),
            observation_type: None,
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "attribute".to_string(),
            event_name: None,
            source_attribute: None,
            category: MessageCategory::GenAIUserMessage,
            content_hash: "hash".to_string(),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
        }
    }

    #[test]
    fn test_is_input_source_llm_input_messages() {
        let mut block = make_test_block();
        block.source_attribute = Some("llm.input_messages.0.message.content".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_input_value() {
        let mut block = make_test_block();
        block.source_attribute = Some("input.value".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_gen_ai_prompt() {
        let mut block = make_test_block();
        block.source_attribute = Some("gen_ai.prompt.0.content".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_input_event() {
        let mut block = make_test_block();
        block.source_type = "event".to_string();
        block.event_name = Some("gen_ai.user.message".to_string());
        assert!(block.is_input_source());
    }

    #[test]
    fn test_is_output_source_llm_output_messages() {
        let mut block = make_test_block();
        block.source_attribute = Some("llm.output_messages.0.message.content".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_output_source_output_value() {
        let mut block = make_test_block();
        block.source_attribute = Some("output.value".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_output_source_gen_ai_completion() {
        let mut block = make_test_block();
        block.source_attribute = Some("gen_ai.completion.0.content".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_output_source_output_event() {
        let mut block = make_test_block();
        block.source_type = "event".to_string();
        block.event_name = Some("gen_ai.choice".to_string());
        assert!(block.is_output_source());
    }

    #[test]
    fn test_neither_input_nor_output_source() {
        let mut block = make_test_block();
        block.source_attribute = Some("some.other.attribute".to_string());
        assert!(!block.is_input_source());
        assert!(!block.is_output_source());
    }

    // ========================================================================
    // FRAMEWORK-SPECIFIC SOURCE CLASSIFICATION TESTS
    // ========================================================================

    #[test]
    fn test_is_input_source_adk_llm_request() {
        let mut block = make_test_block();
        block.source_attribute = Some("gcp.vertex.agent.llm_request".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_adk_data() {
        let mut block = make_test_block();
        block.source_attribute = Some("gcp.vertex.agent.data".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_output_source_adk_llm_response() {
        let mut block = make_test_block();
        block.source_attribute = Some("gcp.vertex.agent.llm_response".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_adk_tool_attrs_are_neutral() {
        let mut block = make_test_block();
        block.source_attribute = Some("gcp.vertex.agent.tool_call_args".to_string());
        assert!(!block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_vercel() {
        let mut block = make_test_block();
        block.source_attribute = Some("ai.prompt".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_output_source_vercel() {
        let mut block = make_test_block();
        block.source_attribute = Some("ai.result.text".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_input_source_livekit() {
        for attr in &[
            "lk.input_text",
            "lk.user_input",
            "lk.instructions",
            "lk.chat_ctx",
        ] {
            let mut block = make_test_block();
            block.source_attribute = Some(attr.to_string());
            assert!(block.is_input_source(), "Expected input for {attr}");
            assert!(!block.is_output_source(), "Expected not output for {attr}");
        }
    }

    #[test]
    fn test_is_output_source_livekit() {
        let mut block = make_test_block();
        block.source_attribute = Some("lk.response.text".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_input_source_mlflow() {
        let mut block = make_test_block();
        block.source_attribute = Some("mlflow.spanInputs".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_output_source_mlflow() {
        let mut block = make_test_block();
        block.source_attribute = Some("mlflow.spanOutputs".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_input_source_traceloop() {
        let mut block = make_test_block();
        block.source_attribute = Some("traceloop.entity.input".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_output_source_traceloop() {
        let mut block = make_test_block();
        block.source_attribute = Some("traceloop.entity.output".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_input_source_pydantic_ai() {
        let mut block = make_test_block();
        block.source_attribute = Some("pydantic_ai.all_messages".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_input_source_logfire_request_data() {
        let mut block = make_test_block();
        block.source_attribute = Some("request_data".to_string());
        assert!(block.is_input_source());
        assert!(!block.is_output_source());
    }

    #[test]
    fn test_is_output_source_logfire_response_data() {
        let mut block = make_test_block();
        block.source_attribute = Some("response_data".to_string());
        assert!(block.is_output_source());
        assert!(!block.is_input_source());
    }

    #[test]
    fn test_is_json_block() {
        let mut block = make_test_block();
        assert!(!block.is_json_block());

        block.content = ContentBlock::Json {
            data: serde_json::json!({"name": "Jane"}),
        };
        assert!(block.is_json_block());
    }
}
