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
use crate::utils::time::is_storable;

/// How many of a request's data points were stored, out of how many it had, and **why** the rest were not.
///
/// Returned rather than folded into a bare success, because a caller told success for records that were
/// dropped has no way to learn otherwise - which is the failure this path exists to remove. The reason is
/// carried for the same reason the trace path carries `DropReason`: the two causes imply different actions.
/// "The project is gone" means stop sending there; "no backend can store that instant" means the exporter's
/// clock is wrong and a retry cannot help. Reporting every rejection as the first sent an operator hunting a
/// deletion that never happened.
pub struct Stored {
    pub stored: usize,
    pub total: usize,
    /// Dropped because the project will not accept writes.
    pub gone: usize,
    /// Dropped because the timestamp is outside every backend's storable range.
    pub unstorable: usize,
}

impl Stored {
    fn nothing_of(total: usize) -> Self {
        Self {
            stored: 0,
            total,
            gone: 0,
            unstorable: 0,
        }
    }

    /// What to tell the exporter about the records that did not land, or `None` when they all did.
    pub fn rejection(&self) -> Option<(usize, String)> {
        let rejected = self.total.saturating_sub(self.stored);
        if rejected == 0 {
            return None;
        }
        let message = match (self.gone, self.unstorable) {
            (0, 0) => "some data points were not stored".to_string(),
            (_, 0) => "the project is unknown or is being deleted".to_string(),
            (0, _) => "the data point timestamps are outside the storable range".to_string(),
            (_, _) => "some projects are unknown or being deleted, and some data point timestamps \
                       are outside the storable range"
                .to_string(),
        };
        Some((rejected, message))
    }
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
        return Ok(Stored::nothing_of(total));
    }
    let mut gone = 0usize;
    let mut unstorable = 0usize;

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
        gone = before - metrics.len();
        tracing::warn!(
            dropped = gone,
            projects = ?refusing,
            "Dropped metrics for projects that do not accept writes"
        );
        if metrics.is_empty() {
            return Ok(Stored {
                stored: 0,
                total,
                gone,
                unstorable,
            });
        }
    }

    // An instant no backend can store is refused here rather than written somewhere it fits.
    //
    // The row conversion used to reach ClickHouse's column through `timestamp_nanos_opt`, whose `None` past
    // 2262 fell back to the epoch - so a datapoint with a broken clock was stored at 1970, where the 90-day
    // TTL removed it, after the export was answered 200. DuckDB kept the same datapoint at its stated time,
    // so the two backends also disagreed about one export. Reported through `Stored`, which the OTLP
    // response turns into `rejected_data_points`: an exporter with a bad clock can then see it.
    // An out-of-range *exemplar* timestamp drops the exemplar, not the datapoint. The exemplar is an
    // auxiliary debugging sample; refusing a real measurement because its exemplar has a bad clock would be
    // wrong, and storing the exemplar's timestamp unvalidated diverged the backends - DuckDB kept year 3000
    // while ClickHouse clamped it (and, before the clamp, mis-stored it). Cleared here, backend-agnostically,
    // so neither store ever sees an unstorable exemplar instant. The measurement survives.
    let mut exemplars_dropped = 0usize;
    for m in metrics.iter_mut() {
        if m.exemplar_timestamp.is_some_and(|ts| !is_storable(ts)) {
            m.exemplar_trace_id = None;
            m.exemplar_span_id = None;
            m.exemplar_value_int = None;
            m.exemplar_value_double = None;
            m.exemplar_timestamp = None;
            m.exemplar_attributes = serde_json::Value::Null;
            exemplars_dropped += 1;
        }
    }
    if exemplars_dropped > 0 {
        tracing::warn!(
            exemplars_dropped,
            "Dropped exemplars whose timestamp is outside the storable range; the measurements were kept"
        );
    }

    let before = metrics.len();
    metrics.retain(|m| {
        let ok = is_storable(m.timestamp) && m.start_timestamp.map(is_storable).unwrap_or(true);
        if !ok {
            tracing::warn!(
                metric_name = %m.metric_name,
                timestamp = %m.timestamp,
                start_timestamp = ?m.start_timestamp,
                "Rejecting a data point whose timestamp is outside the storable range"
            );
        }
        ok
    });
    if metrics.len() < before {
        unstorable = before - metrics.len();
        tracing::warn!(
            rejected = unstorable,
            "Rejected data points with timestamps no analytics backend can store"
        );
    }
    if metrics.is_empty() {
        return Ok(Stored {
            stored: 0,
            total,
            gone,
            unstorable,
        });
    }

    persist_batch(&metrics, analytics).await?;
    Ok(Stored {
        stored: metrics.len(),
        total,
        gone,
        unstorable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rejection says which of the two causes applied, because they imply different actions.
    ///
    /// "The project is gone" tells an exporter to stop sending there; "that instant is unstorable" tells it
    /// its clock is wrong and a retry cannot help. Every rejection used to be reported as the first, so a
    /// year-2300 datapoint sent an operator hunting a deletion that never happened.
    #[test]
    fn a_rejection_names_the_cause_that_applied() {
        let all_stored = Stored {
            stored: 3,
            total: 3,
            gone: 0,
            unstorable: 0,
        };
        assert!(
            all_stored.rejection().is_none(),
            "nothing was rejected, so there is nothing to report"
        );

        let gone = Stored {
            stored: 1,
            total: 3,
            gone: 2,
            unstorable: 0,
        };
        let (n, msg) = gone.rejection().expect("a rejection");
        assert_eq!(n, 2);
        assert!(msg.contains("deleted"), "{msg}");
        assert!(!msg.contains("storable range"), "{msg}");

        let clock = Stored {
            stored: 1,
            total: 3,
            gone: 0,
            unstorable: 2,
        };
        let (n, msg) = clock.rejection().expect("a rejection");
        assert_eq!(n, 2);
        assert!(msg.contains("storable range"), "{msg}");
        assert!(!msg.contains("deleted"), "{msg}");

        // Both, in one request: the message has to admit both rather than pick one.
        let mixed = Stored {
            stored: 1,
            total: 4,
            gone: 2,
            unstorable: 1,
        };
        let (n, msg) = mixed.rejection().expect("a rejection");
        assert_eq!(n, 3);
        assert!(
            msg.contains("deleted") && msg.contains("storable range"),
            "{msg}"
        );
    }
}
