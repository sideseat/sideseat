//! Microsoft Semantic Kernel framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, get_string_attr,
    has_attribute,
};

/// Microsoft Semantic Kernel framework detector
pub struct SemanticKernelDetector;

impl SemanticKernelDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SemanticKernelDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for SemanticKernelDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::SemanticKernel
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        scope.name.contains("semantic_kernel")
            || scope.name.contains("Microsoft.SemanticKernel")
            || has_attribute(span, "semantic_kernel.function.name")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        if normalized.gen_ai_tool_name.is_none() {
            normalized.gen_ai_tool_name = get_string_attr(span, "semantic_kernel.function.name");
        }
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        if has_attribute(span, "semantic_kernel.function.name") {
            SpanCategory::Tool
        } else {
            SpanCategory::Unknown
        }
    }
}
