//! Exports grouped so that a batch means exactly what writing them one at a time would.
//!
//! A batch is equivalent to sequential ingestion only when its exports do not interact. Exports of different
//! traces commute - every fence, tombstone and canonical-session decision is made per trace - but two exports
//! of one trace do not: the redelivery filter compares both against the winner from before the batch, the
//! canonical session is decided across both at once, the confirmation that settles each export sees only the
//! batch's winning revision, and their raw records tie on receipt time and replay in hash order. So exports
//! that share a trace are never batched together: each conflicting export goes in a later wave than the one it
//! conflicts with, which keeps them in arrival order, while unrelated exports share a wave.
//!
//! A wave that fails is retried an export at a time. A failure caused by one export's data - an attachment
//! that does not decode, a panic in its preparation - would otherwise fail every export it was grouped with,
//! and keep failing them on every retry for as long as it kept arriving among them.
//!
//! An export that names a stored file - a `#!B64!#` reference that arrived already formed - is a barrier: it has a
//! wave of its own, after every export before it and before every export after it. Whether its reference is
//! backed depends on which exports stored their files first, and through files any export can depend on any
//! other, whatever their traces: grouped with an export that arrived after it and supplied the file, in its wave
//! or an earlier one, the reference held where alone it would have found nothing.
//!
//! Each export's staged payload is settled after its own wave, before the next wave is written, so it is settled
//! while its content is the stored winner. Settled after a later wave that holds another revision of the same
//! span, it is settled as superseded by that revision instead (`StagingService::disposition`), which reads the
//! export's own record as well.

use std::collections::{HashMap, HashSet};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use super::{IngestOutcome, TracePipeline};
use crate::received::ReceivedPayload;
use sideseat_ports::types::StagedPayload;

/// What became of one export of [`TracePipeline::run_waves`]: its outcome, and whether its staged payload, when it
/// had one, was settled after its wave.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WaveAnswer {
    pub outcome: IngestOutcome,
    pub settled: bool,
}

/// Whether an export's body names a stored file. A byte search over the body as received, before it is decoded:
/// the prefix is not escaped in either OTLP encoding, and a false match only costs a wave.
fn names_a_file(received: &ReceivedPayload) -> bool {
    let prefix = sideseat_core::utils::file_uri::FILE_URI_PREFIX.as_bytes();
    received
        .bytes
        .windows(prefix.len())
        .any(|window| window == prefix)
}

/// Indices of `requests`, grouped into waves whose exports share no trace, with every export placed after any
/// earlier export it shares a trace with, and an export that names a file alone between those before and after.
pub(super) fn conflict_free_waves(
    requests: &[ExportTraceServiceRequest],
    received: &[ReceivedPayload],
) -> Vec<Vec<usize>> {
    let mut waves: Vec<Vec<usize>> = Vec::new();
    // The last wave each trace appears in.
    let mut latest: HashMap<&[u8], usize> = HashMap::new();
    // The first wave an export may join: the one after the last barrier.
    let mut floor = 0;
    for (index, request) in requests.iter().enumerate() {
        if received.get(index).is_some_and(names_a_file) {
            waves.push(vec![index]);
            floor = waves.len();
            continue;
        }
        let traces: HashSet<&[u8]> = request
            .resource_spans
            .iter()
            .flat_map(|rs| &rs.scope_spans)
            .flat_map(|ss| &ss.spans)
            .map(|span| span.trace_id.as_slice())
            .collect();
        let wave = traces
            .iter()
            .filter_map(|trace| latest.get(trace))
            .max()
            .map_or(0, |last| last + 1)
            .max(floor);
        if wave == waves.len() {
            waves.push(Vec::new());
        }
        waves[wave].push(index);
        for trace in traces {
            latest.insert(trace, wave);
        }
    }
    waves
}

impl TracePipeline {
    /// Persist exports in conflict-free waves, answering each; exports of a failed wave are retried alone. An
    /// export with a staged payload (`staged`, by index) is settled after its wave, before the next is written.
    pub(super) async fn run_waves(
        &self,
        requests: &[ExportTraceServiceRequest],
        received: &[ReceivedPayload],
        staged: &[Option<StagedPayload>],
    ) -> Vec<WaveAnswer> {
        let mut outcomes = vec![IngestOutcome::Failed; requests.len()];
        let mut settled = vec![false; requests.len()];
        for wave in conflict_free_waves(requests, received) {
            let batch: Vec<ExportTraceServiceRequest> =
                wave.iter().map(|&index| requests[index].clone()).collect();
            let bodies: Vec<ReceivedPayload> =
                wave.iter().map(|&index| received[index].clone()).collect();
            let answers = self.run_batch(&batch, &bodies).await;
            for (&index, answer) in wave.iter().zip(answers) {
                outcomes[index] = answer;
            }
            if wave.len() > 1
                && wave
                    .iter()
                    .any(|&index| outcomes[index] == IngestOutcome::Failed)
            {
                // Alone, an export is answered for what its own data does; ingestion is idempotent by span id,
                // so what the failed batch did write is simply written again.
                for &index in &wave {
                    if outcomes[index] == IngestOutcome::Failed {
                        outcomes[index] = self
                            .run_batch(
                                std::slice::from_ref(&requests[index]),
                                std::slice::from_ref(&received[index]),
                            )
                            .await
                            .into_iter()
                            .next()
                            .unwrap_or(IngestOutcome::Failed);
                    }
                }
            }
            for &index in &wave {
                if let (true, Some(Some(payload))) = (outcomes[index].is_final(), staged.get(index))
                {
                    settled[index] = self.settle_staged_trace(payload).await;
                }
            }
        }
        outcomes
            .into_iter()
            .zip(settled)
            .map(|(outcome, settled)| WaveAnswer { outcome, settled })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn export(traces: &[u8]) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: traces
                        .iter()
                        .map(|&trace| Span {
                            trace_id: vec![trace; 16],
                            span_id: vec![1; 8],
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    fn bodies(requests: &[ExportTraceServiceRequest]) -> Vec<ReceivedPayload> {
        requests
            .iter()
            .map(|request| {
                ReceivedPayload::new(
                    prost::Message::encode_to_vec(request),
                    sideseat_domain::raw_payload::RawContent::Protobuf,
                    chrono::Utc::now(),
                )
            })
            .collect()
    }

    fn waves(requests: &[ExportTraceServiceRequest]) -> Vec<Vec<usize>> {
        conflict_free_waves(requests, &bodies(requests))
    }

    #[test]
    fn unrelated_exports_share_a_wave() {
        assert_eq!(
            waves(&[export(&[1]), export(&[2]), export(&[3])]),
            vec![vec![0, 1, 2]]
        );
    }

    /// An export naming a file waits for every export before it, and every export after it waits for it,
    /// whatever their traces.
    #[test]
    fn an_export_naming_a_file_has_a_wave_of_its_own_in_arrival_order() {
        let requests = [export(&[1]), export(&[2]), export(&[3]), export(&[4])];
        let mut received = bodies(&requests);
        received[1]
            .bytes
            .extend_from_slice(b"#!B64!#image/png::0123");
        assert_eq!(
            conflict_free_waves(&requests, &received),
            vec![vec![0], vec![1], vec![2, 3]]
        );
    }

    /// Exports of one trace go in successive waves, in arrival order; an unrelated one stays in the first.
    #[test]
    fn exports_of_one_trace_are_never_batched_together() {
        let planned = waves(&[export(&[1]), export(&[1]), export(&[2]), export(&[1, 2])]);
        assert_eq!(planned, vec![vec![0, 2], vec![1], vec![3]]);
    }
}
