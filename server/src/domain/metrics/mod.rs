//! OTLP metrics ingestion
//!
//! Processes OTLP metrics: extraction, flattening, and persistence - inside the request, so a 200 means
//! the rows are committed. See [`ingest`] for why there is no queue here when traces have one.
//! Supports all 5 OTLP metric types: Gauge, Sum, Histogram, ExponentialHistogram, Summary.

mod extract;
mod ingest;
mod persist;

pub use ingest::ingest;
