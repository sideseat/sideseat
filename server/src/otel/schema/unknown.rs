//! Unknown OTLP field preservation
//!
//! Preserves any fields from incoming OTLP data that are not explicitly handled
//! by our normalization process. This ensures we don't lose data.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Container for unknown/unhandled OTLP fields
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnknownFields {
    /// Resource attributes not explicitly handled
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub resource_attributes: HashMap<String, serde_json::Value>,

    /// Span attributes not explicitly handled
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub span_attributes: HashMap<String, serde_json::Value>,

    /// Scope attributes not explicitly handled
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scope_attributes: HashMap<String, serde_json::Value>,

    /// Links data (if present)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<serde_json::Value>,

    /// Dropped counts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_attributes_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_events_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_links_count: Option<u32>,
}

impl UnknownFields {
    /// Create empty unknown fields container
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if there are any unknown fields
    pub fn is_empty(&self) -> bool {
        self.resource_attributes.is_empty()
            && self.span_attributes.is_empty()
            && self.scope_attributes.is_empty()
            && self.links.is_empty()
            && self.dropped_attributes_count.is_none()
            && self.dropped_events_count.is_none()
            && self.dropped_links_count.is_none()
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> Option<String> {
        if self.is_empty() { None } else { serde_json::to_string(self).ok() }
    }

    /// Parse from JSON string
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// Add an unknown resource attribute
    pub fn add_resource_attr(&mut self, key: String, value: serde_json::Value) {
        self.resource_attributes.insert(key, value);
    }

    /// Add an unknown span attribute
    pub fn add_span_attr(&mut self, key: String, value: serde_json::Value) {
        self.span_attributes.insert(key, value);
    }
}

/// Known attribute keys that we explicitly handle
/// These should NOT be stored in unknown_fields
pub const KNOWN_RESOURCE_ATTRS: &[&str] = &[
    "service.name",
    "service.version",
    "telemetry.sdk.name",
    "telemetry.sdk.language",
    "telemetry.sdk.version",
    "server.address",
    "server.port",
];

pub const KNOWN_SPAN_ATTRS: &[&str] = &[
    // GenAI common
    "gen_ai.system",
    "gen_ai.provider.name",
    "gen_ai.request.model",
    "gen_ai.response.model",
    "gen_ai.operation.name",
    "gen_ai.conversation.id",
    "gen_ai.response.id",
    "gen_ai.agent.id",
    "gen_ai.agent.name",
    "gen_ai.request.instructions",
    "gen_ai.response.finish_reasons",
    // Token usage
    "gen_ai.usage.input_tokens",
    "gen_ai.usage.output_tokens",
    "gen_ai.usage.total_tokens",
    "gen_ai.usage.prompt_tokens",
    "gen_ai.usage.completion_tokens",
    "gen_ai.usage.cache_read_input_tokens",
    "gen_ai.usage.cache_write_input_tokens",
    // Request params
    "gen_ai.request.temperature",
    "gen_ai.request.top_p",
    "gen_ai.request.top_k",
    "gen_ai.request.max_tokens",
    "gen_ai.request.frequency_penalty",
    "gen_ai.request.presence_penalty",
    "gen_ai.request.stop_sequences",
    // Tool
    "gen_ai.tool.name",
    "gen_ai.tool.call.id",
    "tool.status",
    // Strands
    "event_loop.cycle_id",
    "event_loop.parent_cycle_id",
    "strands.agent.name",
    // LangSmith
    "langsmith.trace.name",
    "langsmith.span.kind",
    "langsmith.trace.session_id",
    "langsmith.trace.session_name",
    "langsmith.trace.tags",
    // LangGraph
    "langgraph.node",
    "langgraph.edge.source",
    "langgraph.edge.target",
    "langgraph.state.version",
    "langgraph.state.changes_count",
    // OpenInference
    "session.id",
    "user.id",
    "openinference.span.kind",
    "llm.model_name",
    // AutoGen
    "autogen.agent.name",
    "autogen.message.type",
    // Semantic Kernel
    "semantic_kernel.function.name",
];

/// Check if an attribute key is known/handled
pub fn is_known_resource_attr(key: &str) -> bool {
    KNOWN_RESOURCE_ATTRS.contains(&key)
}

pub fn is_known_span_attr(key: &str) -> bool {
    KNOWN_SPAN_ATTRS.contains(&key)
}
