//! SQL abstraction layer for multi-database support
//!
//! This module provides abstractions for generating SQL that works across
//! different database backends (DuckDB, PostgreSQL, SQLite, ClickHouse).

mod clickhouse_dialect;
mod dialect;
/// The SQL for displayed values, per dialect - moved here from the DTOs, which must not emit SQL.
pub mod display;
mod duckdb_dialect;
/// Rendering an order clause, moved out of the DTOs for the same reason as the display SQL.
pub mod order;
mod postgres_dialect;
mod sqlite_dialect;

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
