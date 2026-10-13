//! OTLP logs ingestion and stable identity.

mod content;
mod extract;
mod identity;
mod ingest;
mod media;
mod messages;
#[cfg(test)]
mod views_tests;

pub use content::log_record_content;
pub use extract::extract_logs_batch;
pub use identity::log_digest;
pub use ingest::{Stored, ingest, ingest_governed};
