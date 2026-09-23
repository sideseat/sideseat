//! The seams SideSeat's domain talks through: the traits, the DTOs, the filter vocabulary and the error type.
//!
//! No implementations, and nothing here may name one. See `Cargo.toml` for what that rules out and why the
//! manifest is where it is enforced.

pub mod blobs;
pub mod cache;
pub mod cache_key;
pub mod clock;
pub mod error;
pub mod filters;
pub mod queue;
pub mod registrations;
pub mod secrets;
pub mod traits;
pub mod types;
