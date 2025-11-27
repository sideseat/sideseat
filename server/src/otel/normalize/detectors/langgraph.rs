//! LangGraph framework detector (extends LangChain)

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use crate::otel::normalize::genai::{get_i64_attr, get_string_attr, has_attribute};
use crate::otel::normalize::{
    DetectedFramework, FrameworkDetector, NormalizedSpan, SpanCategory, extract_common_genai_fields,
};

/// LangGraph framework detector (extends LangChain)
pub struct LangGraphDetector;

impl LangGraphDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LangGraphDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDetector for LangGraphDetector {
    fn framework(&self) -> DetectedFramework {
        DetectedFramework::LangGraph
    }

    fn detect(&self, span: &OtlpSpan, _resource: &Resource, scope: &InstrumentationScope) -> bool {
        // LangGraph has graph-specific attributes
        scope.name.contains("langgraph")
            || has_attribute(span, "langgraph.node")
            || has_attribute(span, "langgraph.state.version")
    }

    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan) {
        // LangGraph-specific fields
        normalized.langgraph_node = get_string_attr(span, "langgraph.node");
        normalized.langgraph_edge_source = get_string_attr(span, "langgraph.edge.source");
        normalized.langgraph_edge_target = get_string_attr(span, "langgraph.edge.target");
        normalized.langgraph_state_version = get_string_attr(span, "langgraph.state.version");
        normalized.langgraph_state_changes_count =
            get_i64_attr(span, "langgraph.state.changes_count");

        // Also extract LangSmith fields (LangGraph uses LangSmith tracing)
        normalized.langsmith_trace_name = get_string_attr(span, "langsmith.trace.name");
        normalized.langsmith_span_kind = get_string_attr(span, "langsmith.span.kind");

        extract_common_genai_fields(span, normalized);
    }

    fn categorize(&self, span: &OtlpSpan) -> SpanCategory {
        if has_attribute(span, "langgraph.node") {
            SpanCategory::Chain // Graph nodes are essentially chain steps
        } else {
            SpanCategory::Unknown
        }
    }
}
