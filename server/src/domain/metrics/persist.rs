//! Metric persistence (analytics backend writes with shared retry)
//!
//! Simple batch writes for normalized metrics.

use std::sync::Arc;

use crate::data::AnalyticsService;
use crate::data::types::NormalizedMetric;
use crate::utils::retry::{DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS, retry_with_backoff_async};

/// Persist metrics batch to analytics backend with exponential backoff retry.
pub async fn persist_batch(metrics: &[NormalizedMetric], analytics: &Arc<AnalyticsService>) {
    let metric_count = metrics.len();
    let repo = analytics.repository();

    let result = retry_with_backoff_async(DEFAULT_MAX_ATTEMPTS, DEFAULT_BASE_DELAY_MS, || {
        repo.insert_metrics(metrics)
    })
    .await;

    match result {
        Ok(attempts) => {
            if attempts > 1 {
                tracing::debug!(
                    metrics = metric_count,
                    attempts,
                    "Wrote metrics to analytics backend after retry"
                );
            } else {
                tracing::debug!(metrics = metric_count, "Wrote metrics to analytics backend");
            }
        }
        Err((e, attempts)) => {
            tracing::error!(
                error = %e,
                metrics = metric_count,
                attempts,
                "Failed to write metrics to analytics backend after retries"
            );
        }
    }
}
