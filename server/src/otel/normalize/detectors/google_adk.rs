//! Google ADK / Vertex AI framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::genai::get_string_attr;
use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, extract_common_genai_fields,
};

/// Google ADK / Vertex AI framework detector
pub struct GoogleAdkDetector;

impl GoogleAdkDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GoogleAdkDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for GoogleAdkDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::GoogleAdk
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        scope.name.contains("google.adk")
            || scope.name.contains("vertexai")
            || get_string_attr(span, "gen_ai.system").as_deref() == Some("gcp.vertex_ai")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // Google ADK uses standard GenAI conventions
        extract_common_genai_fields(span, normalized);
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        match get_string_attr(span, "gen_ai.operation.name").as_deref() {
            Some("generate_content") | Some("chat") => SpanCategory::Llm,
            Some("call_tool") => SpanCategory::Tool,
            _ => SpanCategory::Unknown,
        }
    }
}
