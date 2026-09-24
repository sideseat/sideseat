//! Centralized shutdown management.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use crate::app::storage::{AnalyticsService, TransactionalService};
use sideseat_core::constants::SHUTDOWN_TIMEOUT_SECS;
use sideseat_messaging::TopicService;

/// Coordinates graceful shutdown across background tasks and storage backends.
#[derive(Clone)]
pub struct ShutdownService {
    tx: Arc<watch::Sender<bool>>,
    rx: watch::Receiver<bool>,
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    topics: Arc<TopicService>,
    database: Arc<TransactionalService>,
    analytics: Arc<AnalyticsService>,
}

impl ShutdownService {
    pub fn new(
        topics: Arc<TopicService>,
        database: Arc<TransactionalService>,
        analytics: Arc<AnalyticsService>,
    ) -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx: Arc::new(tx),
            rx,
            handles: Arc::new(Mutex::new(Vec::new())),
            topics,
            database,
            analytics,
        }
    }

    /// Register a background task handle to be awaited during shutdown.
    pub async fn register(&self, handle: JoinHandle<()>) {
        self.handles.lock().await.push(handle);
    }

    /// Subscribe to the shutdown signal.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }

    /// Trigger shutdown.
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    /// Trigger shutdown and wait for all registered tasks to complete.
    ///
    /// Shutdown order (to prevent data loss):
    /// 1. Signal all tasks to stop accepting new work
    /// 2. Wait for background tasks to finish processing pending work
    /// 3. Shutdown topic dispatchers (channels should be empty by now)
    /// 4. Checkpoint and close databases
    pub async fn shutdown(&self) {
        tracing::debug!("Initiating graceful shutdown...");
        self.trigger();

        // Wait for background tasks FIRST to let them drain pending messages
        let handles = std::mem::take(&mut *self.handles.lock().await);
        let task_count = handles.len();
        tracing::debug!(
            count = task_count,
            "Waiting for background tasks to finish..."
        );

        drain_background_tasks(handles, Duration::from_secs(SHUTDOWN_TIMEOUT_SECS)).await;

        // Shutdown topic dispatchers AFTER tasks have finished
        tracing::debug!("Shutting down topic dispatchers...");
        self.topics.shutdown().await;

        // Checkpoint and close databases in parallel
        tracing::debug!("Closing database connections...");
        let database = self.database.clone();
        let analytics = self.analytics.clone();
        tokio::join!(
            async {
                if let Err(e) = database.checkpoint().await {
                    tracing::warn!("SQLite checkpoint failed: {}", e);
                }
                database.close().await;
                tracing::debug!("SQLite closed");
            },
            async {
                if let Err(e) = analytics.checkpoint().await {
                    tracing::warn!("DuckDB checkpoint failed: {}", e);
                }
                if let Err(e) = analytics.close().await {
                    tracing::warn!("DuckDB close failed: {}", e);
                }
                tracing::debug!("DuckDB closed");
            }
        );

        tracing::debug!("Shutdown complete");
    }

    /// Install OS signal handlers and trigger on Ctrl+C or SIGTERM.
    pub fn install_signal_handlers(&self) {
        let service = self.clone();
        tokio::spawn(async move {
            let ctrl_c = async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to install Ctrl+C handler");
            };

            #[cfg(unix)]
            let terminate = async {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to install SIGTERM handler")
                    .recv()
                    .await;
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();

            tokio::select! {
                _ = ctrl_c => tracing::debug!("Received Ctrl+C, shutting down"),
                _ = terminate => tracing::debug!("Received SIGTERM, shutting down"),
            }

            service.trigger();
        });
    }
}

async fn drain_background_tasks(mut handles: Vec<JoinHandle<()>>, timeout: Duration) {
    match tokio::time::timeout(timeout, futures::future::join_all(&mut handles)).await {
        Ok(results) => {
            for error in results.into_iter().filter_map(Result::err) {
                tracing::warn!(%error, "Background task exited abnormally");
            }
            tracing::debug!("All background tasks completed");
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs = timeout.as_secs(),
                "Timeout waiting for background tasks; aborting remaining tasks"
            );
            for handle in &handles {
                handle.abort();
            }
            let _ = futures::future::join_all(handles).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideseat_core::storage::AppStorage;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn make_shutdown() -> ShutdownService {
        use sideseat_core::config::{AnalyticsBackend, TransactionalBackend};

        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = temp_dir.keep();
        // Create subdirectories needed by services
        std::fs::create_dir_all(data_dir.join("sqlite")).unwrap();
        std::fs::create_dir_all(data_dir.join("duckdb")).unwrap();
        let storage = AppStorage::init_for_test(data_dir);
        let database = Arc::new(
            TransactionalService::init(
                TransactionalBackend::Sqlite,
                &storage,
                None,
                None,
                Arc::new(crate::runtime::clock::SystemClock),
            )
            .await
            .unwrap(),
        );
        let analytics = Arc::new(
            AnalyticsService::init(
                AnalyticsBackend::Duckdb,
                &storage,
                None,
                Arc::new(crate::runtime::clock::SystemClock),
            )
            .await
            .unwrap(),
        );
        let topics = Arc::new(TopicService::new(sideseat_adapter_topics::memory_backend()));
        ShutdownService::new(topics, database, analytics)
    }

    #[tokio::test]
    async fn subscriber_receives_shutdown() {
        let shutdown = make_shutdown().await;
        let rx = shutdown.subscribe();

        assert!(!*rx.borrow());
        shutdown.trigger();
        assert!(*rx.borrow());
    }

    #[tokio::test]
    async fn timed_out_background_tasks_are_cancelled_before_returning() {
        struct DropFlag(Arc<AtomicBool>);

        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            let _drop_flag = DropFlag(task_dropped);
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.await.unwrap();

        drain_background_tasks(vec![handle], Duration::ZERO).await;

        assert!(dropped.load(Ordering::SeqCst));
    }
}
