//! Backend selection and lifecycle owned by the composition root.

use std::sync::Arc;

use sideseat_adapter_clickhouse::{ClickhouseRepository, ClickhouseService};
use sideseat_adapter_duckdb::{DuckdbRepository, DuckdbService};
use sideseat_adapter_postgres::{PostgresRepository, PostgresService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_domain::dedup::DedupAnalyticsRepository;
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{AnalyticsRepository, StorageGovernance, TransactionalRepository};

use sideseat_ports::blobs::RetentionFileReconciler;
use sideseat_ports::cache::CacheStore;
use sideseat_ports::clock::Clock;
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
        cache: Option<Arc<dyn CacheStore>>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, DataError> {
        match backend {
            TransactionalBackend::Sqlite => {
                let service = SqliteService::init(storage, clock).await?.with_cache(cache);
                Ok(Self::Sqlite(Arc::new(service)))
            }
            TransactionalBackend::Postgres => {
                let config = postgres_config.ok_or_else(|| {
                    DataError::Config("PostgreSQL configuration required".to_string())
                })?;
                let service = PostgresService::init(config, clock)
                    .await?
                    .with_cache(cache);
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
            Self::Sqlite(s) => Box::new(SqliteRepository(Arc::clone(s))),
            Self::Postgres(p) => Box::new(PostgresRepository(Arc::clone(p))),
        }
    }

    pub fn governance_repository(&self) -> Box<dyn StorageGovernance + Send + Sync> {
        match self {
            Self::Sqlite(s) => Box::new(SqliteRepository(Arc::clone(s))),
            Self::Postgres(p) => Box::new(PostgresRepository(Arc::clone(p))),
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
        clock: Arc<dyn Clock>,
    ) -> Result<Self, DataError> {
        match backend {
            AnalyticsBackend::Duckdb => {
                let service = DuckdbService::init(storage, clock).await?;
                Ok(Self::Duckdb(Arc::new(service)))
            }
            AnalyticsBackend::Clickhouse => {
                let config = clickhouse_config.ok_or_else(|| {
                    DataError::Config("ClickHouse configuration required".to_string())
                })?;
                let service = ClickhouseService::init(config, clock).await?;
                Ok(Self::Clickhouse(Arc::new(service)))
            }
        }
    }

    pub fn clock(&self) -> &dyn Clock {
        match self {
            Self::Duckdb(service) => service.clock(),
            Self::Clickhouse(service) => service.clock(),
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
        quota_bytes: u64,
        shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<sideseat_domain::files::FileService>>,
        database: Arc<TransactionalService>,
        governance: Arc<dyn StorageGovernance + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        match self {
            Self::Duckdb(d) => Arc::clone(d).start_retention_task(
                config,
                quota_bytes,
                shutdown_rx,
                file_service.map(|service| service as Arc<dyn RetentionFileReconciler>),
                Arc::from(database.repository()),
                Arc::clone(&governance),
            ),
            Self::Clickhouse(c) => Arc::clone(c).start_retention_task(
                config,
                quota_bytes,
                shutdown_rx,
                file_service.map(|service| service as Arc<dyn RetentionFileReconciler>),
                Arc::from(database.repository()),
                governance,
            ),
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
            Self::Duckdb(d) => Box::new(DuckdbRepository(Arc::clone(d))),
            Self::Clickhouse(c) => Box::new(ClickhouseRepository(Arc::clone(c))),
        };
        Box::new(DedupAnalyticsRepository::new(inner))
    }
}
