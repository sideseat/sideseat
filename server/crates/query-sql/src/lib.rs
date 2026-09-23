//! Shared analytical SQL rendering vocabulary.
//!
//! Queries in this crate target the analytical backends: DuckDB and ClickHouse.

/// Typed analytical statements and their migration registry.
pub mod analytics;
/// Strict content confirmation reads for staged signal deliveries.
pub mod confirmations;
/// Dialect-specific SQL for displayed values.
pub mod display;
/// Typed tenant-scoped mutations for analytical stores.
pub mod dml;
/// Typed OTLP log reads.
pub mod logs;
/// Typed message-context and message-feed statements.
pub mod messages;
/// Typed metric reads and aggregates.
pub mod metrics;
/// Order-clause rendering.
pub mod order;
/// Typed three-valued chronological search queries.
pub mod search;
/// Typed project-statistics statements and bucket planning.
pub mod stats;

/// Analytical database backend identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Duckdb,
    Clickhouse,
}

impl Backend {
    /// Get the backend name.
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Duckdb => "duckdb",
            Backend::Clickhouse => "clickhouse",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
