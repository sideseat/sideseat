//! # SideSeat
//!
//! OpenTelemetry for AI agents, configured for [SideSeat](https://sideseat.ai) in one call.
//!
//! [`init`] installs tracer, logger, and meter providers that export over OTLP/HTTP to a SideSeat
//! project. Inside a [`Session`], every span the process starts - including spans a library
//! creates through the global tracer - carries `session.id` and `user.id`.
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use sideseat::{Options, Session, SpanOptions};
//!
//! # async fn run_agent() -> Result<String, std::io::Error> { Ok(String::new()) }
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let telemetry = sideseat::init(Options::new().service_name("travel-agent"))?;
//!
//!     let conversation = Session::new("conversation-42").user("user-7");
//!     let answer = conversation
//!         .scope(telemetry.trace("plan-trip", SpanOptions::new(), run_agent))
//!         .await?;
//!     println!("{answer}");
//!
//!     telemetry.shutdown(Duration::from_secs(5));
//!     Ok(())
//! }
//! ```
//!
//! The crate implements the
//! [SideSeat SDK contract](https://github.com/sideseat/sideseat/blob/main/docs/engineering/sdk-contract.md).
//! Rust has no framework integrations: spans from any library that uses the global tracer reach
//! SideSeat, and prompts and responses are recorded by following the OpenTelemetry GenAI semantic
//! conventions.

mod client;
mod config;
mod correlation;
mod error;

pub use client::{SideSeat, SpanOptions, client, init};
pub use config::{DEFAULT_ENDPOINT, DEFAULT_PROJECT, Options, Settings};
pub use correlation::Session;
pub use error::Error;
pub use opentelemetry::KeyValue;
pub use opentelemetry::trace::SpanKind;

/// This crate's version, recorded as `telemetry.sdk.version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
