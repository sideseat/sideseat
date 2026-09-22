//! Domain logic for LLM observability
//!
//! - `metrics` - OpenTelemetry metrics processing pipeline
//! - `pricing` - LLM cost calculation and model pricing
//! - `rules` - framework knowledge as data, interpreted generically
//! - `sideml` - Universal AI message format normalization
//! - `traces` - OpenTelemetry trace processing pipeline

pub mod traces;

pub use sideseat_domain::metrics::Stored as MetricsStored;
pub use sideseat_domain::metrics::ingest as ingest_metrics;
pub use sideseat_domain::{metrics, pricing, rules, sideml};
pub use traces::{MessageSource, RawMessage, SseSpanEvent, TracePipeline};
