//! Disk space monitoring

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::watch;

use super::error::OtelResult;

pub struct DiskMonitor {
    storage_path: PathBuf,
    warning_percent: u8,
    critical_percent: u8,
    is_critical: AtomicBool,
    is_warning: AtomicBool,
}

impl DiskMonitor {
    pub fn new(storage_path: PathBuf, warning_percent: u8, critical_percent: u8) -> Self {
        Self {
            storage_path,
            warning_percent,
            critical_percent,
            is_critical: AtomicBool::new(false),
            is_warning: AtomicBool::new(false),
        }
    }

    /// Check if ingestion should be paused
    pub fn should_pause_ingestion(&self) -> bool {
        self.is_critical.load(Ordering::SeqCst)
    }

    /// Get current disk usage percent (cross-platform)
    pub fn get_usage_percent(&self) -> OtelResult<u8> {
        let total = fs2::total_space(&self.storage_path)?;
        let available = fs2::available_space(&self.storage_path)?;
        let used = total.saturating_sub(available);
        Ok(((used as f64 / total as f64) * 100.0) as u8)
    }

    /// Background monitoring task
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match self.get_usage_percent() {
                        Ok(usage) => {
                            let was_critical = self.is_critical.load(Ordering::SeqCst);
                            let was_warning = self.is_warning.load(Ordering::SeqCst);

                            if usage >= self.critical_percent {
                                self.is_critical.store(true, Ordering::SeqCst);
                                if !was_critical {
                                    tracing::error!(
                                        "DISK CRITICAL: {}% used - ingestion paused",
                                        usage
                                    );
                                }
                            } else {
                                self.is_critical.store(false, Ordering::SeqCst);
                            }

                            if usage >= self.warning_percent && usage < self.critical_percent {
                                self.is_warning.store(true, Ordering::SeqCst);
                                if !was_warning {
                                    tracing::warn!("DISK WARNING: {}% used", usage);
                                }
                            } else {
                                self.is_warning.store(false, Ordering::SeqCst);
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to check disk usage: {}", e);
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}
