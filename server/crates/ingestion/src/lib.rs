//! OTLP decoding, normalization, durability, and signal ingestion.

// A map's iteration order differs from one process to the next: where it reaches stored or answered bytes, a
// hash or the order of a write, iterate in order; elsewhere say why order cannot matter.
#![deny(clippy::iter_over_hash_type)]

mod accounting;
pub mod logs;
mod message_events;
pub mod metrics;
pub mod otlp;
mod raw_coverage;
mod raw_identities;
pub mod received;
pub mod signals;
pub mod staging;
pub mod traces;
