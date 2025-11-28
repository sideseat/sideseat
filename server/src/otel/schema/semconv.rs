//! OpenTelemetry Semantic Conventions
//!
//! Defines known attribute keys based on OTel semantic conventions.
//! Unknown fields from incoming OTLP data are preserved to ensure no data loss.

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
    // GenAI semantic conventions (https://opentelemetry.io/docs/specs/semconv/registry/attributes/gen-ai/)
    "gen_ai.system", // Deprecated: use gen_ai.provider.name
    "gen_ai.provider.name",
    "gen_ai.request.model",
    "gen_ai.response.model",
    "gen_ai.operation.name",
    "gen_ai.conversation.id",
    "gen_ai.response.id",
    "gen_ai.agent.id",
    "gen_ai.agent.name",
    "gen_ai.agent.description",
    "gen_ai.system_instructions",
    "gen_ai.response.finish_reasons",
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    "gen_ai.output.type",
    "gen_ai.data_source.id",
    // Token usage
    "gen_ai.usage.input_tokens",
    "gen_ai.usage.output_tokens",
    "gen_ai.usage.total_tokens",
    "gen_ai.usage.prompt_tokens", // Deprecated: use gen_ai.usage.input_tokens
    "gen_ai.usage.completion_tokens", // Deprecated: use gen_ai.usage.output_tokens
    "gen_ai.usage.cache_read_input_tokens",
    "gen_ai.usage.cache_write_input_tokens",
    "gen_ai.token.type",
    // Request params
    "gen_ai.request.temperature",
    "gen_ai.request.top_p",
    "gen_ai.request.top_k",
    "gen_ai.request.max_tokens",
    "gen_ai.request.frequency_penalty",
    "gen_ai.request.presence_penalty",
    "gen_ai.request.stop_sequences",
    "gen_ai.request.seed",
    "gen_ai.request.choice.count",
    "gen_ai.request.encoding_formats",
    // Tool semantic conventions
    "gen_ai.tool.name",
    "gen_ai.tool.call.id",
    "gen_ai.tool.call.arguments",
    "gen_ai.tool.call.result",
    "gen_ai.tool.type",
    "gen_ai.tool.description",
    "gen_ai.tool.definitions",
    "tool.status",
    // AWS Bedrock semantic conventions (https://opentelemetry.io/docs/specs/semconv/registry/attributes/aws/)
    "aws.bedrock.guardrail.id",
    "aws.bedrock.knowledge_base.id",
    // OpenAI semantic conventions (https://opentelemetry.io/docs/specs/semconv/registry/attributes/openai/)
    "openai.request.service_tier",
    "openai.response.service_tier",
    "openai.response.system_fingerprint",
    // Session semantic conventions (https://opentelemetry.io/docs/specs/semconv/registry/attributes/session/)
    "session.id",
    "session.previous_id",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_fields_new() {
        let fields = UnknownFields::new();
        assert!(fields.is_empty());
    }

    #[test]
    fn test_unknown_fields_default() {
        let fields = UnknownFields::default();
        assert!(fields.is_empty());
    }

    #[test]
    fn test_unknown_fields_is_empty() {
        let mut fields = UnknownFields::new();
        assert!(fields.is_empty());

        fields.resource_attributes.insert("key".to_string(), serde_json::json!("value"));
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_unknown_fields_add_resource_attr() {
        let mut fields = UnknownFields::new();
        fields.add_resource_attr("custom.attr".to_string(), serde_json::json!("value"));
        assert!(!fields.is_empty());
        assert_eq!(
            fields.resource_attributes.get("custom.attr"),
            Some(&serde_json::json!("value"))
        );
    }

    #[test]
    fn test_unknown_fields_add_span_attr() {
        let mut fields = UnknownFields::new();
        fields.add_span_attr("custom.span.attr".to_string(), serde_json::json!(123));
        assert!(!fields.is_empty());
        assert_eq!(fields.span_attributes.get("custom.span.attr"), Some(&serde_json::json!(123)));
    }

    #[test]
    fn test_unknown_fields_to_json_empty() {
        let fields = UnknownFields::new();
        assert!(fields.to_json().is_none());
    }

    #[test]
    fn test_unknown_fields_to_json_with_data() {
        let mut fields = UnknownFields::new();
        fields.add_resource_attr("key".to_string(), serde_json::json!("value"));
        let json = fields.to_json();
        assert!(json.is_some());
        assert!(json.unwrap().contains("resource_attributes"));
    }

    #[test]
    fn test_unknown_fields_from_json() {
        let json = r#"{"resource_attributes":{"key":"value"}}"#;
        let fields = UnknownFields::from_json(json);
        assert!(fields.is_some());
        let fields = fields.unwrap();
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_unknown_fields_from_json_invalid() {
        let fields = UnknownFields::from_json("not valid json");
        assert!(fields.is_none());
    }

    #[test]
    fn test_unknown_fields_with_links() {
        let mut fields = UnknownFields::new();
        fields.links.push(serde_json::json!({"trace_id": "abc"}));
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_unknown_fields_with_dropped_counts() {
        let mut fields = UnknownFields::new();
        fields.dropped_attributes_count = Some(5);
        assert!(!fields.is_empty());

        fields.dropped_attributes_count = None;
        fields.dropped_events_count = Some(3);
        assert!(!fields.is_empty());

        fields.dropped_events_count = None;
        fields.dropped_links_count = Some(1);
        assert!(!fields.is_empty());
    }

    #[test]
    fn test_is_known_resource_attr() {
        assert!(is_known_resource_attr("service.name"));
        assert!(is_known_resource_attr("service.version"));
        assert!(is_known_resource_attr("telemetry.sdk.name"));
        assert!(!is_known_resource_attr("custom.resource.attr"));
    }

    #[test]
    fn test_is_known_span_attr() {
        assert!(is_known_span_attr("gen_ai.system"));
        assert!(is_known_span_attr("gen_ai.request.model"));
        assert!(is_known_span_attr("gen_ai.usage.input_tokens"));
        assert!(!is_known_span_attr("custom.span.attr"));
    }

    #[test]
    fn test_known_resource_attrs_not_empty() {
        let attrs = KNOWN_RESOURCE_ATTRS;
        assert!(!attrs.is_empty());
    }

    #[test]
    fn test_known_span_attrs_not_empty() {
        let attrs = KNOWN_SPAN_ATTRS;
        assert!(!attrs.is_empty());
    }

    #[test]
    fn test_unknown_fields_serialization() {
        let mut fields = UnknownFields::new();
        fields.add_resource_attr("test".to_string(), serde_json::json!("value"));
        fields.dropped_attributes_count = Some(2);

        let json = serde_json::to_string(&fields).unwrap();
        let deserialized: UnknownFields = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.resource_attributes.get("test"), Some(&serde_json::json!("value")));
        assert_eq!(deserialized.dropped_attributes_count, Some(2));
    }

    #[test]
    fn test_unknown_fields_clone() {
        let mut fields = UnknownFields::new();
        fields.add_resource_attr("key".to_string(), serde_json::json!("value"));

        let cloned = fields.clone();
        assert_eq!(cloned.resource_attributes.get("key"), Some(&serde_json::json!("value")));
    }
}
