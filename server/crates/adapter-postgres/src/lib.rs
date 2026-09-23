//! PostgreSQL transactional-store adapter.
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
pub mod migrations;
pub mod repositories;
mod repository_impl;
pub use repository_impl::PostgresRepository;
pub mod schema;

pub use error::PostgresError;
pub use sqlx::PgPool;

use std::sync::Arc;

use sideseat_ports::cache::CacheStore;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, Executor};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::log::LevelFilter;

use sideseat_core::config::PostgresConfig;
use sideseat_core::constants::{
    POSTGRES_DEFAULT_ACQUIRE_TIMEOUT_SECS, POSTGRES_DEFAULT_IDLE_TIMEOUT_SECS,
    POSTGRES_DEFAULT_MAX_CONNECTIONS, POSTGRES_DEFAULT_MAX_LIFETIME_SECS,
    POSTGRES_DEFAULT_MIN_CONNECTIONS, POSTGRES_DEFAULT_STATEMENT_TIMEOUT_SECS,
};
use sideseat_ports::clock::Clock;
use sideseat_ports::types::ProjectId;

/// PostgreSQL database service
///
/// Handles database initialization, connection pooling, and background tasks.
/// Optimized for scalable SaaS with connection pooling and query protection.
/// Should be created once at server startup and shared across all modules.
pub struct PostgresService {
    /// Schema owner, retained only for migrations and explicit schema administration.
    schema_pool: PgPool,
    /// Privileged cross-project operations. Every connection assumes `sideseat_maintenance`.
    pool: PgPool,
    /// Ordinary application operations. Every connection assumes `sideseat_runtime`; tenant context must
    /// still be bound transaction-locally before touching a project-scoped table.
    runtime_pool: PgPool,
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

pub const POSTGRES_RUNTIME_ROLE: &str = "sideseat_runtime";
pub const POSTGRES_MAINTENANCE_ROLE: &str = "sideseat_maintenance";

const ROLE_BOOTSTRAP_SQL: &str = r#"
DO $sideseat_roles$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'sideseat_runtime') THEN
        CREATE ROLE sideseat_runtime
            NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'sideseat_maintenance') THEN
        CREATE ROLE sideseat_maintenance
            NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
    END IF;

    EXECUTE format('GRANT sideseat_runtime TO %I', current_user);
    EXECUTE format('GRANT sideseat_maintenance TO %I', current_user);
END
$sideseat_roles$;

ALTER ROLE sideseat_runtime
    NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
ALTER ROLE sideseat_maintenance
    NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
"#;

const ROLE_GRANTS_SQL: &str = r#"
GRANT USAGE ON SCHEMA public TO sideseat_runtime, sideseat_maintenance;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON ALL TABLES IN SCHEMA public TO sideseat_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE, TRUNCATE
    ON ALL TABLES IN SCHEMA public TO sideseat_maintenance;
GRANT USAGE, SELECT, UPDATE
    ON ALL SEQUENCES IN SCHEMA public TO sideseat_runtime, sideseat_maintenance;

ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO sideseat_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE, TRUNCATE ON TABLES TO sideseat_maintenance;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO sideseat_runtime, sideseat_maintenance;
"#;

impl PostgresService {
    /// Initialize the database service from configuration
    ///
    /// Creates a connection pool with SaaS-optimized settings:
    /// - Min connections kept warm for low latency
    /// - Max connections sized for concurrent load
    /// - Idle timeout to release unused connections
    /// - Max lifetime to cycle connections and prevent stale state
    /// - Statement timeout to prevent runaway queries
    pub async fn init(
        config: &PostgresConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, PostgresError> {
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

        let owner_pool = Self::connect_pool(
            options.clone(),
            1,
            1,
            acquire_timeout,
            idle_timeout,
            max_lifetime,
            None,
        )
        .await?;
        sqlx::raw_sql(ROLE_BOOTSTRAP_SQL)
            .execute(&owner_pool)
            .await?;
        migrations::run_migrations(&owner_pool, clock.as_ref()).await?;
        sqlx::raw_sql(ROLE_GRANTS_SQL).execute(&owner_pool).await?;

        let pool = Self::connect_pool(
            options.clone(),
            max_connections,
            min_connections,
            acquire_timeout,
            idle_timeout,
            max_lifetime,
            Some(POSTGRES_MAINTENANCE_ROLE),
        )
        .await?;
        let runtime_pool = Self::connect_pool(
            options,
            max_connections,
            min_connections,
            acquire_timeout,
            idle_timeout,
            max_lifetime,
            Some(POSTGRES_RUNTIME_ROLE),
        )
        .await?;

        tracing::debug!(
            max_connections,
            min_connections,
            acquire_timeout_secs = acquire_timeout,
            idle_timeout_secs = idle_timeout,
            max_lifetime_secs = max_lifetime,
            statement_timeout_secs = statement_timeout,
            "PostgresService initialized (SaaS mode)"
        );
        Ok(Self {
            schema_pool: owner_pool,
            pool,
            runtime_pool,
            cache: None,
            clock,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_pool(
        options: PgConnectOptions,
        max_connections: u32,
        min_connections: u32,
        acquire_timeout: u64,
        idle_timeout: u64,
        max_lifetime: u64,
        role: Option<&'static str>,
    ) -> Result<PgPool, PostgresError> {
        let mut pool_options = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(min_connections)
            .acquire_timeout(Duration::from_secs(acquire_timeout))
            .idle_timeout(Duration::from_secs(idle_timeout))
            .max_lifetime(Duration::from_secs(max_lifetime));
        if let Some(role) = role {
            let statement = format!("SET ROLE {role}");
            pool_options = pool_options.after_connect(move |connection, _| {
                let statement = statement.clone();
                Box::pin(async move {
                    connection.execute(statement.as_str()).await?;
                    Ok(())
                })
            });
        }
        pool_options.connect_with(options).await.map_err(Into::into)
    }

    /// Privileged, cross-project pool for migrations, retention and repair.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Schema-owner pool. Ordinary and maintenance code must not use this path.
    pub fn schema_pool(&self) -> &PgPool {
        &self.schema_pool
    }

    /// Restricted pool for ordinary project-scoped requests.
    pub fn runtime_pool(&self) -> &PgPool {
        &self.runtime_pool
    }

    /// Start an ordinary application transaction scoped to exactly one project.
    ///
    /// `set_config(..., true)` makes the tenant selector transaction-local, so pooled
    /// connections cannot leak scope between requests.
    pub async fn tenant_transaction(
        &self,
        project_id: &ProjectId,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresError> {
        let mut transaction = self.runtime_pool.begin().await?;
        sqlx::query("SELECT set_config('sideseat.project_id', $1, true)")
            .bind(project_id.as_str())
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    /// Start a privileged transaction for an explicitly cross-project maintenance operation.
    pub async fn maintenance_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresError> {
        self.pool.begin().await.map_err(Into::into)
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

    /// Close the connection pool gracefully
    pub async fn close(&self) {
        self.runtime_pool.close().await;
        self.pool.close().await;
        self.schema_pool.close().await;
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
