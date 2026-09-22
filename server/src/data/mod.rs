//! Test-only compatibility namespace for legacy parity and golden harnesses.

pub mod cache;
pub mod clickhouse;
pub mod duckdb;
pub mod postgres;
pub mod registrations;
pub mod secrets;
pub mod sql;
pub mod sqlite;

pub use crate::app::storage::{AnalyticsService, TransactionalService};
