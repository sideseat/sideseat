//! Trace processing pipeline
//!
//! This module contains the trace processing pipeline:
//!
//! - `extract` - Stage 1: Parse protobuf, extract GenAI attributes and messages
//! - `enrich` - Stage 3: Cost calculation and preview extraction
//! - `persist` - Stage 4: Build raw span JSON, SSE publishing, DuckDB writes
//! - `pipeline` - Pipeline orchestrator
//!
//! Note: Stage 2 (SideML) is in the `domain::sideml` module.

mod enrich;
mod extract;
mod persist;
mod pipeline;

// Public API - only types needed by external modules
pub use extract::{MessageSource, RawMessage};
pub use persist::SseSpanEvent;
pub use pipeline::TracePipeline;

// Internal re-exports for use within domain crate
pub(crate) use extract::SpanData;
pub(crate) use extract::files;
