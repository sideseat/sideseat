//! OTLP logs ingestion and stable identity.

mod extract;
mod identity;
mod ingest;
mod messages;

pub use extract::extract_logs_batch;
pub use identity::log_digest;
pub use ingest::{Stored, ingest, ingest_governed};
pub use messages::log_event_payload;
