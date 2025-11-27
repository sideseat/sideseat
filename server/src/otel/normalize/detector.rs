//! Framework detection traits and registry

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use super::NormalizedSpan;
use super::detectors::*;

/// Detected AI framework
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectedFramework {
    StrandsAgents,
    LangChain,
    LangGraph,
    LlamaIndex,
    AutoGen,
    SemanticKernel,
    AzureAiFoundry,
    GoogleAdk,
    Unknown,
}

impl DetectedFramework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StrandsAgents => "strands-agents",
            Self::LangChain => "langchain",
            Self::LangGraph => "langgraph",
            Self::LlamaIndex => "llamaindex",
            Self::AutoGen => "autogen",
            Self::SemanticKernel => "semantic-kernel",
            Self::AzureAiFoundry => "azure-ai-foundry",
            Self::GoogleAdk => "google-adk",
            Self::Unknown => "unknown",
        }
    }
}

/// Span category within a framework
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanCategory {
    Agent,
    Llm,
    Tool,
    Chain,
    Retriever,
    Embedding,
    Memory,
    Unknown,
}

/// Trait for framework-specific detectors
pub trait FrameworkDetector: Send + Sync {
    /// Returns the framework this detector handles
    fn framework(&self) -> DetectedFramework;

    /// Check if this detector matches the span based on attributes/scope
    fn detect(&self, span: &OtlpSpan, resource: &Resource, scope: &InstrumentationScope) -> bool;

    /// Extract framework-specific fields into the normalized span
    fn extract(&self, span: &OtlpSpan, normalized: &mut NormalizedSpan);

    /// Determine the span category (agent, llm, tool, etc.)
    fn categorize(&self, span: &OtlpSpan) -> SpanCategory;
}

/// Result of framework detection
pub struct DetectionResult<'a> {
    pub framework: DetectedFramework,
    pub category: SpanCategory,
    pub extractor: Option<&'a dyn FrameworkDetector>,
}

/// Registry of all framework detectors
pub struct DetectorRegistry {
    detectors: Vec<Box<dyn FrameworkDetector>>,
}

impl DetectorRegistry {
    pub fn new() -> Self {
        let detectors: Vec<Box<dyn FrameworkDetector>> = vec![
            Box::new(StrandsDetector::new()),
            Box::new(LangChainDetector::new()),
            Box::new(LangGraphDetector::new()),
            Box::new(LlamaIndexDetector::new()),
            Box::new(AutoGenDetector::new()),
            Box::new(SemanticKernelDetector::new()),
            Box::new(AzureFoundryDetector::new()),
            Box::new(GoogleAdkDetector::new()),
        ];
        Self { detectors }
    }

    /// Detect framework and extract fields
    pub fn process(
        &self,
        span: &OtlpSpan,
        resource: &Resource,
        scope: &InstrumentationScope,
    ) -> DetectionResult<'_> {
        for detector in &self.detectors {
            if detector.detect(span, resource, scope) {
                return DetectionResult {
                    framework: detector.framework(),
                    category: detector.categorize(span),
                    extractor: Some(detector.as_ref()),
                };
            }
        }
        DetectionResult {
            framework: DetectedFramework::Unknown,
            category: SpanCategory::Unknown,
            extractor: None,
        }
    }
}

impl Default for DetectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};

    fn create_test_span(name: &str, attrs: Vec<(&str, &str)>) -> OtlpSpan {
        let attributes: Vec<KeyValue> = attrs
            .into_iter()
            .map(|(k, v)| KeyValue {
                key: k.to_string(),
                value: Some(AnyValue { value: Some(any_value::Value::StringValue(v.to_string())) }),
            })
            .collect();

        OtlpSpan {
            name: name.to_string(),
            attributes,
            trace_id: vec![0; 16],
            span_id: vec![0; 8],
            parent_span_id: vec![],
            start_time_unix_nano: 0,
            end_time_unix_nano: 0,
            kind: 0,
            status: None,
            events: vec![],
            links: vec![],
            dropped_attributes_count: 0,
            dropped_events_count: 0,
            dropped_links_count: 0,
            trace_state: String::new(),
            flags: 0,
        }
    }

    fn create_test_scope(name: &str) -> InstrumentationScope {
        InstrumentationScope {
            name: name.to_string(),
            version: "1.0".to_string(),
            attributes: vec![],
            dropped_attributes_count: 0,
        }
    }

    fn create_empty_resource() -> Resource {
        Resource { attributes: vec![], dropped_attributes_count: 0 }
    }

    #[test]
    fn test_detected_framework_as_str() {
        assert_eq!(DetectedFramework::StrandsAgents.as_str(), "strands-agents");
        assert_eq!(DetectedFramework::LangChain.as_str(), "langchain");
        assert_eq!(DetectedFramework::LangGraph.as_str(), "langgraph");
        assert_eq!(DetectedFramework::LlamaIndex.as_str(), "llamaindex");
        assert_eq!(DetectedFramework::AutoGen.as_str(), "autogen");
        assert_eq!(DetectedFramework::SemanticKernel.as_str(), "semantic-kernel");
        assert_eq!(DetectedFramework::AzureAiFoundry.as_str(), "azure-ai-foundry");
        assert_eq!(DetectedFramework::GoogleAdk.as_str(), "google-adk");
        assert_eq!(DetectedFramework::Unknown.as_str(), "unknown");
    }

    #[test]
    fn test_detector_registry_new() {
        let registry = DetectorRegistry::new();
        assert_eq!(registry.detectors.len(), 8); // All 8 framework detectors
    }

    #[test]
    fn test_detector_registry_default() {
        let registry = DetectorRegistry::default();
        assert_eq!(registry.detectors.len(), 8);
    }

    #[test]
    fn test_detect_langchain_by_scope() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![]);
        let resource = create_empty_resource();
        let scope = create_test_scope("langchain.trace");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::LangChain);
    }

    #[test]
    fn test_detect_langchain_by_attribute() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![("langsmith.trace.name", "my-trace")]);
        let resource = create_empty_resource();
        let scope = create_test_scope("generic");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::LangChain);
    }

    #[test]
    fn test_detect_llamaindex_by_scope() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![]);
        let resource = create_empty_resource();
        let scope = create_test_scope("llama_index.something");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::LlamaIndex);
    }

    #[test]
    fn test_detect_strands_by_scope() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![]);
        let resource = create_empty_resource();
        let scope = create_test_scope("strands.agent");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::StrandsAgents);
    }

    #[test]
    fn test_detect_unknown_framework() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![("custom.attribute", "value")]);
        let resource = create_empty_resource();
        let scope = create_test_scope("some-random-scope");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::Unknown);
        assert_eq!(result.category, SpanCategory::Unknown);
        assert!(result.extractor.is_none());
    }

    #[test]
    fn test_span_category_detection_langchain_llm() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![("langsmith.span.kind", "llm")]);
        let resource = create_empty_resource();
        let scope = create_test_scope("langchain");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::LangChain);
        assert_eq!(result.category, SpanCategory::Llm);
    }

    #[test]
    fn test_span_category_detection_langchain_tool() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![("langsmith.span.kind", "tool")]);
        let resource = create_empty_resource();
        let scope = create_test_scope("langchain");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::LangChain);
        assert_eq!(result.category, SpanCategory::Tool);
    }

    #[test]
    fn test_detector_priority_first_match() {
        // The registry should return the first matching detector
        let registry = DetectorRegistry::new();
        // Strands detector is first in the list
        let span = create_test_span("test", vec![]);
        let resource = create_empty_resource();
        let scope = create_test_scope("strands");

        let result = registry.process(&span, &resource, &scope);
        assert_eq!(result.framework, DetectedFramework::StrandsAgents);
    }

    #[test]
    fn test_detection_result_has_extractor() {
        let registry = DetectorRegistry::new();
        let span = create_test_span("test", vec![]);
        let resource = create_empty_resource();
        let scope = create_test_scope("langchain");

        let result = registry.process(&span, &resource, &scope);
        assert!(result.extractor.is_some());
        assert_eq!(result.extractor.unwrap().framework(), DetectedFramework::LangChain);
    }
}
