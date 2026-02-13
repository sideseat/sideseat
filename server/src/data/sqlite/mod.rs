//! SQLite database service
//!
//! Provides centralized database management for local/embedded deployments.
//! Optimized for single-user, low-latency local use with:
//! - WAL mode for concurrent reads during writes
//! - In-memory temp storage for fast queries
//! - Automatic WAL checkpointing
//!
//! For scalable multi-tenant SaaS deployments, use PostgreSQL instead.
//! All schema definitions and migrations are managed here.

pub mod error;
mod migrations;
pub mod repositories;
mod repository_impl;
pub mod schema;

pub use error::SqliteError;
pub use sqlx::SqlitePool;

use std::sync::Arc;
use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::log::LevelFilter;

use crate::core::constants::{
    SQLITE_BUSY_TIMEOUT_SECS, SQLITE_CACHE_SIZE, SQLITE_CHECKPOINT_INTERVAL_SECS,
    SQLITE_DB_FILENAME, SQLITE_MAX_CONNECTIONS, SQLITE_WAL_AUTOCHECKPOINT,
};
use crate::core::storage::{AppStorage, DataSubdir};

/// SQLite database service
///
/// Handles database initialization, connection pooling, and background tasks.
/// Should be created once at server startup and shared across all modules.
pub struct SqliteService {
    pool: SqlitePool,
}

impl SqliteService {
    /// Initialize the database service
    ///
    /// Creates the database file if it doesn't exist, configures connection
    /// options with optimized pragmas, and runs any pending migrations.
    pub async fn init(storage: &AppStorage) -> Result<Self, SqliteError> {
        let db_path = storage.subdir(DataSubdir::Sqlite).join(SQLITE_DB_FILENAME);

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(SQLITE_BUSY_TIMEOUT_SECS))
            .pragma("cache_size", SQLITE_CACHE_SIZE)
            .pragma("temp_store", "MEMORY")
            .pragma("wal_autocheckpoint", SQLITE_WAL_AUTOCHECKPOINT)
            .log_statements(LevelFilter::Trace);

        let pool = SqlitePoolOptions::new()
            .max_connections(SQLITE_MAX_CONNECTIONS)
            .connect_with(options)
            .await?;

        migrations::run_migrations(&pool).await?;

        tracing::debug!(path = %db_path.display(), "SqliteService initialized");
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Create a SqliteService from an existing pool (primarily for testing)
    #[cfg(test)]
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
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
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
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
