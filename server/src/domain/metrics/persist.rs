//! Metric persistence (analytics backend writes with shared retry)
//!
//! Simple batch writes for normalized metrics.

use std::sync::Arc;

use crate::data::AnalyticsService;
use crate::data::types::NormalizedMetric;
use crate::utils::retry::{DEFAULT_BASE_DELAY_MS, DEFAULT_MAX_ATTEMPTS, retry_with_backoff_async};

/// Persist metrics batch to analytics backend with exponential backoff retry.
///
/// Returns the failure rather than only logging it. It used to return `()`, so a batch that exhausted
/// its retries left an error line and nothing else - and since the caller was a background task whose
/// request had already been answered 200, the records were gone with the exporter believing otherwise.
pub async fn persist_batch(
    metrics: &[NormalizedMetric],
    analytics: &Arc<AnalyticsService>,
) -> Result<(), String> {
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
            return Err(format!("{e}"));
        }
    }
    Ok(())
}
