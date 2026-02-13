//! DuckDB data models
//!
//! Re-exports shared types from the data/types module for backward compatibility.
//! All types are now defined in data/types/ to be shared across all database backends.

// Re-export all types from the shared types module
pub use crate::data::types::{
    AggregationTemporality, Framework, MessageCategory, MessageSourceType, MetricType,
    NormalizedMetric, NormalizedSpan, ObservationType, SpanCategory,
};
