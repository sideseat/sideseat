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
//! alone (`batch_equivalence_tests`), and nothing is acknowledged before its own outcome arrives. The queue is
//! bounded in bytes, so a burst waits for room instead of growing memory.

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

    /// Persist one export, in whatever batch it joins, and answer it.
    pub async fn ingest(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
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

async fn run(
    pipeline: Arc<TracePipeline>,
    mut receiver: mpsc::Receiver<Job>,
    #[cfg(test)] batches: Arc<std::sync::atomic::AtomicUsize>,
) {
    while let Some(first) = receiver.recv().await {
        let mut bytes = first.bytes;
        let mut jobs = vec![first];
        while jobs.len() < MAX_BATCH_EXPORTS && bytes < MAX_BATCH_BYTES {
            match receiver.try_recv() {
                Ok(job) => {
                    bytes += job.bytes;
                    jobs.push(job);
                }
                Err(_) => break,
            }
        }
        #[cfg(test)]
        batches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let exports = jobs.len();
        let mut requests = Vec::with_capacity(exports);
        let mut received = Vec::with_capacity(exports);
        // Each export's answer channel and its share of the byte budget, held until it is answered.
        let mut waiting = Vec::with_capacity(exports);
        for job in jobs {
            requests.push(job.request);
            received.push(job.received);
            waiting.push((job.reply, job.room));
        }
        // Its own task, so a panic fails this batch's exports and not the batcher.
        let batch = Arc::clone(&pipeline);
        let outcomes = tokio::spawn(async move { batch.run_waves(&requests, &received).await })
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
