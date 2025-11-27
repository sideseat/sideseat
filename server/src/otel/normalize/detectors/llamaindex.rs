//! LlamaIndex / OpenInference framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::genai::{get_string_attr, has_attribute};
use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, extract_common_genai_fields,
};

/// LlamaIndex / OpenInference framework detector
pub struct LlamaIndexDetector;

impl LlamaIndexDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LlamaIndexDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for LlamaIndexDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::LlamaIndex
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        // OpenInference / LlamaIndex / Arize
        scope.name.contains("llama")
            || scope.name.contains("openinference")
            || has_attribute(span, "session.id")
            || has_attribute(span, "openinference.span.kind")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // OpenInference fields
        normalized.session_id = get_string_attr(span, "session.id");
        normalized.user_id = get_string_attr(span, "user.id");

        // Map OpenInference to GenAI conventions
        if normalized.gen_ai_request_model.is_none() {
            normalized.gen_ai_request_model = get_string_attr(span, "llm.model_name");
        }

        extract_common_genai_fields(span, normalized);
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        match get_string_attr(span, "openinference.span.kind").as_deref() {
            Some("LLM") => SpanCategory::Llm,
            Some("TOOL") => SpanCategory::Tool,
            Some("CHAIN") => SpanCategory::Chain,
            Some("RETRIEVER") => SpanCategory::Retriever,
            Some("EMBEDDING") => SpanCategory::Embedding,
            Some("AGENT") => SpanCategory::Agent,
            _ => SpanCategory::Unknown,
        }
    }
}
