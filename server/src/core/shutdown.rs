//! Centralized shutdown management

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use super::constants::SHUTDOWN_TIMEOUT_SECS;
use crate::data::topics::TopicService;
use crate::data::{AnalyticsService, TransactionalService};

/// Centralized shutdown service for coordinating graceful shutdown
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

    /// Register a background task handle to be awaited during shutdown
    pub async fn register(&self, handle: JoinHandle<()>) {
        self.handles.lock().await.push(handle);
    }

    /// Subscribe to shutdown signal
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }

    /// Trigger shutdown
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }

    /// Check if shutdown was triggered
    pub fn is_triggered(&self) -> bool {
        *self.rx.borrow()
    }

    /// Trigger shutdown and wait for all registered tasks to complete
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

        let timeout = Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
        match tokio::time::timeout(timeout, futures::future::join_all(handles)).await {
            Ok(_) => {
                tracing::debug!("All background tasks completed");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_secs = timeout.as_secs(),
                    "Timeout waiting for background tasks"
                );
            }
        }

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

    /// Wait for shutdown signal (for use with axum graceful shutdown)
    /// Returns an owned future that can be passed to graceful_shutdown
    pub fn wait(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.rx.clone();
        async move {
            let _ = rx.wait_for(|&v| v).await;
        }
    }

    /// Install OS signal handlers and auto-trigger on Ctrl+C/SIGTERM
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::AppStorage;

    async fn make_shutdown() -> ShutdownService {
        use crate::core::config::{AnalyticsBackend, TransactionalBackend};

        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = temp_dir.keep();
        // Create subdirectories needed by services
        std::fs::create_dir_all(data_dir.join("sqlite")).unwrap();
        std::fs::create_dir_all(data_dir.join("duckdb")).unwrap();
        let storage = AppStorage::init_for_test(data_dir);
        let database = Arc::new(
            TransactionalService::init(TransactionalBackend::Sqlite, &storage, None)
                .await
                .unwrap(),
        );
        let analytics = Arc::new(
            AnalyticsService::init(AnalyticsBackend::Duckdb, &storage, None)
                .await
                .unwrap(),
        );
        let topics = Arc::new(TopicService::new());
        ShutdownService::new(topics, database, analytics)
    }

    #[tokio::test]
    async fn test_shutdown_not_triggered_initially() {
        let shutdown = make_shutdown().await;
        assert!(!shutdown.is_triggered());
    }

    #[tokio::test]
    async fn test_shutdown_trigger() {
        let shutdown = make_shutdown().await;
        shutdown.trigger();
        assert!(shutdown.is_triggered());
    }

    #[tokio::test]
    async fn test_shutdown_wait_returns_after_trigger() {
        let shutdown = make_shutdown().await;
        let wait_future = shutdown.wait();

        let handle = tokio::spawn(wait_future);

        tokio::task::yield_now().await;

        shutdown.trigger();

        tokio::time::timeout(std::time::Duration::from_millis(100), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn test_subscriber_receives_shutdown() {
        let shutdown = make_shutdown().await;
        let rx = shutdown.subscribe();

        assert!(!*rx.borrow());
        shutdown.trigger();
        assert!(*rx.borrow());
    }
}
