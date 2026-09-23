//! OTLP logs ingestion and stable identity.

mod extract;
mod identity;
mod ingest;

pub use extract::extract_logs_batch;
pub use identity::log_digest;
pub use ingest::{Stored, ingest, ingest_governed};
