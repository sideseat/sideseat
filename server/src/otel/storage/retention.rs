//! Data retention management (time-based cleanup)

use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::otel::error::OtelError;

/// Retention manager for time-based data cleanup
/// Cleans up SQLite data older than retention_days
pub struct RetentionManager {
    pool: SqlitePool,
    retention_days: Option<u32>,
    check_interval: Duration,
}

impl RetentionManager {
    /// Create a new retention manager
    pub fn new(pool: SqlitePool, retention_days: Option<u32>, check_interval_secs: u64) -> Self {
        Self { pool, retention_days, check_interval: Duration::from_secs(check_interval_secs) }
    }

    /// Start the retention cleanup background task
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        // If no retention limit configured, don't run cleanup
        let Some(days) = self.retention_days else {
            debug!("Retention manager disabled (no retention.days configured)");
            // Just wait for shutdown
            let _ = shutdown.changed().await;
            return;
        };

        debug!("Starting retention manager: retention {} days", days);

        let mut interval = tokio::time::interval(self.check_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.cleanup().await {
                        warn!("Retention cleanup error: {}", e);
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        debug!("Retention manager shutting down");
                        break;
                    }
                }
            }
        }
    }

    /// Perform cleanup based on retention policy
    pub async fn cleanup(&self) -> Result<(), OtelError> {
        let Some(days) = self.retention_days else {
            return Ok(());
        };

        // Calculate cutoff time in nanoseconds
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(days as u64 * 24 * 60 * 60);
        let cutoff_ns =
            cutoff.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0);

        self.cleanup_sqlite(cutoff_ns).await
    }

    /// Clean up SQLite data older than cutoff time
    async fn cleanup_sqlite(&self, cutoff_ns: i64) -> Result<(), OtelError> {
        // Delete old spans
        let spans_deleted = sqlx::query("DELETE FROM spans WHERE start_time_ns < ?")
            .bind(cutoff_ns)
            .execute(&self.pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to delete old spans: {}", e)))?
            .rows_affected();

        // Delete old span events
        let events_deleted = sqlx::query("DELETE FROM span_events WHERE event_time_ns < ?")
            .bind(cutoff_ns)
            .execute(&self.pool)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to delete old events: {}", e)))?
            .rows_affected();

        // Delete old span attributes (orphaned by span deletion due to CASCADE)
        // Note: span_attributes has ON DELETE CASCADE, so they're auto-deleted

        // Delete traces with no remaining spans
        let traces_deleted = sqlx::query(
            "DELETE FROM traces WHERE trace_id NOT IN (SELECT DISTINCT trace_id FROM spans)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to delete orphan traces: {}", e)))?
        .rows_affected();

        // Delete sessions with no remaining traces
        let sessions_deleted = sqlx::query(
            "DELETE FROM sessions WHERE session_id NOT IN (SELECT DISTINCT session_id FROM traces WHERE session_id IS NOT NULL)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to delete orphan sessions: {}", e)))?
        .rows_affected();

        if spans_deleted > 0 || traces_deleted > 0 || sessions_deleted > 0 {
            info!(
                "Retention cleanup: deleted {} spans, {} events, {} traces, {} sessions",
                spans_deleted, events_deleted, traces_deleted, sessions_deleted
            );
        }

        Ok(())
    }
}
