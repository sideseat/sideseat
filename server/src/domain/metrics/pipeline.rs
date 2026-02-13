//! Metrics Processing Pipeline
//!
//! Subscribes to metrics topic, extracts and persists to DuckDB.

use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::extract::extract_metrics_batch;
use super::persist::persist_batch;
use crate::core::{Topic, TopicError};
use crate::data::AnalyticsService;

pub struct MetricsPipeline {
    analytics: Arc<AnalyticsService>,
}

impl MetricsPipeline {
    pub fn new(analytics: Arc<AnalyticsService>) -> Self {
        Self { analytics }
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
        let metrics = extract_metrics_batch(request);
        if metrics.is_empty() {
            return;
        }
        persist_batch(&metrics, &self.analytics).await;
    }
}
