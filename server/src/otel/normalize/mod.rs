//! Framework detection and span normalization

mod common;
mod detector;
pub mod detectors;

pub use common::{
    extract_common_fields, get_f64_attr, get_i64_attr, get_string_array_attr, get_string_attr,
    has_attribute,
};
pub use detector::{
    DetectedFramework, DetectionResult, DetectorRegistry, FrameworkDetector, SpanCategory,
};

use std::sync::Arc;

/// Normalized span after framework detection and field extraction
#[derive(Debug, Clone)]
pub struct NormalizedSpan {
    // Core identification
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,

    // Timing
    pub start_time_unix_nano: i64,
    pub end_time_unix_nano: Option<i64>,
    pub duration_ns: Option<i64>,

    // Resource info
    pub service_name: String,
    pub service_version: Option<String>,
    pub sdk_name: Option<String>,
    pub sdk_language: Option<String>,
    pub server_address: Option<String>,
    pub server_port: Option<i32>,

    // Span info
    pub span_name: String,
    pub span_kind: i8,
    pub status_code: i8,
    pub status_message: Option<String>,

    // Detection results
    pub detected_framework: String,
    pub detected_framework_version: Option<String>,
    pub detected_category: Option<String>,

    // GenAI fields
    pub gen_ai_system: Option<String>,
    pub gen_ai_provider_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub gen_ai_response_model: Option<String>,
    pub gen_ai_operation_name: Option<String>,
    pub gen_ai_conversation_id: Option<String>,
    pub gen_ai_response_id: Option<String>,
    pub gen_ai_agent_id: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_system_instructions: Option<String>,
    pub gen_ai_response_finish_reasons: Option<String>,

    // Token usage
    pub usage_input_tokens: Option<i64>,
    pub usage_output_tokens: Option<i64>,
    pub usage_total_tokens: Option<i64>,
    pub usage_prompt_tokens: Option<i64>,
    pub usage_completion_tokens: Option<i64>,
    pub usage_cache_read_tokens: Option<i64>,
    pub usage_cache_write_tokens: Option<i64>,

    // Request parameters
    pub gen_ai_request_temperature: Option<f64>,
    pub gen_ai_request_top_p: Option<f64>,
    pub gen_ai_request_top_k: Option<i64>,
    pub gen_ai_request_max_tokens: Option<i64>,
    pub gen_ai_request_frequency_penalty: Option<f64>,
    pub gen_ai_request_presence_penalty: Option<f64>,
    pub gen_ai_request_stop_sequences: Option<String>,

    // Tool fields
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_tool_call_id: Option<String>,
    pub tool_status: Option<String>,

    // Framework-specific fields (Strands)
    pub event_loop_cycle_id: Option<String>,
    pub event_loop_parent_cycle_id: Option<String>,

    // Framework-specific fields (LangSmith/LangChain)
    pub langsmith_trace_name: Option<String>,
    pub langsmith_span_kind: Option<String>,
    pub langsmith_session_id: Option<String>,
    pub langsmith_session_name: Option<String>,
    pub langsmith_tags: Option<String>,

    // Framework-specific fields (LangGraph)
    pub langgraph_node: Option<String>,
    pub langgraph_edge_source: Option<String>,
    pub langgraph_edge_target: Option<String>,
    pub langgraph_state_version: Option<String>,
    pub langgraph_state_changes_count: Option<i64>,

    // Common cross-framework fields (normalized from various sources)
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub tags: Option<String>,
    pub environment: Option<String>,

    // Generic storage
    pub attributes_json: String,
    pub resource_attributes_json: Option<String>,
    pub unknown_fields_json: Option<String>,

    // Instrumentation scope
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
}

impl Default for NormalizedSpan {
    fn default() -> Self {
        Self {
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: None,
            start_time_unix_nano: 0,
            end_time_unix_nano: None,
            duration_ns: None,
            service_name: "unknown".to_string(),
            service_version: None,
            sdk_name: None,
            sdk_language: None,
            server_address: None,
            server_port: None,
            span_name: String::new(),
            span_kind: 0,
            status_code: 0,
            status_message: None,
            detected_framework: "unknown".to_string(),
            detected_framework_version: None,
            detected_category: None,
            gen_ai_system: None,
            gen_ai_provider_name: None,
            gen_ai_request_model: None,
            gen_ai_response_model: None,
            gen_ai_operation_name: None,
            gen_ai_conversation_id: None,
            gen_ai_response_id: None,
            gen_ai_agent_id: None,
            gen_ai_agent_name: None,
            gen_ai_system_instructions: None,
            gen_ai_response_finish_reasons: None,
            usage_input_tokens: None,
            usage_output_tokens: None,
            usage_total_tokens: None,
            usage_prompt_tokens: None,
            usage_completion_tokens: None,
            usage_cache_read_tokens: None,
            usage_cache_write_tokens: None,
            gen_ai_request_temperature: None,
            gen_ai_request_top_p: None,
            gen_ai_request_top_k: None,
            gen_ai_request_max_tokens: None,
            gen_ai_request_frequency_penalty: None,
            gen_ai_request_presence_penalty: None,
            gen_ai_request_stop_sequences: None,
            gen_ai_tool_name: None,
            gen_ai_tool_call_id: None,
            tool_status: None,
            event_loop_cycle_id: None,
            event_loop_parent_cycle_id: None,
            langsmith_trace_name: None,
            langsmith_span_kind: None,
            langsmith_session_id: None,
            langsmith_session_name: None,
            langsmith_tags: None,
            langgraph_node: None,
            langgraph_edge_source: None,
            langgraph_edge_target: None,
            langgraph_state_version: None,
            langgraph_state_changes_count: None,
            session_id: None,
            user_id: None,
            tags: None,
            environment: None,
            attributes_json: "{}".to_string(),
            resource_attributes_json: None,
            unknown_fields_json: None,
            scope_name: None,
            scope_version: None,
        }
    }
}

/// Span event (logs attached to spans)
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub span_id: String,
    pub trace_id: String,
    pub event_time_ns: i64,
    pub event_name: String,
    pub attributes_json: String,
}

/// Normalizer wraps the detector registry and handles span normalization
pub struct Normalizer {
    detector_registry: Arc<DetectorRegistry>,
}

impl Normalizer {
    pub fn new() -> Self {
        Self { detector_registry: Arc::new(DetectorRegistry::new()) }
    }

    /// Get reference to the detector registry
    pub fn detector_registry(&self) -> &DetectorRegistry {
        &self.detector_registry
    }
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalized_span_default() {
        let span = NormalizedSpan::default();

        // Core identification
        assert!(span.trace_id.is_empty());
        assert!(span.span_id.is_empty());
        assert!(span.parent_span_id.is_none());

        // Timing
        assert_eq!(span.start_time_unix_nano, 0);
        assert!(span.end_time_unix_nano.is_none());
        assert!(span.duration_ns.is_none());

        // Resource info
        assert_eq!(span.service_name, "unknown");
        assert!(span.service_version.is_none());
        assert!(span.sdk_name.is_none());
        assert!(span.sdk_language.is_none());

        // Span info
        assert!(span.span_name.is_empty());
        assert_eq!(span.span_kind, 0);
        assert_eq!(span.status_code, 0);
        assert!(span.status_message.is_none());

        // Detection results
        assert_eq!(span.detected_framework, "unknown");
        assert!(span.detected_framework_version.is_none());
        assert!(span.detected_category.is_none());

        // GenAI fields
        assert!(span.gen_ai_system.is_none());
        assert!(span.gen_ai_request_model.is_none());
        assert!(span.gen_ai_response_model.is_none());

        // Token usage
        assert!(span.usage_input_tokens.is_none());
        assert!(span.usage_output_tokens.is_none());
        assert!(span.usage_total_tokens.is_none());

        // Generic storage
        assert_eq!(span.attributes_json, "{}");
        assert!(span.resource_attributes_json.is_none());
        assert!(span.unknown_fields_json.is_none());
    }

    #[test]
    fn test_normalized_span_clone() {
        let span = NormalizedSpan {
            trace_id: "test-trace-id".to_string(),
            span_id: "test-span-id".to_string(),
            service_name: "test-service".to_string(),
            gen_ai_request_model: Some("gpt-4".to_string()),
            usage_input_tokens: Some(100),
            ..Default::default()
        };

        let cloned = span.clone();
        assert_eq!(cloned.trace_id, "test-trace-id");
        assert_eq!(cloned.span_id, "test-span-id");
        assert_eq!(cloned.service_name, "test-service");
        assert_eq!(cloned.gen_ai_request_model, Some("gpt-4".to_string()));
        assert_eq!(cloned.usage_input_tokens, Some(100));
    }

    #[test]
    fn test_span_event_fields() {
        let event = SpanEvent {
            span_id: "span-123".to_string(),
            trace_id: "trace-456".to_string(),
            event_time_ns: 1234567890,
            event_name: "test-event".to_string(),
            attributes_json: r#"{"key": "value"}"#.to_string(),
        };

        assert_eq!(event.span_id, "span-123");
        assert_eq!(event.trace_id, "trace-456");
        assert_eq!(event.event_time_ns, 1234567890);
        assert_eq!(event.event_name, "test-event");
        assert_eq!(event.attributes_json, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_span_event_clone() {
        let event = SpanEvent {
            span_id: "span-123".to_string(),
            trace_id: "trace-456".to_string(),
            event_time_ns: 1234567890,
            event_name: "test-event".to_string(),
            attributes_json: "{}".to_string(),
        };

        let cloned = event.clone();
        assert_eq!(cloned.span_id, event.span_id);
        assert_eq!(cloned.trace_id, event.trace_id);
        assert_eq!(cloned.event_time_ns, event.event_time_ns);
    }

    #[test]
    fn test_normalizer_new() {
        let normalizer = Normalizer::new();
        // Should be able to get detector registry
        let registry = normalizer.detector_registry();
        // Registry should exist (not null)
        assert!(!std::ptr::eq(registry, std::ptr::null()));
    }

    #[test]
    fn test_normalizer_default() {
        let normalizer = Normalizer::default();
        let _registry = normalizer.detector_registry();
        // Default should work the same as new()
    }
}
