//! Data retention management (FIFO cleanup)

use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, warn};

use super::sqlite::files;
use crate::otel::error::OtelError;

/// Retention manager for FIFO data cleanup
/// Uses span timestamps (min_start_time_ns) for age-based retention, not file mtime
pub struct RetentionManager {
    max_total_bytes: u64,
    retention_days: Option<u32>,
    check_interval: Duration,
    pool: SqlitePool,
}

impl RetentionManager {
    /// Create a new retention manager
    pub fn new(
        max_total_mb: u32,
        retention_days: Option<u32>,
        check_interval_secs: u64,
        pool: SqlitePool,
    ) -> Self {
        Self {
            max_total_bytes: (max_total_mb as u64) * 1024 * 1024,
            retention_days,
            check_interval: Duration::from_secs(check_interval_secs),
            pool,
        }
    }

    /// Start the retention cleanup background task
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        debug!(
            "Starting retention manager: max {}MB, retention {:?} days",
            self.max_total_bytes / (1024 * 1024),
            self.retention_days
        );

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
    /// Uses span timestamps from database for accurate age-based retention
    pub async fn cleanup(&self) -> Result<(), OtelError> {
        // Query parquet files from database with their span timestamps
        let mut files = self.collect_files_from_db().await?;

        if files.is_empty() {
            return Ok(());
        }

        // Sort by min_start_time_ns (oldest spans first)
        files.sort_by_key(|f| f.min_start_time_ns);

        // Calculate total size
        let total_bytes: u64 = files.iter().map(|f| f.size).sum();

        let mut deleted_count = 0;
        let mut deleted_bytes = 0u64;
        let mut remaining_bytes = total_bytes;

        // Delete files based on retention policy
        for file in &files {
            let should_delete = self.should_delete(file, remaining_bytes);

            if should_delete {
                debug!("Deleting old trace file: {:?}", file.path);
                let file_path_str = file.path.to_str().unwrap_or("");

                // Delete parquet file from disk FIRST
                // Only clean up SQLite if disk delete succeeds (or file already gone)
                let disk_result = tokio::fs::remove_file(&file.path).await;
                let disk_ok = match &disk_result {
                    Ok(()) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // File already deleted, still clean up SQLite
                        debug!("Parquet file already gone: {:?}", file.path);
                        true
                    }
                    Err(e) => {
                        warn!("Failed to delete parquet file {:?}: {}", file.path, e);
                        false
                    }
                };

                // Only clean up SQLite metadata if disk delete succeeded
                if disk_ok {
                    if let Err(e) = files::remove_file(&self.pool, file_path_str).await {
                        warn!("Failed to cleanup SQLite for {:?}: {}", file.path, e);
                        // SQLite cleanup failed but file is gone - will be orphan record
                        // Next cleanup will retry SQLite deletion
                    }
                    deleted_count += 1;
                    deleted_bytes += file.size;
                    remaining_bytes -= file.size;
                }
            }
        }

        if deleted_count > 0 {
            debug!(
                "Retention cleanup: deleted {} files ({} MB)",
                deleted_count,
                deleted_bytes / (1024 * 1024)
            );
        }

        Ok(())
    }

    /// Check if a file should be deleted based on retention policy
    fn should_delete(&self, file: &FileInfo, current_total: u64) -> bool {
        // Check size-based retention
        if current_total > self.max_total_bytes {
            return true;
        }

        // Check time-based retention using span timestamps
        if let Some(days) = self.retention_days {
            let cutoff_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                - (days as i64) * 24 * 60 * 60 * 1_000_000_000;
            if file.min_start_time_ns < cutoff_ns {
                return true;
            }
        }

        false
    }

    /// Query parquet files from database with span timestamps
    async fn collect_files_from_db(&self) -> Result<Vec<FileInfo>, OtelError> {
        let rows = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT file_path, file_size_bytes, min_start_time_ns FROM parquet_files ORDER BY min_start_time_ns",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to query parquet files: {}", e)))?;

        Ok(rows
            .into_iter()
            .map(|(file_path, size, min_start_time_ns)| FileInfo {
                path: PathBuf::from(file_path),
                size: size as u64,
                min_start_time_ns,
            })
            .collect())
    }
}

/// File information for retention decisions
struct FileInfo {
    path: PathBuf,
    size: u64,
    /// Minimum span start time in nanoseconds (used for age-based retention)
    min_start_time_ns: i64,
}
