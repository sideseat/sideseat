//! ClickHouse analytics service
//!
//! Provides centralized analytics database management for distributed deployments.
//! Uses async HTTP/S connections to ClickHouse cluster with connection pooling.
//!
//! Optimized for high-volume SaaS workloads:
//! - LZ4 compression for efficient network transfer
//! - Async inserts for high-throughput ingestion
//! - HTTP keep-alive for connection reuse

pub mod error;
mod filters;
#[cfg(test)]
#[path = "parity_tests.rs"]
mod parity_tests;
pub mod repositories;
mod repository_impl;
pub mod schema;

pub use error::ClickhouseError;

use std::sync::Arc;
use std::time::Duration;

use clickhouse::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::core::config::{ClickhouseConfig, RetentionConfig};

/// ClickHouse analytics service
///
/// Handles database initialization and provides access to the ClickHouse client.
/// The clickhouse crate's Client internally uses hyper with connection pooling
/// via HTTP keep-alive for efficient connection reuse.
///
/// Supports both single-node and distributed cluster deployments:
/// - Single-node: Uses ReplacingMergeTree for local development
/// - Distributed: Uses ReplicatedReplacingMergeTree with sharding for SaaS
pub struct ClickhouseService {
    client: Client,
    config: ClickhouseConfig,
}

impl ClickhouseService {
    /// Initialize the analytics service with ClickHouse connection
    ///
    /// Configures the client for high-throughput SaaS workloads:
    /// - LZ4 compression reduces network bandwidth
    /// - Async inserts enable server-side batching for high write throughput
    /// - HTTP keep-alive provides connection pooling
    pub async fn init(config: &ClickhouseConfig) -> Result<Self, ClickhouseError> {
        let mut client = Client::default()
            .with_url(&config.url)
            .with_database(&config.database);

        // Apply authentication if provided
        if let Some(ref user) = config.user {
            client = client.with_user(user);
        }
        if let Some(ref password) = config.password {
            client = client.with_password(password);
        }

        // Enable LZ4 compression for efficient network transfer
        if config.compression {
            client = client.with_compression(clickhouse::Compression::Lz4);
        }

        // FINAL optimization: process each partition independently during FINAL queries.
        // Without this, ReplacingMergeTree FINAL merges all partitions in a single pass,
        // which at TB scale causes merge storms processing billions of rows. With this
        // setting, each monthly partition is processed in parallel.
        client = client.with_option("do_not_merge_across_partitions_select_final", "1");

        // A distributed insert has to reach the shard before it is reported stored.
        //
        // Writing to a `Distributed` table is asynchronous by default: the initiating node spools the rows
        // into a local directory and forwards them in the background. That is the same lie as
        // `wait_for_async_insert = 0` by a different route - the OTLP route answers 200 and the ingestion
        // queue acknowledges its message, both on the strength of a spool file on one node's disk, which
        // an ephemeral instance takes with it when it goes.
        if config.distributed {
            client = client.with_option("insert_distributed_sync", "1");
        }

        // Replica quorum, when the table is replicated.
        //
        // `insert_distributed_sync` makes the insert reach a shard rather than a spool file on the
        // initiating node - but the node holding those rows can fail before replication carries them, and
        // they go with it, after the exporter was told 200. `insert_quorum` blocks until enough replicas
        // confirm. `insert_quorum_parallel = 0` goes with it: in parallel mode ClickHouse does not
        // guarantee a *linearizable* sequence, and a quorum that can be satisfied by different replicas for
        // adjacent inserts is not the guarantee the setting is being used for here.
        if config.insert_quorum > 0 {
            client = client.with_option("insert_quorum", config.insert_quorum.to_string());
            client = client.with_option("insert_quorum_parallel", "0");
        } else if config.distributed {
            // The window is open and nothing says so otherwise. In distributed mode the tables are
            // `Replicated*` by construction, so an insert that `insert_distributed_sync` carried to a shard
            // still lives on one replica until replication catches up - and that is after the exporter was
            // answered 200 and the ingestion queue acknowledged. The Redis backend warns on exactly this
            // shape (replicas present, no acknowledgement required); this is its ClickHouse twin.
            //
            // Warned rather than refused, unlike `insert_quorum = 1`: a cluster with one replica per shard is
            // a legitimate deployment, and a quorum of two would block every insert there forever. So the
            // operator is told what the default costs and left to decide.
            tracing::warn!(
                "database.clickhouse.distributed is on with no insert_quorum, so an accepted export lives \
                 on one replica until replication carries it - losing that node loses data already \
                 acknowledged. Set database.clickhouse.insert_quorum to at least 2 where each shard has two \
                 or more replicas, at the cost of waiting for them."
            );
        }

        // Configure async inserts for high-throughput ingestion
        // This enables server-side batching - inserts are buffered and flushed periodically
        if config.async_insert {
            client = client.with_option("async_insert", "1");
            // wait_for_async_insert: 0 = fire-and-forget (max throughput), 1 = wait for flush
            let wait_value = if config.wait_for_async_insert {
                "1"
            } else {
                "0"
            };
            client = client.with_option("wait_for_async_insert", wait_value);
        }

        let service = Self {
            client,
            config: config.clone(),
        };

        // Run migrations to ensure schema exists
        service.run_migrations().await?;

        tracing::debug!(
            url = %config.url,
            database = %config.database,
            compression = %config.compression,
            async_insert = %config.async_insert,
            distributed = %config.distributed,
            cluster = ?config.cluster,
            "ClickhouseService initialized"
        );

        Ok(service)
    }

    /// Get the ClickHouse client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get the table name to insert into (local table for distributed mode)
    pub fn insert_table(&self, base_name: &str) -> String {
        schema::get_insert_table(&self.config, base_name)
    }

    /// Get the table name for DELETE operations (local table for distributed mode)
    pub fn delete_table(&self, base_name: &str) -> String {
        schema::get_delete_table(&self.config, base_name)
    }

    /// Get the ON CLUSTER clause for mutations (empty for single-node)
    pub fn on_cluster_clause(&self) -> String {
        schema::get_on_cluster_clause(&self.config)
    }

    /// Check if running in distributed mode
    pub fn is_distributed(&self) -> bool {
        self.config.distributed
    }

    /// Health check - verify connection to ClickHouse
    pub async fn health_check(&self) -> Result<(), ClickhouseError> {
        self.client
            .query("SELECT 1")
            .execute()
            .await
            .map_err(ClickhouseError::from)
    }

    /// Run schema migrations
    async fn run_migrations(&self) -> Result<(), ClickhouseError> {
        // Check if schema_version table exists
        let table_exists: bool = self
            .client
            .query(
                "SELECT count() > 0 FROM system.tables WHERE database = currentDatabase() AND name = 'schema_version'",
            )
            .fetch_one()
            .await
            .map_err(|e| ClickhouseError::Connection(format!(
                "Failed to check schema_version table: {}. Verify ClickHouse is running and accessible.",
                e
            )))?;

        if !table_exists {
            tracing::debug!(
                "Applying initial ClickHouse schema v{}",
                schema::SCHEMA_VERSION
            );
            self.apply_initial_schema().await?;
            return Ok(());
        }

        // Get current version
        let current_version: Option<i32> = self
            .client
            .query("SELECT version FROM schema_version WHERE id = 1")
            .fetch_optional()
            .await
            .ok()
            .flatten();

        match current_version {
            None => {
                tracing::debug!(
                    "Applying initial ClickHouse schema v{}",
                    schema::SCHEMA_VERSION
                );
                self.apply_initial_schema().await?;
            }
            Some(v) if v < schema::SCHEMA_VERSION => {
                tracing::debug!(
                    "Migrating ClickHouse schema from v{} to v{}",
                    v,
                    schema::SCHEMA_VERSION
                );
                for version in (v + 1)..=schema::SCHEMA_VERSION {
                    self.apply_versioned_migration(version).await?;
                }
            }
            Some(v) if v > schema::SCHEMA_VERSION => {
                return Err(ClickhouseError::MigrationFailed {
                    version: v,
                    name: "version_check".to_string(),
                    error: format!(
                        "Database schema version {} is newer than application version {}. Upgrade the application.",
                        v,
                        schema::SCHEMA_VERSION
                    ),
                });
            }
            _ => {
                tracing::debug!(
                    "ClickHouse schema is up to date (v{})",
                    schema::SCHEMA_VERSION
                );
            }
        }

        Ok(())
    }

    /// Apply initial schema
    async fn apply_initial_schema(&self) -> Result<(), ClickhouseError> {
        // Generate schema based on configuration (single-node vs distributed)
        let statements = schema::generate_schema(&self.config);

        tracing::debug!(
            distributed = %self.config.distributed,
            cluster = ?self.config.cluster,
            statements = statements.len(),
            "Applying ClickHouse schema"
        );

        // Create tables
        for table_sql in &statements {
            self.client
                .query(table_sql)
                .execute()
                .await
                .map_err(ClickhouseError::from)?;
        }

        // Record schema version
        let now = chrono::Utc::now().timestamp();
        self.client
            .query(
                "INSERT INTO schema_version (id, version, applied_at, description) VALUES (?, ?, ?, ?)",
            )
            .bind(1u8)
            .bind(schema::SCHEMA_VERSION)
            .bind(now)
            .bind("Initial schema")
            .execute()
            .await
            .map_err(ClickhouseError::from)?;

        tracing::debug!(
            version = schema::SCHEMA_VERSION,
            distributed = %self.config.distributed,
            "ClickHouse schema applied successfully"
        );
        Ok(())
    }

    /// Apply one versioned migration, from [`schema::MIGRATIONS`].
    ///
    /// Data rather than match arms: the table is what `migrations_cover_every_version` checks, so a
    /// version bump with no migration fails a test instead of failing every existing database at its
    /// first restart after the upgrade.
    ///
    /// Statements carry `{on_cluster}` where DDL needs it - required in distributed mode, empty
    /// otherwise - and are applied in order. The version is recorded only after all of them succeed,
    /// so a partial failure leaves the database at the previous version and the migration is retried
    /// on the next start rather than being silently skipped.
    /// One migration, on demand, so `every_clickhouse_migration_applies_to_the_state_it_upgrades` can
    /// invoke it exactly as a startup upgrade does.
    #[cfg(test)]
    pub(crate) async fn apply_migration_for_test(
        &self,
        version: i32,
    ) -> Result<(), ClickhouseError> {
        self.apply_versioned_migration(version).await
    }

    async fn apply_versioned_migration(&self, version: i32) -> Result<(), ClickhouseError> {
        let Some(migration) = schema::MIGRATIONS.iter().find(|m| m.version == version) else {
            return Err(ClickhouseError::MigrationFailed {
                version,
                name: "unknown".to_string(),
                error: format!(
                    "No migration defined for version {version}. A database at v{} cannot be \
                     upgraded by this build; recreate it or add the migration.",
                    version - 1
                ),
            });
        };

        let name = migration.name;
        let on_cluster = schema::get_on_cluster_clause(&self.config);
        let local = schema::local_table_suffix(&self.config);
        let render = |statement: &str| {
            statement
                .replace("{on_cluster}", &on_cluster)
                .replace("{local}", local)
        };
        let failed = |e: ClickhouseError| ClickhouseError::MigrationFailed {
            version,
            name: name.to_string(),
            error: e.to_string(),
        };

        // Two questions here, resolved separately: is there work left on the local table (guarded by the
        // migration's `precondition`), and does the `Distributed` front end also need catching up. Both
        // asked below, in the order that keeps a partly-applied state recoverable.
        // Distributed statements first, and idempotent. A `Distributed` table is created from the
        // local table's structure once and does not follow later changes, so its front-end schema has to
        // be updated too - and if the local statement then fails, or the version record fails, the next
        // start must be able to try again without every previous step colliding. The ordering also lets
        // us run distributed_statements even when the precondition already flipped for the local table:
        // a local ALTER that succeeded but a distributed one that failed left the version at N-1 and the
        // local at N, and re-running has to add the column on the distributed side without repeating the
        // local ALTER (which is not idempotent).
        if self.config.distributed {
            for statement in migration.distributed_statements {
                self.client
                    .query(&render(statement))
                    .execute()
                    .await
                    .map_err(|e| failed(ClickhouseError::from(e)))?;
            }
        }

        // Local statements only when the precondition still says work is due, since they are not
        // idempotent.
        let local_pending = match migration.precondition {
            None => true,
            Some(precondition) => {
                let pending: Option<u8> = self
                    .client
                    .query(&render(precondition))
                    .fetch_optional()
                    .await
                    .map_err(|e| failed(ClickhouseError::from(e)))?;
                pending.is_some()
            }
        };
        if local_pending {
            for statement in migration.statements {
                self.client
                    .query(&render(statement))
                    .execute()
                    .await
                    .map_err(|e| failed(ClickhouseError::from(e)))?;
            }
        }

        tracing::debug!("ClickHouse migration v{} ({}) applied", version, name);
        self.record_schema_version(version).await
    }

    async fn record_schema_version(&self, version: i32) -> Result<(), ClickhouseError> {
        self.client
            .query("ALTER TABLE schema_version UPDATE version = ?, applied_at = ? WHERE id = 1")
            .bind(version)
            .bind(chrono::Utc::now().timestamp())
            .execute()
            .await
            .map_err(ClickhouseError::from)
    }

    /// Start health check task
    pub fn start_health_check_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("ClickHouse health check task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = service.health_check().await {
                            tracing::warn!("ClickHouse health check failed: {}", e);
                        }
                    }
                }
            }
        })
    }

    /// Start retention cleanup task
    pub fn start_retention_task(
        self: &Arc<Self>,
        config: RetentionConfig,
        mut shutdown_rx: watch::Receiver<bool>,
        _file_service: Option<Arc<crate::data::files::FileService>>,
        _database: Arc<crate::data::TransactionalService>,
    ) -> Option<JoinHandle<()>> {
        if config.max_spans.is_none() && config.max_age_minutes.is_none() {
            tracing::debug!("Retention disabled (no limits configured)");
            return None;
        }

        let service = Arc::clone(self);
        tracing::debug!(
            max_spans = ?config.max_spans,
            max_age_minutes = ?config.max_age_minutes,
            "Starting ClickHouse retention task"
        );

        Some(tokio::spawn(async move {
            // ClickHouse handles TTL natively, but we may want manual cleanup for count-based limits
            let mut interval = tokio::time::interval(Duration::from_secs(3600)); // hourly

            // Pre-compute table name and ON CLUSTER clause for retention cleanup
            let delete_table = service.delete_table("otel_spans");
            let on_cluster = service.on_cluster_clause();

            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("ClickHouse retention task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Some(max_age_minutes) = config.max_age_minutes {
                            // ClickHouse has native TTL but we can also run explicit cleanup
                            let cutoff = chrono::Utc::now() - chrono::Duration::minutes(max_age_minutes as i64);
                            let cutoff_ts = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

                            // In distributed mode, must use local table with ON CLUSTER
                            // Synchronous, like every other delete here: an unbounded pile of
                            // scheduled mutations is how a retention sweep starts overlapping itself.
                            let sql = format!(
                                "ALTER TABLE {}{} DELETE WHERE timestamp_start < ? SETTINGS mutations_sync = 2",
                                delete_table, on_cluster
                            );

                            if let Err(e) = service.client
                                .query(&sql)
                                .bind(&cutoff_ts)
                                .execute()
                                .await
                            {
                                tracing::warn!("ClickHouse retention cleanup failed: {}", e);
                            } else {
                                tracing::debug!("ClickHouse retention cleanup completed (cutoff: {})", cutoff_ts);
                            }
                        }
                    }
                }
            }
        }))
    }

    /// Close the connection gracefully (no-op for ClickHouse HTTP client)
    pub async fn close(&self) {
        tracing::debug!("ClickHouse connection closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clickhouse_error_types() {
        let err = ClickhouseError::Connection("test".to_string());
        assert!(err.to_string().contains("test"));
    }
}
