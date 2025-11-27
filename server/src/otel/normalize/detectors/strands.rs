//! Strands Agents framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::genai::{get_i64_attr, get_string_attr, has_attribute};
use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, extract_common_genai_fields,
};

/// Strands Agents framework detector
pub struct StrandsDetector;

impl StrandsDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StrandsDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for StrandsDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::StrandsAgents
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        // Check scope name
        if scope.name.starts_with("strands") || scope.name.contains("strands_agents") {
            return true;
        }
        // Check for Strands-specific attributes
        has_attribute(span, "event_loop.cycle_id")
            || has_attribute(span, "strands.agent.name")
            || has_attribute(span, "gen_ai.agent.name")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // Strands-specific fields
        normalized.event_loop_cycle_id = get_string_attr(span, "event_loop.cycle_id");
        normalized.event_loop_parent_cycle_id = get_string_attr(span, "event_loop.parent_cycle_id");

        // GenAI fields (Strands uses standard conventions)
        extract_common_genai_fields(span, normalized);

        // Tool fields
        normalized.gen_ai_tool_name = normalized
            .gen_ai_tool_name
            .take()
            .or_else(|| get_string_attr(span, "gen_ai.tool.name"));
        normalized.tool_status = get_string_attr(span, "tool.status");

        // Cache tokens (Strands/Anthropic)
        normalized.usage_cache_read_tokens =
            get_i64_attr(span, "gen_ai.usage.cache_read_input_tokens");
        normalized.usage_cache_write_tokens =
            get_i64_attr(span, "gen_ai.usage.cache_write_input_tokens");
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        let name = &span.name;
        if name.contains("Agent.") || name.starts_with("invoke_agent") {
            SpanCategory::Agent
        } else if name.contains("Model.") || name.contains("converse") {
            SpanCategory::Llm
        } else if name.contains("Tool.") || has_attribute(span, "gen_ai.tool.name") {
            SpanCategory::Tool
        } else {
            SpanCategory::Unknown
        }
    }
}
