//! Shared SQL dialect and rendering vocabulary.
//!
//! This module provides abstractions for generating SQL that works across
//! different database backends (DuckDB, PostgreSQL, SQLite, ClickHouse).

/// Typed analytical statements and their migration registry.
pub mod analytics;
mod clickhouse_dialect;
/// Strict content confirmation reads for staged signal deliveries.
pub mod confirmations;
mod dialect;
/// The SQL for displayed values, per dialect - moved here from the DTOs, which must not emit SQL.
pub mod display;
/// Typed tenant-scoped mutations for analytical stores.
pub mod dml;
mod duckdb_dialect;
/// Typed OTLP log reads.
pub mod logs;
/// Typed message-context and message-feed statements.
pub mod messages;
/// Typed metric reads and aggregates.
pub mod metrics;
/// Rendering an order clause, moved out of the DTOs for the same reason as the display SQL.
pub mod order;
mod postgres_dialect;
mod sqlite_dialect;
/// Typed project-statistics statements and bucket planning.
pub mod stats;

pub use clickhouse_dialect::ClickhouseDialect;
pub use dialect::SqlDialect;
pub use duckdb_dialect::DuckdbDialect;
pub use postgres_dialect::PostgresDialect;
pub use sqlite_dialect::SqliteDialect;

/// Database backend identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
    Duckdb,
    Clickhouse,
}

impl Backend {
    /// Get the SQL dialect for this backend
    pub fn dialect(&self) -> &'static dyn SqlDialect {
        match self {
            Backend::Sqlite => &SqliteDialect,
            Backend::Postgres => &PostgresDialect,
            Backend::Duckdb => &DuckdbDialect,
            Backend::Clickhouse => &ClickhouseDialect,
        }
    }

    /// Get the backend name
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
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
