//! Centralized shutdown management.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use crate::app::storage::{AnalyticsService, TransactionalService};
use sideseat_core::constants::SHUTDOWN_TIMEOUT_SECS;
use sideseat_messaging::TopicService;

/// How long a server whose analytics store failed for good may drain before it exits anyway.
pub const FATAL_DRAIN_DEADLINE: Duration = Duration::from_secs(30);

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

    /// Shut the server down when the analytics store fails for good - also while it is already shutting down.
    ///
    /// The store refuses everything after such a failure, and only a new process opens it again: draining and
    /// exiting - non-zero, see [`Self::fatal_failure`] - lets a supervisor start one, where serving on would
    /// answer every request with an error for as long as the process lived. Draining is bounded: every request
    /// that reaches the store fails at once, so one still open after [`FATAL_DRAIN_DEADLINE`] is a client that
    /// stopped sending, and the process exits without it - as from a crash, which every store survives.
    ///
    /// Not a registered task: shutdown waits for those, and this one outlives shutdown on purpose.
    pub fn stop_on_fatal_failure(&self) -> JoinHandle<()> {
        self.watch_fatal_failure(FATAL_DRAIN_DEADLINE, || std::process::exit(1))
    }

    fn watch_fatal_failure(
        &self,
        deadline: Duration,
        exit: impl FnOnce() + Send + 'static,
    ) -> JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            let failure = service.analytics.wait_for_fatal_failure().await;
            tracing::error!(%failure, "The analytics store failed for good; shutting down to restart it");
            service.trigger();
            tokio::time::sleep(deadline).await;
            tracing::error!(
                deadline_secs = deadline.as_secs(),
                "The server did not drain in time after the analytics store failed; exiting now"
            );
            exit();
        })
    }

    /// The analytics store's fatal failure, if it had one: a server that stopped for it exits with an error.
    pub fn fatal_failure(&self) -> Option<String> {
        self.analytics.fatal_failure()
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

    /// Make the analytics store fail for good, as a checkpoint that cannot complete does.
    async fn fail_analytics(shutdown: &ShutdownService) {
        let AnalyticsService::Duckdb(duckdb) = shutdown.analytics.as_ref() else {
            panic!("embedded analytics");
        };
        duckdb
            .write(|conn| {
                conn.execute_batch(
                    "CREATE TABLE checkpointed (n INTEGER); INSERT INTO checkpointed VALUES (1); \
                     SET debug_checkpoint_abort = 'before_header'",
                )
                .map_err(Into::into)
            })
            .expect("the setup");
        assert!(shutdown.analytics.checkpoint().await.is_err());
    }

    /// A fatal failure of the analytics store shuts the server down, is reported through the port health reads
    /// and as the failure the server exits on, and ends the process if draining outlasts the deadline.
    #[tokio::test]
    async fn a_fatal_analytics_failure_shuts_the_server_down() {
        let shutdown = make_shutdown().await;
        let exited = Arc::new(AtomicBool::new(false));
        let watcher = shutdown.watch_fatal_failure(Duration::from_millis(50), {
            let exited = Arc::clone(&exited);
            move || exited.store(true, Ordering::SeqCst)
        });
        fail_analytics(&shutdown).await;

        let mut stopping = shutdown.subscribe();
        tokio::time::timeout(Duration::from_secs(5), stopping.wait_for(|stop| *stop))
            .await
            .expect("the server shuts down")
            .expect("the shutdown signal");
        tokio::time::timeout(Duration::from_secs(5), watcher)
            .await
            .expect("the deadline passes")
            .expect("the watcher");
        assert!(exited.load(Ordering::SeqCst), "the process is ended");
        assert!(
            shutdown
                .fatal_failure()
                .is_some_and(|failure| failure.contains("Failed to create checkpoint")),
            "the failure the server exits on"
        );
        assert!(
            shutdown.analytics.repository().fatal_failure().is_some(),
            "the failure health reads"
        );
    }

    /// A failure while the server is already shutting down for another reason still bounds the drain.
    #[tokio::test]
    async fn a_failure_during_an_ordinary_shutdown_still_ends_the_process() {
        let shutdown = make_shutdown().await;
        let exited = Arc::new(AtomicBool::new(false));
        let watcher = shutdown.watch_fatal_failure(Duration::from_millis(50), {
            let exited = Arc::clone(&exited);
            move || exited.store(true, Ordering::SeqCst)
        });
        shutdown.trigger();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !watcher.is_finished(),
            "the watch outlives the shutdown signal"
        );
        fail_analytics(&shutdown).await;
        tokio::time::timeout(Duration::from_secs(5), watcher)
            .await
            .expect("the deadline passes")
            .expect("the watcher");
        assert!(exited.load(Ordering::SeqCst), "the process is ended");
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
