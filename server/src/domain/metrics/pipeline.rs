//! Metrics Processing Pipeline
//!
//! Subscribes to metrics topic, extracts and persists to DuckDB.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::extract::extract_metrics_batch;
use super::persist::persist_batch;
use crate::core::constants::DEFAULT_PROJECT_ID;
use crate::core::{Topic, TopicError};
use crate::data::AnalyticsService;

pub struct MetricsPipeline {
    analytics: Arc<AnalyticsService>,
    /// For the project deletion fence, which the trace pipeline applies next to its own write.
    database: Arc<crate::data::TransactionalService>,
}

impl MetricsPipeline {
    pub fn new(
        analytics: Arc<AnalyticsService>,
        database: Arc<crate::data::TransactionalService>,
    ) -> Self {
        Self {
            analytics,
            database,
        }
    }

    pub fn start(
        self,
        topic: Topic<ExportMetricsServiceRequest>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let mut subscriber = topic.subscribe();

        tokio::spawn(async move {
            let mut shutdown_requested = false;

            loop {
                if shutdown_requested {
                    // Drain remaining messages before shutdown
                    match tokio::time::timeout(Duration::from_millis(100), subscriber.recv()).await
                    {
                        Ok(Ok(msg)) => {
                            self.run(&msg).await;
                            continue;
                        }
                        Ok(Err(TopicError::Lagged(n))) => {
                            tracing::warn!(lagged = n, "MetricsPipeline lagged during drain");
                            continue;
                        }
                        _ => break,
                    }
                }

                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::debug!("MetricsPipeline received shutdown, draining...");
                            shutdown_requested = true;
                        }
                    }
                    result = subscriber.recv() => {
                        match result {
                            Ok(msg) => self.run(&msg).await,
                            Err(TopicError::Lagged(n)) => {
                                tracing::warn!(lagged = n, "MetricsPipeline lagged");
                            }
                            Err(TopicError::ChannelClosed) => break,
                            Err(_) => break,
                        }
                    }
                }
            }
            tracing::debug!("MetricsPipeline shutdown complete");
        })
    }

    async fn run(&self, request: &ExportMetricsServiceRequest) {
        let mut metrics = extract_metrics_batch(request);
        if metrics.is_empty() {
            return;
        }

        // The project deletion fence, which this pipeline did not apply at all. A metrics request
        // admitted before a project was deleted would otherwise persist after its cleanup, and metrics
        // are as unreachable as spans once the project row is gone - nothing finds them, nothing counts
        // them, and the deletion that was supposed to remove them has already run.
        //
        // Dropped rather than retried, exactly as the trace path drops them: the project is not coming
        // back. A lookup failure keeps them, because losing a live project's metrics to a database blip
        // would be worse than writing to a project that is going away - which the sweep still cleans up.
        let mut projects: Vec<&str> = metrics
            .iter()
            .map(|m| m.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            .collect();
        projects.sort_unstable();
        projects.dedup();
        let repo = self.database.repository();
        let mut refusing: HashSet<String> = HashSet::new();
        for project in projects {
            match repo.project_accepts_writes(project).await {
                Ok(false) => {
                    refusing.insert(project.to_string());
                }
                Ok(true) => {}
                Err(e) => {
                    tracing::warn!(project, error = %e, "Could not check the project fence for metrics")
                }
            }
        }
        if !refusing.is_empty() {
            let before = metrics.len();
            metrics.retain(|m| {
                !refusing.contains(m.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
            });
            tracing::warn!(
                dropped = before - metrics.len(),
                projects = ?refusing,
                "Dropped metrics for projects that do not accept writes"
            );
            if metrics.is_empty() {
                return;
            }
        }

        persist_batch(&metrics, &self.analytics).await;
    }
}
