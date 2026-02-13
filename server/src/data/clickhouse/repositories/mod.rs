//! ClickHouse repository modules
//!
//! Provides data access for OTEL storage:
//! - **messages**: Message query operations
//! - **metric**: Batch insert operations for metrics
//! - **query**: List, detail, and aggregate queries (traces, spans, sessions, events, links)
//! - **span**: Batch insert operations for spans (with embedded events/links)

pub mod messages;
pub mod metric;
pub mod query;
pub mod span;
pub mod stats;
