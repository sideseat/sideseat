//! Metrics ingestion: extract, fence, write - before the request is answered.
//!
//! # Why there is no queue here, when traces have one
//!
//! Metrics used to go onto an in-process topic and be written by a background task. That made the
//! response a lie in the one way that matters: a 200 meant "queued in memory", and a crash, a lagging
//! consumer or a database that stayed down through its retries lost records the exporter had already
//! counted as delivered. Nothing surfaced it - persistence returned `()`.
//!
//! Traces keep their queue because it is a *durable* stream with consumer groups: a message is
//! acknowledged only after it is written, and an unacknowledged one is reclaimed. That machinery is what
//! makes an asynchronous acknowledgement honest, and metrics did not have it.
//!
//! Rather than duplicate it, metrics are written inside the request. It is the smaller change and the
//! stronger property - a 200 now means the rows are committed - and the trade is one an exporter is built
//! for: a slow write becomes backpressure and a failed one a retry, instead of silence. Metric payloads
//! are also far smaller than trace payloads, which is what makes the trade affordable rather than merely
//! correct.

use std::collections::HashSet;
use std::sync::Arc;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;

use super::extract::extract_metrics_batch;
use super::persist::persist_batch;
use crate::core::constants::DEFAULT_PROJECT_ID;
use crate::data::{AnalyticsService, TransactionalService};

/// How many of a request's data points were stored, out of how many it had.
///
/// The two differ when a project will not accept writes. Returned rather than folded into a bare success,
/// because a caller told success for records that were dropped has no way to learn otherwise - which is
/// the failure this path exists to remove.
pub struct Stored {
    pub stored: usize,
    pub total: usize,
}

/// Extract, fence and write a metrics request. `Err` means nothing was stored and a retry is warranted.
pub async fn ingest(
    request: &ExportMetricsServiceRequest,
    analytics: &Arc<AnalyticsService>,
    database: &Arc<TransactionalService>,
) -> Result<Stored, String> {
    let mut metrics = extract_metrics_batch(request);
    let total = metrics.len();
    if metrics.is_empty() {
        return Ok(Stored { stored: 0, total });
    }

    // The project deletion fence, which this path did not apply at all. A request admitted before a
    // deletion would otherwise persist after its cleanup, and metrics are exactly as unreachable as spans
    // once the project row is gone - nothing finds them, nothing counts them, and the deletion that was
    // meant to remove them has already run.
    //
    // A project that will not accept writes has its metrics dropped rather than refused: it is not coming
    // back, so a retry would be doomed. A fence that cannot be *read* is the opposite case - refused, so
    // the exporter retries, because failing open there writes rows nothing can reach.
    let mut projects: Vec<&str> = metrics
        .iter()
        .map(|m| m.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID))
        .collect();
    projects.sort_unstable();
    projects.dedup();
    let repo = database.repository();
    let mut refusing: HashSet<String> = HashSet::new();
    for project in projects {
        match repo.project_accepts_writes(project).await {
            Ok(false) => {
                refusing.insert(project.to_string());
            }
            Ok(true) => {}
            // Unknown, so refused - the trace path's rule, and for the reason it has it. Keeping the
            // metrics would fail *open* on exactly the failure that matters: PostgreSQL down and
            // ClickHouse up means the fence cannot be read while the write can still succeed, so a
            // deleted project's metrics land where nothing will ever find or collect them. A 503 has the
            // exporter retry instead.
            Err(e) => return Err(format!("could not read the project fence: {e}")),
        }
    }
    if !refusing.is_empty() {
        let before = metrics.len();
        metrics
            .retain(|m| !refusing.contains(m.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID)));
        tracing::warn!(
            dropped = before - metrics.len(),
            projects = ?refusing,
            "Dropped metrics for projects that do not accept writes"
        );
        if metrics.is_empty() {
            return Ok(Stored { stored: 0, total });
        }
    }

    persist_batch(&metrics, analytics).await?;
    Ok(Stored {
        stored: metrics.len(),
        total,
    })
}
