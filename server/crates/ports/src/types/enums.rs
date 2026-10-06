//! Classification enums for analytics data
//!
//! These enums are used across all database backends for consistent
//! classification of spans, messages, and metrics.

use serde::{Deserialize, Serialize};
use strum::{EnumString, IntoStaticStr, VariantArray};

// ============================================================================
// CLASSIFICATION ENUMS
// ============================================================================

/// Observation types for LLM telemetry spans
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    IntoStaticStr,
    EnumString,
    VariantArray,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
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
    /// The stored spelling, which is also the JSON one.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

/// Span categories for high-level classification
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    IntoStaticStr,
    EnumString,
    VariantArray,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
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
    /// The stored spelling, which is also the JSON one.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

// ============================================================================
// MESSAGE ENUMS
// ============================================================================

/// Message categories for GenAI and other message types
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    utoipa::ToSchema,
    VariantArray,
)]
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

/// Source type for GenAI messages
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    IntoStaticStr,
    EnumString,
    VariantArray,
)]
#[strum(serialize_all = "lowercase")]
pub enum MessageSourceType {
    /// Extracted from OTEL span events
    #[default]
    Event,
    /// Extracted from span attributes (framework adapters)
    Attribute,
}

impl MessageSourceType {
    /// The identity spelling, which is *not* the JSON one: `serde` writes the variant name
    /// (`"Event"`), and this is lowercase because it is an input to a block's dedup digest.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

// ============================================================================
// METRIC ENUMS
// ============================================================================

/// Metric type classification (from OTLP)
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    IntoStaticStr,
    EnumString,
    VariantArray,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MetricType {
    #[default]
    Gauge,
    Sum,
    Histogram,
    ExponentialHistogram,
    Summary,
}

impl MetricType {
    /// The stored spelling, which is also the JSON one.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

/// Aggregation temporality (for Sum/Histogram types)
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    IntoStaticStr,
    EnumString,
    VariantArray,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AggregationTemporality {
    #[default]
    Unspecified,
    Delta,
    Cumulative,
}

impl AggregationTemporality {
    /// The stored spelling, which is also the JSON one.
    pub fn as_str(&self) -> &'static str {
        self.into()
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
    use std::str::FromStr;

    use super::*;

    /// The spelling every one of these variants is stored under, written out.
    ///
    /// This is the table the `as_str` match arms used to be, moved from production code into the test
    /// that proves `strum` reproduces it. These strings are in SQLite, DuckDB, PostgreSQL and
    /// ClickHouse rows that already exist, so a changed spelling is a silent read failure on old data,
    /// not a compile error - which is exactly why the list has to live somewhere a test can check it.
    #[test]
    fn every_stored_spelling_round_trips_and_is_unchanged() {
        fn check<T>(expected: &[(T, &str)])
        where
            T: Copy + PartialEq + std::fmt::Debug + Into<&'static str> + FromStr + VariantArray,
            <T as FromStr>::Err: std::fmt::Debug,
        {
            for (variant, spelling) in expected {
                let rendered: &'static str = (*variant).into();
                assert_eq!(rendered, *spelling, "spelling changed for {variant:?}");
                assert_eq!(
                    &T::from_str(spelling).expect("the stored spelling must parse back"),
                    variant
                );
            }
            assert_eq!(
                T::VARIANTS.len(),
                expected.len(),
                "a variant was added without a spelling in this table"
            );
        }

        check(&[
            (ObservationType::Generation, "generation"),
            (ObservationType::Embedding, "embedding"),
            (ObservationType::Agent, "agent"),
            (ObservationType::Tool, "tool"),
            (ObservationType::Chain, "chain"),
            (ObservationType::Retriever, "retriever"),
            (ObservationType::Guardrail, "guardrail"),
            (ObservationType::Evaluator, "evaluator"),
            (ObservationType::Span, "span"),
        ]);
        check(&[
            (SpanCategory::LLM, "llm"),
            (SpanCategory::Tool, "tool"),
            (SpanCategory::Agent, "agent"),
            (SpanCategory::Chain, "chain"),
            (SpanCategory::Retriever, "retriever"),
            (SpanCategory::Embedding, "embedding"),
            (SpanCategory::DB, "db"),
            (SpanCategory::Storage, "storage"),
            (SpanCategory::HTTP, "http"),
            (SpanCategory::Messaging, "messaging"),
            (SpanCategory::Other, "other"),
        ]);
        check(&[
            (MessageSourceType::Event, "event"),
            (MessageSourceType::Attribute, "attribute"),
        ]);
        check(&[
            (MetricType::Gauge, "gauge"),
            (MetricType::Sum, "sum"),
            (MetricType::Histogram, "histogram"),
            (MetricType::ExponentialHistogram, "exponential_histogram"),
            (MetricType::Summary, "summary"),
        ]);
        check(&[
            (AggregationTemporality::Unspecified, "unspecified"),
            (AggregationTemporality::Delta, "delta"),
            (AggregationTemporality::Cumulative, "cumulative"),
        ]);
    }

    /// `MessageCategory` had an `as_str` table too, and nothing outside its own test called it: the
    /// spellings exist for JSON, where `serde` writes the variant name. This is that table, asserted
    /// through the direction that actually has users.
    #[test]
    fn message_category_json_spellings_are_unchanged() {
        let expected = [
            (MessageCategory::Log, "Log"),
            (MessageCategory::Exception, "Exception"),
            (MessageCategory::GenAISystemMessage, "GenAISystemMessage"),
            (MessageCategory::GenAIUserMessage, "GenAIUserMessage"),
            (
                MessageCategory::GenAIAssistantMessage,
                "GenAIAssistantMessage",
            ),
            (MessageCategory::GenAIToolMessage, "GenAIToolMessage"),
            (MessageCategory::GenAIToolInput, "GenAIToolInput"),
            (
                MessageCategory::GenAIToolDefinitions,
                "GenAIToolDefinitions",
            ),
            (MessageCategory::GenAIChoice, "GenAIChoice"),
            (MessageCategory::GenAIContext, "GenAIContext"),
            (MessageCategory::Retrieval, "Retrieval"),
            (MessageCategory::Observation, "Observation"),
            (MessageCategory::Other, "Other"),
        ];
        for (variant, spelling) in expected {
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                format!("\"{spelling}\"")
            );
            assert_eq!(
                serde_json::from_str::<MessageCategory>(&format!("\"{spelling}\"")).unwrap(),
                variant
            );
        }
        assert_eq!(MessageCategory::VARIANTS.len(), expected.len());
    }

    /// The `serde` spelling and the stored one agree for every enum that has both - except
    /// `MessageSourceType`, whose JSON form is the variant name and whose stored form is lowercase.
    #[test]
    fn serde_and_stored_spellings_agree_where_they_are_meant_to() {
        for variant in ObservationType::VARIANTS {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(json.trim_matches('"'), variant.as_str());
        }
        for variant in SpanCategory::VARIANTS {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(json.trim_matches('"'), variant.as_str());
        }
        for variant in MetricType::VARIANTS {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(json.trim_matches('"'), variant.as_str());
        }
        for variant in AggregationTemporality::VARIANTS {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(json.trim_matches('"'), variant.as_str());
        }
        assert_eq!(
            serde_json::to_string(&MessageSourceType::Event).unwrap(),
            "\"Event\"",
            "the JSON form is deliberately not the stored one"
        );
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
