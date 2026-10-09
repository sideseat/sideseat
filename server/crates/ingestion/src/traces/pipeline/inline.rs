//! Concurrent inline exports persisted together, each answered for its own spans.
//!
//! With no durable queue, a trace export is persisted before it is acknowledged, and it used to be persisted
//! alone: one raw record write, one span write and one round of fences per request, each paying its own synced
//! commits. Requests that arrive while a batch is being written now wait for the next one and go in together,
//! so the per-batch costs - the commits above all - are shared by every request in it.
//!
//! Batching is opportunistic: a request never waits for company. The batcher takes the first waiting request
//! and whatever else is already queued, up to an export cap and a byte cap, writes them in conflict-free waves
//! (`waves.rs`: exports sharing a trace are never batched together, and a failed wave is retried an export at a
//! time), and starts again. Alone, a request is written at once; under load the batches
//! grow by themselves, because requests accumulate while the previous batch is written. Batches are written one
//! at a time - the embedded analytics store has one connection, so a second concurrent batch would only queue
//! behind it.
//!
//! Correctness does not depend on the grouping: a batch answers each export exactly as it would be answered
//! alone (`batch_equivalence_tests`), and nothing is acknowledged before its own outcome arrives. Each export's
//! staged payload is settled after its wave, before the next is written (`waves.rs`). The queue is bounded in
//! bytes, so a burst waits for room instead of growing memory, and a batch in bytes too: an export that would take
//! it past the cap starts the next one, alone if it is over the cap by itself.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use super::{IngestOutcome, TracePipeline};
use crate::received::ReceivedPayload;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

/// Exports one batch takes at most.
const MAX_BATCH_EXPORTS: usize = 256;
/// Received bytes one batch takes at most; a larger export is written in a batch of its own.
const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
/// Received bytes waiting to be written, across all requests. A request beyond it waits for room.
const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
/// Requests waiting to be written; the byte budget is the bound that matters, this keeps the channel finite.
const MAX_QUEUED_EXPORTS: usize = 4096;

struct Job {
    request: ExportTraceServiceRequest,
    received: ReceivedPayload,
    /// The id of the export's staged payload, settled after its wave.
    staged: Option<String>,
    bytes: usize,
    reply: oneshot::Sender<IngestOutcome>,
    /// Held until the export is answered, so queued bytes stay within `MAX_QUEUED_BYTES`.
    room: OwnedSemaphorePermit,
}

/// Groups concurrent inline exports into batches. Owned by the trace signal; the batcher task ends when the
/// last sender does.
pub struct InlineBatcher {
    pipeline: Arc<TracePipeline>,
    sender: OnceLock<mpsc::Sender<Job>>,
    room: Arc<Semaphore>,
    #[cfg(test)]
    pub(super) batches: Arc<std::sync::atomic::AtomicUsize>,
}

impl InlineBatcher {
    pub fn new(pipeline: Arc<TracePipeline>) -> Self {
        Self {
            pipeline,
            sender: OnceLock::new(),
            room: Arc::new(Semaphore::new(MAX_QUEUED_BYTES)),
            #[cfg(test)]
            batches: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Persist one export, in whatever batch it joins, and answer it; settle its staged payload, `staged` by id,
    /// once it is written.
    pub async fn ingest(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
        staged: Option<&str>,
    ) -> IngestOutcome {
        let bytes = received.bytes.len();
        // An export larger than the whole budget takes all of it: it waits for the queue to drain, then goes.
        let units = u32::try_from(bytes.min(MAX_QUEUED_BYTES)).unwrap_or(u32::MAX);
        let Ok(room) = Arc::clone(&self.room).acquire_many_owned(units).await else {
            return IngestOutcome::Failed;
        };
        let (reply, answer) = oneshot::channel();
        let job = Job {
            request: request.clone(),
            received: received.clone(),
            staged: staged.map(str::to_string),
            bytes,
            reply,
            room,
        };
        if self.sender().send(job).await.is_err() {
            return IngestOutcome::Failed;
        }
        // A batcher that died before answering leaves the export unwritten as far as anyone can prove.
        answer.await.unwrap_or(IngestOutcome::Failed)
    }

    fn sender(&self) -> &mpsc::Sender<Job> {
        self.sender.get_or_init(|| {
            let (sender, receiver) = mpsc::channel(MAX_QUEUED_EXPORTS);
            tokio::spawn(run(
                Arc::clone(&self.pipeline),
                receiver,
                #[cfg(test)]
                Arc::clone(&self.batches),
            ));
            sender
        })
    }
}

/// Whether an export of `next` bytes joins a batch of `exports` exports and `bytes` bytes: only within both caps,
/// so a batch never passes either, and an export over the byte cap by itself is never joined by another.
fn joins(bytes: usize, exports: usize, next: usize) -> bool {
    exports < MAX_BATCH_EXPORTS
        && bytes < MAX_BATCH_BYTES
        && bytes.saturating_add(next) <= MAX_BATCH_BYTES
}

/// The batch that starts with `first`: it and whatever is already queued that [`joins`] it. A queued export that
/// does not join is left in `carried`, to start the next batch.
fn take_batch(
    first: Job,
    receiver: &mut mpsc::Receiver<Job>,
    carried: &mut Option<Job>,
) -> Vec<Job> {
    let mut bytes = first.bytes;
    let mut jobs = vec![first];
    while jobs.len() < MAX_BATCH_EXPORTS && bytes < MAX_BATCH_BYTES {
        match receiver.try_recv() {
            Ok(job) if joins(bytes, jobs.len(), job.bytes) => {
                bytes += job.bytes;
                jobs.push(job);
            }
            Ok(job) => {
                *carried = Some(job);
                break;
            }
            Err(_) => break,
        }
    }
    jobs
}

async fn run(
    pipeline: Arc<TracePipeline>,
    mut receiver: mpsc::Receiver<Job>,
    #[cfg(test)] batches: Arc<std::sync::atomic::AtomicUsize>,
) {
    // The export that would have taken the last batch past its byte cap: the first of the next.
    let mut carried: Option<Job> = None;
    loop {
        let first = match carried.take() {
            Some(job) => job,
            None => match receiver.recv().await {
                Some(job) => job,
                None => break,
            },
        };
        let jobs = take_batch(first, &mut receiver, &mut carried);
        #[cfg(test)]
        batches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let exports = jobs.len();
        let mut requests = Vec::with_capacity(exports);
        let mut received = Vec::with_capacity(exports);
        let mut staged = Vec::with_capacity(exports);
        // Each export's answer channel and its share of the byte budget, held until it is answered.
        let mut waiting = Vec::with_capacity(exports);
        for job in jobs {
            requests.push(job.request);
            received.push(job.received);
            staged.push(job.staged);
            waiting.push((job.reply, job.room));
        }
        // Its own task, so a panic fails this batch's exports and not the batcher.
        let batch = Arc::clone(&pipeline);
        let outcomes = tokio::spawn(async move {
            // A registration that cannot be read is settled by its requester instead, as before batching.
            let mut payloads = Vec::with_capacity(staged.len());
            for id in &staged {
                payloads.push(match id {
                    Some(id) => batch.staging.registration(id).await.ok().flatten(),
                    None => None,
                });
            }
            batch
                .run_waves(&requests, &received, &payloads)
                .await
                .into_iter()
                .map(|answer| answer.outcome)
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_else(|_| {
            tracing::error!(exports, "An inline batch panicked; refusing its exports");
            vec![IngestOutcome::Failed; exports]
        });
        for ((reply, _room), outcome) in waiting.into_iter().zip(outcomes) {
            // The requester may have gone - a closed connection - which changes nothing about what was written.
            let _ = reply.send(outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    /// A queued job of `bytes` bytes, as the batcher sees it: its body is not read here, only its size.
    fn job(bytes: usize, room: &Arc<Semaphore>) -> Job {
        Job {
            request: ExportTraceServiceRequest::default(),
            received: ReceivedPayload::new(
                Vec::new(),
                sideseat_domain::raw_payload::RawContent::Protobuf,
                chrono::Utc::now(),
            ),
            staged: None,
            bytes,
            reply: oneshot::channel().0,
            room: Arc::clone(room).try_acquire_owned().expect("room"),
        }
    }

    /// The batcher's own loop keeps a batch within its byte cap: two queued 31 MiB exports go in two batches, the
    /// second carried to start the next, and a 40 MiB export after a small one runs alone.
    #[test]
    fn queued_exports_past_the_cap_start_the_next_batch() {
        let room = Arc::new(Semaphore::new(16));
        let sizes = |batch: &[Job]| batch.iter().map(|job| job.bytes).collect::<Vec<_>>();
        let (sender, mut receiver) = mpsc::channel(16);
        for bytes in [31 * MIB, 31 * MIB, MIB, 40 * MIB, MIB] {
            assert!(sender.try_send(job(bytes, &room)).is_ok(), "queued");
        }
        let mut carried = None;
        let mut batches = Vec::new();
        loop {
            let first = match carried.take() {
                Some(job) => job,
                None => match receiver.try_recv() {
                    Ok(job) => job,
                    Err(_) => break,
                },
            };
            batches.push(sizes(&take_batch(first, &mut receiver, &mut carried)));
        }
        assert_eq!(
            batches,
            vec![
                vec![31 * MIB],
                vec![31 * MIB, MIB],
                vec![40 * MIB],
                vec![MIB]
            ]
        );
    }

    /// A batch stays within its byte cap: an export that would take it past starts the next batch, and one over
    /// the cap by itself is joined by nothing.
    #[test]
    fn a_batch_never_passes_its_caps() {
        assert!(joins(MIB, 1, MIB));
        assert!(
            !joins(31 * MIB, 1, 31 * MIB),
            "two 31 MiB exports are 62 MiB"
        );
        assert!(
            !joins(MIB, 1, 40 * MIB),
            "a 40 MiB export after a small one"
        );
        assert!(!joins(40 * MIB, 1, 1), "an oversized export runs alone");
        assert!(joins(MAX_BATCH_BYTES - 1, 1, 1));
        assert!(!joins(1, MAX_BATCH_EXPORTS, 1), "the export cap");
    }
}
