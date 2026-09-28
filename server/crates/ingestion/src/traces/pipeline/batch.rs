use super::*;

impl TracePipeline {
    // ========================================================================
    // PIPELINE EXECUTION
    // ========================================================================

    /// Run the complete pipeline for a batch of OTLP requests.
    ///
    /// Processes requests in parallel across CPU cores (extract, sideml, enrich,
    /// base64 extraction are all CPU-bound), then DuckDB write + file I/O in parallel,
    /// SSE publish after both complete.
    ///
    /// Returns true if the DuckDB write succeeded (messages should be ACKed),
    /// false if it failed (messages should NOT be ACKed for redelivery).
    /// [`Self::run_batch`], for a benchmark that needs the whole write path rather than its CPU half.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn run_batch_for_test(&self, requests: &[ExportTraceServiceRequest]) -> bool {
        self.run_batch(requests).await
    }

    pub(super) async fn run_batch(&self, requests: &[ExportTraceServiceRequest]) -> bool {
        let t_batch_start = std::time::Instant::now();

        let pricing = &self.pricing;
        let files_enabled = self.file_service.is_enabled();
        let file_cache = &self.file_cache;

        // Process requests in parallel using scoped threads.
        // base64 extraction can take 100ms-1s per request for image-heavy spans,
        // so parallel processing across CPU cores significantly reduces batch time.
        // The FileExtractionCache is shared across threads to skip
        // redundant decode + BLAKE3 for the same base64 content.
        let results: Vec<Prepared> = tokio::task::block_in_place(|| {
            let num_workers = std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(4);

            // Waves bounded by *bytes*, not by worker count.
            //
            // One thread per core, each expanding its own request, made peak CPU-phase memory "cores times
            // the largest request" - so a 32-core host expanded thirty-two 15.8 MB image-heavy exports at
            // once, and the expansion is several times its input. The host decided the multiplier, which is
            // not a bound. Grouping by summed size keeps full parallelism for small payloads and reduces
            // concurrency only where each request is large, which is where it had to.
            //
            // `encoded_len` is O(number of fields) rather than O(payload bytes) - a length-delimited field
            // contributes its length, not a walk of its contents - so measuring is cheap next to the base64
            // decode and BLAKE3 this phase exists for.
            let mut results: Vec<Prepared> = Vec::with_capacity(requests.len());
            for wave in byte_bounded_waves(requests, num_workers) {
                // Each request is wrapped in `catch_unwind` so a panic in one does not propagate through
                // `thread::scope` and drop the batch.
                let wave_results: Vec<Prepared> = std::thread::scope(|s| {
                    let handles: Vec<_> = wave
                        .iter()
                        .map(|request| {
                            s.spawn(|| {
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    process_request(
                                        request,
                                        pricing,
                                        files_enabled,
                                        file_cache,
                                        ExtractionMode::PerCarrier,
                                    )
                                })) {
                                    Ok(Some((spans, files, incoming))) => {
                                        Prepared::Ready(spans, files, incoming)
                                    }
                                    Ok(None) => Prepared::Nothing,
                                    Err(_) => {
                                        tracing::error!(
                                            "process_request panicked, refusing the batch"
                                        );
                                        Prepared::Panicked
                                    }
                                }
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| match h.join() {
                            Ok(result) => result,
                            Err(_) => {
                                // A worker thread died, so its request is unaccounted for. Reported as
                                // `Panicked` rather than omitted, because a short result vector is what the
                                // cardinality check below has to catch and a silent gap defeats it.
                                tracing::error!("process_request thread panicked unexpectedly");
                                Prepared::Panicked
                            }
                        })
                        .collect()
                });
                results.extend(wave_results);
            }
            results
        });

        let mut all_db_spans: Vec<NormalizedSpan> = Vec::new();
        let mut all_pending_files: Vec<PendingFileWrite> = Vec::new();
        let mut all_incoming: Vec<IncomingReference> = Vec::new();
        // Short results mean a worker thread died with its whole chunk, which no per-request outcome can
        // report - so cardinality is checked, not just the outcomes.
        let mut lost_requests = requests.len().saturating_sub(results.len());
        for result in results {
            match result {
                Prepared::Ready(db_spans, pending_files, incoming) => {
                    all_db_spans.extend(db_spans);
                    all_pending_files.extend(pending_files);
                    all_incoming.extend(incoming);
                }
                // Normal: nothing to persist, nothing to refuse.
                Prepared::Nothing => {}
                // `catch_unwind` stops one request's panic taking the batch down, which is right, and
                // the result used to be dropped here - so the batch reported success, the exporter
                // acknowledged, and those spans were gone with nothing but a log line. The batch is
                // refused instead: the exporter retries, and ingestion is idempotent by span id, so
                // re-delivering the requests that did succeed costs a rewrite rather than a duplicate.
                Prepared::Panicked => lost_requests += 1,
            }
        }
        if lost_requests > 0 {
            // The requests that *did* succeed left cache entries but were never persisted, so the retry
            // must not find them claiming otherwise. Every refusal clears the cache for that reason.
            self.file_cache.invalidate_all();
            tracing::error!(
                lost_requests,
                requests = requests.len(),
                "Refusing the batch: a request panicked and its spans would otherwise be acknowledged"
            );
            return false;
        }

        if all_db_spans.is_empty() {
            return true;
        }

        if let Some(governance) = &self.storage_governance
            && let Err(error) = governance.stamp_spans(&mut all_db_spans).await
        {
            tracing::error!(%error, "Could not apply the legal-hold writer fence");
            return false;
        }

        // Spans for a project that will not accept writes are dropped, not written.
        //
        // Checked twice, and the second one is the check that matters. The fence lives in the
        // transactional store and the spans in the analytics store, so no transaction spans both and the
        // gap between reading the fence and committing is real. Making that gap as small as possible is
        // what bounds it: the second check sits immediately before the analytics write, so what remains
        // is the write itself rather than the whole batch - and a batch can take a hundred milliseconds
        // on inlined base64 images, or much longer if object storage is slow.
        //
        // The first check is an optimisation with the same rule: no point storing a dying project's file
        // bytes. Neither can be replaced by an HTTP-edge check, which says nothing about a write that
        // happens seconds later on a topic consumer.
        //
        // "Will not accept writes" covers a project claimed for deletion *and* one whose row is gone.
        // The second is what makes the fence airtight rather than narrow, together with the row
        // outliving its claim: a batch that read the fence before a claim commits within one batch, and
        // after the row goes such writes are refused outright. Spans for a project with no row were
        // never readable anyway - every read path finds data through the project row.
        //
        // Dropped rather than refused: the project is not coming back, so a refusal would have the
        // exporter retry a doomed batch, and a batch mixing a live project with a dying one would lose
        // the live one's spans too.
        match self
            .drop_spans_for_dead_projects(
                &mut all_db_spans,
                &mut all_pending_files,
                &mut all_incoming,
            )
            .await
        {
            Ok(_) if all_db_spans.is_empty() => return true, // nothing left to write
            Ok(_) => {}
            Err(()) => return false,
        }

        // Before the files are written, so a rejected span's attachments are never stored either.
        if drop_unstorable_spans(&mut all_db_spans) > 0 && all_db_spans.is_empty() {
            return true; // nothing storable, and nothing was queued for a retry that cannot help
        }

        let t_prepare_done = std::time::Instant::now();
        let span_count = all_db_spans.len();

        // Build SSE events before write (captures span metadata)
        let sse_events = sse_events_for(&all_db_spans);

        // Files first, then the rows that reference them.
        //
        // These used to run under one `tokio::join!`, which is faster and admits a state the reader
        // cannot recover from: a span row committed with a `#!B64!#` reference to a file whose write
        // failed. Writing the referenced object before the reference makes that impossible - the
        // remaining failure mode is an orphaned file, which is reclaimable and invisible to a reader.
        //
        // The cost is their sum rather than their maximum. Worth it: parity between the analytics
        // backends proves they *agree*, never that either faithfully represents the OTLP input, so a
        // dangling reference is a class of corruption no read-side test can catch.
        let files = persist_extracted_files(all_pending_files, &self.file_service).await;
        if files.failed > 0 {
            // A row referencing a file that is not there is worse than no row: the reference cannot
            // be repaired by a later delivery, while the batch can. Refusing it lets the exporter
            // retry, and ingestion is idempotent by span id.
            //
            // The cache has to be cleared first. Its entries are written when bytes are *extracted*,
            // before they are stored, and a hit carries no data - which persistence reads as "already
            // in storage". Left in place, the retry this refusal invites would commit exactly the
            // dangling reference the refusal is meant to prevent.
            self.file_cache.invalidate_all();
            // The files that *did* store already have associations, and this batch is not going to write the
            // rows that justify them. Released, or they hold `ref_count` above zero forever: the orphan
            // sweeper selects on `ref_count = 0`, so nothing would reclaim the bytes and the project's quota
            // would shrink permanently. A redelivery re-creates them, so this costs nothing when it succeeds.
            self.release_created_associations(&files.created_associations)
                .await;
            tracing::error!(
                failed = files.failed,
                spans = span_count,
                "Refusing to commit spans whose extracted files could not be stored"
            );
            return false;
        }
        // Deliberate, not transient: retrying will not help, so the spans are still committed - but
        // their references to the rejected files are rewritten first. A reader cannot tell a reference
        // to a rejected file from a corrupt one; both render as a broken image, and nothing on the span
        // says which. The placeholder says which.
        // References that arrived already formed are claims about storage that nothing verified. Checked
        // here, with the ones that hold getting an association and the rest joining the quota-rejected
        // set - both are "a reference a reader cannot resolve", and both are replaced with a note.
        let mut unresolvable = files.quota_skipped;
        let (unbacked, reconcile_failed, incoming_associations) = reconcile_incoming_references(
            &all_incoming,
            &self.file_service,
            &files.created_associations,
        )
        .await;
        // Folded into the batch's own set, so every compensation path - a failed write, a tombstoned trace,
        // the post-write re-check - covers a reference that arrived already formed as well as one whose
        // bytes this batch wrote.
        let mut created_associations = files.created_associations;
        created_associations.extend(incoming_associations);
        // One entry per association, matching the one increment each now gets. The increment side is where
        // this is actually enforced - the file-write path dedupes by `(trace, hash)` and the incoming path is
        // seeded with what the file-write path already associated - so this is a belt-and-braces no-op that
        // keeps resolution count equal to increment count if a third source is ever added.
        created_associations.sort_unstable();
        created_associations.dedup();
        if reconcile_failed > 0 {
            self.file_cache.invalidate_all();
            // Same reason as above, and here the set is the merged one: this batch's own file writes plus the
            // references that arrived already formed. Both kinds hold quota until released.
            self.release_created_associations(&created_associations)
                .await;
            tracing::error!(
                failed = reconcile_failed,
                "Refusing the batch: could not settle whether some file references are backed"
            );
            return false;
        }
        unresolvable.extend(unbacked);
        if !unresolvable.is_empty() {
            let rewritten = note_unstored_files(&mut all_db_spans, &unresolvable);
            tracing::error!(
                files = unresolvable.len(),
                references_rewritten = rewritten,
                spans = span_count,
                "File content is not stored for some references; replaced them with a note"
            );
        }
        // Again, with the write next in line. Between the first check and here the batch stored its
        // files, which can take a while, and a claim may have landed in that time.
        let mut no_files: Vec<PendingFileWrite> = Vec::new();
        let mut no_incoming: Vec<IncomingReference> = Vec::new();
        match self
            .drop_spans_for_dead_projects(&mut all_db_spans, &mut no_files, &mut no_incoming)
            .await
        {
            Ok(0) => {}
            Ok(_) => {
                // Whole or partial, the dropped traces' associations go and leave `created_associations`,
                // so the write path confirms only survivors and never resolves a dropped one twice.
                self.release_associations_of_dropped(&mut created_associations, &all_db_spans)
                    .await;
                if all_db_spans.is_empty() {
                    return true;
                }
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        // And traces deleted individually, which the project fence says nothing about. Checked here, in
        // the same position and for the same reason: this batch's files are already stored, so a trace
        // deleted since they were written must not gain a row pointing at bytes the deletion reclaimed.
        //
        // A dropped span's associations are released, for the same reason a failed write's are: they hold
        // `ref_count` above zero for a row that will never exist, and the orphan sweeper selects on zero.
        // Deleted sessions, first: a trace of a deleted session may itself have no tombstone, because the
        // session's deletion resolved its traces at one instant and this one arrived after.
        match self
            .drop_spans_for_deleted_sessions(&mut all_db_spans)
            .await
        {
            Ok(0) => {}
            Ok(_) if all_db_spans.is_empty() => {
                self.release_created_associations(&created_associations)
                    .await;
                return true;
            }
            // A *partial* deleted-session drop must release the dropped traces' associations too - and this
            // path did not, so a batch mixing a deleted session with a live trace confirmed the dropped
            // session's association as durable, permanently holding its file's quota with no row behind it.
            Ok(_) => {
                self.release_associations_of_dropped(&mut created_associations, &all_db_spans)
                    .await;
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        match self
            .drop_spans_for_journalled_deletions(&mut all_db_spans)
            .await
        {
            Ok(0) => {}
            Ok(_) if all_db_spans.is_empty() => {
                self.release_created_associations(&created_associations)
                    .await;
                return true;
            }
            Ok(_) => {
                self.release_associations_of_dropped(&mut created_associations, &all_db_spans)
                    .await;
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        match self.drop_spans_for_deleted_traces(&mut all_db_spans).await {
            Ok(0) => {}
            Ok(_) if all_db_spans.is_empty() => {
                self.release_created_associations(&created_associations)
                    .await;
                return true;
            }
            Ok(_) => {
                self.release_associations_of_dropped(&mut created_associations, &all_db_spans)
                    .await;
            }
            Err(()) => {
                self.release_created_associations(&created_associations)
                    .await;
                return false;
            }
        }

        if self.drop_exact_redeliveries(&mut all_db_spans).await > 0 {
            self.release_associations_of_dropped(&mut created_associations, &all_db_spans)
                .await;
            if all_db_spans.is_empty() {
                return true;
            }
        }
        sideseat_domain::search::index_spans(&mut all_db_spans);

        // Transitional dual-write: inline analytics columns remain populated and are the read fallback.
        // This branch is therefore allowed to fail without refusing an otherwise durable OTLP write, but
        // every provisional reference it created is rolled back inside `stage`.
        let mut staged_bodies = match self
            .content_bodies
            .stage(&all_db_spans, self.storage_governance.as_ref())
            .await
        {
            Ok(staged) => Some(staged),
            Err(error) => {
                self.content_bodies
                    .mark_incomplete_for_spans(&all_db_spans)
                    .await;
                tracing::warn!(
                    %error,
                    "Could not content-address this batch's span bodies; retaining inline columns only"
                );
                None
            }
        };

        // Captured before the write consumes the spans: the compensating re-check below needs to know
        // exactly what was written, and only those rows may be removed.
        let written: Vec<(String, String, String)> = all_db_spans
            .iter()
            .map(|s| {
                (
                    s.project_id
                        .as_deref()
                        .unwrap_or(DEFAULT_PROJECT_ID)
                        .to_string(),
                    s.trace_id.clone(),
                    s.span_id.clone(),
                )
            })
            .collect();
        let db_ok = write_to_duckdb(all_db_spans, self.analytics.as_ref()).await;

        let t_persist_done = std::time::Instant::now();

        if db_ok {
            if let Some(governance) = &self.storage_governance {
                let projects = written
                    .iter()
                    .map(|(project_id, _, _)| ProjectId::from(project_id.as_str()))
                    .collect::<Vec<_>>();
                if let Err(error) = governance.patch_after_write(&projects).await {
                    if let Some(staged) = staged_bodies.as_mut() {
                        self.content_bodies
                            .confirm_winners(staged, self.analytics.as_ref())
                            .await;
                    }
                    tracing::error!(
                        %error,
                        "Could not close the writer-admitted-before-hold window"
                    );
                    return false;
                }
            }
            // Compensate *before* confirming, and the order is load-bearing under the counter model. A
            // deletion that landed between the pre-write check and the write left the trace resurrected;
            // the compensation deletes those spans and releases their associations. Were confirm to run
            // first it would mark those associations durable, and a durable row's release cannot delete it -
            // so the file would be held forever by a row that was just removed. Releasing before confirm
            // means the association is at zero pending and non-durable, so it goes; confirm then sees only
            // the survivors, which `collect_spans_written_for_deleted_traces` has pruned from the set.
            // Sessions first: tombstoning their traces is what makes the trace compensation see them.
            self.tombstone_traces_of_deleted_sessions(&written).await;
            let compensated = self
                .collect_spans_written_for_deleted_traces(&written, &mut created_associations)
                .await;
            if let Some(staged) = staged_bodies.as_mut() {
                self.content_bodies
                    .remove_identities(staged, &compensated)
                    .await;
                self.content_bodies
                    .confirm_winners(staged, self.analytics.as_ref())
                    .await;
            }
            // Now the survivors are durable, and no failure path may take them.
            self.confirm_associations(&created_associations, "batch")
                .await;
            // Published *after* every drop and compensation, and only for spans that survived both. Built
            // before the filtering, `sse_events` announced spans this batch went on to discard - so a
            // reader saw a span appear and then never find it.
            let surviving: HashSet<(&str, &str, &str)> = written
                .iter()
                .map(|(project, trace, span)| (project.as_str(), trace.as_str(), span.as_str()))
                .filter(|(project, trace, span)| {
                    !compensated.contains(&(
                        (*project).to_string(),
                        (*trace).to_string(),
                        (*span).to_string(),
                    ))
                })
                .collect();
            let mut sse_events: Vec<SseSpanEvent> = sse_events
                .into_iter()
                .filter(|e| {
                    surviving.contains(&(
                        e.project_id.as_deref().unwrap_or(DEFAULT_PROJECT_ID),
                        e.trace_id.as_str(),
                        e.span_id.as_str(),
                    ))
                })
                .collect();
            // The rows are in, so the store can answer authoritatively - which the batch cannot.
            self.stamp_stored_sessions(&mut sse_events).await;
            publish_sse_events(&sse_events, &self.topics).await;
        } else {
            if let Some(staged) = staged_bodies.as_mut() {
                self.content_bodies.release_all(staged).await;
            }
            // Release the associations this batch created, since the rows that would have justified them
            // are not there. Files are written before the rows deliberately, so a failed write leaves
            // associations holding `ref_count` above zero - and the orphan sweeper selects on
            // `ref_count = 0`, so nothing would ever reclaim those bytes and the project's quota would
            // shrink permanently. A redelivery re-creates them, so this costs nothing when the retry
            // succeeds.
            self.release_created_associations(&created_associations)
                .await;
        }

        tracing::debug!(
            requests = requests.len(),
            spans = span_count,
            db_ok,
            prepare_ms = t_prepare_done.duration_since(t_batch_start).as_millis() as u64,
            persist_ms = t_persist_done.duration_since(t_prepare_done).as_millis() as u64,
            total_ms = t_persist_done.duration_since(t_batch_start).as_millis() as u64,
            "Pipeline batch completed"
        );

        db_ok
    }
}
