//! LangChain/LangSmith framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::genai::{get_string_array_attr, get_string_attr, has_attribute};
use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, extract_common_genai_fields,
};

/// LangChain/LangSmith framework detector
pub struct LangChainDetector;

impl LangChainDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LangChainDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for LangChainDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::LangChain
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        // Check scope for langchain
        if scope.name.contains("langchain") || scope.name.contains("langsmith") {
            return true;
        }
        // Check for LangSmith-specific attributes
        has_attribute(span, "langsmith.trace.name") || has_attribute(span, "langsmith.span.kind")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // LangSmith-specific fields
        normalized.langsmith_trace_name = get_string_attr(span, "langsmith.trace.name");
        normalized.langsmith_span_kind = get_string_attr(span, "langsmith.span.kind");
        normalized.langsmith_session_id = get_string_attr(span, "langsmith.trace.session_id");
        normalized.langsmith_session_name = get_string_attr(span, "langsmith.trace.session_name");
        normalized.langsmith_tags = get_string_array_attr(span, "langsmith.trace.tags");

        extract_common_genai_fields(span, normalized);
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        match get_string_attr(span, "langsmith.span.kind").as_deref() {
            Some("chain") => SpanCategory::Chain,
            Some("llm") => SpanCategory::Llm,
            Some("tool") => SpanCategory::Tool,
            Some("retriever") => SpanCategory::Retriever,
            Some("embedding") => SpanCategory::Embedding,
            _ => SpanCategory::Unknown,
        }
    }
}
