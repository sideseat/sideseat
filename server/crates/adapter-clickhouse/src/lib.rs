//! ClickHouse analytics-store adapter.
//!
//! Provides centralized analytics database management for distributed deployments.
//! Uses async HTTP/S connections to ClickHouse cluster with connection pooling.
//!
//! Optimized for high-volume SaaS workloads:
//! - LZ4 compression for efficient network transfer
//! - Async inserts for high-throughput ingestion
//! - HTTP keep-alive for connection reuse

// A map's iteration order differs from one process to the next: where it reaches stored or answered bytes, a
// hash or the order of a write, iterate in order; elsewhere say why order cannot matter.
#![deny(clippy::iter_over_hash_type)]

mod consistency;
mod error;
mod repositories;
mod repository_impl;
mod restore_retention;
mod retention;
pub use consistency::{CheckOutcome, PartitionAnomaly};
pub use error::ClickhouseError;
pub use repository_impl::ClickhouseRepository;
pub mod schema;

use sideseat_core::utils::collection_fence::CollectionFence;
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Client;
use sideseat_core::schema_version::{SchemaCheck, check};
use sideseat_ports::blobs::RetentionFileReconciler;
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{
    AnalyticsMaintenance, DeletionCause, DeletionRecord, DeletionScope, EntityQuery, SpanStore,
    StorageGovernance, TransactionalRepository, retention_cleanup_logical_bytes,
};
use sideseat_ports::types::ProjectId;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use sideseat_core::config::{ClickhouseConfig, RetentionConfig};

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
    clock: Arc<dyn Clock>,
}

impl ClickhouseService {
    const RETENTION_CLEANUP_CLAIM: i64 = 256;
    const RETENTION_CLEANUP_LEASE_SECS: i64 = 600;
    const PRESSURE_BATCH_SIZE: usize = 100_000;

    async fn finish_retention_cleanup(
        analytics: &ClickhouseRepository,
        database: &Arc<dyn TransactionalRepository + Send + Sync>,
        file_service: Option<&Arc<dyn RetentionFileReconciler>>,
        project_id: &ProjectId,
        trace_ids: &[String],
        completed: &[(String, i64)],
        fence: &CollectionFence,
    ) {
        // Every step deletes or releases; each only while this process holds its store lease, and refused, the
        // cleanup stays recorded for the next pass.
        let mut clean = true;
        match database
            .journaled_span_deletions_for_traces(project_id, trace_ids)
            .await
        {
            Ok(spans) if !spans.is_empty() => {
                if !fence.is_open() {
                    return;
                }
                if let Err(error) = SpanStore::delete_spans(analytics, project_id, &spans).await {
                    clean = false;
                    tracing::warn!(
                        project_id = %project_id,
                        spans = spans.len(),
                        %error,
                        "Failed to re-apply journalled ClickHouse span deletions"
                    );
                }
            }
            Ok(_) => {}
            Err(error) => {
                clean = false;
                tracing::warn!(
                    project_id = %project_id,
                    %error,
                    "Could not read journalled ClickHouse span deletions"
                );
            }
        }

        if !fence.is_open() {
            tracing::debug!(project_id = %project_id, "Collection is fenced; the cleanup stays recorded");
            return;
        }
        // A trace the reconciliation deferred - an association a writer was pending on, or a confirm since its
        // horizon stamped - stays recorded on its own, retried at the claim's capped backoff, while the rest of the
        // batch completes.
        let mut deferred = Vec::new();
        match file_service {
            Some(files) if files.is_enabled() => {
                match files
                    .reconcile_trace_survivors(project_id, trace_ids, analytics)
                    .await
                {
                    Ok(skipped) => deferred = skipped,
                    Err(error) => {
                        clean = false;
                        tracing::warn!(
                            project_id = %project_id,
                            %error,
                            "Failed to reconcile ClickHouse retention file survivors"
                        );
                    }
                }
            }
            _ => clean = false,
        }

        if !fence.is_open() {
            tracing::debug!(project_id = %project_id, "Collection is fenced; the cleanup stays recorded");
            return;
        }
        // The rows are gone, so their traces' ownership of the records they named goes too - and the records are
        // queued after the release, so a reconciliation pass sees them unowned and collects them.
        match sideseat_ports::traits::release_retained_raw_owners(
            database.as_ref(),
            analytics,
            project_id,
            trace_ids,
            fence,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                clean = false;
                tracing::debug!(
                    project_id = %project_id,
                    "An owner granted after the release's read stays; the cleanup is retried"
                );
            }
            Err(error) => {
                clean = false;
                tracing::warn!(
                    project_id = %project_id,
                    %error,
                    "Failed to release ClickHouse retained raw-record ownership"
                );
            }
        }

        match EntityQuery::traces_without_spans(analytics, project_id, trace_ids).await {
            Ok(emptied) if !emptied.is_empty() => {
                if !fence.is_open() {
                    return;
                }
                if let Err(error) = database
                    .delete_favorites_by_entity("trace", &emptied, project_id)
                    .await
                {
                    clean = false;
                    tracing::warn!(
                        project_id = %project_id,
                        %error,
                        "Failed to clean ClickHouse retention favorites"
                    );
                }
            }
            Ok(_) => {}
            Err(error) => {
                clean = false;
                tracing::warn!(
                    project_id = %project_id,
                    %error,
                    "Could not identify traces emptied by ClickHouse retention"
                );
            }
        }

        if clean
            && fence.is_open()
            && let Err(error) = database
                .complete_retention_cleanup(
                    project_id,
                    &sideseat_ports::traits::retention_cleanup_owed_kept(completed, &deferred),
                )
                .await
        {
            tracing::debug!(
                project_id = %project_id,
                %error,
                "Could not complete ClickHouse retention cleanup tokens"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn trim_project_to_span_limit(
        service: &Arc<Self>,
        max_spans: u64,
        quota_bytes: u64,
        database: &Arc<dyn TransactionalRepository + Send + Sync>,
        governance: &Arc<dyn StorageGovernance + Send + Sync>,
        file_service: Option<&Arc<dyn RetentionFileReconciler>>,
        project_id: &ProjectId,
        fence: &CollectionFence,
    ) -> Result<usize, String> {
        let fenced = || {
            (!fence.is_open()).then(|| {
                "collection is fenced: this process does not hold its store lease".to_string()
            })
        };
        let analytics = ClickhouseRepository(Arc::clone(service));
        let counts = AnalyticsMaintenance::count_spans_by_project(
            &analytics,
            std::slice::from_ref(project_id),
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut overage = counts
            .get(project_id.as_str())
            .copied()
            .unwrap_or_default()
            .saturating_sub(max_spans);
        let mut deleted = 0usize;

        for _ in 0..10 {
            if overage == 0 {
                break;
            }
            let candidates = AnalyticsMaintenance::oldest_reclaimable_spans(
                &analytics,
                project_id,
                u64::MAX,
                service.clock.now(),
                usize::try_from(overage)
                    .unwrap_or(Self::PRESSURE_BATCH_SIZE)
                    .min(Self::PRESSURE_BATCH_SIZE),
            )
            .await
            .map_err(|error| error.to_string())?;
            if candidates.is_empty() {
                break;
            }

            let journal_bytes = candidates.iter().fold(0u64, |total, candidate| {
                total.saturating_add(DeletionRecord::logical_bytes_for(
                    project_id.as_str(),
                    DeletionCause::Pressure,
                    DeletionScope::Span,
                    &candidate.trace_id,
                    Some(&candidate.span_id),
                ))
            });
            let mut trace_ids = candidates
                .iter()
                .map(|candidate| candidate.trace_id.clone())
                .collect::<Vec<_>>();
            trace_ids.sort();
            trace_ids.dedup();
            let cleanup_bytes = trace_ids.iter().fold(0u64, |total, trace_id| {
                total.saturating_add(retention_cleanup_logical_bytes(
                    project_id.as_str(),
                    trace_id,
                ))
            });
            let additional = journal_bytes.saturating_add(cleanup_bytes);
            let reserved = governance
                .reserve_project_storage(project_id, additional, quota_bytes, service.clock.now())
                .await
                .map_err(|error| error.to_string())?;
            if reserved.is_none() {
                return Err(format!(
                    "maintenance reserve exhausted before {} pressure evictions",
                    candidates.len()
                ));
            }

            let spans = candidates
                .iter()
                .map(|candidate| (candidate.trace_id.clone(), candidate.span_id.clone()))
                .collect::<Vec<_>>();
            if let Some(error) = fenced() {
                return Err(error);
            }
            let tokens = database
                .record_pressure_eviction(project_id, &spans)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(error) = fenced() {
                return Err(error);
            }
            SpanStore::delete_spans(&analytics, project_id, &spans)
                .await
                .map_err(|error| error.to_string())?;
            Self::finish_retention_cleanup(
                &analytics,
                database,
                file_service,
                project_id,
                &trace_ids,
                &tokens,
                fence,
            )
            .await;
            deleted = deleted.saturating_add(candidates.len());
            overage = overage.saturating_sub(candidates.len() as u64);
        }
        Ok(deleted)
    }

    /// Initialize the analytics service with ClickHouse connection
    ///
    /// Configures the client for high-throughput SaaS workloads:
    /// - LZ4 compression reduces network bandwidth
    /// - Async inserts enable server-side batching for high write throughput
    /// - HTTP keep-alive provides connection pooling
    pub async fn init(
        config: &ClickhouseConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ClickhouseError> {
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
        //
        // **Stated residual: a correction that moves a span's `timestamp_start` across a month boundary is
        // still returned twice.** Schema v3 took `toDate(timestamp_start)` out of the span sorting key, which
        // is what collapses the *midnight*-crossing case - the common one, since the key became a function of
        // identity alone. `PARTITION BY toYYYYMM(timestamp_start)` remains, deliberately, because time pruning
        // is what it is for; so two revisions of one identity can sit in different partitions, parts in
        // different partitions never merge, and this setting makes `FINAL` per-partition - so both survive.
        //
        // Turning the setting off is not the remedy: it was measured at 10-12x the read cost, and it would
        // trade a rare duplicate for a permanent regression on every query. Nor can the duplicate be resolved
        // at read time - that needs a stable tie-break for equal `ingested_at`, and none exists here
        // (`(_part, _part_offset)` is physical placement a merge changes, and no per-delivery discriminator is
        // stored). So the case is **reported rather than engineered around**: `consistency.rs` detects it,
        // records each identity durably in `span_partition_anomalies`, and runs on a schedule - a detector
        // nobody runs reports nothing. Both boundaries are pinned by the parity suite:
        // `a_correction_crossing_midnight_utc_is_one_span_on_both_backends` covers the case v3 fixes, and
        // `a_correction_crossing_a_month_boundary_is_reported_by_the_consistency_check` asserts the residual
        // is real *and* reported - so if the residual is ever closed, that test fails rather than quietly
        // over-asserting.
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

        // Both async-insert settings are pinned on every query, never inherited from the server.
        //
        // A server profile decides them otherwise - ClickHouse 26 turns `async_insert` on by default - and a
        // profile with `wait_for_async_insert = 0` returns from an INSERT while the rows are still only in the
        // server's buffer, which this process would answer 200 for. The wait is pinned on: configuration refuses
        // async inserts without it, because an acknowledgement must not rest on a buffer a restart discards.
        client = client
            .with_option("async_insert", if config.async_insert { "1" } else { "0" })
            .with_option("wait_for_async_insert", "1");

        let service = Self {
            client,
            config: config.clone(),
            clock,
        };

        // Run migrations to ensure schema exists
        service.ensure_schema().await?;

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

    /// A clone whose tenant selector is sent as an HTTP query option on every request.
    ///
    /// This is not a session-level `SET`: each ClickHouse request carries its own value, so
    /// keep-alive connection reuse cannot leak the previous borrower's project.
    pub fn tenant_client(&self, project_id: &ProjectId) -> Client {
        self.tenant_client_str(project_id.as_str())
    }

    pub fn tenant_client_str(&self, project_id: &str) -> Client {
        self.client
            .clone()
            .with_option(schema::TENANT_PROJECT_SETTING, project_id)
    }

    /// Explicit cross-project path for retention, repair, consistency checks and migrations.
    pub fn maintenance_client(&self) -> Client {
        self.client
            .clone()
            .with_option(schema::TENANT_MAINTENANCE_SETTING, "1")
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
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

    /// The span rows a read that releases raw ownership must see: on one node the table itself; on a cluster
    /// every replica of every shard, so a row an acknowledged insert committed on any one replica is seen
    /// whatever the replication lag, with or without quorum inserts. An unreachable replica fails the read
    /// rather than being skipped, so the release waits instead of deciding on a partial answer.
    pub fn every_replica_spans(&self) -> String {
        self.every_replica_table("otel_spans")
    }

    /// `base_name` read on every replica, as [`Self::every_replica_spans`] reads the span rows.
    pub fn every_replica_table(&self, base_name: &str) -> String {
        schema::every_replica_table(&self.config, base_name)
    }

    /// Record a retention cleanup for every trace an age delete at `cutoff` would touch: the intent the age pass
    /// writes before it deletes, so the cleanup loop later releases those traces' files and raw-record ownership.
    pub async fn record_age_expiry(
        &self,
        project_id: &ProjectId,
        cutoff: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
        database: &(dyn TransactionalRepository + Send + Sync),
    ) -> Result<usize, String> {
        retention::record_expiring_traces(
            &self.tenant_client(project_id),
            &self.every_replica_spans(),
            &self.every_replica_table("otel_logs"),
            project_id,
            cutoff,
            now,
            database,
        )
        .await
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
    /// Create the schema in an empty database, accept the current one, refuse any other.
    ///
    /// See `sideseat_core::schema_version`: there are no upgrades.
    async fn ensure_schema(&self) -> Result<(), ClickhouseError> {
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
        let found: Option<i32> = if table_exists {
            self.client
                .query("SELECT version FROM schema_version FINAL WHERE id = 1")
                .fetch_optional()
                .await
                .map_err(ClickhouseError::from)?
        } else {
            None
        };
        let decision =
            check(found, schema::SCHEMA_VERSION).map_err(ClickhouseError::UnsupportedSchema)?;
        // Whichever the decision: on a cluster the version row is the shard's own, so a shard other than the one
        // that created the store finds none and would create - keeping, through `IF NOT EXISTS`, a table it holds.
        // Only layouts this process would read wrongly are refused. A column's codec or compress block size changes
        // no answer, so a store created with other ones (LZ4 throughout, say) is accepted as it is.
        self.refuse_partitioned_raw_records().await?;
        self.refuse_unkeyed_raw_versions().await?;
        self.refuse_receipt_partitioned_logs().await?;
        match decision {
            SchemaCheck::Current => Ok(()),
            SchemaCheck::Create => self.apply_initial_schema().await,
        }
    }

    /// Refuse a store whose raw records are partitioned, as one created before they stopped being is: the client
    /// resolves `FINAL` a partition at a time, so such a store reads a record's versions in two partitions as two
    /// latest ones (`schema::raw_tables`).
    async fn refuse_partitioned_raw_records(&self) -> Result<(), ClickhouseError> {
        let partitioned: Vec<(String, String)> = self
            .client
            .query(
                "SELECT name, partition_key FROM system.tables \
                 WHERE database = currentDatabase() AND name IN ('otel_raw', 'otel_raw_local') \
                   AND partition_key != ''",
            )
            .fetch_all()
            .await
            .map_err(ClickhouseError::from)?;
        match partitioned.into_iter().next() {
            Some((table, partition_key)) => Err(ClickhouseError::IncompatibleLayout {
                table,
                partition_key,
            }),
            None => Ok(()),
        }
    }

    /// Refuse a store whose raw records still carry receipt-instant versions in a `version` column, as one
    /// created before the arbiter keyed them by epoch does (`schema::raw_tables`).
    async fn refuse_unkeyed_raw_versions(&self) -> Result<(), ClickhouseError> {
        let unkeyed: Vec<String> = self
            .client
            .query(
                "SELECT table FROM system.columns \
                 WHERE database = currentDatabase() AND table IN ('otel_raw', 'otel_raw_local') \
                   AND name = 'version'",
            )
            .fetch_all()
            .await
            .map_err(ClickhouseError::from)?;
        match unkeyed.into_iter().next() {
            Some(table) => Err(ClickhouseError::UnkeyedRawVersions { table }),
            None => Ok(()),
        }
    }

    /// Refuse a store whose log records are partitioned by `timestamp`, as one created before they stopped being
    /// is: `timestamp` falls back to the receipt, so a record delivered again in another month reads twice
    /// (`OTEL_LOGS_PARTITION` in `schema/logs.rs`).
    async fn refuse_receipt_partitioned_logs(&self) -> Result<(), ClickhouseError> {
        let partitioned: Vec<(String, String)> = self
            .client
            .query(
                "SELECT name, partition_key FROM system.tables \
                 WHERE database = currentDatabase() AND name IN ('otel_logs', 'otel_logs_local') \
                   AND partition_key = 'toYYYYMM(timestamp)'",
            )
            .fetch_all()
            .await
            .map_err(ClickhouseError::from)?;
        match partitioned.into_iter().next() {
            Some((table, partition_key)) => Err(ClickhouseError::IncompatibleLayout {
                table,
                partition_key,
            }),
            None => Ok(()),
        }
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
        let now = self.clock.now().timestamp();
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

    /// Start health check task
    pub fn start_health_check_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
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

    /// The age the explicit pass expires at when none shorter is configured: a day inside the schema's 90-day
    /// TTL, so the pass, running hourly, records a trace's cleanup before the TTL can reap its rows unrecorded -
    /// its spans' and its media-bearing logs', which the pass records together (`record_age_expiry`).
    const AGE_AHEAD_OF_TTL_MINUTES: u64 = 89 * 24 * 60;

    /// Start retention cleanup task
    #[allow(clippy::too_many_arguments)]
    pub fn start_retention_task(
        self: &Arc<Self>,
        config: RetentionConfig,
        quota_bytes: u64,
        mut shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<dyn RetentionFileReconciler>>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        governance: Arc<dyn StorageGovernance + Send + Sync>,
        fence: CollectionFence,
    ) -> Option<JoinHandle<()>> {
        // Always started, limits or not: the schema's own 90-day TTL removes rows too, and the age pass below
        // runs ahead of it so that what the TTL reaps has its cleanup recorded first - the files and raw-record
        // ownership of those traces are otherwise never released.
        let service = Arc::clone(self);
        tracing::debug!(
            max_spans = ?config.max_spans,
            max_age_minutes = ?config.max_age_minutes,
            "Starting ClickHouse retention task"
        );

        Some(tokio::spawn(async move {
            // ClickHouse handles TTL natively, but we may want manual cleanup for count-based limits
            let mut interval = tokio::time::interval(Duration::from_secs(3600)); // hourly
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            // Pre-compute table name and ON CLUSTER clause for retention cleanup
            let delete_table = service.delete_table("otel_spans");
            let metrics_table = service.delete_table("otel_metrics");
            let logs_table = service.delete_table("otel_logs");
            let on_cluster = service.on_cluster_clause();

            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            tracing::debug!("ClickHouse retention task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        // Retention deletes; none of it runs while this process's store lease is lost.
                        let Some(_pass) = fence.enter("retention") else {
                            tracing::debug!("Collection is fenced; retention waits");
                            continue;
                        };
                        match database
                            .claim_retention_cleanup(
                                Self::RETENTION_CLEANUP_CLAIM,
                                Self::RETENTION_CLEANUP_LEASE_SECS,
                            )
                            .await
                        {
                            Ok(claimed) if !claimed.is_empty() => {
                                // In order, projects and their traces, so the cleanup writes go out the same way
                                // every time: the claim returns its rows in no order of its own.
                                let mut claimed = claimed;
                                claimed.sort_unstable();
                                let mut by_project =
                                    std::collections::BTreeMap::<String, Vec<(String, i64)>>::new();
                                for (project_id, trace_id, token) in claimed {
                                    by_project
                                        .entry(project_id)
                                        .or_default()
                                        .push((trace_id, token));
                                }
                                let analytics = ClickhouseRepository(Arc::clone(&service));
                                for (project, tokens) in by_project {
                                    if !fence.is_open() {
                                        break;
                                    }
                                    let project_id = ProjectId::from(project);
                                    let cleanup_owner = format!(
                                        "clickhouse-cleanup:{}:{}",
                                        std::process::id(),
                                        service.clock.now().timestamp_micros()
                                    );
                                    let now = service.clock.now();
                                    let acquired = governance
                                        .acquire_project_maintenance(
                                            &project_id,
                                            &cleanup_owner,
                                            now,
                                            now + chrono::TimeDelta::hours(2),
                                        )
                                        .await
                                        .unwrap_or(false);
                                    if !acquired {
                                        continue;
                                    }
                                    let held = governance
                                        .active_project_hold(&project_id, service.clock.now())
                                        .await
                                        .map(|hold| hold.is_some())
                                        .unwrap_or(true);
                                    if !held {
                                        let trace_ids = tokens
                                            .iter()
                                            .map(|(trace_id, _)| trace_id.clone())
                                            .collect::<Vec<_>>();
                                        Self::finish_retention_cleanup(
                                            &analytics,
                                            &database,
                                            file_service.as_ref(),
                                            &project_id,
                                            &trace_ids,
                                            &tokens,
                                            &fence,
                                        )
                                        .await;
                                    }
                                    if let Err(error) = governance
                                        .release_project_maintenance(&project_id, &cleanup_owner)
                                        .await
                                    {
                                        tracing::warn!(
                                            project_id = %project_id,
                                            %error,
                                            "Could not release ClickHouse cleanup fence"
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => tracing::warn!(
                                %error,
                                "Could not claim outstanding ClickHouse retention cleanup"
                            ),
                        }

                        let owner = format!(
                            "clickhouse-retention:{}:{}",
                            std::process::id(),
                            service.clock.now().timestamp_micros()
                        );
                        let projects = match governance.storage_project_ids(usize::MAX).await {
                            Ok(projects) => projects,
                            Err(error) => {
                                tracing::warn!(%error, "Could not list projects for retention fencing");
                                continue;
                            }
                        };
                        for project_id in projects {
                            if !fence.is_open() {
                                break;
                            }
                            let lock_now = service.clock.now();
                            let lease_until = lock_now + chrono::TimeDelta::hours(2);
                            let acquired = match governance
                                .acquire_project_maintenance(
                                    &project_id,
                                    &owner,
                                    lock_now,
                                    lease_until,
                                )
                                .await
                            {
                                Ok(acquired) => acquired,
                                Err(error) => {
                                    tracing::warn!(project_id = %project_id, %error, "Could not acquire retention fence");
                                    false
                                }
                            };
                            if !acquired {
                                continue;
                            }

                            let held = match governance
                                .active_project_hold(&project_id, service.clock.now())
                                .await
                            {
                                Ok(hold) => hold.is_some(),
                                Err(error) => {
                                    tracing::warn!(
                                        project_id = %project_id,
                                        %error,
                                        "Could not check legal hold before ClickHouse retention"
                                    );
                                    true
                                }
                            };
                            if !held {
                                {
                                    // ClickHouse has native TTL, but this explicit pass is project-scoped
                                    // so it participates in the same legal-hold fence as hold patching, and
                                    // it runs a day ahead of that TTL when no shorter age is configured.
                                    let max_age_minutes = config
                                        .max_age_minutes
                                        .map_or(Self::AGE_AHEAD_OF_TTL_MINUTES, |minutes| {
                                            minutes.min(Self::AGE_AHEAD_OF_TTL_MINUTES)
                                        });
                                    let now = service.clock.now();
                                    let cutoff = now
                                        - chrono::Duration::minutes(
                                            i64::try_from(max_age_minutes).unwrap_or(i64::MAX),
                                        );
                                    let client = service.tenant_client(&project_id);
                                    // The intent first: a delete with no cleanup recorded would leave its
                                    // traces' files and raw-record ownership with nothing to release them.
                                    if let Err(error) = service
                                        .record_age_expiry(&project_id, cutoff, now, database.as_ref())
                                        .await
                                    {
                                        tracing::warn!(
                                            project_id = %project_id,
                                            %error,
                                            "Could not record the traces ClickHouse age retention expires; \
                                             the delete waits for the next pass"
                                        );
                                    } else if !fence.is_open() {
                                        tracing::debug!(project_id = %project_id, "Collection is fenced; retention waits");
                                    } else if let Err(error) = retention::run_retention(
                                        &client,
                                        &delete_table,
                                        &metrics_table,
                                        &logs_table,
                                        &on_cluster,
                                        project_id.as_str(),
                                        cutoff,
                                        now,
                                        &fence,
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            project_id = %project_id,
                                            %error,
                                            "ClickHouse age retention cleanup failed"
                                        );
                                    }
                                }
                                if let Some(max_spans) = config.max_spans
                                    && fence.is_open()
                                    && let Err(error) = Self::trim_project_to_span_limit(
                                        &service,
                                        max_spans,
                                        quota_bytes,
                                        &database,
                                        &governance,
                                        file_service.as_ref(),
                                        &project_id,
                                        &fence,
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        project_id = %project_id,
                                        %error,
                                        "ClickHouse count retention cleanup failed"
                                    );
                                }
                            }
                            if let Err(error) = governance
                                .release_project_maintenance(&project_id, &owner)
                                .await
                            {
                                tracing::warn!(project_id = %project_id, %error, "Could not release retention fence");
                            }
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
