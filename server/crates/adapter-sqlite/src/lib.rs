//! SQLite transactional-store adapter.
//!
//! Provides centralized database management for local/embedded deployments.
//! Optimized for single-user, low-latency local use with:
//! - WAL mode for concurrent reads during writes
//! - In-memory temp storage for fast queries
//! - Automatic WAL checkpointing
//!
//! For scalable multi-tenant SaaS deployments, use PostgreSQL instead.
//! All schema definitions and migrations are managed here.

// A map's iteration order differs from one process to the next: where it reaches stored or answered bytes, a
// hash or the order of a write, iterate in order; elsewhere say why order cannot matter.
#![deny(clippy::iter_over_hash_type)]

mod error;
mod migrations;
mod repositories;
mod repository_impl;
pub use error::SqliteError;
pub use repository_impl::SqliteRepository;
pub mod schema;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use sqlx::SqlitePool;

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestClock;

#[cfg(test)]
impl sideseat_ports::clock::Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

use std::sync::Arc;

use sideseat_ports::cache::CacheStore;
use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::log::LevelFilter;

use sideseat_core::constants::{
    SQLITE_BUSY_TIMEOUT_SECS, SQLITE_CACHE_SIZE, SQLITE_CHECKPOINT_INTERVAL_SECS,
    SQLITE_DB_FILENAME, SQLITE_MAX_CONNECTIONS, SQLITE_WAL_AUTOCHECKPOINT,
};
use sideseat_core::storage::{AppStorage, DataSubdir};
use sideseat_ports::clock::Clock;

/// SQLite database service
///
/// Handles database initialization, connection pooling, and background tasks.
/// Should be created once at server startup and shared across all modules.
pub struct SqliteService {
    pool: SqlitePool,
    /// The cache, held by the service rather than passed to every port method.
    ///
    /// 29 of `TransactionalRepository`'s 95 methods took `cache: Option<&dyn CacheStore>`, which put an
    /// adapter type in the port's own signature - so the trait could not move to a crate that does not
    /// know about caching, and every caller had to decide per call whether to use it. Holding it here
    /// removes the parameter from the port; hoisting the *duplicated* caching bodies out of the two
    /// backends into one decorator is a later step, once the god-trait is split into the ports that
    /// actually need it.
    cache: Option<Arc<dyn CacheStore>>,
    clock: Arc<dyn Clock>,
}

impl SqliteService {
    /// Initialize the database service
    ///
    /// Creates the database file if it doesn't exist, configures connection
    /// options with optimized pragmas, and runs any pending migrations.
    pub async fn init(storage: &AppStorage, clock: Arc<dyn Clock>) -> Result<Self, SqliteError> {
        let db_path = storage.subdir(DataSubdir::Sqlite).join(SQLITE_DB_FILENAME);

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            // FULL, not NORMAL: in WAL mode NORMAL does not sync the log on commit, so a committed
            // transaction survives a process crash but not an OS crash or power loss. Rows written here
            // stand behind acknowledgements - a staged payload's registry row is all a durable queue's
            // consumer can find it by - so a commit has to be durable when it returns.
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(SQLITE_BUSY_TIMEOUT_SECS))
            .pragma("cache_size", SQLITE_CACHE_SIZE)
            .pragma("temp_store", "MEMORY")
            .pragma("wal_autocheckpoint", SQLITE_WAL_AUTOCHECKPOINT)
            .log_statements(LevelFilter::Trace);
        // On Apple platforms `fsync` does not flush the drive's write cache, so a synced commit can still be
        // lost to a power failure; `F_FULLFSYNC` is the call that makes it durable. SQLite issues it only when
        // asked, for commits and for checkpoints separately.
        #[cfg(target_vendor = "apple")]
        let options = options
            .pragma("fullfsync", "ON")
            .pragma("checkpoint_fullfsync", "ON");

        let pool = SqlitePoolOptions::new()
            .max_connections(SQLITE_MAX_CONNECTIONS)
            .connect_with(options)
            .await?;

        migrations::ensure_schema(&pool, clock.as_ref()).await?;

        tracing::debug!(path = %db_path.display(), "SqliteService initialized");
        Ok(Self {
            pool,
            cache: None,
            clock,
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The cache this service was built with, if any.
    pub fn cache(&self) -> Option<&dyn CacheStore> {
        self.cache.as_deref()
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Attach a cache. Called once by the composition root, before the service is shared.
    pub fn with_cache(mut self, cache: Option<Arc<dyn CacheStore>>) -> Self {
        self.cache = cache;
        self
    }

    /// Create a `SqliteService` from an existing pool.
    ///
    /// Primarily useful to integration tests and migration tooling that own
    /// the pool lifecycle.
    pub fn from_pool(pool: SqlitePool, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            cache: None,
            clock,
        }
    }

    pub async fn checkpoint(&self) -> Result<(), SqliteError> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await?;
        tracing::debug!("WAL checkpoint completed");
        Ok(())
    }

    /// Close the connection pool gracefully
    pub async fn close(&self) {
        self.pool.close().await;
        tracing::debug!("SQLite pool closed");
    }

    pub fn start_checkpoint_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let db = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(SQLITE_CHECKPOINT_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            tracing::debug!("WAL checkpoint task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = db.checkpoint().await {
                            tracing::warn!("WAL checkpoint failed: {}", e);
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod durability_tests {
    use super::*;
    use sideseat_core::storage::AppStorage;

    /// Every pooled connection commits durably: WAL with `synchronous = FULL` (2).
    ///
    /// Asserted on several connections because the setting is per connection, and the pool hands out
    /// whichever is free. Under `NORMAL` (1) a commit returns before the log is synced, so a power loss can
    /// roll back a staged payload's registry row after its export was acknowledged - and a durable queue's
    /// consumer, finding no row, acknowledges the message and drops the export.
    #[tokio::test]
    async fn every_connection_commits_durably() {
        let root = tempfile::TempDir::new().expect("temp dir");
        let storage = AppStorage::init_for_test(root.path().to_path_buf());
        let service = SqliteService::init(&storage, std::sync::Arc::new(TestClock))
            .await
            .expect("sqlite");
        let mut held = Vec::new();
        for _ in 0..SQLITE_MAX_CONNECTIONS.min(4) {
            let mut connection = service.pool().acquire().await.expect("connection");
            let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut *connection)
                .await
                .expect("pragma");
            let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut *connection)
                .await
                .expect("pragma");
            assert_eq!(
                synchronous, 2,
                "synchronous must be FULL on every connection"
            );
            assert_eq!(journal, "wal");
            held.push(connection);
        }
    }
}
