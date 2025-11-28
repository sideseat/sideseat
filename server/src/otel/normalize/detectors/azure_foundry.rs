//! Azure AI Foundry framework detector

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, get_string_attr,
};

/// Azure AI Foundry framework detector
pub struct AzureFoundryDetector;

impl AzureFoundryDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AzureFoundryDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for AzureFoundryDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::AzureAiFoundry
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        scope.name.contains("azure.ai")
            || scope.name.contains("azure_ai_projects")
            || get_string_attr(span, "gen_ai.system").as_deref() == Some("az.ai.inference")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // OTel standard: gen_ai.system_instructions
        normalized.gen_ai_system_instructions = get_string_attr(span, "gen_ai.system_instructions");
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        match get_string_attr(span, "gen_ai.operation.name").as_deref() {
            Some("create_agent") | Some("invoke_agent") => SpanCategory::Agent,
            Some("chat") | Some("generate_content") => SpanCategory::Llm,
            Some("execute_tool") => SpanCategory::Tool,
            _ => SpanCategory::Unknown,
        }
    }
}
