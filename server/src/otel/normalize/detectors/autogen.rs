//! Microsoft AutoGen framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, get_string_attr,
    has_attribute,
};

/// Microsoft AutoGen framework detector
pub struct AutoGenDetector;

impl AutoGenDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AutoGenDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for AutoGenDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::AutoGen
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        scope.name.contains("autogen")
            || has_attribute(span, "autogen.agent.name")
            || has_attribute(span, "autogen.message.type")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        if normalized.gen_ai_agent_name.is_none() {
            normalized.gen_ai_agent_name = get_string_attr(span, "autogen.agent.name");
        }
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        if has_attribute(span, "autogen.agent.name") {
            SpanCategory::Agent
        } else {
            SpanCategory::Unknown
        }
    }
}
