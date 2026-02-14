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
mod retention;
pub mod schema;
pub mod sql_types;

// Re-export repositories for convenient access
pub use repositories::metric as metric_repository;
pub use repositories::query as query_repository;
pub use repositories::span as span_repository;
pub use repositories::stats as stats_repository;

pub use models::{
    AggregationTemporality, Framework, MessageCategory, MessageSourceType, MetricType,
    NormalizedMetric, NormalizedSpan, ObservationType, SpanCategory,
};

pub use error::DuckdbError;

use std::sync::Arc;
use std::time::Duration;

use duckdb::Connection;
use parking_lot::{Mutex, MutexGuard};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::core::config::RetentionConfig;
use crate::core::constants::{
    DUCKDB_CHECKPOINT_INTERVAL_SECS, DUCKDB_DB_FILENAME, DUCKDB_QUERY_TIMEOUT_SECS,
    DUCKDB_RETENTION_INTERVAL_SECS,
};
use crate::core::storage::{AppStorage, DataSubdir};

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
    /// Initialize the analytics service with a single connection
    pub async fn init(storage: &AppStorage) -> Result<Self, DuckdbError> {
        let db_path = storage.subdir(DataSubdir::Duckdb).join(DUCKDB_DB_FILENAME);

        let conn = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            conn.execute_batch(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET extension_directory = '';
                 SET force_compression = 'auto';
                 PRAGMA enable_checkpoint_on_shutdown;
                 LOAD json;",
            )?;
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

    pub fn start_retention_task(
        self: &Arc<Self>,
        config: RetentionConfig,
        mut shutdown_rx: watch::Receiver<bool>,
        file_service: Option<Arc<crate::data::files::FileService>>,
        database: Arc<crate::data::TransactionalService>,
    ) -> Option<JoinHandle<()>> {
        if config.max_spans.is_none() && config.max_age_minutes.is_none() {
            tracing::debug!("Retention disabled (no limits configured)");
            return None;
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
                        match db.run_retention(&config).await {
                            Ok(result) => {
                                // Async cleanup (outside DuckDB transaction)
                                for (project_id, trace_ids) in &result.trace_ids_by_project {
                                    // File cleanup
                                    if let Some(ref fs) = file_service
                                        && fs.is_enabled()
                                        && let Err(e) = fs.cleanup_traces(project_id, trace_ids).await
                                    {
                                        tracing::warn!(
                                            error = %e,
                                            project_id,
                                            traces = trace_ids.len(),
                                            "Failed to cleanup files during retention"
                                        );
                                    }

                                    // Favorites cleanup
                                    let repo = database.repository();
                                    if let Err(e) = repo.delete_favorites_by_entity(
                                        "trace",
                                        trace_ids,
                                        project_id,
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            error = %e,
                                            project_id,
                                            traces = trace_ids.len(),
                                            "Failed to cleanup favorites during retention"
                                        );
                                    }
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
    ) -> Result<retention::RetentionResult, DuckdbError> {
        tracing::debug!("Running retention check");
        let db = Arc::clone(self);
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn();
            retention::run_retention(&conn, &config)
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
