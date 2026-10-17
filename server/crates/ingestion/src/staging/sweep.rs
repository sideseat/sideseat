//! The staging sweep: what repairs queue loss and writes interrupted after staging, and the recovery path for a
//! payload whose queue reference was acknowledged while its bytes could not be read.

use super::*;

/// A staging sweep's place in its traversal ([`StagingService::redrive_once`]).
#[derive(Default)]
pub(super) struct SweepCursor {
    after: Option<StagedPayload>,
    until: Option<chrono::DateTime<chrono::Utc>>,
}

impl StagingService {
    /// Reconcile one page of registry rows. This repairs both queue loss and
    /// inline requests interrupted after their durable staging write.
    pub async fn redrive_once(
        &self,
        trace_pipeline: &TracePipeline,
        limit: usize,
    ) -> Result<usize, StagingError> {
        // Nothing is redriven while this process's store lease is lost: no attempt then can settle, or count.
        let Some(pass) = self.fence.enter("staging sweep") else {
            return Ok(0);
        };
        let (after, until) = {
            let cursor = self
                .sweep_cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                cursor.after.clone(),
                // The wall clock receipts are stamped with when an export arrives, not the injected one.
                cursor.until.unwrap_or_else(chrono::Utc::now),
            )
        };
        let mut payloads = self.pending_after(after.as_ref(), limit).await?;
        // Ordered by receipt, so a row past the traversal's bound means every later one is too: the traversal ends
        // there, as at the end of the rows.
        let full = payloads.len() == limit;
        let before = payloads.len();
        payloads.retain(|payload| payload.created_at <= until);
        let ended = !full || payloads.len() < before;
        *self
            .sweep_cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = if ended {
            SweepCursor::default()
        } else {
            SweepCursor {
                after: payloads.last().cloned(),
                until: Some(until),
            }
        };
        let count = payloads.len();
        // Each payload on its own: one that cannot be read or loaded does not stop the ones behind it, which may be
        // exports whose queue references were acknowledged while the store was unavailable, left to this sweep.
        for payload in payloads {
            match self.disposition(&payload).await {
                Ok(StagingDisposition::Confirmed | StagingDisposition::DeliberatelyAbsent) => {
                    // Fenced meanwhile, it stays staged for the next sweep that holds the lease.
                    self.release_in(Some(&pass), &payload).await?;
                    continue;
                }
                Ok(StagingDisposition::Pending) => {}
                // A payload whose own state cannot be read - its bytes gone, unreadable - is counted, so it is
                // quarantined at the cap instead of holding its place at the head of every pass; a store that
                // failed or was unavailable is not.
                Err(error) => {
                    tracing::warn!(
                        staged_payload_id = %payload.id,
                        %error,
                        "Could not settle a staged payload in the sweep; trying the next"
                    );
                    if error.is_payload_fault() {
                        self.count_in_sweep(&payload.id).await?;
                    }
                    continue;
                }
            }

            let (current, bytes) = match self.load(&payload.id).await {
                Ok(Some(loaded)) => loaded,
                Ok(None) => continue,
                // The payload's own failure is counted, so it is quarantined at the cap instead of blocking every pass;
                // a store that failed or was unavailable is not.
                Err(error) => {
                    tracing::warn!(
                        staged_payload_id = %payload.id,
                        %error,
                        "Could not load a staged payload in the sweep; trying the next"
                    );
                    if error.is_payload_fault() {
                        self.count_in_sweep(&payload.id).await?;
                    }
                    continue;
                }
            };
            let write_ok = match current.signal {
                StagedSignal::Traces => {
                    match crate::received::staged_traces(
                        &bytes,
                        current.project_id.as_str(),
                        current.created_at,
                    ) {
                        Ok((request, received)) => !matches!(
                            trace_pipeline.ingest_now(&request, &received).await,
                            IngestOutcome::Failed
                        ),
                        Err(error) => {
                            tracing::error!(
                                staged_payload_id = %current.id,
                                %error,
                                "Could not decode a staged trace payload during redrive"
                            );
                            false
                        }
                    }
                }
                StagedSignal::Metrics => {
                    match crate::received::staged_metrics(&bytes, current.project_id.as_str()) {
                        // Governed as the export was: a hold stamps its rows, which the backend keeps by it.
                        Ok(request) => match &self.governance {
                            Some(governance) => crate::metrics::ingest_governed(
                                &request,
                                self.analytics.as_ref(),
                                self.database.as_ref(),
                                current.created_at,
                                governance,
                            )
                            .await
                            .is_ok(),
                            None => crate::metrics::ingest(
                                &request,
                                self.analytics.as_ref(),
                                self.database.as_ref(),
                                current.created_at,
                            )
                            .await
                            .is_ok(),
                        },
                        Err(error) => {
                            tracing::error!(
                                staged_payload_id = %current.id,
                                %error,
                                "Could not decode a staged metrics payload during redrive"
                            );
                            false
                        }
                    }
                }
                StagedSignal::Logs => {
                    match crate::received::staged_logs(&bytes, current.project_id.as_str()) {
                        Ok(request) => match &self.governance {
                            Some(governance) => crate::logs::ingest_governed(
                                &request,
                                self.analytics.as_ref(),
                                self.database.as_ref(),
                                Some(trace_pipeline.file_service()),
                                current.created_at,
                                governance,
                            )
                            .await
                            .is_ok(),
                            None => crate::logs::ingest(
                                &request,
                                self.analytics.as_ref(),
                                self.database.as_ref(),
                                Some(trace_pipeline.file_service()),
                                current.created_at,
                            )
                            .await
                            .is_ok(),
                        },
                        Err(error) => {
                            tracing::error!(
                                staged_payload_id = %current.id,
                                %error,
                                "Could not decode a staged logs payload during redrive"
                            );
                            false
                        }
                    }
                }
            };

            if write_ok {
                match self.settle(&current).await {
                    Ok(StagingDisposition::Pending) => {}
                    Ok(_) => continue,
                    // The lease was lost during the sweep: nothing more settles in this pass, nor counts.
                    Err(StagingError::Fenced) => return Ok(count),
                    Err(error) => return Err(error),
                }
            }
            let _ = self.note_failed_attempt(&current.id).await?;
        }
        Ok(count)
    }

    /// Count a failed attempt from the sweep. A count that cannot be recorded leaves the payload for the next
    /// pass, without stopping this one - unless the lease is lost or the store has failed for good, which stop it.
    async fn count_in_sweep(&self, id: &str) -> Result<(), StagingError> {
        match self.note_failed_attempt(id).await {
            Ok(_) => Ok(()),
            Err(error @ (StagingError::Fenced | StagingError::StoreFailed(_))) => Err(error),
            Err(error) => {
                tracing::warn!(staged_payload_id = id, %error, "Could not count a staged payload's failed attempt");
                Ok(())
            }
        }
    }
}

/// Periodically repairs queue loss and writes interrupted after staging.
pub fn start_staging_sweep(
    staging: Arc<StagingService>,
    trace_pipeline: Arc<TracePipeline>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    if let Err(error) = staging.redrive_once(trace_pipeline.as_ref(), 100).await {
                        tracing::error!(%error, "Staged-payload sweep failed");
                    }
                }
            }
        }
    })
}
