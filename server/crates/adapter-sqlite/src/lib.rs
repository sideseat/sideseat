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
mod lease_connection;
mod migrations;
mod repositories;
mod repository_impl;
pub use error::SqliteError;
pub use lease_connection::{LeaseConnection, LeaseWork};
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
    /// The connection process-lease statements run on, of their own, so a renewal never queues behind the pool's
    /// other work (`lease_connection`); the pool itself when built from one.
    lease: LeaseConnection,
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

        let lease_bound =
            Duration::from_secs(sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS);
        let lease_options = options.clone().busy_timeout(lease_bound);
        let pool = SqlitePoolOptions::new()
            .max_connections(SQLITE_MAX_CONNECTIONS)
            .connect_with(options)
            .await?;

        migrations::ensure_schema(&pool, clock.as_ref()).await?;
        let lease = LeaseConnection::owned(lease_options, lease_bound);

        tracing::debug!(path = %db_path.display(), "SqliteService initialized");
        Ok(Self {
            pool,
            lease,
            cache: None,
            clock,
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The process-lease connection: one, owned, its open and busy timeouts the lease statement bound.
    pub fn lease_connection(&self) -> &LeaseConnection {
        &self.lease
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
            lease: LeaseConnection::shared(
                pool.clone(),
                Duration::from_secs(sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS),
            ),
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
        self.lease.close().await;
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

#[cfg(test)]
mod lease_pool_tests {
    use super::*;
    use sideseat_core::storage::AppStorage;
    use sideseat_ports::traits::{ProcessLeases, ProcessRole};

    /// A lease renewal never waits on the pool every other statement shares: with each of its connections held,
    /// the lease's own connection still renews, durably, and within the lease statement bound.
    #[tokio::test]
    async fn a_lease_renews_while_every_shared_connection_is_held() {
        let root = tempfile::TempDir::new().expect("temp dir");
        let storage = AppStorage::init_for_test(root.path().to_path_buf());
        let service = std::sync::Arc::new(
            SqliteService::init(&storage, std::sync::Arc::new(TestClock))
                .await
                .expect("sqlite"),
        );
        let mut held = Vec::new();
        for _ in 0..SQLITE_MAX_CONNECTIONS {
            held.push(service.pool().acquire().await.expect("connection"));
        }
        let repo = SqliteRepository(std::sync::Arc::clone(&service));
        let renewed = tokio::time::timeout(
            Duration::from_secs(sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS),
            repo.renew_process_lease("server-a", ProcessRole::Server, 30),
        )
        .await;
        assert!(
            matches!(renewed, Ok(Ok(()))),
            "renewed through its own connection: {renewed:?}"
        );
        let synchronous: i64 = service
            .lease_connection()
            .run(|connection| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar("PRAGMA synchronous")
                        .fetch_one(&mut *connection)
                        .await?)
                })
            })
            .await
            .expect("pragma");
        assert_eq!(synchronous, 2, "the lease connection commits durably too");
        drop(held);
    }

    /// An attempt abandoned while its statement runs takes its connection with it, and the next attempt opens a
    /// new one: the abandoned connection's temporary table is not there. And once closed, the lease connection
    /// refuses work rather than reopening.
    #[tokio::test]
    async fn an_abandoned_lease_statement_takes_its_connection_and_close_is_final() {
        let root = tempfile::TempDir::new().expect("temp dir");
        let storage = AppStorage::init_for_test(root.path().to_path_buf());
        let service = std::sync::Arc::new(
            SqliteService::init(&storage, std::sync::Arc::new(TestClock))
                .await
                .expect("sqlite"),
        );
        let lease = service.lease_connection();
        lease
            .run(|connection| {
                Box::pin(async move {
                    sqlx::query("CREATE TEMP TABLE marker (x INTEGER)")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .await
            .expect("marked");
        // A statement that runs for seconds, abandoned while it runs.
        let abandoned = tokio::time::timeout(
            Duration::from_millis(100),
            lease.run(|connection| {
                Box::pin(async move {
                    sqlx::query(
                        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200000000) \
                         SELECT count(*) FROM n",
                    )
                    .execute(&mut *connection)
                    .await?;
                    Ok(())
                })
            }),
        )
        .await;
        assert!(
            abandoned.is_err(),
            "the attempt was abandoned while its statement ran"
        );
        let marked: i64 = lease
            .run(|connection| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar(
                        "SELECT count(*) FROM temp.sqlite_master WHERE name = 'marker'",
                    )
                    .fetch_one(&mut *connection)
                    .await?)
                })
            })
            .await
            .expect("the next attempt runs");
        assert_eq!(
            marked, 0,
            "the next attempt runs on a new connection, not the abandoned one"
        );
        let repo = SqliteRepository(std::sync::Arc::clone(&service));
        repo.renew_process_lease("server-a", ProcessRole::Server, 30)
            .await
            .expect("renewed");
        service.close().await;
        assert!(
            repo.renew_process_lease("server-a", ProcessRole::Server, 30)
                .await
                .is_err(),
            "closed, the lease connection refuses rather than reopening"
        );
    }

    /// Closed while its slot is held past the close's own wait, the lease connection still refuses every later
    /// call: closure is marked before the wait, not after it.
    #[tokio::test]
    async fn close_that_cannot_take_the_slot_is_still_final() {
        let root = tempfile::TempDir::new().expect("temp dir");
        let db = root.path().join("lease.db");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let lease = LeaseConnection::owned(options, Duration::from_millis(100));
        lease
            .run(|connection| {
                Box::pin(async move {
                    sqlx::query("SELECT 1").execute(&mut *connection).await?;
                    Ok(())
                })
            })
            .await
            .expect("an open connection");
        // The slot held by someone else past the close's 100 ms wait.
        let held = lease.slot_for_test().lock().await;
        tokio::time::timeout(Duration::from_secs(1), lease.close())
            .await
            .expect("close gives up its wait");
        drop(held);
        let after = lease
            .run(|connection| {
                Box::pin(async move {
                    sqlx::query("SELECT 1").execute(&mut *connection).await?;
                    Ok(())
                })
            })
            .await;
        assert!(
            matches!(after, Err(SqliteError::Database(sqlx::Error::PoolClosed))),
            "refused after a close that never took the slot: {after:?}"
        );
    }
}
