use super::*;

impl TracePipeline {
    /// Extract, enrich and write one request, answering whether it was stored.
    ///
    /// Used by the ingest path when the topic backend is not durable: with an in-memory queue an
    /// acknowledgement before the write is a promise the process cannot keep, so the request writes
    /// first.
    pub async fn ingest_now(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
    ) -> IngestOutcome {
        self.run(request, received).await
    }

    /// The complete pipeline for one request - the inline path, the shutdown drain and claimed-message
    /// recovery - as a batch of one, so a request is persisted by exactly the code that persists a batch and
    /// `batch_equivalence_tests` can hold the two to the same answers.
    pub(super) async fn run(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
    ) -> IngestOutcome {
        self.run_batch(
            std::slice::from_ref(request),
            std::slice::from_ref(received),
        )
        .await
        .into_iter()
        .next()
        .unwrap_or(IngestOutcome::Failed)
    }
}
