//! Durable OTLP staging shared by every signal lifecycle.
//!
//! The blob is written before its registry row. That ordering makes an
//! acknowledged payload discoverable even if the queue loses every entry. A
//! payload leaves staging only after strict producer-content confirmation, or
//! after every absent record is explained by the same deletion/retention rules
//! as ingestion.

use sideseat_core::utils::collection_fence::{CollectionFence, Pass};
use sideseat_domain::storage_governance::StorageGovernanceService;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::TimeDelta;
use prost::Message;
use sideseat_core::config::RetentionConfig;
use sideseat_ports::blobs::{FileStorage, FileStorageError, ObjectName};
use sideseat_ports::clock::Clock;
use sideseat_ports::traits::{AnalyticsRepository, TransactionalRepository};
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

/// What one staged payload is written as, beyond its bytes and records.
#[derive(Debug, Clone, Default)]
pub struct StagedWrite {
    /// Keeps the queue's relative order for records that need one.
    pub partition_key: String,
    /// The raw record a traces payload writes: registering it as the record's pending writer is what blocks
    /// the raw arbiter's collection decision while the payload is unsettled (`RawRecordOwnership.tla`).
    pub raw_id: Option<String>,
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
    /// The registry could not be reached - a dropped or stale connection, a timeout. Nothing about the payload,
    /// and not counted against it; nor is it known whether the payload is still registered, so its queue reference
    /// is kept and the load retried.
    #[error("staging registry unavailable: {0}")]
    RegistryUnavailable(String),
    /// The blob store could not be reached for a payload the registry holds. Nothing about the payload, and not
    /// counted; it stays staged and pending, which the staging sweep redrives.
    #[error("staging blob store unavailable: {0}")]
    BlobUnavailable(String),
    /// The staged bytes are not what the payload's signal says: an attempt that is the payload's own fault.
    #[error("staged payload cannot be read: {0}")]
    Unreadable(String),
    /// The analytics store has failed for good: the attempt said nothing about the payload, and only the process
    /// that replaces this one can write it.
    #[error("the analytics store has failed and the server must restart: {0}")]
    StoreFailed(String),
    /// This process does not hold its store lease: the attempt said nothing about the payload either, and is
    /// not counted against it.
    #[error("this process does not hold its store lease")]
    Fenced,
}

/// Owns the blob/registry lifecycle and the signal-specific confirmation
/// predicates. It deliberately has no time-to-live.
/// The name a staged payload's bytes are stored under. A payload's blob hash covers its own id, so no two
/// payloads share one and no name is reincarnated: incarnation 0.
fn staged_object(blob_hash: &str) -> ObjectName<'_> {
    ObjectName::new(blob_hash, 0)
}

pub struct StagingService {
    storage: Arc<dyn FileStorage>,
    database: Arc<dyn TransactionalRepository + Send + Sync>,
    analytics: Arc<dyn AnalyticsRepository + Send + Sync>,
    clock: Arc<dyn Clock>,
    retention: RetentionConfig,
    redrive_cap: u32,
    fence: CollectionFence,
    /// The legal holds a record older than the backend's expiry horizon is kept under; without it, no record is
    /// taken for expired.
    governance: Option<Arc<StorageGovernanceService>>,
    /// Where this process's staging sweep is in its traversal: the last row it took, and the receipt bound the
    /// traversal goes up to. Each pass continues after the last; a traversal ends at its bound - rows received
    /// after it began wait for the next - and the next starts from the oldest. So rows it could not take, a store
    /// unavailable for them, do not hold the first page while rows behind them wait, and arrivals cannot keep it
    /// from coming back to them. Process-local, and only an order: another instance's sweep keeps its own.
    sweep_cursor: std::sync::Mutex<sweep::SweepCursor>,
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
            fence: CollectionFence::open(),
            governance: None,
            sweep_cursor: std::sync::Mutex::new(sweep::SweepCursor::default()),
        }
    }

    /// Take a record older than the backend's expiry horizon for deliberately absent - the backend dropped it as
    /// it was written - unless `governance` holds its project.
    pub fn with_storage_governance(mut self, governance: Arc<StorageGovernanceService>) -> Self {
        self.governance = Some(governance);
        self
    }

    /// Retire payloads only while `fence` is open: while this process holds its store lease.
    pub fn with_collection_fence(mut self, fence: CollectionFence) -> Self {
        self.fence = fence;
        self
    }

    /// How old a record may be and still be kept by the analytics backend ([`AnalyticsMaintenance`]).
    pub fn expiry_horizon(&self) -> Option<TimeDelta> {
        self.analytics.expiry_horizon()
    }

    /// The fence of this process's store lease, which exports are acknowledged under too.
    pub fn collection_fence(&self) -> &CollectionFence {
        &self.fence
    }

    /// Stage `bytes`, received at `received_at`: the registration keeps the receipt, which every later write of the
    /// payload stores its rows at ([`crate::received::ReceivedPayload`]). A traces payload names the raw record
    /// it writes, which registers it as that record's pending writer (`RawRecordOwnership.tla`, `Register`).
    pub async fn stage(
        &self,
        project_id: &str,
        signal: StagedSignal,
        bytes: &[u8],
        received_at: chrono::DateTime<chrono::Utc>,
        records: Vec<StagedRecord>,
        write: StagedWrite,
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
            .store(&project_id, staged_object(&blob_hash), bytes)
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
            raw_id: write.raw_id,
        };
        let seq = match self.database.create_staged_payload(&payload).await {
            Ok(seq) => seq,
            Err(error) => {
                if let Err(cleanup_error) = self
                    .storage
                    .delete(&payload.project_id, staged_object(&payload.blob_hash))
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
            partition_key: write.partition_key,
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
            .map_err(registry_error)?;
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
            .map_err(registry_error)
    }

    /// A staged payload's registration - what settling it reads - without its bytes, or `None` once it is
    /// retired.
    pub async fn registration(&self, id: &str) -> Result<Option<StagedPayload>, StagingError> {
        self.database
            .get_staged_payload(id)
            .await
            .map_err(registry_error)
    }

    pub async fn load(&self, id: &str) -> Result<Option<(StagedPayload, Vec<u8>)>, StagingError> {
        let Some(payload) = self
            .database
            .get_staged_payload(id)
            .await
            .map_err(registry_error)?
        else {
            return Ok(None);
        };
        let bytes = match self
            .storage
            .get(&payload.project_id, staged_object(&payload.blob_hash))
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
        self.pending_after(None, limit).await
    }

    /// [`Self::pending`] after `after`'s `(created_at, id)`.
    pub async fn pending_after(
        &self,
        after: Option<&StagedPayload>,
        limit: usize,
    ) -> Result<Vec<StagedPayload>, StagingError> {
        self.database
            .pending_staged_payloads(after.map(|p| (p.created_at, p.id.as_str())), limit)
            .await
            .map_err(registry_error)
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
            if self.raw_covers(payload, &payload.records).await? {
                return Ok(StagingDisposition::Confirmed);
            }
            // Rows still found whose raw record the backend has already expired: past every record's age, the
            // record is not coming back, and writing it again would expire it again.
            if payload.records.is_empty() {
                return Ok(StagingDisposition::Pending);
            }
            for record in &payload.records {
                if !self
                    .expired(&payload.project_id, record.timestamp())
                    .await?
                {
                    return Ok(StagingDisposition::Pending);
                }
            }
            return Ok(StagingDisposition::DeliberatelyAbsent);
        }

        if !self
            .database
            .project_accepts_writes(payload.project_id.as_str())
            .await
            .map_err(registry_error)?
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
            // Its rows found but its raw record gone: past every one's age, the backend has expired the record,
            // and a write would expire it again. Any other is still owed.
            for record in &confirmed {
                if !self
                    .expired(&payload.project_id, record.timestamp())
                    .await?
                {
                    return Ok(StagingDisposition::Pending);
                }
            }
            saw_absence = true;
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
        let registry = registry_error;
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
            .get(&payload.project_id, staged_object(&payload.blob_hash))
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
            self.database.as_ref(),
            &payload.project_id,
            &received,
            &spans,
        )
        .await
        .map_err(registry_error)
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
        let covered = crate::raw_coverage::covered(
            self.analytics.as_ref(),
            self.database.as_ref(),
            &payload.project_id,
            &spans,
        )
        .await
        .map_err(registry_error)?;
        Ok(spans.iter().all(|span| covered.contains(span)))
    }

    /// Record a failed load of payload `id`, and answer whether its queue reference may be acknowledged.
    ///
    /// A store that could not be reached is no evidence against the payload, and is not counted: counted, a dropped
    /// connection on a payload's last allowed attempt quarantined it - which takes it out of the staging sweep - and
    /// acknowledged its reference, so persistence stopped until an operator recovered it, on the first failure at a
    /// redrive cap of one.
    ///
    /// - The blob store unavailable for a payload the registry holds: acknowledged, the payload left staged and
    ///   pending for the staging sweep. Left unacknowledged, a Redpanda delivery is never redelivered, and its
    ///   offset would hold back every commit after it.
    /// - The registry unavailable: not acknowledged. Whether the payload is still registered is not known, and the
    ///   reference is what records a registration's loss (`settle_missing`); the caller retries the load.
    /// - A failure that is the payload's own ([`StagingError::is_payload_fault`]) is counted, and the reference
    ///   acknowledged only once the payload is quarantined.
    /// - Anything else - a registry statement that failed, a timeout the store does not call transient - says
    ///   nothing certain about the payload either: kept, and not counted.
    ///
    /// `term` is the store lease's term when the load began ([`Self::lease_term`]). A blob-unavailable reference is
    /// acknowledged only if the lease was held throughout: a restore taken meanwhile may have removed the
    /// registration the load read, and the reference is what would record that loss.
    pub async fn note_load_failure(
        &self,
        id: &str,
        error: &StagingError,
        term: Option<u64>,
    ) -> Result<bool, StagingError> {
        match error {
            StagingError::BlobUnavailable(_) => {
                let held = term.is_some_and(|term| self.fence.held_through(term));
                tracing::warn!(
                    staged_payload_id = id,
                    %error,
                    acknowledged = held,
                    "A staged payload's bytes could not be read; it stays staged for the staging sweep"
                );
                Ok(held)
            }
            error if error.is_payload_fault() => self.note_failed_attempt(id).await,
            _ => Ok(false),
        }
    }

    /// The store lease's term now, for [`Self::note_load_failure`]; None while it is not held.
    pub fn lease_term(&self) -> Option<u64> {
        self.fence.term()
    }

    /// Whether the store lease taken as `term` has been held ever since ([`Self::lease_term`]).
    pub fn lease_held_through(&self, term: Option<u64>) -> bool {
        term.is_some_and(|term| self.fence.held_through(term))
    }

    /// Release terminal payloads. Returns the observed disposition.
    pub async fn settle(
        &self,
        payload: &StagedPayload,
    ) -> Result<StagingDisposition, StagingError> {
        // The reads and the release in one pass: a lease lost between them is not opened again until it is done.
        let pass = self.fence.enter("staging settlement");
        let disposition = self.disposition(payload).await?;
        if disposition != StagingDisposition::Pending {
            self.retire_in(pass.as_ref(), payload).await?;
        }
        Ok(disposition)
    }

    /// Record a failed write/read-back cycle. At the configured cap the row is
    /// quarantined and the bytes remain held indefinitely.
    ///
    /// Not while the analytics store has failed for good: every attempt fails then, whatever the payload, and
    /// counting them would quarantine exports - acknowledged ones among them - that the restarted process
    /// writes at once. The caller stops instead, leaving the payload as it was.
    pub async fn note_failed_attempt(&self, id: &str) -> Result<bool, StagingError> {
        if let Some(failure) = self.analytics.fatal_failure() {
            return Err(StagingError::StoreFailed(failure));
        }
        // An attempt the fence refused - a closed session fence, a settlement left staged - is no evidence
        // against the payload; counted, a long outage would quarantine exports the next pass stores.
        if !self.fence.is_open() {
            return Err(StagingError::Fenced);
        }
        let attempts = self
            .database
            .increment_staged_redrive_attempts(id)
            .await
            .map_err(registry_error)?;
        if attempts >= self.redrive_cap {
            // Again before the quarantine: the count may have been read from a store a restore has since
            // taken back.
            if !self.fence.is_open() {
                return Err(StagingError::Fenced);
            }
            self.database
                .mark_staged_unconfirmed(id)
                .await
                .map_err(registry_error)?;
            tracing::error!(
                staged_payload_id = id,
                attempts,
                "Staged OTLP payload exhausted its redrive cap and remains held as unconfirmed"
            );
            return Ok(true);
        }
        Ok(false)
    }

    /// Release a terminal payload, in a pass of its own.
    pub async fn release(&self, payload: &StagedPayload) -> Result<(), StagingError> {
        let pass = self.fence.enter("staging release");
        self.release_in(pass.as_ref(), payload).await.map(|_| ())
    }

    /// Release a payload a settlement found terminal, or fail with [`StagingError::Fenced`] if the fence kept it
    /// staged: a payload is settled only once retired, so a caller never acknowledges one the fence held back.
    pub(crate) async fn retire_in(
        &self,
        pass: Option<&Pass>,
        payload: &StagedPayload,
    ) -> Result<(), StagingError> {
        if self.release_in(pass, payload).await? {
            Ok(())
        } else {
            Err(StagingError::Fenced)
        }
    }

    /// Release a payload a pass found terminal. A staged payload's bytes are under a name a restore brings back
    /// with the registry row that names them: released on a decision from before the restore, the restored row
    /// would name nothing. So without a pass - the fence was closed when the decision began - or with the fence
    /// closed now, the payload stays staged, and the sweep releases it once the lease is held again. Answers whether
    /// it released.
    async fn release_in(
        &self,
        pass: Option<&Pass>,
        payload: &StagedPayload,
    ) -> Result<bool, StagingError> {
        if pass.is_none() || !self.fence.is_open() {
            tracing::debug!(staged_payload_id = %payload.id, "Collection is fenced; the payload stays staged");
            return Ok(false);
        }
        // Registry first: a crash after it leaves an unreferenced blob, while
        // the opposite order leaves a live registry row pointing at no bytes.
        self.database
            .delete_staged_payload(&payload.id)
            .await
            .map_err(registry_error)?;
        if let Err(error) = self
            .storage
            .delete(&payload.project_id, staged_object(&payload.blob_hash))
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
        Ok(true)
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
        .map_err(registry_error)
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
            .map_err(registry_error)
    }
}

/// A registry failure: unavailable if the store said it was transient - nothing about the payload - else a
/// failure of the operation.
fn registry_error(error: sideseat_ports::error::DataError) -> StagingError {
    if error.is_transient() {
        StagingError::RegistryUnavailable(error.to_string())
    } else {
        StagingError::Registry(error.to_string())
    }
}

impl StagingError {
    /// The failure is the payload's own - its bytes gone, unreadable, or refused by the blob store as what they
    /// are - and so evidence against it, counted toward its quarantine. Nothing else is.
    pub fn is_payload_fault(&self) -> bool {
        matches!(
            self,
            Self::MissingBlob(_) | Self::Unreadable(_) | Self::Blob(_)
        )
    }
}

fn blob_error(error: FileStorageError) -> StagingError {
    match error {
        // The store answered no, or not at all: nothing about the bytes.
        FileStorageError::Backend(_) | FileStorageError::Io(_) => {
            StagingError::BlobUnavailable(error.to_string())
        }
        _ => StagingError::Blob(error.to_string()),
    }
}

mod settlement;
mod sweep;

pub use sweep::start_staging_sweep;

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
