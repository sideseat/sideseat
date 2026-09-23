//! Core application infrastructure: configuration, constants, storage layout and the CLI.
//!
//! **No re-exports of other layers.** This module used to `pub use` the analytics and transactional service
//! enums, the pricing service and the whole topic vocabulary "for backward compatibility" - which made
//! `core` name every layer above it, so a crate boundary here could not exist and `cargo tree` said nothing
//! true about the dependency direction. Callers import from where a thing is defined; project convention
//! forbids compatibility re-exports for exactly this reason.
//!
//! `shutdown` moved to `runtime` for the same reason: it drains the topic service, so it is composition
//! rather than configuration.

// `pub`, not `pub(crate)`: the consumer is the composition root, which now lives in another crate.
pub mod banner;
pub mod cli;
pub mod config;
pub mod constants;
pub mod storage;
pub mod update;
