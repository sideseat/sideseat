//! PostgreSQL database service
//!
//! Provides centralized database management for PostgreSQL backend.
//! Optimized for scalable SaaS deployments with:
//! - Connection pooling with min/max bounds
//! - Idle connection cleanup
//! - Connection lifetime cycling
//! - Query timeout protection
//!
//! All schema definitions and migrations are managed here.

pub mod error;
mod migrations;
pub mod repositories;
mod repository_impl;
pub mod schema;

pub use error::PostgresError;
pub use sqlx::PgPool;

use std::sync::Arc;
use std::time::Duration;

use sqlx::ConnectOptions;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::log::LevelFilter;

use crate::core::config::PostgresConfig;
use crate::core::constants::{
    POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS, POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS,
    POSTGRES_DEFAULT_MAX_CONNECTIONS, POSTGRES_DEFAULT_MAX_LIFETIME_SECS,
    POSTGRES_DEFAULT_MIN_CONNECTIONS, POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS,
};

/// PostgreSQL database service
///
/// Handles database initialization, connection pooling, and background tasks.
/// Optimized for scalable SaaS with connection pooling and query protection.
/// Should be created once at server startup and shared across all modules.
pub struct PostgresService {
    pool: PgPool,
}

impl PostgresService {
    /// Initialize the database service from configuration
    ///
    /// Creates a connection pool with SaaS-optimized settings:
    /// - Min connections kept warm for low latency
    /// - Max connections sized for concurrent load
    /// - Idle timeout to release unused connections
    /// - Max lifetime to cycle connections and prevent stale state
    /// - Statement timeout to prevent runaway queries
    pub async fn init(config: &PostgresConfig) -> Result<Self, PostgresError> {
        let url = config.url.as_str();
        if url.is_empty() {
            return Err(PostgresError::Config("PostgreSQL URL is required".into()));
        }

        // Use config values with sensible defaults for SaaS workloads
        let max_connections = if config.max_connections > 0 {
            config.max_connections
        } else {
            POSTGRES_DEFAULT_MAX_CONNECTIONS
        };

        let min_connections = if config.min_connections > 0 {
            config.min_connections
        } else {
            POSTGRES_DEFAULT_MIN_CONNECTIONS
        };

        let acquire_timeout = if config.acquire_timeout_secs > 0 {
            config.acquire_timeout_secs
        } else {
            POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS
        };

        let idle_timeout = if config.idle_timeout_secs > 0 {
            config.idle_timeout_secs
        } else {
            POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS
        };

        let max_lifetime = if config.max_lifetime_secs > 0 {
            config.max_lifetime_secs
        } else {
            POSTGRES_DEFAULT_MAX_LIFETIME_SECS
        };

        let statement_timeout = if config.statement_timeout_secs > 0 {
            config.statement_timeout_secs
        } else {
            POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS
        };

        let mut options: PgConnectOptions = url
            .parse()
            .map_err(|e| PostgresError::Config(format!("Invalid PostgreSQL URL: {}", e)))?;

        options = options.log_statements(LevelFilter::Trace);

        // Set statement timeout at connection level for query protection
        if statement_timeout > 0 {
            options = options.options([("statement_timeout", format!("{}s", statement_timeout))]);
        }

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(min_connections)
            .acquire_timeout(Duration::from_secs(acquire_timeout))
            .idle_timeout(Duration::from_secs(idle_timeout))
            .max_lifetime(Duration::from_secs(max_lifetime))
            .connect_with(options)
            .await?;

        migrations::run_migrations(&pool).await?;

        tracing::debug!(
            max_connections,
            min_connections,
            acquire_timeout_secs = acquire_timeout,
            idle_timeout_secs = idle_timeout,
            max_lifetime_secs = max_lifetime,
            statement_timeout_secs = statement_timeout,
            "PostgresService initialized (SaaS mode)"
        );
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Close the connection pool gracefully
    pub async fn close(&self) {
        self.pool.close().await;
        tracing::debug!("PostgreSQL pool closed");
    }

    /// Start a background health check task (optional for PostgreSQL)
    pub fn start_health_check_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let db = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("PostgreSQL health check task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = sqlx::query("SELECT 1").execute(&db.pool).await {
                            tracing::warn!("PostgreSQL health check failed: {}", e);
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    // PostgreSQL tests require a running PostgreSQL instance
    // and are typically run as integration tests
}
