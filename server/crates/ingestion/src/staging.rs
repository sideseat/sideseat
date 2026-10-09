//! Durable OTLP staging shared by every signal lifecycle.
//!
//! The blob is written before its registry row. That ordering makes an
//! acknowledged payload discoverable even if the queue loses every entry. A
//! payload leaves staging only after strict producer-content confirmation, or
//! after every absent record is explained by the same deletion/retention rules
//! as ingestion.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::TimeDelta;
use prost::Message;
use sideseat_core::config::RetentionConfig;
use sideseat_ports::blobs::{FileStorage, FileStorageError};
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, DeletionScope, TransactionalRepository};
use sideseat_ports::types::{
    ProjectId, StagedPayload, StagedRecord, StagedSignal, WinningRevision,
};
use thiserror::Error;
use uuid::Uuid;

use crate::traces::{IngestOutcome, TracePipeline};

/// What a queue reference whose registry row is missing turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingReference {
    /// Retired or deleted; the reference outlived its row legitimately.
    Finished,
    /// The registration was lost after its export was acknowledged.
    Lost,
}

/// Compact durable-queue value. Payload bytes remain in the blob store.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct StagedPayloadRef {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub partition_key: String,
    /// The sequence the registration took, so a consumer can tell a retired payload from a lost one.
    #[prost(int64, tag = "3")]
    pub seq: i64,
}

impl StagedPayloadRef {
    #[must_use]
    pub fn partition_key(&self) -> String {
        self.partition_key.clone()
    }
}

/// The stored winners of a payload's span identities and datapoints.
#[derive(Default)]
struct Winners {
    spans: HashMap<(String, String), WinningRevision>,
    metrics: HashMap<String, WinningRevision>,
}

/// How one staged record stands against the store.
enum Standing {
    /// Its content is stored as the winner.
    Stored,
    /// A revision received no earlier than its payload is the winner.
    Superseded,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingDisposition {
    Confirmed,
    DeliberatelyAbsent,
    Pending,
}

#[derive(Debug, Error)]
pub enum StagingError {
    #[error("staging registry operation failed: {0}")]
    Registry(String),
    #[error("staging blob operation failed: {0}")]
    Blob(String),
    #[error("staged payload {0} has no blob")]
    MissingBlob(String),
}

/// Owns the blob/registry lifecycle and the signal-specific confirmation
/// predicates. It deliberately has no time-to-live.
pub struct StagingService {
    storage: Arc<dyn FileStorage>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    clock: Arc<dyn Clock>,
    retention: RetentionConfig,
    redrive_cap: u32,
}

impl StagingService {
    pub fn new(
        storage: Arc<dyn FileStorage>,
        database: Arc<dyn TransactionalRepository + Send + Sync>,
        analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
        clock: Arc<dyn Clock>,
        retention: RetentionConfig,
        redrive_cap: u32,
    ) -> Self {
        Self {
            storage,
            database,
            analytics,
            clock,
            retention,
            redrive_cap: redrive_cap.max(1),
        }
    }

    /// Stage `bytes`, received at `received_at`: the registration keeps the receipt, which every later write of the
    /// payload stores its rows at ([`crate::received::ReceivedPayload`]).
    pub async fn stage(
        &self,
        project_id: &str,
        signal: StagedSignal,
        bytes: &[u8],
        received_at: chrono::DateTime<chrono::Utc>,
        records: Vec<StagedRecord>,
        partition_key: String,
    ) -> Result<StagedPayloadRef, StagingError> {
        let id = Uuid::new_v4().to_string();
        let project_id = ProjectId::from(project_id);
        let mut hasher = blake3::Hasher::new();
        hasher.update(id.as_bytes());
        hasher.update(project_id.as_bytes());
        hasher.update(signal.as_str().as_bytes());
        hasher.update(bytes);
        let blob_hash = hasher.finalize().to_hex().to_string();

        self.storage
            .store(&project_id, &blob_hash, bytes)
            .await
            .map_err(blob_error)?;

        let payload = StagedPayload {
            id: id.clone(),
            project_id,
            signal,
            blob_hash,
            byte_len: bytes.len() as u64,
            created_at: received_at,
            redrive_attempts: 0,
            unconfirmed: false,
            records,
        };
        let seq = match self.database.create_staged_payload(&payload).await {
            Ok(seq) => seq,
            Err(error) => {
                if let Err(cleanup_error) = self
                    .storage
                    .delete(&payload.project_id, &payload.blob_hash)
                    .await
                {
                    tracing::error!(
                        staged_payload_id = %payload.id,
                        %cleanup_error,
                        "Could not remove a staging blob after its registry write failed"
                    );
                }
                return Err(StagingError::Registry(error.to_string()));
            }
        };

        Ok(StagedPayloadRef {
            id,
            partition_key,
            seq,
        })
    }

    /// Decide what a queue reference whose registry row is missing means.
    ///
    /// A row is missing legitimately - retired by redrive, by another consumer holding a claimed copy, or
    /// before a crash interrupted the acknowledgement, or removed with its project - and illegitimately when
    /// its registration was lost after the export was acknowledged. The sequence tells them apart: a lost
    /// registration's value is above the high-water mark, or held by another payload once reused. The
    /// protocol and the double fault it cannot see are modelled in `server/specs/StagingRetirement.tla`.
    pub async fn classify_missing(
        &self,
        reference: &StagedPayloadRef,
    ) -> Result<MissingReference, StagingError> {
        let state = self
            .database
            .staged_sequence_state(reference.seq)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?;
        let lost = reference.seq > state.high_water
            || state
                .holder
                .as_deref()
                .is_some_and(|holder| holder != reference.id);
        Ok(if lost {
            MissingReference::Lost
        } else {
            MissingReference::Finished
        })
    }

    /// Record a lost registration durably, so it is counted and visible rather than acknowledged in silence.
    pub async fn record_lost(&self, reference: &StagedPayloadRef) -> Result<(), StagingError> {
        tracing::error!(
            staged_payload_id = %reference.id,
            seq = reference.seq,
            "An acknowledged export's staging registration is missing: it was lost after acknowledgement"
        );
        self.database
            .record_staging_anomaly(&reference.id, reference.seq, self.clock.now())
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))
    }

    /// A staged payload's registration - what settling it reads - without its bytes, or `None` once it is
    /// retired.
    pub async fn registration(&self, id: &str) -> Result<Option<StagedPayload>, StagingError> {
        self.database
            .get_staged_payload(id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))
    }

    pub async fn load(&self, id: &str) -> Result<Option<(StagedPayload, Vec<u8>)>, StagingError> {
        let Some(payload) = self
            .database
            .get_staged_payload(id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?
        else {
            return Ok(None);
        };
        let bytes = match self
            .storage
            .get(&payload.project_id, &payload.blob_hash)
            .await
        {
            Ok(bytes) => bytes,
            Err(FileStorageError::NotFound { .. }) => {
                return Err(StagingError::MissingBlob(payload.id));
            }
            Err(error) => return Err(blob_error(error)),
        };
        Ok(Some((payload, bytes)))
    }

    pub async fn pending(&self, limit: usize) -> Result<Vec<StagedPayload>, StagingError> {
        self.database
            .pending_staged_payloads(limit)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))
    }

    /// Check strict content equality - for a datapoint, stored at a receipt no earlier than this export's. On a
    /// miss, settle what a revision received later superseded and prove every other missing record was
    /// intentionally removed before declaring the payload terminal.
    ///
    /// A span or datapoint whose winner is another revision is settled when that revision was received no
    /// earlier than this export: the export is not the latest. Without that, an export written after a later
    /// revision - redrive holding a copy it loaded, a requester retrying a failed settlement - found that
    /// revision the winner, stayed pending, and was written again for as long as it was retried. Its rows are
    /// stored at its receipt, so writing it again changes nothing a read answers
    /// (`server/specs/StagingRetirement.tla`). A superseded span is settled only once the export's own record
    /// holds it, so an export is never retired unwritten; a datapoint keeps only its winning revision, so a
    /// superseded one is settled by the correction alone, as a write of it would only have been replaced
    /// (`server/specs/StagingCorrections.tla`).
    pub async fn disposition(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        if self.all_confirmed(payload).await? {
            return Ok(if self.raw_covers(payload, &payload.records).await? {
                StagingDisposition::Confirmed
            } else {
                StagingDisposition::Pending
            });
        }

        if !self
            .database
            .project_accepts_writes(payload.project_id.as_str())
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?
        {
            return Ok(StagingDisposition::DeliberatelyAbsent);
        }

        let winners = self.winners(payload).await?;
        let mut saw_absence = false;
        let mut confirmed = Vec::new();
        let mut superseded = Vec::new();
        for record in &payload.records {
            match self.standing(payload, record, &winners).await? {
                Standing::Stored => {
                    confirmed.push(record.clone());
                    continue;
                }
                Standing::Superseded => {
                    superseded.push(record);
                    continue;
                }
                Standing::Missing => {}
            }
            if self
                .record_deliberately_absent(&payload.project_id, record)
                .await?
            {
                saw_absence = true;
                continue;
            }
            return Ok(StagingDisposition::Pending);
        }

        if !self.raw_covers(payload, &confirmed).await? {
            return Ok(StagingDisposition::Pending);
        }
        let held = self.held_by_own_record(payload, &superseded).await?;
        for record in superseded {
            match record {
                StagedRecord::Span {
                    trace_id, span_id, ..
                } if held.contains(&(trace_id.clone(), span_id.clone())) => continue,
                StagedRecord::Metric { .. } => continue,
                _ => {}
            }
            if self
                .record_deliberately_absent(&payload.project_id, record)
                .await?
            {
                saw_absence = true;
                continue;
            }
            return Ok(StagingDisposition::Pending);
        }
        Ok(if saw_absence {
            StagingDisposition::DeliberatelyAbsent
        } else {
            StagingDisposition::Confirmed
        })
    }

    /// The winning revisions of a payload's spans or datapoints, in one read; empty for logs.
    async fn winners(&self, payload: &StagedPayload) -> Result<Winners, StagingError> {
        let mut spans = std::collections::BTreeSet::new();
        let mut datapoints = std::collections::BTreeSet::new();
        for record in &payload.records {
            match record {
                StagedRecord::Span {
                    trace_id, span_id, ..
                } => {
                    spans.insert((trace_id.clone(), span_id.clone()));
                }
                StagedRecord::Metric {
                    datapoint_id,
                    timestamp,
                    ..
                } => {
                    datapoints.insert((datapoint_id.clone(), *timestamp));
                }
                StagedRecord::Log { .. } => {}
            }
        }
        let registry =
            |error: sideseat_ports::error::DataError| StagingError::Registry(error.to_string());
        let mut winners = Winners::default();
        if !spans.is_empty() {
            let spans: Vec<_> = spans.into_iter().collect();
            winners.spans = self
                .analytics
                .span_winners(&payload.project_id, &spans)
                .await
                .map_err(registry)?;
        }
        if !datapoints.is_empty() {
            let datapoints: Vec<_> = datapoints.into_iter().collect();
            winners.metrics = self
                .analytics
                .metric_winners(&payload.project_id, &datapoints)
                .await
                .map_err(registry)?;
        }
        Ok(winners)
    }

    /// How one record stands in the store: its content stored, superseded by a revision received no earlier
    /// than its payload, or neither.
    async fn standing(
        &self,
        payload: &StagedPayload,
        record: &StagedRecord,
        winners: &Winners,
    ) -> Result<Standing, StagingError> {
        if let Some(standing) = Self::revision_standing(payload, record, winners) {
            return Ok(standing);
        }
        Ok(if self.log_confirmed(&payload.project_id, record).await? {
            Standing::Stored
        } else {
            Standing::Missing
        })
    }

    /// How a span or a datapoint stands against its identity's winning revision; `None` for a log record, which
    /// has no revisions.
    ///
    /// A datapoint's content found stored counts only when it was stored at a receipt no earlier than this
    /// export's. A write stores a datapoint at its export's receipt even when its content is unchanged, so this
    /// export written would move the datapoint to its receipt; settled on an earlier export's copy, it would
    /// leave the datapoint at that earlier receipt, and a correction received in between - still to be written -
    /// would take over once written (`StagingCorrections.tla`, `RetiredIsAnswered`).
    fn revision_standing(
        payload: &StagedPayload,
        record: &StagedRecord,
        winners: &Winners,
    ) -> Option<Standing> {
        let (winner, content_digest, received_no_earlier) = match record {
            StagedRecord::Span {
                trace_id,
                span_id,
                content_digest,
                ..
            } => (
                winners.spans.get(&(trace_id.clone(), span_id.clone())),
                content_digest,
                false,
            ),
            StagedRecord::Metric {
                datapoint_id,
                content_digest,
                ..
            } => (winners.metrics.get(datapoint_id), content_digest, true),
            StagedRecord::Log { .. } => return None,
        };
        Some(match winner {
            Some(winner)
                if &winner.content_digest == content_digest
                    && (!received_no_earlier || winner.ingested_at >= payload.created_at) =>
            {
                Standing::Stored
            }
            // No earlier: two exports received in the same microsecond have no order, and either may win.
            Some(winner)
                if &winner.content_digest != content_digest
                    && winner.ingested_at >= payload.created_at =>
            {
                Standing::Superseded
            }
            _ => Standing::Missing,
        })
    }

    /// The spans of `records` the payload's own raw record holds, read from its staged body.
    async fn held_by_own_record(
        &self,
        payload: &StagedPayload,
        records: &[&StagedRecord],
    ) -> Result<HashSet<(String, String)>, StagingError> {
        let spans: Vec<(String, String)> = records
            .iter()
            .filter_map(|record| match record {
                StagedRecord::Span {
                    trace_id, span_id, ..
                } => Some((trace_id.clone(), span_id.clone())),
                _ => None,
            })
            .collect();
        if spans.is_empty() {
            return Ok(HashSet::new());
        }
        let bytes = match self
            .storage
            .get(&payload.project_id, &payload.blob_hash)
            .await
        {
            Ok(bytes) => bytes,
            Err(FileStorageError::NotFound { .. }) => {
                return Err(StagingError::MissingBlob(payload.id.clone()));
            }
            Err(error) => return Err(blob_error(error)),
        };
        let received = crate::received::staged_received(&bytes, payload.created_at)
            .map_err(StagingError::Blob)?;
        crate::raw_coverage::held_by_own_record(
            self.analytics.as_ref(),
            &payload.project_id,
            &received,
            &spans,
        )
        .await
        .map_err(|error| StagingError::Registry(error.to_string()))
    }

    /// Whether the raw authority holds every span of `records`: each in the record its winning row names.
    ///
    /// Rows matching the export's content are not enough to settle it. The rows are a cache of the raw record,
    /// so an export is stored only when a record holds its spans - and the write path can commit rows whose
    /// record it then failed to repair. An exact redelivery, re-encoded or not, is covered by the record its
    /// rows already name: the telemetry is the same, and keeping a second encoding of it would be a second
    /// copy. Only traces have raw records.
    async fn raw_covers(
        &self,
        payload: &StagedPayload,
        records: &[StagedRecord],
    ) -> Result<bool, StagingError> {
        if payload.signal != StagedSignal::Traces {
            return Ok(true);
        }
        let spans: Vec<(String, String)> = records
            .iter()
            .filter_map(|record| match record {
                StagedRecord::Span {
                    trace_id, span_id, ..
                } => Some((trace_id.clone(), span_id.clone())),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let covered =
            crate::raw_coverage::covered(self.analytics.as_ref(), &payload.project_id, &spans)
                .await
                .map_err(|error| StagingError::Registry(error.to_string()))?;
        Ok(spans.iter().all(|span| covered.contains(span)))
    }

    /// Release terminal payloads. Returns the observed disposition.
    pub async fn settle(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        let disposition = self.disposition(payload).await?;
        if disposition != StagingDisposition::Pending {
            self.release(payload).await?;
        }
        Ok(disposition)
    }

    /// Record a failed write/read-back cycle. At the configured cap the row is
    /// quarantined and the bytes remain held indefinitely.
    pub async fn note_failed_attempt(&self, id: &str) -> Result<bool, StagingError> {
        let attempts = self
            .database
            .increment_staged_redrive_attempts(id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?;
        if attempts >= self.redrive_cap {
            self.database
                .mark_staged_unconfirmed(id)
                .await
                .map_err(|error| StagingError::Registry(error.to_string()))?;
            tracing::error!(
                staged_payload_id = id,
                attempts,
                "Staged OTLP payload exhausted its redrive cap and remains held as unconfirmed"
            );
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn release(&self, payload: &StagedPayload) -> Result<(), StagingError> {
        // Registry first: a crash after it leaves an unreferenced blob, while
        // the opposite order leaves a live registry row pointing at no bytes.
        self.database
            .delete_staged_payload(&payload.id)
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))?;
        if let Err(error) = self
            .storage
            .delete(&payload.project_id, &payload.blob_hash)
            .await
        {
            // The terminal fact is the registry deletion. A failed physical
            // delete leaves reclaimable garbage, not an unconfirmed delivery.
            tracing::error!(
                staged_payload_id = %payload.id,
                blob_hash = %payload.blob_hash,
                %error,
                "Could not remove a terminal staging blob; it is now an orphan"
            );
        }
        Ok(())
    }

    /// Reconcile one page of registry rows. This repairs both queue loss and
    /// inline requests interrupted after their durable staging write.
    pub async fn redrive_once(
        &self,
        trace_pipeline: &TracePipeline,
        limit: usize,
    ) -> Result<usize, StagingError> {
        let payloads = self.pending(limit).await?;
        let count = payloads.len();
        for payload in payloads {
            match self.disposition(&payload).await? {
                StagingDisposition::Confirmed | StagingDisposition::DeliberatelyAbsent => {
                    self.release(&payload).await?;
                    continue;
                }
                StagingDisposition::Pending => {}
            }

            let Some((current, bytes)) = self.load(&payload.id).await? else {
                continue;
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
                        Ok(request) => crate::metrics::ingest(
                            &request,
                            self.analytics.as_ref(),
                            self.database.as_ref(),
                            current.created_at,
                        )
                        .await
                        .is_ok(),
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
                        Ok(request) => crate::logs::ingest(
                            &request,
                            self.analytics.as_ref(),
                            self.database.as_ref(),
                            current.created_at,
                        )
                        .await
                        .is_ok(),
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

            if write_ok && self.settle(&current).await? != StagingDisposition::Pending {
                continue;
            }
            let _ = self.note_failed_attempt(&current.id).await?;
        }
        Ok(count)
    }

    async fn all_confirmed(&self, payload: &StagedPayload) -> Result<bool, StagingError> {
        match payload.signal {
            StagedSignal::Traces => {
                let records = payload
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
                    .collect::<Vec<_>>();
                self.analytics
                    .spans_match_content(&payload.project_id, &records)
                    .await
            }
            // The rule settlement applies record by record, read from the same winners: a fast path that only
            // matched content would settle an export whose content an earlier export stored (`revision_standing`).
            StagedSignal::Metrics => {
                let winners = self.winners(payload).await?;
                return Ok(payload.records.iter().all(|record| {
                    matches!(
                        Self::revision_standing(payload, record, &winners),
                        Some(Standing::Stored)
                    )
                }));
            }
            StagedSignal::Logs => {
                let records = payload
                    .records
                    .iter()
                    .filter_map(|record| match record {
                        StagedRecord::Log {
                            log_digest,
                            ordinal,
                            timestamp,
                            own_instant,
                            ..
                        } => Some((
                            log_digest.clone(),
                            *ordinal,
                            own_instant.then_some(*timestamp),
                        )),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                self.analytics
                    .logs_match_content(&payload.project_id, &records)
                    .await
            }
        }
        .map_err(|error| StagingError::Registry(error.to_string()))
    }

    /// Whether a log record is stored; spans and datapoints are read through their winners ([`Self::standing`]).
    async fn log_confirmed(
        &self,
        project_id: &ProjectId,
        record: &StagedRecord,
    ) -> Result<bool, StagingError> {
        let StagedRecord::Log {
            log_digest,
            ordinal,
            timestamp,
            own_instant,
            ..
        } = record
        else {
            return Ok(false);
        };
        self.analytics
            .logs_match_content(
                project_id,
                &[(
                    log_digest.clone(),
                    *ordinal,
                    own_instant.then_some(*timestamp),
                )],
            )
            .await
            .map_err(|error| StagingError::Registry(error.to_string()))
    }

    async fn record_deliberately_absent(
        &self,
        project_id: &ProjectId,
        record: &StagedRecord,
    ) -> Result<bool, StagingError> {
        if let Some(max_age) = self.retention.max_age_minutes {
            let cutoff = self.clock.now()
                - TimeDelta::try_minutes(i64::try_from(max_age).unwrap_or(i64::MAX))
                    .unwrap_or(TimeDelta::MAX);
            if record.timestamp() < cutoff {
                return Ok(true);
            }
        }

        let journaled = match record {
            StagedRecord::Span {
                trace_id, span_id, ..
            } => {
                self.database
                    .deletion_is_journaled(project_id, DeletionScope::Span, trace_id, Some(span_id))
                    .await
            }
            StagedRecord::Log {
                trace_id: Some(trace_id),
                span_id,
                ..
            } => {
                self.database
                    .deletion_is_journaled(
                        project_id,
                        if span_id.is_some() {
                            DeletionScope::Span
                        } else {
                            DeletionScope::Trace
                        },
                        trace_id,
                        span_id.as_deref(),
                    )
                    .await
            }
            StagedRecord::Metric { .. } | StagedRecord::Log { trace_id: None, .. } => {
                return Ok(false);
            }
        };
        journaled.map_err(|error| StagingError::Registry(error.to_string()))
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

fn blob_error(error: FileStorageError) -> StagingError {
    StagingError::Blob(error.to_string())
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "staging_retirement_tests.rs"]
mod retirement_tests;

#[cfg(test)]
#[path = "staging_fault_tests.rs"]
mod fault_tests;

#[cfg(test)]
#[path = "staging_precedence_tests.rs"]
mod precedence_tests;
