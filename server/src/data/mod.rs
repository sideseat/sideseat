//! Data storage layer
//!
//! Provides database services for the application:
//! - `duckdb` - Analytics database for OTEL data (high-throughput writes)
//! - `sqlite` - Transactional database for metadata
//! - `files` - Binary file storage with deduplication
//! - `cleanup` - Cross-database cleanup logic
//! - `cache` - In-memory and Redis caching with rate limiting
//! - `types` - Shared data types across all backends
//! - `traits` - Repository traits for multi-database support
//! - `sql` - SQL abstraction layer for multi-database support
//! - `error` - Unified error type for all backends
//!
//! ## Backend Support
//!
//! The data layer supports multiple database backends through traits:
//! - `AnalyticsRepository` - Implemented by DuckDB and ClickHouse
//! - `TransactionalRepository` - Implemented by SQLite and PostgreSQL

pub mod cache;
pub mod cleanup;
pub mod clickhouse;
pub mod dedup;
pub mod duckdb;
pub mod error;
pub mod files;
pub mod filters;
pub mod postgres;
pub mod registrations;
pub mod secrets;
pub mod sql;
pub mod sqlite;
pub mod topics;
pub mod traits;
pub mod types;

// Re-export backend-specific services
pub use clickhouse::ClickhouseService;
pub use duckdb::DuckdbService;
pub use postgres::PostgresService;
pub use sqlite::SqliteService;

// Re-export unified error type
pub use error::DataError;

// Re-export repository traits
pub use traits::{
    AnalyticsRepository, FilterOptionRow, TransactionalRepository, has_min_role_level,
};

// Re-export shared types for convenient access
pub use types::{
    AggregationTemporality, MessageCategory, MessageSourceType, MetricType, NormalizedMetric,
    NormalizedSpan, ObservationType, SpanCategory,
};

// Re-export filters for API usage (analytics backend SQL building)

use std::sync::Arc;

use crate::data::cache::CacheService;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use sideseat_core::core::config::{
    AnalyticsBackend, ClickhouseConfig, PostgresConfig, RetentionConfig, TransactionalBackend,
};
use sideseat_core::core::storage::AppStorage;

/// Transactional database service enum
///
/// Wraps the underlying backend-specific service (SQLite or PostgreSQL).
/// Provides a unified interface for all transactional operations.
/// Services are stored as Arc to enable safe extraction.
pub enum TransactionalService {
    /// SQLite backend (default, embedded)
    Sqlite(Arc<SqliteService>),
    /// PostgreSQL backend (for distributed deployments)
    Postgres(Arc<PostgresService>),
}

impl TransactionalService {
    /// Initialize the transactional service based on configuration
    ///
    /// For SQLite backend, uses the storage path.
    /// For PostgreSQL backend, requires a PostgresConfig.
    /// `cache` is held by the service rather than passed to every port method.
    ///
    /// 29 of `TransactionalRepository`'s 95 methods took `cache: Option<&CacheService>`, so an adapter type
    /// sat in the port's own signature - which is what stops the trait moving into a crate that knows
    /// nothing about caching. Which methods actually cache is now stated once, in each backend's `impl`,
    /// instead of being decided independently at 36 call sites.
    pub async fn init(
        backend: TransactionalBackend,
        storage: &AppStorage,
        postgres_config: Option<&PostgresConfig>,
        cache: Option<Arc<CacheService>>,
    ) -> Result<Self, DataError> {
        match backend {
            TransactionalBackend::Sqlite => {
                let service = SqliteService::init(storage).await?.with_cache(cache);
                Ok(Self::Sqlite(Arc::new(service)))
            }
            TransactionalBackend::Postgres => {
                let config = postgres_config.ok_or_else(|| {
                    DataError::Config("PostgreSQL configuration required".to_string())
                })?;
                let service = PostgresService::init(config).await?.with_cache(cache);
                Ok(Self::Postgres(Arc::new(service)))
            }
        }
    }

    /// Run a WAL checkpoint (SQLite) or equivalent maintenance task
    pub async fn checkpoint(&self) -> Result<(), DataError> {
        match self {
            Self::Sqlite(s) => s.checkpoint().await.map_err(Into::into),
            Self::Postgres(_) => {
                // PostgreSQL manages its own maintenance via autovacuum
                // No explicit checkpoint needed
                Ok(())
            }
        }
    }

    /// Close the database connection gracefully
    pub async fn close(&self) {
        match self {
            Self::Sqlite(s) => s.close().await,
            Self::Postgres(p) => p.close().await,
        }
    }

    /// Start the background checkpoint task (SQLite only)
    /// For PostgreSQL, starts a health check task instead.
    pub fn start_checkpoint_task(&self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        match self {
            Self::Sqlite(s) => Arc::clone(s).start_checkpoint_task(shutdown_rx),
            Self::Postgres(p) => Arc::clone(p).start_health_check_task(shutdown_rx),
        }
    }

    /// Get the backend type
    pub fn backend(&self) -> TransactionalBackend {
        match self {
            Self::Sqlite(_) => TransactionalBackend::Sqlite,
            Self::Postgres(_) => TransactionalBackend::Postgres,
        }
    }

    /// Get the repository trait object for data operations
    ///
    /// This returns a boxed trait object, allowing backend-agnostic
    /// data operations through the TransactionalRepository interface.
    pub fn repository(&self) -> Box<dyn TransactionalRepository + Send + Sync> {
        match self {
            Self::Sqlite(s) => Box::new(Arc::clone(s)),
            Self::Postgres(p) => Box::new(Arc::clone(p)),
        }
    }
}

/// Analytics database service enum
///
/// Wraps the underlying backend-specific service (DuckDB or ClickHouse).
/// Provides a unified interface for all analytics operations.
/// Services are stored as Arc to enable safe extraction.
pub enum AnalyticsService {
    /// DuckDB backend (default, embedded)
    Duckdb(Arc<DuckdbService>),
    /// ClickHouse backend (for distributed deployments)
    Clickhouse(Arc<ClickhouseService>),
}

impl AnalyticsService {
    /// Initialize the analytics service based on configuration
    ///
    /// For DuckDB backend, uses the storage path.
    /// For ClickHouse backend, requires a ClickhouseConfig.
    pub async fn init(
        backend: AnalyticsBackend,
        storage: &AppStorage,
        clickhouse_config: Option<&ClickhouseConfig>,
    ) -> Result<Self, DataError> {
        match backend {
            AnalyticsBackend::Duckdb => {
                let service = DuckdbService::init(storage).await?;
                Ok(Self::Duckdb(Arc::new(service)))
            }
            AnalyticsBackend::Clickhouse => {
                let config = clickhouse_config.ok_or_else(|| {
                    DataError::Config("ClickHouse configuration required".to_string())
                })?;
                let service = ClickhouseService::init(config).await?;
                Ok(Self::Clickhouse(Arc::new(service)))
            }
        }
    }

    /// Run a checkpoint operation
    pub async fn checkpoint(&self) -> Result<(), DataError> {
        match self {
            Self::Duckdb(d) => Arc::clone(d).checkpoint().await.map_err(Into::into),
            Self::Clickhouse(_) => {
                // ClickHouse doesn't need explicit checkpoints
                Ok(())
            }
        }
    }

    /// Close the database connection gracefully
    pub async fn close(&self) -> Result<(), DataError> {
        match self {
            Self::Duckdb(d) => Arc::clone(d).close().await.map_err(Into::into),
            Self::Clickhouse(c) => {
                c.close().await;
                Ok(())
            }
        }
    }

    /// Start the background checkpoint task
    pub fn start_checkpoint_task(&self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        match self {
            Self::Duckdb(d) => Arc::clone(d).start_checkpoint_task(shutdown_rx),
            Self::Clickhouse(c) => Arc::clone(c).start_health_check_task(shutdown_rx),
        }
    }

    /// Start the cross-partition consistency check, where the backend can have that defect.
    ///
    /// `None` on DuckDB, and structurally rather than as a gap: the residual being detected is two revisions of
    /// one identity in different **partitions**, and DuckDB has no partitions - its reads go through
    /// `DEDUP_SPANS`, a window function over the whole table, which picks one winner per identity whatever the
    /// physical layout. So there is nothing there for such a check to find, and returning `None` says that
    /// rather than scheduling a task that would always report clean.
    pub fn start_consistency_check_task(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        match self {
            Self::Duckdb(_) => None,
            Self::Clickhouse(c) => Some(Arc::clone(c).start_consistency_check_task(shutdown_rx)),
        }
    }

    /// Start the retention cleanup task
    pub fn start_retention_task(
        &self,
        config: RetentionConfig,
        shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<crate::data::files::FileService>>,
        database: Arc<TransactionalService>,
    ) -> Option<JoinHandle<()>> {
        match self {
            Self::Duckdb(d) => {
                Arc::clone(d).start_retention_task(config, shutdown_rx, file_service, database)
            }
            Self::Clickhouse(c) => {
                Arc::clone(c).start_retention_task(config, shutdown_rx, file_service, database)
            }
        }
    }

    /// Get the backend type
    pub fn backend(&self) -> AnalyticsBackend {
        match self {
            Self::Duckdb(_) => AnalyticsBackend::Duckdb,
            Self::Clickhouse(_) => AnalyticsBackend::Clickhouse,
        }
    }

    /// Get the repository trait object for data operations
    ///
    /// Returns a DedupAnalyticsRepository wrapper that deduplicates SpanRow
    /// and MessageSpanRow results in Rust, while aggregation queries use
    /// SQL-level dedup directly.
    pub fn repository(&self) -> Box<dyn AnalyticsRepository + Send + Sync> {
        let inner: Box<dyn AnalyticsRepository + Send + Sync> = match self {
            Self::Duckdb(d) => Box::new(Arc::clone(d)),
            Self::Clickhouse(c) => Box::new(Arc::clone(c)),
        };
        Box::new(dedup::DedupAnalyticsRepository::new(inner))
    }
}
