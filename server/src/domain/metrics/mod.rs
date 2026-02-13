//! Metrics Processing Pipeline
//!
//! Processes OTLP metrics: extraction, flattening, and persistence.
//! Supports all 5 OTLP metric types: Gauge, Sum, Histogram, ExponentialHistogram, Summary.

mod extract;
mod persist;
mod pipeline;

pub use pipeline::MetricsPipeline;
