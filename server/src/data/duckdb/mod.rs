//! DuckDB analytics service
//!
//! Provides centralized analytics database management for the server.
//! All schema definitions and migrations are managed here.

pub mod error;
pub mod filters;
mod migrations;
pub mod models;
pub mod repositories;
mod repository_impl;
pub use repository_impl::DuckdbRepository;
mod retention;
pub mod schema;
pub mod sql_types;

// Re-export repositories for convenient access
pub use repositories::metric as metric_repository;
pub use repositories::query as query_repository;
pub use repositories::span as span_repository;
pub use repositories::stats as stats_repository;

pub use models::{
    AggregationTemporality, MessageCategory, MessageSourceType, MetricType, NormalizedMetric,
    NormalizedSpan, ObservationType, SpanCategory,
};

pub use error::DuckdbError;

use std::sync::Arc;
use std::time::Duration;

use duckdb::Connection;
use parking_lot::{Mutex, MutexGuard};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use sideseat_core::core::config::RetentionConfig;
use sideseat_core::core::constants::{
    DUCKDB_CHECKPOINT_INTERVAL_SECS, DUCKDB_DB_FILENAME, DUCKDB_MEMORY_LIMIT_BYTES,
    DUCKDB_QUERY_TIMEOUT_SECS, DUCKDB_RETENTION_INTERVAL_SECS,
};
use sideseat_core::core::storage::{AppStorage, DataSubdir};

/// DuckDB analytics service
///
/// Handles database initialization and background tasks.
/// Uses a single shared connection protected by a mutex.
pub struct DuckdbService {
    conn: Mutex<Option<Connection>>,
}

impl Drop for DuckdbService {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.get_mut().take() {
            // Best-effort close - log but don't panic on error
            if let Err((_, e)) = conn.close() {
                tracing::warn!("DuckDB connection close failed during drop: {}", e);
            }
        }
    }
}

impl DuckdbService {
    /// Physical metric row count for a project. Test-only: production counts through
    /// `count_project_rows`, and the point of this is to see the rows *behind* that answer - a count that
    /// deduplicates would pass while the table held two rows for one datapoint.
    #[cfg(test)]
    pub async fn count_metric_rows_for_test(
        self: &Arc<Self>,
        project_id: &str,
    ) -> Result<u64, DuckdbError> {
        let db = Arc::clone(self);
        let id = project_id.to_string();
        Self::run_query(move || {
            let conn = db.conn();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM otel_metrics WHERE project_id = ?",
                [id.as_str()],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        })
        .await?
    }

    /// Initialize the analytics service with a single connection
    pub async fn init(storage: &AppStorage) -> Result<Self, DuckdbError> {
        let db_path = storage.subdir(DataSubdir::Duckdb).join(DUCKDB_DB_FILENAME);

        // The engine participates in the process's footprint ceiling rather than sizing itself from the host.
        //
        // DuckDB's default `memory_limit` is 80% of physical RAM, so on any ordinary server the embedded
        // engine alone is allowed two orders of magnitude more than the whole process is supposed to use -
        // which makes the ceiling a statement about everything except the component most likely to breach it.
        //
        // `temp_directory` goes with it and is the half that makes it safe: DuckDB's hash aggregates, sorts
        // and window functions are out-of-core, so a tight limit costs latency on a large read rather than
        // failing it - but only if there is somewhere to spill. Left unset it defaults to a location derived
        // from the database path, which is usually right and is not something to leave to chance when the
        // limit is deliberately tight. Pointed at the DuckDB subdirectory, which SideSeat owns and creates.
        let temp_dir = storage.subdir(DataSubdir::Duckdb);
        let conn = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            // Doubled single quotes, because a path is not a literal until it is escaped and a user's data
            // directory may contain an apostrophe. `SET` takes no bind parameters, so this is the escape.
            let temp_dir_literal = temp_dir.display().to_string().replace('\'', "''");
            conn.execute_batch(&format!(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET extension_directory = '';
                 SET force_compression = 'auto';
                 SET memory_limit = '{limit}B';
                 SET temp_directory = '{temp_dir_literal}';
                 PRAGMA enable_checkpoint_on_shutdown;
                 LOAD json;",
                limit = DUCKDB_MEMORY_LIMIT_BYTES,
            ))?;
            Ok::<_, duckdb::Error>(conn)
        })
        .await
        .map_err(|e| DuckdbError::Io(std::io::Error::other(e)))??;

        migrations::run_migrations(&conn)?;

        tracing::debug!(path = %storage.subdir(DataSubdir::Duckdb).join(DUCKDB_DB_FILENAME).display(), "DuckdbService initialized");
        Ok(Self {
            conn: Mutex::new(Some(conn)),
        })
    }

    /// Get exclusive access to the connection.
    ///
    /// # Panics
    /// Panics if the connection has been closed via `close()`.
    pub fn conn(&self) -> parking_lot::MappedMutexGuard<'_, Connection> {
        MutexGuard::map(self.conn.lock(), |opt| {
            opt.as_mut()
                .expect("DuckDB connection already closed - do not call conn() after close()")
        })
    }

    /// Check if the connection is still open (test utility only)
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.conn.lock().is_some()
    }

    /// Run a blocking DuckDB query with timeout
    pub async fn run_query<T, F>(f: F) -> Result<T, DuckdbError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let timeout = Duration::from_secs(DUCKDB_QUERY_TIMEOUT_SECS);
        tokio::time::timeout(timeout, tokio::task::spawn_blocking(f))
            .await
            .map_err(|_| {
                tracing::warn!(
                    "DuckDB query timed out after {}s",
                    DUCKDB_QUERY_TIMEOUT_SECS
                );
                DuckdbError::Timeout {
                    timeout_secs: DUCKDB_QUERY_TIMEOUT_SECS,
                }
            })?
            .map_err(|e| {
                tracing::error!(error = %e, "DuckDB query task failed");
                DuckdbError::Io(std::io::Error::other(format!(
                    "Query execution failed: {}",
                    e
                )))
            })
    }

    /// Run a checkpoint to flush WAL to the main database file.
    ///
    /// Returns `Ok(())` if the connection is already closed (no-op).
    pub async fn checkpoint(self: &Arc<Self>) -> Result<(), DuckdbError> {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let conn_guard = db.conn.lock();
            if let Some(ref conn) = *conn_guard {
                conn.execute("CHECKPOINT", [])?;
                tracing::debug!("DuckDB checkpoint completed");
            }
            Ok(())
        })
        .await
        .map_err(|e| DuckdbError::Io(std::io::Error::other(e)))?
    }

    /// Close the DuckDB connection gracefully with explicit error handling
    pub async fn close(self: Arc<Self>) -> Result<(), DuckdbError> {
        tokio::task::spawn_blocking(move || {
            let mut conn_guard = self.conn.lock();
            if let Some(conn) = conn_guard.take() {
                // Best-effort checkpoint before close - log but don't fail on error
                if let Err(e) = conn.execute("CHECKPOINT", []) {
                    tracing::warn!("CHECKPOINT failed during close: {}", e);
                }
                conn.close().map_err(|(_, e)| DuckdbError::Database(e))?;
                tracing::debug!("DuckDB connection closed");
            }
            Ok(())
        })
        .await
        .map_err(|e| DuckdbError::Io(std::io::Error::other(e)))?
    }

    pub fn start_checkpoint_task(
        self: &Arc<Self>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let db = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(DUCKDB_CHECKPOINT_INTERVAL_SECS));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("DuckDB checkpoint task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = db.checkpoint().await {
                            tracing::warn!("DuckDB checkpoint failed: {}", e);
                        }
                    }
                }
            }
        })
    }

    /// Candidates claimed per cycle, and how long a claim holds them.
    ///
    /// The lease exists so a slow reconciliation is not re-claimed while it runs, which is what turns several
    /// replicas draining the same table into several replicas draining different rows.
    const RETENTION_CLEANUP_CLAIM: i64 = 256;
    const RETENTION_CLEANUP_LEASE_SECS: i64 = 600;

    /// Reconcile a set of traces' files and favourites, and drop their cleanup records.
    ///
    /// One place, used by the retention cycle *and* by the crash-recovery claim above, because two copies of
    /// this would drift - and the recovery path is the one nobody exercises by hand.
    ///
    /// The record is removed **only on success**, and each half is independent: a failure in either leaves the
    /// record in place with its backoff advanced, so the work is owed rather than lost. That is the whole point
    /// of recording the intent before the delete.
    async fn finish_retention_cleanup(
        analytics: &super::duckdb::DuckdbRepository,
        database: &Arc<crate::data::TransactionalService>,
        file_service: Option<&Arc<crate::data::files::FileService>>,
        project_id: &str,
        trace_ids: &[String],
        claimed: Option<&[(String, i64)]>,
    ) {
        let mut clean = true;

        // Survivor reconciliation, **not** the trace-wide cleanup. Retention expires individual span
        // identities, so a trace it touched usually still has live spans; `cleanup_traces` removes *every*
        // association for a trace, which left those survivors pointing at bytes that had been reclaimed.
        // A **disabled** file service is not a completed cleanup. Skipping the reconciliation while leaving
        // `clean` true deleted the durable record, so associations recorded while files were enabled could never
        // be recovered by re-enabling them - the record is the only thing that knows they are owed.
        match file_service {
            Some(fs) if fs.is_enabled() => {}
            _ => {
                clean = false;
                tracing::debug!(
                    project_id,
                    traces = trace_ids.len(),
                    "File storage is disabled, so this cleanup stays recorded rather than being marked done"
                );
            }
        }

        if let Some(fs) = file_service
            && fs.is_enabled()
            && let Err(e) = fs
                .reconcile_trace_survivors(project_id, trace_ids, analytics)
                .await
        {
            clean = false;
            tracing::warn!(
                error = %e,
                project_id,
                traces = trace_ids.len(),
                "Failed to reconcile files during retention; the cleanup stays recorded and is retried"
            );
        }

        // Favourites are keyed on the **trace**, so only a trace that is actually gone may lose one. Retention
        // expires span identities, not traces, so "was in the retention batch" is not that: a favourited trace
        // with one expired span and one live span stayed visible and lost its favourite anyway - the same defect
        // as the trace-wide file cleanup, in the call beside it.
        //
        // **Stated residual: this is a read-then-act pair and cannot be compensated.** A span for T can commit
        // between `traces_without_spans` reporting T empty and the delete, and the favourite of a live trace is
        // then removed. The file path solves the same race with a re-check that *restores* what it released;
        // there is nothing to restore here, because a favourite records which user marked it and deleting the row
        // destroys that. Narrowing the window is all that is available, so the check is immediately before the
        // delete. The cost of getting it wrong is a lost bookmark rather than lost telemetry, which is why this
        // is stated rather than engineered around - the alternative is never removing a favourite, and a
        // favourite pointing at a deleted trace is its own defect.
        let repo = database.repository();
        match sideseat_ports::traits::EntityQuery::traces_without_spans(
            analytics, project_id, trace_ids,
        )
        .await
        {
            Ok(emptied) if !emptied.is_empty() => {
                if let Err(e) = repo
                    .delete_favorites_by_entity("trace", &emptied, project_id)
                    .await
                {
                    clean = false;
                    tracing::warn!(
                        error = %e,
                        project_id,
                        traces = emptied.len(),
                        "Failed to cleanup favorites during retention; the cleanup stays recorded and is \
                         retried"
                    );
                }
            }
            Ok(_) => {}
            Err(e) => {
                clean = false;
                tracing::warn!(
                    error = %e,
                    project_id,
                    "Could not tell which traces retention emptied, so no favourite is removed - the cleanup \
                     stays recorded and is retried"
                );
            }
        }

        // Completed only on the tokens actually held - the claim's, or the ones the recording pass wrote. With
        // none, the record is left for the sweep rather than deleted on a guess.
        let Some(completed) = claimed else {
            return;
        };

        if clean && let Err(e) = repo.complete_retention_cleanup(project_id, completed).await {
            // Harmless: the record is idempotent work, so a stale one costs one extra reconciliation.
            tracing::debug!(
                error = %e,
                project_id,
                "Could not drop completed retention cleanup records"
            );
        }
    }

    pub fn start_retention_task(
        self: &Arc<Self>,
        config: RetentionConfig,
        mut shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<crate::data::files::FileService>>,
        database: Arc<crate::data::TransactionalService>,
    ) -> Option<JoinHandle<()>> {
        // **The task starts even with no limits configured**, because it is also the only thing that drains
        // outstanding cleanup. A deployment that crashed mid-cleanup and restarted with retention switched off
        // would otherwise leave those associations, ref-counts and favourites owed forever - and the record is
        // the only thing that knows about them, so nothing else can find them. Retention itself is skipped in
        // that case; the claim loop is not.
        let retention_configured = config.max_spans.is_some() || config.max_age_minutes.is_some();
        if !retention_configured {
            tracing::debug!(
                "Retention has no limits configured; the task still runs to drain any outstanding cleanup"
            );
        }

        let db = Arc::clone(self);
        tracing::debug!(
            max_spans = ?config.max_spans,
            max_age_minutes = ?config.max_age_minutes,
            "Starting retention task"
        );

        Some(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(DUCKDB_RETENTION_INTERVAL_SECS));
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("Retention task shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        // First, whatever a previous cycle recorded and did not finish. Claimed and leased, so
                        // several instances drain different candidates; the reconciliation is idempotent, so
                        // acting on one whose deletion never committed is a no-op rather than damage - which is
                        // why this needs no state machine to tell the two apart.
                        match database.repository()
                            .claim_retention_cleanup(Self::RETENTION_CLEANUP_CLAIM, Self::RETENTION_CLEANUP_LEASE_SECS)
                            .await
                        {
                            Ok(claimed) if !claimed.is_empty() => {
                                // Grouped with their **claim tokens**, because completion is conditional on
                                // them: a stale worker that finished old work must not delete a newer intent.
                                let mut by_project: std::collections::HashMap<String, Vec<(String, i64)>> =
                                    std::collections::HashMap::new();
                                for (project_id, trace_id, token) in claimed {
                                    by_project
                                        .entry(project_id)
                                        .or_default()
                                        .push((trace_id, token));
                                }
                                tracing::debug!(
                                    projects = by_project.len(),
                                    "Resuming retention cleanup a previous cycle did not finish"
                                );
                                for (project_id, claimed) in &by_project {
                                    let trace_ids: Vec<String> =
                                        claimed.iter().map(|(t, _)| t.clone()).collect();
                                    Self::finish_retention_cleanup(
                                        &DuckdbRepository(Arc::clone(&db)),
                                        &database,
                                        file_service.as_ref(),
                                        project_id,
                                        &trace_ids,
                                        Some(claimed),
                                    )
                                    .await;
                                }
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!(
                                error = %e,
                                "Could not claim outstanding retention cleanup; it stays recorded and is \
                                 retried next cycle"
                            ),
                        }

                        if !retention_configured {
                            continue;
                        }

                        match db.run_retention(&config, &database).await {
                            Ok(result) => {
                                // Async cleanup (outside DuckDB transaction)
                                for (project_id, trace_ids) in &result.trace_ids_by_project {
                                    Self::finish_retention_cleanup(
                                        &DuckdbRepository(Arc::clone(&db)),
                                        &database,
                                        file_service.as_ref(),
                                        project_id,
                                        trace_ids,
                                        result.cleanup_tokens.get(project_id).map(|t| t.as_slice()),
                                    )
                                    .await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Retention cleanup failed: {}", e);
                            }
                        }
                    }
                }
            }
        }))
    }

    async fn run_retention(
        self: &Arc<Self>,
        config: &RetentionConfig,
        database: &Arc<crate::data::TransactionalService>,
    ) -> Result<retention::RetentionResult, DuckdbError> {
        tracing::debug!("Running retention check");
        let db = Arc::clone(self);
        let config = config.clone();
        let database = Arc::clone(database);
        // The current runtime handle, taken here rather than inside the blocking closure: `Handle::current`
        // panics off-runtime, and `spawn_blocking` threads are off it.
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn();
            // The recorder runs between selecting a batch and deleting it, so the intent is durable before the
            // spans go. `block_on` inside `spawn_blocking` is the legal direction - this thread is not a
            // runtime worker, so blocking it cannot stall the reactor - and the retention sweep is sync DuckDB
            // work that has to interleave with one async write to another store.
            //
            // A failure here **fails the batch**, deliberately: without a durable record the deletion would be
            // a loss with nothing able to find it afterwards, so not deleting is the correct outcome.
            let tokens: std::sync::Mutex<std::collections::HashMap<String, Vec<(String, i64)>>> =
                std::sync::Mutex::new(std::collections::HashMap::new());
            let record_intent = |by_project: &std::collections::HashMap<String, Vec<String>>| {
                for (project_id, trace_ids) in by_project {
                    let written = handle
                        .block_on(
                            database
                                .repository()
                                .record_retention_cleanup(project_id, trace_ids),
                        )
                        .map_err(|e| DuckdbError::Io(std::io::Error::other(e.to_string())))?;
                    // Kept, because completion is conditional on the token: guessing one either fails to
                    // complete - leaving a record the sweep re-drives - or matches a *newer* row and discards
                    // work someone else recorded.
                    tokens
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .entry(project_id.clone())
                        .or_default()
                        .extend(written);
                }
                Ok(())
            };
            let outcome = retention::run_retention(&conn, &config, &record_intent);
            let recorded = tokens.into_inner().unwrap_or_else(|e| e.into_inner());
            outcome.map(|mut result| {
                result.cleanup_tokens = recorded;
                result
            })
        })
        .await
        .map_err(|e| DuckdbError::Io(std::io::Error::other(e)))?
    }
}

/// Execute a function within a transaction, automatically rolling back on error.
pub(crate) fn in_transaction<F, T>(conn: &Connection, f: F) -> Result<T, DuckdbError>
where
    F: FnOnce(&Connection) -> Result<T, DuckdbError>,
{
    conn.execute_batch("BEGIN TRANSACTION")?;
    match f(conn) {
        Ok(val) => {
            conn.execute_batch("COMMIT")?;
            Ok(val)
        }
        Err(e) => {
            // Best-effort rollback - log but return original error
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK") {
                tracing::warn!("ROLLBACK failed after transaction error: {}", rollback_err);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_storage() -> (TempDir, AppStorage) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let duckdb_dir = temp_dir.path().join("duckdb");
        tokio::fs::create_dir_all(&duckdb_dir)
            .await
            .expect("Failed to create duckdb dir");
        let storage = AppStorage::init_for_test(temp_dir.path().to_path_buf());
        (temp_dir, storage)
    }

    #[tokio::test]
    async fn test_analytics_service_init() {
        let (_temp_dir, storage) = create_test_storage().await;
        let result = DuckdbService::init(&storage).await;
        assert!(
            result.is_ok(),
            "DuckdbService should initialize successfully"
        );
    }

    /// The engine's memory limit is the one this crate declares, not 80% of the host's RAM.
    ///
    /// Asserted rather than assumed, because DuckDB's default sizes itself from the machine: on any ordinary
    /// server that is two orders of magnitude above the whole process's footprint ceiling, so a ceiling with
    /// this `SET` missing or silently ignored would be a statement about everything except the component most
    /// likely to breach it. Reads the setting back through `current_setting`, since a `SET` DuckDB accepted and
    /// interpreted differently is indistinguishable from one that worked.
    #[tokio::test]
    async fn the_engine_takes_the_declared_memory_limit() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = DuckdbService::init(&storage)
            .await
            .expect("Init should succeed");

        let (limit, temp_dir): (String, String) = {
            let conn = service.conn();
            conn.query_row(
                "SELECT current_setting('memory_limit'), current_setting('temp_directory')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("both settings are readable")
        };

        // Compared as bytes rather than against a formatted string: DuckDB normalises the value it was given
        // ("200.0 MiB" for 209715200 bytes), and pinning its formatting would make this a test of DuckDB's
        // display code. Within 1 MiB, because that normalisation rounds.
        let reported = parse_duckdb_size(&limit)
            .unwrap_or_else(|| panic!("could not read a byte count out of {limit:?}"));
        let declared = DUCKDB_MEMORY_LIMIT_BYTES as f64;
        assert!(
            (reported - declared).abs() < 1_048_576.0,
            "the engine reports a {limit} limit, which is not the declared {declared} bytes"
        );

        assert!(
            !temp_dir.is_empty(),
            "a tight memory limit is only safe because DuckDB can spill, so it needs somewhere to spill to"
        );
    }

    /// Bytes from a DuckDB size string such as `200.0 MiB`, `1.5GB` or `1024`.
    fn parse_duckdb_size(value: &str) -> Option<f64> {
        let trimmed = value.trim();
        let split = trimmed
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(trimmed.len());
        let (number, unit) = trimmed.split_at(split);
        let number: f64 = number.parse().ok()?;
        let scale = match unit.trim().to_ascii_uppercase().as_str() {
            "" | "B" => 1.0,
            "KIB" | "KB" => 1024.0,
            "MIB" | "MB" => 1024.0 * 1024.0,
            "GIB" | "GB" => 1024.0 * 1024.0 * 1024.0,
            "TIB" | "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        };
        Some(number * scale)
    }

    #[tokio::test]
    async fn test_analytics_service_conn() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = DuckdbService::init(&storage)
            .await
            .expect("Init should succeed");

        let conn = service.conn();
        drop(conn); // Successfully acquired connection
    }

    #[tokio::test]
    async fn test_analytics_service_checkpoint() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = Arc::new(
            DuckdbService::init(&storage)
                .await
                .expect("Init should succeed"),
        );

        let result = service.checkpoint().await;
        assert!(result.is_ok(), "Checkpoint should succeed");
    }

    #[tokio::test]
    async fn test_analytics_service_schema_applied() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = DuckdbService::init(&storage)
            .await
            .expect("Init should succeed");

        let conn = service.conn();
        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("Should read schema version");

        assert_eq!(version, schema::SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_analytics_service_is_open() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = Arc::new(
            DuckdbService::init(&storage)
                .await
                .expect("Init should succeed"),
        );

        assert!(service.is_open(), "Connection should be open after init");
    }

    #[tokio::test]
    async fn test_analytics_service_close() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = Arc::new(
            DuckdbService::init(&storage)
                .await
                .expect("Init should succeed"),
        );

        assert!(service.is_open());
        let result = service.close().await;
        assert!(result.is_ok(), "Close should succeed");
    }

    #[tokio::test]
    async fn test_checkpoint_after_close_is_noop() {
        let (_temp_dir, storage) = create_test_storage().await;
        let service = Arc::new(
            DuckdbService::init(&storage)
                .await
                .expect("Init should succeed"),
        );

        // Clone before close since close consumes Arc
        let service_for_checkpoint = Arc::clone(&service);

        service.close().await.expect("Close should succeed");

        // Checkpoint after close should be a no-op, not panic
        let result = service_for_checkpoint.checkpoint().await;
        assert!(
            result.is_ok(),
            "Checkpoint after close should succeed as no-op"
        );
    }
}
