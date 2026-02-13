//! Classification enums for analytics data
//!
//! These enums are used across all database backends for consistent
//! classification of spans, messages, and metrics.

use serde::{Deserialize, Serialize};

// ============================================================================
// CLASSIFICATION ENUMS
// ============================================================================

/// Observation types for LLM telemetry spans
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ObservationType {
    Generation,
    Embedding,
    Agent,
    Tool,
    Chain,
    Retriever,
    Guardrail,
    Evaluator,
    #[default]
    Span,
}

impl ObservationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Generation => "generation",
            Self::Embedding => "embedding",
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::Chain => "chain",
            Self::Retriever => "retriever",
            Self::Guardrail => "guardrail",
            Self::Evaluator => "evaluator",
            Self::Span => "span",
        }
    }
}

/// Span categories for high-level classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpanCategory {
    LLM,
    Tool,
    Agent,
    Chain,
    Retriever,
    Embedding,
    DB,
    Storage,
    HTTP,
    Messaging,
    #[default]
    Other,
}

impl SpanCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LLM => "llm",
            Self::Tool => "tool",
            Self::Agent => "agent",
            Self::Chain => "chain",
            Self::Retriever => "retriever",
            Self::Embedding => "embedding",
            Self::DB => "db",
            Self::Storage => "storage",
            Self::HTTP => "http",
            Self::Messaging => "messaging",
            Self::Other => "other",
        }
    }
}

/// AI/ML framework identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Framework {
    StrandsAgents,
    LangChain,
    LangGraph,
    LlamaIndex,
    OpenInference,
    AutoGen,
    CrewAI,
    SemanticKernel,
    AzureOpenAI,
    AzureAIFoundry,
    GoogleAdk,
    VertexAI,
    VercelAISdk,
    Logfire,
    MLFlow,
    TraceLoop,
    LiveKit,
    OpenAIAgents,
    AWSBedrock,
    #[default]
    Unknown,
}

impl Framework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StrandsAgents => "StrandsAgents",
            Self::LangChain => "LangChain",
            Self::LangGraph => "LangGraph",
            Self::LlamaIndex => "LlamaIndex",
            Self::OpenInference => "OpenInference",
            Self::AutoGen => "AutoGen",
            Self::CrewAI => "CrewAI",
            Self::SemanticKernel => "SemanticKernel",
            Self::AzureOpenAI => "AzureOpenAI",
            Self::AzureAIFoundry => "AzureAIFoundry",
            Self::GoogleAdk => "GoogleADK",
            Self::VertexAI => "VertexAI",
            Self::VercelAISdk => "VercelAISDK",
            Self::Logfire => "Logfire",
            Self::MLFlow => "MLflow",
            Self::TraceLoop => "TraceLoop",
            Self::LiveKit => "LiveKit",
            Self::OpenAIAgents => "OpenAIAgents",
            Self::AWSBedrock => "AWSBedrock",
            Self::Unknown => "Unknown",
        }
    }
}

// ============================================================================
// MESSAGE ENUMS
// ============================================================================

/// Message categories for GenAI and other message types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub enum MessageCategory {
    Log,
    Exception,
    GenAISystemMessage,
    GenAIUserMessage,
    GenAIAssistantMessage,
    GenAIToolMessage,
    /// Tool input/invocation (arguments passed to tool)
    GenAIToolInput,
    /// Tool definitions (available tools for the model)
    GenAIToolDefinitions,
    GenAIChoice,
    /// Context/conversation history (e.g., Google ADK data, LiveKit context)
    GenAIContext,
    Retrieval,
    Observation,
    #[default]
    Other,
}

impl MessageCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Log => "Log",
            Self::Exception => "Exception",
            Self::GenAISystemMessage => "GenAISystemMessage",
            Self::GenAIUserMessage => "GenAIUserMessage",
            Self::GenAIAssistantMessage => "GenAIAssistantMessage",
            Self::GenAIToolMessage => "GenAIToolMessage",
            Self::GenAIToolInput => "GenAIToolInput",
            Self::GenAIToolDefinitions => "GenAIToolDefinitions",
            Self::GenAIChoice => "GenAIChoice",
            Self::GenAIContext => "GenAIContext",
            Self::Retrieval => "Retrieval",
            Self::Observation => "Observation",
            Self::Other => "Other",
        }
    }
}

/// Source type for GenAI messages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MessageSourceType {
    /// Extracted from OTEL span events
    #[default]
    Event,
    /// Extracted from span attributes (framework adapters)
    Attribute,
}

impl MessageSourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Attribute => "attribute",
        }
    }
}

// ============================================================================
// METRIC ENUMS
// ============================================================================

/// Metric type classification (from OTLP)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    #[default]
    Gauge,
    Sum,
    Histogram,
    ExponentialHistogram,
    Summary,
}

impl MetricType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Gauge => "gauge",
            Self::Sum => "sum",
            Self::Histogram => "histogram",
            Self::ExponentialHistogram => "exponential_histogram",
            Self::Summary => "summary",
        }
    }
}

/// Aggregation temporality (for Sum/Histogram types)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AggregationTemporality {
    #[default]
    Unspecified,
    Delta,
    Cumulative,
}

impl AggregationTemporality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Delta => "delta",
            Self::Cumulative => "cumulative",
        }
    }

    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => Self::Delta,
            2 => Self::Cumulative,
            _ => Self::Unspecified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observation_type_as_str() {
        assert_eq!(ObservationType::Generation.as_str(), "generation");
        assert_eq!(ObservationType::Span.as_str(), "span");
    }

    #[test]
    fn test_span_category_as_str() {
        assert_eq!(SpanCategory::LLM.as_str(), "llm");
        assert_eq!(SpanCategory::Other.as_str(), "other");
    }

    #[test]
    fn test_framework_as_str() {
        assert_eq!(Framework::StrandsAgents.as_str(), "StrandsAgents");
        assert_eq!(Framework::LangChain.as_str(), "LangChain");
        assert_eq!(Framework::Unknown.as_str(), "Unknown");
    }

    #[test]
    fn test_message_category_as_str() {
        assert_eq!(
            MessageCategory::GenAIUserMessage.as_str(),
            "GenAIUserMessage"
        );
        assert_eq!(MessageCategory::Exception.as_str(), "Exception");
        assert_eq!(MessageCategory::GenAIToolInput.as_str(), "GenAIToolInput");
    }

    #[test]
    fn test_message_source_type_as_str() {
        assert_eq!(MessageSourceType::Event.as_str(), "event");
        assert_eq!(MessageSourceType::Attribute.as_str(), "attribute");
    }

    #[test]
    fn test_metric_type_as_str() {
        assert_eq!(MetricType::Gauge.as_str(), "gauge");
        assert_eq!(MetricType::Sum.as_str(), "sum");
        assert_eq!(MetricType::Histogram.as_str(), "histogram");
        assert_eq!(
            MetricType::ExponentialHistogram.as_str(),
            "exponential_histogram"
        );
        assert_eq!(MetricType::Summary.as_str(), "summary");
    }

    #[test]
    fn test_aggregation_temporality_as_str() {
        assert_eq!(AggregationTemporality::Unspecified.as_str(), "unspecified");
        assert_eq!(AggregationTemporality::Delta.as_str(), "delta");
        assert_eq!(AggregationTemporality::Cumulative.as_str(), "cumulative");
    }

    #[test]
    fn test_aggregation_temporality_from_i32() {
        assert_eq!(
            AggregationTemporality::from_i32(0),
            AggregationTemporality::Unspecified
        );
        assert_eq!(
            AggregationTemporality::from_i32(1),
            AggregationTemporality::Delta
        );
        assert_eq!(
            AggregationTemporality::from_i32(2),
            AggregationTemporality::Cumulative
        );
        assert_eq!(
            AggregationTemporality::from_i32(99),
            AggregationTemporality::Unspecified
        );
    }
}
