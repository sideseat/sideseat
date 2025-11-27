//! Data retention management (FIFO cleanup)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::otel::error::OtelError;

/// Retention manager for FIFO data cleanup
pub struct RetentionManager {
    traces_dir: PathBuf,
    max_total_bytes: u64,
    retention_days: Option<u32>,
    check_interval: Duration,
}

impl RetentionManager {
    /// Create a new retention manager
    pub fn new(
        traces_dir: PathBuf,
        max_total_gb: u32,
        retention_days: Option<u32>,
        check_interval_secs: u64,
    ) -> Self {
        Self {
            traces_dir,
            max_total_bytes: (max_total_gb as u64) * 1024 * 1024 * 1024,
            retention_days,
            check_interval: Duration::from_secs(check_interval_secs),
        }
    }

    /// Start the retention cleanup background task
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        debug!(
            "Starting retention manager: max {}GB, retention {:?} days",
            self.max_total_bytes / (1024 * 1024 * 1024),
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
    pub async fn cleanup(&self) -> Result<(), OtelError> {
        // Collect all parquet files with their metadata
        let mut files = self.collect_files().await?;

        if files.is_empty() {
            return Ok(());
        }

        // Sort by modification time (oldest first)
        files.sort_by_key(|f| f.modified);

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
                if let Err(e) = tokio::fs::remove_file(&file.path).await {
                    warn!("Failed to delete {:?}: {}", file.path, e);
                } else {
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

    /// Check if a file should be deleted
    fn should_delete(&self, file: &FileInfo, current_total: u64) -> bool {
        // Check size-based retention
        if current_total > self.max_total_bytes {
            return true;
        }

        // Check time-based retention
        if let Some(days) = self.retention_days {
            let cutoff =
                std::time::SystemTime::now() - Duration::from_secs((days as u64) * 24 * 60 * 60);
            if file.modified < cutoff {
                return true;
            }
        }

        false
    }

    /// Collect all parquet files in the traces directory
    async fn collect_files(&self) -> Result<Vec<FileInfo>, OtelError> {
        let mut files = Vec::new();

        if !self.traces_dir.exists() {
            return Ok(files);
        }

        self.collect_files_recursive(&self.traces_dir, &mut files).await?;

        Ok(files)
    }

    /// Recursively collect parquet files
    async fn collect_files_recursive(
        &self,
        dir: &PathBuf,
        files: &mut Vec<FileInfo>,
    ) -> Result<(), OtelError> {
        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to read dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| OtelError::StorageError(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| OtelError::StorageError(format!("Failed to get metadata: {}", e)))?;

            if metadata.is_dir() {
                // Box to avoid deep recursion stack
                Box::pin(self.collect_files_recursive(&path, files)).await?;
            } else if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                let modified = metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push(FileInfo { path, size: metadata.len(), modified });
            }
        }

        Ok(())
    }
}

/// File information for retention decisions
struct FileInfo {
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
}
