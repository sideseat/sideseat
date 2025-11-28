//! Global SQLite database manager
//!
//! Provides centralized database management for the entire server.
//! All schema definitions and migrations are managed here.

pub mod error;
mod migrations;
pub mod schema;

pub use error::SqliteError;
pub use sqlx::SqlitePool;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Global database manager
///
/// Handles database initialization, connection pooling, and background tasks.
/// Should be created once at server startup and shared across all modules.
pub struct DatabaseManager {
    pool: SqlitePool,
}

impl DatabaseManager {
    /// Initialize the database manager
    ///
    /// Creates the database file if it doesn't exist, configures connection
    /// options with optimized pragmas, and runs any pending migrations.
    pub async fn init(data_dir: &Path) -> Result<Self, SqliteError> {
        // Ensure data directory exists
        tokio::fs::create_dir_all(data_dir).await?;

        let db_path = data_dir.join("sideseat.db");
        let url = format!("sqlite:{}?mode=rwc", db_path.display());

        // Configure connection options with per-connection pragmas
        let options = SqliteConnectOptions::from_str(&url)?
            .foreign_keys(true) // PRAGMA foreign_keys = ON (per-connection)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(30)) // Prevent SQLITE_BUSY
            .pragma("cache_size", "-64000") // 64MB cache
            .pragma("temp_store", "MEMORY")
            .pragma("wal_autocheckpoint", "1000"); // Auto-checkpoint at ~4MB WAL

        let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;

        // Run migrations
        migrations::run_migrations(&pool).await?;

        tracing::debug!("DatabaseManager initialized: {:?}", db_path);
        Ok(Self { pool })
    }

    /// Get the connection pool
    ///
    /// Modules should clone this pool for their own use.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Run WAL checkpoint to merge WAL into main database
    ///
    /// Called periodically and on shutdown for clean WAL rotation.
    pub async fn checkpoint(&self) -> Result<(), SqliteError> {
        // TRUNCATE mode: checkpoint and truncate WAL to zero bytes
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&self.pool).await?;
        tracing::debug!("WAL checkpoint completed");
        Ok(())
    }

    /// Start periodic WAL checkpoint task (every 5 minutes)
    ///
    /// This task ensures the WAL file doesn't grow too large and
    /// data is periodically merged into the main database file.
    pub fn start_checkpoint_task(self: &Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        let db = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = db.checkpoint().await {
                            tracing::warn!("Periodic WAL checkpoint failed: {}", e);
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("WAL checkpoint task shutting down");
                            break;
                        }
                    }
                }
            }
        });
    }
}
