use super::*;

impl TracePipeline {
    /// Extract, enrich and write one request, answering whether it was stored.
    ///
    /// Used by the ingest path when the topic backend is not durable: with an in-memory queue an
    /// acknowledgement before the write is a promise the process cannot keep, so the request writes
    /// first. Measured at roughly nine milliseconds per request against four when batched - which for an
    /// exporter that ships every few seconds is not a cost worth a lost trace.
    pub async fn ingest_now(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
    ) -> IngestOutcome {
        self.run(request, received).await
    }

    /// Run the complete pipeline for a single request (used during shutdown drain
    /// and claimed message recovery). File I/O is done inline for reliability.
    pub(super) async fn run(
        &self,
        request: &ExportTraceServiceRequest,
        received: &ReceivedPayload,
    ) -> IngestOutcome {
        // Wrapped, like every call on the batch path. This one was not, and it is the path that
        // handles *recovery* - so a message the batch path refused for panicking could be claimed here
        // and take down the whole pipeline task rather than one request.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_request(
                request,
                &self.pricing,
                self.file_service.is_enabled(),
                &self.file_cache,
                ExtractionMode::PerCarrier,
            )
        }));
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                self.file_cache.invalidate_all();
                tracing::error!("process_request panicked; refusing this request");
                return IngestOutcome::Failed;
            }
        };
        if let Some((db_spans, pending_files, incoming)) = result {
            if db_spans.is_empty() {
                return IngestOutcome::Stored; // nothing to store, and nothing was refused
            }
            let mut db_spans = db_spans;

            // The project fence, exactly as the batch path applies it. This path had no check at all,
            // which is worse than it sounds: it is the *recovery* path, so it handles messages queued
            // before a restart - precisely the ones most likely to name a project deleted since.
            let mut pending_files = pending_files;
            let mut incoming = incoming;
            let mut partly_dropped = 0usize;
            // The cause of the *most recent* partial drop. Where a batch mixes causes, `Gone` wins: it is the
            // one that names a target the exporter should stop sending to, which is the more actionable of
            // the two.
            let mut partly_reason = DropReason::Unstorable;
            match self
                .drop_spans_for_dead_projects(&mut db_spans, &mut pending_files, &mut incoming)
                .await
            {
                // Reported as dropped, whether *all* of the request's spans went or only some: a batch can
                // name a live project and a dying one, and answering an unqualified success for the half
                // that was discarded is the failure this path exists to remove.
                // No associations to release here: on this path the project fence runs *before* the files
                // are written, so nothing has been associated yet - which is also why it takes the pending
                // file list and prunes it directly.
                Ok(dropped) if db_spans.is_empty() => {
                    return IngestOutcome::Dropped {
                        spans: dropped,
                        reason: DropReason::Gone,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped = dropped;
                    partly_reason = DropReason::Gone;
                }
                Err(()) => return IngestOutcome::Failed,
            }

            // The same rule as the batch path, and reported the same way: a retry cannot fix a clock, so
            // these are dropped rather than refused, and the caller is told how many.
            let unstorable = drop_unstorable_spans(&mut db_spans);
            if unstorable > 0 {
                if db_spans.is_empty() {
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + unstorable,
                        // `Gone` still wins a mixed cause. If an earlier drop already took spans for a dead
                        // project, the count reported here includes them, so blaming only the timestamps
                        // would describe the batch wrongly - and `Gone` is the more actionable half, because
                        // it names a target the exporter should stop sending to. Where nothing was Gone, the
                        // project is fine and the payload is not, which is what `Unstorable` says.
                        reason: if partly_dropped > 0 {
                            partly_reason
                        } else {
                            DropReason::Unstorable
                        },
                    };
                }
                partly_dropped += unstorable;
            }

            let sse_events = sse_events_for(&db_spans);
            // Ordered, exactly as the batch path is: this path ran the two concurrently, so it could
            // commit a row referencing a file whose write had failed - the same defect, in the path
            // that runs at shutdown and recovery, where it is least likely to be noticed.
            // The raw record first, so its media joins the files this request stores, under the same ownership.
            let project_id = db_spans
                .first()
                .and_then(|span| span.project_id.clone())
                .unwrap_or_else(|| DEFAULT_PROJECT_ID.to_string());
            let mut raw = RawDraft::new(&project_id, received, self.file_service.is_enabled());
            pending_files.extend(raw.media_writes(request));
            let files = persist_extracted_files(pending_files, &self.file_service).await;
            if files.failed > 0 {
                self.file_cache.invalidate_all();
                // Released for the same reason as on the batch path: the stored files' associations hold
                // quota that nothing would ever reclaim, since the rows justifying them are not being
                // written and the orphan sweeper only sees `ref_count = 0`.
                self.release_created_associations(&files.created_associations)
                    .await;
                tracing::error!(
                    failed = files.failed,
                    "Refusing to commit spans whose extracted files could not be stored"
                );
                return IngestOutcome::Failed;
            }
            // Media the store refused stays in the record, which must decode whatever the files' fate.
            raw.keep_inline(&files.quota_skipped);
            let mut unresolvable = files.quota_skipped;
            let (unbacked, reconcile_failed, incoming_associations) =
                reconcile_incoming_references(
                    &incoming,
                    &self.file_service,
                    &files.created_associations,
                )
                .await;
            let mut created_associations = files.created_associations;
            created_associations.extend(incoming_associations);
            // One entry per association, matching the one increment each now gets - see the batch path.
            created_associations.sort_unstable();
            created_associations.dedup();
            if reconcile_failed > 0 {
                self.file_cache.invalidate_all();
                self.release_created_associations(&created_associations)
                    .await;
                tracing::error!(
                    failed = reconcile_failed,
                    "Refusing the batch: could not settle whether some file references are backed"
                );
                return IngestOutcome::Failed;
            }
            unresolvable.extend(unbacked);
            if !unresolvable.is_empty() {
                let rewritten = note_unstored_files(&mut db_spans, &unresolvable);
                tracing::error!(
                    files = unresolvable.len(),
                    references_rewritten = rewritten,
                    "File content is not stored for some references; replaced them with a note"
                );
            }
            // The trace tombstone, in the same position as the batch path's and for the same reason.
            //
            // This path had no check at all, and it is the *default*: with an in-memory topic the request
            // writes inline, and it is also the shutdown drain and the claimed-message recovery. So a
            // deleted trace could be re-posted through the ordinary configuration and answered success,
            // which is the exact failure the tombstone exists to remove.
            match self.drop_spans_for_deleted_sessions(&mut db_spans).await {
                Ok(dropped) if db_spans.is_empty() => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + dropped,
                        reason: DropReason::Gone,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped += dropped;
                    partly_reason = DropReason::Gone;
                    self.release_associations_of_dropped(&mut created_associations, &db_spans)
                        .await;
                }
                Err(()) => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            }

            match self
                .drop_spans_for_journalled_deletions(&mut db_spans)
                .await
            {
                Ok(dropped) if db_spans.is_empty() => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + dropped,
                        reason: DropReason::Gone,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped += dropped;
                    partly_reason = DropReason::Gone;
                    self.release_associations_of_dropped(&mut created_associations, &db_spans)
                        .await;
                }
                Err(()) => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            }

            match self.drop_spans_for_deleted_traces(&mut db_spans).await {
                Ok(dropped) if db_spans.is_empty() => {
                    // The associations this batch created are released, or they would hold quota for a
                    // trace that will never have a row.
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Dropped {
                        spans: partly_dropped + dropped,
                        reason: DropReason::Gone,
                    };
                }
                Ok(0) => {}
                Ok(dropped) => {
                    partly_dropped += dropped;
                    partly_reason = DropReason::Gone;
                    self.release_associations_of_dropped(&mut created_associations, &db_spans)
                        .await;
                }
                Err(()) => {
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            }

            // What the raw record must keep: everything that survived the deletion fences. See `RawDraft::row`.
            let kept_for_raw: HashSet<(String, String)> = db_spans
                .iter()
                .map(|span| (span.trace_id.clone(), span.span_id.clone()))
                .collect();
            if self.drop_exact_redeliveries(&mut db_spans).await > 0 {
                self.release_associations_of_dropped(&mut created_associations, &db_spans)
                    .await;
                if db_spans.is_empty() {
                    return if partly_dropped > 0 {
                        IngestOutcome::PartlyDropped {
                            spans: partly_dropped,
                            reason: partly_reason,
                        }
                    } else {
                        IngestOutcome::Stored
                    };
                }
            }
            sideseat_domain::search::index_spans(&mut db_spans);

            let mut staged_bodies = match self
                .content_bodies
                .stage(&db_spans, self.storage_governance.as_ref())
                .await
            {
                Ok(staged) => Some(staged),
                Err(error) => {
                    self.content_bodies
                        .mark_incomplete_for_spans(&db_spans)
                        .await;
                    tracing::warn!(
                        %error,
                        "Could not content-address this request's span bodies; retaining inline columns only"
                    );
                    None
                }
            };

            // The raw record is the authority the rows are derived from, so it is stored before them; a request
            // whose raw record cannot be stored stores nothing.
            let hold_until = db_spans.first().and_then(|span| span.hold_until);
            let raw_row = match raw.row(request, &kept_for_raw, chrono::Utc::now(), hold_until) {
                Ok(row) => row,
                Err(error) => {
                    tracing::error!(%error, "Could not encode the request's raw record; refusing it");
                    self.release_created_associations(&created_associations)
                        .await;
                    return IngestOutcome::Failed;
                }
            };
            if let Err(error) = self
                .analytics
                .insert_raw_records(std::slice::from_ref(&raw_row))
                .await
            {
                tracing::error!(%error, "Could not store the request's raw record; refusing it");
                self.release_created_associations(&created_associations)
                    .await;
                return IngestOutcome::Failed;
            }
            for span in &mut db_spans {
                span.raw_id = Some(raw.raw_id().to_string());
            }

            // Same capture as the batch path, for the same compensating re-check.
            let written: Vec<(String, String, String)> = db_spans
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
            let db_ok = write_to_duckdb(db_spans, self.analytics.as_ref()).await;
            if !db_ok {
                if let Some(staged) = staged_bodies.as_mut() {
                    self.content_bodies.release_all(staged).await;
                }
                self.release_created_associations(&created_associations)
                    .await;
            }
            if db_ok {
                // Compensate before confirming - see the batch path for why the order is load-bearing under
                // the counter model.
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
                self.confirm_associations(&created_associations, "request")
                    .await;
                // Only spans that survived every drop and the compensation - see the batch path, including
                // why the survivor identity carries the project id.
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
                // Same as the batch path: resolved from the store, now that the rows are there.
                self.stamp_stored_sessions(&mut sse_events).await;
                publish_sse_events(&sse_events, &self.topics).await;
                // The rows are written: the latest record must hold them. A failure here is answered as a
                // failure, and the retry - idempotent for rows and record alike - repeats the check.
                let repair = WrittenRecord {
                    draft: &raw,
                    request,
                    kept: kept_for_raw.clone(),
                    written: surviving
                        .iter()
                        .map(|(_, trace, span)| ((*trace).to_string(), (*span).to_string()))
                        .collect(),
                    hold_until,
                };
                if let Err(error) = self.repair_raw_records(std::slice::from_ref(&repair)).await {
                    tracing::error!(%error, "Could not check the raw record against the rows written");
                    return IngestOutcome::Failed;
                }
                if partly_dropped > 0 {
                    IngestOutcome::PartlyDropped {
                        spans: partly_dropped,
                        reason: partly_reason,
                    }
                } else {
                    IngestOutcome::Stored
                }
            } else {
                IngestOutcome::Failed
            }
        } else {
            IngestOutcome::Stored
        }
    }
}
