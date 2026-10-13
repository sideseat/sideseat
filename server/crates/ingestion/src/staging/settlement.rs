//! Settling the staged payloads of one wave together, and the exports the write path stored itself.
//!
//! An export's settlement is two reads: whether the winning rows carry its content, and whether the records
//! those rows name hold its spans. Read one payload at a time they were most of a small export's cost, and they
//! are the same reads for every export of a wave - so the wave's payloads are read together, one of each per
//! project, and each payload is decided from the shared answer exactly as its own reads would decide it. A
//! payload the shared answer does not settle goes through the full disposition alone, as before.
//!
//! An export the batch vouches for (`BatchAnswer::written`) is not read back at all where the store's
//! acknowledged writes are durable: its rows committed, survived compensation and are held by the record it
//! claimed, which is what the reads would find (`StagingRetirement.tla`, `Skips`). Everything else reads.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use sideseat_ports::types::{ProjectId, StagedPayload, StagedRecord, StagedSignal};

use super::{StagingDisposition, StagingError, StagingService};

/// A trace payload's span records: `(trace, span, content digest)`.
fn span_records(payload: &StagedPayload) -> Vec<(String, String, String)> {
    payload
        .records
        .iter()
        .filter_map(|record| match record {
            StagedRecord::Span {
                trace_id,
                span_id,
                content_digest,
                ..
            } => Some((trace_id.clone(), span_id.clone(), content_digest.clone())),
            _ => None,
        })
        .collect()
}

impl StagingService {
    /// Retire a payload the write path stored itself, without reading it back: only for an export of a batch
    /// that vouched for it (`BatchAnswer::written`), and only where the store's acknowledged writes are durable.
    /// Anything else is settled by [`Self::settle_traces`].
    pub async fn settle_written(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        if payload.signal != StagedSignal::Traces
            || !self.analytics.acknowledged_writes_are_durable()
        {
            return self.settle(payload).await;
        }
        self.release(payload).await?;
        Ok(StagingDisposition::Confirmed)
    }

    /// Settle trace payloads together, one answer per payload in order: each is released when it is terminal,
    /// as [`Self::settle`] releases it.
    pub async fn settle_traces(
        &self,
        payloads: &[&StagedPayload],
    ) -> Vec<Result<StagingDisposition, StagingError>> {
        let mut by_project: BTreeMap<&ProjectId, Vec<usize>> = BTreeMap::new();
        for (index, payload) in payloads.iter().enumerate() {
            by_project
                .entry(&payload.project_id)
                .or_default()
                .push(index);
        }
        let mut settled_together = vec![false; payloads.len()];
        for (project_id, indices) in &by_project {
            let records: BTreeSet<(String, String, String)> = indices
                .iter()
                .filter(|&&index| payloads[index].signal == StagedSignal::Traces)
                .flat_map(|&index| span_records(payloads[index]))
                .collect();
            if records.is_empty() {
                continue;
            }
            // The shared reads only ever settle; a failure leaves every payload to its own reads.
            let Ok(matching) = self
                .analytics
                .spans_with_matching_content(
                    project_id,
                    &records.iter().cloned().collect::<Vec<_>>(),
                )
                .await
            else {
                continue;
            };
            let spans: Vec<(String, String)> = records
                .iter()
                .filter(|record| matching.contains(*record))
                .map(|(trace, span, _)| (trace.clone(), span.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let Ok(covered) = crate::raw_coverage::covered(
                self.analytics.as_ref(),
                self.database.as_ref(),
                project_id,
                &spans,
            )
            .await
            else {
                continue;
            };
            for &index in indices {
                settled_together[index] = Self::confirmed_by(payloads[index], &matching, &covered);
            }
        }
        let mut answers = Vec::with_capacity(payloads.len());
        for (payload, together) in payloads.iter().zip(settled_together) {
            answers.push(if together {
                self.release(payload)
                    .await
                    .map(|()| StagingDisposition::Confirmed)
            } else {
                self.settle(payload).await
            });
        }
        answers
    }

    /// Whether the shared answer confirms `payload` the way its own fast path would: every span record's content
    /// is the winner's, and the record the winner names holds the span.
    fn confirmed_by(
        payload: &StagedPayload,
        matching: &HashSet<(String, String, String)>,
        covered: &HashSet<(String, String)>,
    ) -> bool {
        let records = span_records(payload);
        payload.signal == StagedSignal::Traces
            && !records.is_empty()
            && records.len() == payload.records.len()
            && records.iter().all(|record| {
                matching.contains(record) && covered.contains(&(record.0.clone(), record.1.clone()))
            })
    }
}
