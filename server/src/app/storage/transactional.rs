use std::sync::Arc;

use sideseat_adapter_postgres::{PostgresRepository, PostgresService};
use sideseat_adapter_sqlite::{SqliteRepository, SqliteService};
use sideseat_core::config::{PostgresConfig, TransactionalBackend};
use sideseat_core::storage::AppStorage;
use sideseat_ports::cache::CacheStore;
use sideseat_ports::clock::Clock;
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{StorageGovernance, TransactionalRepository};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Lifecycle facade over the configured transactional backend.
pub enum TransactionalService {
    /// SQLite backend (default, embedded).
    Sqlite(Arc<SqliteService>),
    /// PostgreSQL backend (distributed deployments).
    Postgres(Arc<PostgresService>),
}

impl TransactionalService {
    /// Initialize the selected backend and attach its shared cache.
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

    /// Run a WAL checkpoint when the backend needs one.
    pub async fn checkpoint(&self) -> Result<(), DataError> {
        match self {
            Self::Sqlite(service) => service.checkpoint().await.map_err(Into::into),
            // PostgreSQL owns maintenance through autovacuum.
            Self::Postgres(_) => Ok(()),
        }
    }

    /// Close the database connection gracefully.
    pub async fn close(&self) {
        match self {
            Self::Sqlite(service) => service.close().await,
            Self::Postgres(service) => service.close().await,
        }
    }

    /// Start checkpoint maintenance or the distributed backend's health check.
    pub fn start_checkpoint_task(&self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        match self {
            Self::Sqlite(service) => Arc::clone(service).start_checkpoint_task(shutdown_rx),
            Self::Postgres(service) => Arc::clone(service).start_health_check_task(shutdown_rx),
        }
    }

    /// Expose backend-agnostic transactional operations.
    pub fn repository(&self) -> Box<dyn TransactionalRepository + Send + Sync> {
        match self {
            Self::Sqlite(service) => Box::new(SqliteRepository(Arc::clone(service))),
            Self::Postgres(service) => Box::new(PostgresRepository(Arc::clone(service))),
        }
    }

    /// Expose backend-agnostic storage governance operations.
    pub fn governance_repository(&self) -> Box<dyn StorageGovernance + Send + Sync> {
        match self {
            Self::Sqlite(service) => Box::new(SqliteRepository(Arc::clone(service))),
            Self::Postgres(service) => Box::new(PostgresRepository(Arc::clone(service))),
        }
    }
}
