//! Keeping raw records and their rows agreeing: the reconciler that removes deleted content and collects
//! records nothing owns. The ingest's side - its claim and check after its rows commit - is `raw_ownership`.
//!
//! The protocol, and the interleavings it survives, is `server/specs/RawRecordOwnership.tla`; each step here
//! names the action it implements. Who owns a record is decided in the transactional store, never from the
//! analytics rows:
//!
//! - every deletion and expiry of span rows enqueues the records holding those traces; a journalled deletion
//!   takes its traces' ownership in its own transaction, and the retention pass takes it once the rows are gone;
//! - the reconciler, for each queued record, asks the arbiter for a collection decision - granted only when
//!   nothing owns or writes the record and no hold is active - and, when it gets one, purges the versions the
//!   decision's epoch bounds and the rows no trace owns, then drops the decision into the record's high-water
//!   mark; otherwise it rewrites the record without the spans the deletion fences now refuse, or purges the
//!   versions below the latest it read; then it looks again and re-enqueues anything still unsettled;
//! - a decision whose collector died, or is merely slow, is purged again by age - safe either way, because the
//!   purge can only reach versions keyed at or below the decision's epoch.
//!
//! A rewrite removes only spans the *deletion* fences refuse - a deleted trace or session, a journalled span -
//! never spans that merely lack a row, because a row can be in flight: tombstones only grow, so a rewrite never
//! drops what a later ingest still needs.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ports::error::DataError;
use sideseat_ports::types::{RawOrigin, RawPending, RawRecordRow};

use super::raw::{self as raw_record, RawDraft, SpanKey};
use super::*;

/// Entries one reconciliation pass reads.
const RAW_RECONCILE_BATCH: usize = 64;

/// How long a queued entry waits before a pass reads it: the landing horizon. A version whose entry was
/// enqueued before the horizon has landed by the time its entry is read, so the pass sees it. Efficiency for
/// the privacy bound, not correctness: a landing later than this is invisible behind its record's cutoff and
/// reclaimed on the record's next touch or its time-to-live (`RawRecordOwnership.tla`, `FencedLanding`).
const RAW_LANDING_HORIZON: chrono::Duration = chrono::Duration::seconds(30);

/// How old a pending collection decision must be before another pass purges it again: long enough that a
/// collector that is merely busy rarely sees its work repeated - repeating it is safe, only wasted.
const RAW_COLLECTION_TAKEOVER_AGE: chrono::Duration = chrono::Duration::minutes(5);

/// How often the reconciler looks at its queue. Deleted content leaves the raw store within about this long
/// of the deletion, and an idle queue costs one indexed read per interval.
const RAW_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// One request's record after its rows were written.
pub(super) struct WrittenRecord<'a> {
    pub draft: &'a RawDraft,
    /// The arbiter epoch this ingest read before storing its record and writing its rows: what its claim rests on.
    pub claim_epoch: i64,
    pub request: &'a ExportTraceServiceRequest,
    /// What the deletion fences kept, the content of this ingest's own version.
    pub kept: HashSet<SpanKey>,
    /// What was written as rows.
    pub written: HashSet<SpanKey>,
    pub hold_until: Option<DateTime<Utc>>,
}

/// What reconciling one record came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Reconciled {
    /// Nothing left to do for the entries read.
    Settled,
    /// A legal hold is active: nothing may be deleted or rewritten, and the entries stay queued.
    Held,
}

impl TracePipeline {
    /// Queue for the reconciler the records a failed write stored for projects whose rows did not commit.
    ///
    /// Records are written before their rows, so a write that fails leaves records no row names, and only the
    /// reconciler collects those: span retention never finds a record without spans. Collecting is safe against
    /// the retry that follows. The reconciler restores a record whose rows appear after its delete, and the
    /// retry's own repair re-creates one deleted before they did - a record that is missing is not an exact
    /// redelivery's cover, so the retry writes its rows and repairs. Best effort: the write has already failed,
    /// and a record left behind still goes with its time-to-live.
    ///
    /// A project whose write is `in_doubt` is left alone: its rows may still land, and a record collected before
    /// they do leaves them naming nothing, with no repair to follow - that write never returned. Its record
    /// waits for the retry, whose write either lands the rows the record holds or settles the question.
    pub(super) async fn enqueue_records_without_rows(
        &self,
        records: &[RawRecordRow],
        committed: &HashSet<String>,
        in_doubt: &HashSet<String>,
    ) {
        let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for record in records.iter().filter(|record| {
            let project = record.project_id.as_str();
            !committed.contains(project) && !in_doubt.contains(project)
        }) {
            by_project
                .entry(record.project_id.as_str())
                .or_default()
                .push(record.raw_id.clone());
        }
        for (project, raw_ids) in by_project {
            if let Err(error) = self
                .analytics
                .enqueue_raw_records(&ProjectId::from(project), &raw_ids)
                .await
            {
                tracing::error!(
                    %error,
                    project_id = project,
                    records = raw_ids.len(),
                    "Could not queue a failed write's raw records for collection; they stay until their \
                     time-to-live"
                );
            }
        }
    }

    /// Reconcile the reconciler's queue in the background until shutdown.
    pub fn start_raw_reconciler(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(RAW_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown.changed() => break,
                }
                // Decisions a collector left unpurged - it died, or is slow - are purged again by age.
                if let Err(error) = self.resume_stale_raw_collections().await {
                    tracing::warn!(%error, "Could not resume stale raw collections; retrying next interval");
                }
                // Drain what is queued, a batch at a time, without starving shutdown.
                loop {
                    let due = Utc::now() - RAW_LANDING_HORIZON;
                    match self
                        .reconcile_raw_records_before(RAW_RECONCILE_BATCH, Some(due))
                        .await
                    {
                        Ok(progress) if progress == RAW_RECONCILE_BATCH => continue,
                        Ok(_) => break,
                        Err(error) => {
                            tracing::warn!(%error, "Raw record reconciliation failed; retrying next interval");
                            break;
                        }
                    }
                }
            }
        })
    }

    /// One pass at a landing horizon of zero: every queued entry is due. Correct at any horizon - a late landing
    /// is invisible behind its record's cutoff and found again - so tests and drains use this one.
    pub async fn reconcile_raw_records(&self, limit: usize) -> Result<usize, DataError> {
        self.reconcile_raw_records_before(limit, None).await
    }

    /// One pass: read up to `limit` entries - those enqueued before `enqueued_before` when given - reconcile
    /// each record once, and clear the entries of every record that settled. Returns how many were cleared.
    pub async fn reconcile_raw_records_before(
        &self,
        limit: usize,
        enqueued_before: Option<DateTime<Utc>>,
    ) -> Result<usize, DataError> {
        let entries = self
            .analytics
            .pending_raw_records(limit, enqueued_before)
            .await?;
        let mut by_record: BTreeMap<(ProjectId, String), Vec<RawPending>> = BTreeMap::new();
        for entry in entries {
            by_record
                .entry((entry.project_id.clone(), entry.raw_id.clone()))
                .or_default()
                .push(entry);
        }
        let mut cleared = 0usize;
        for ((project_id, raw_id), entries) in by_record {
            match self.reconcile_raw_record(&project_id, &raw_id).await {
                Ok(Reconciled::Settled) => {
                    self.analytics.clear_raw_pending(&entries).await?;
                    cleared += entries.len();
                }
                Ok(Reconciled::Held) => {}
                Err(error) => tracing::warn!(
                    project_id = %project_id,
                    raw_id,
                    %error,
                    "Could not reconcile a raw record; its entries stay queued"
                ),
            }
        }
        Ok(cleared)
    }

    /// Reconcile one record: the model's `RRead`, `RAct` and `RRecheck`.
    pub async fn reconcile_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Reconciled, String> {
        if self.hold_active(project_id).await? {
            return Ok(Reconciled::Held);
        }
        let read = self.reconcile_read(project_id, raw_id).await?;
        if let Some(read) = &read
            && read.hold_until.is_some_and(|until| until > Utc::now())
        {
            return Ok(Reconciled::Held);
        }
        self.reconcile_act(project_id, raw_id, read.as_ref())
            .await?;
        self.reconcile_recheck(project_id, raw_id).await?;
        Ok(Reconciled::Settled)
    }

    /// The model's `RRead`: the latest version readers may see, if the record has one.
    pub(super) async fn reconcile_read(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Option<RawRecordRow>, String> {
        Ok(
            super::raw_ownership::live_raw_records(self, project_id, &[raw_id.to_string()])
                .await?
                .remove(raw_id),
        )
    }

    /// The model's `RAct`:
    ///
    /// - nothing live: purge what the record's cutoff covers - a late landing's bytes - and decide nothing;
    /// - a collection decision, asked of the arbiter, which decides it in one transactional statement only when
    ///   nothing owns the record and no writer is registered for it: purge what it bounds and drop it (`RPurge`,
    ///   `RDrop`);
    /// - otherwise the record is owned or written: rewrite it without what the deletion fences refuse, or,
    ///   holding nothing deleted, purge the versions below the latest read, whose excess is deleted content.
    pub(super) async fn reconcile_act(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        read: Option<&RawRecordRow>,
    ) -> Result<(), String> {
        let database = self.file_service.database();
        let Some(read) = read else {
            // Whatever the record's cutoff covers - a late landing's bytes. No reader sees them and the fence never
            // grants on them, so this races nothing, pending decision or not. Nothing is decided: there is no
            // record to collect, and a decision would only leave collection state behind.
            let cutoffs = database
                .raw_record_cutoffs(project_id, &[raw_id.to_string()])
                .await
                .map_err(|error| error.to_string())?;
            if let Some(cutoff) = cutoffs.get(raw_id) {
                self.analytics
                    .purge_raw_versions(project_id, raw_id, cutoff.epoch())
                    .await
                    .map_err(|error| error.to_string())?;
            }
            return Ok(());
        };
        let decided = database
            .collect_raw_records(project_id, &[raw_id.to_string()], Utc::now())
            .await
            .map_err(|error| error.to_string())?;
        if let Some(decision) = decided.first() {
            return self
                .purge_raw_collection(project_id, raw_id, decision.epoch)
                .await;
        }
        if self.rewrite_without_deleted(project_id, read).await? {
            return Ok(());
        }
        self.analytics
            .delete_superseded_raw_versions(project_id, raw_id, read.version)
            .await
            .map_err(|error| error.to_string())
    }

    /// A decision's purge and drop (`RPurge`, `RDrop`): the versions keyed at or below its epoch, then the
    /// decision folded into the record's high-water mark. Safe to repeat, however late: a version above the epoch
    /// is out of reach, and the confirm fence keeps every owned version above it. No row is removed: rows naming
    /// a collected record are those of writes never acknowledged - a batch that failed or was abandoned - which
    /// at-least-once delivery admits; they go with their trace's retention or deletion, on both backends alike.
    pub(super) async fn purge_raw_collection(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
        epoch: i64,
    ) -> Result<(), String> {
        self.analytics
            .purge_raw_versions(project_id, raw_id, epoch)
            .await
            .map_err(|error| error.to_string())?;
        self.file_service
            .database()
            .drop_raw_collection(project_id, raw_id, epoch)
            .await
            .map_err(|error| error.to_string())
    }

    /// Purge again every decision older than the takeover age (`RResume`).
    pub async fn resume_stale_raw_collections(&self) -> Result<usize, String> {
        let stale = self
            .file_service
            .database()
            .stale_raw_collections(
                Utc::now() - RAW_COLLECTION_TAKEOVER_AGE,
                RAW_RECONCILE_BATCH,
            )
            .await
            .map_err(|error| error.to_string())?;
        for decision in &stale {
            self.purge_raw_collection(&decision.project_id, &decision.raw_id, decision.epoch)
                .await?;
            self.reconcile_recheck(&decision.project_id, &decision.raw_id)
                .await?;
        }
        Ok(stale.len())
    }

    /// The model's `RRecheck`: look again, and queue the record if anything is unsettled - deleted content in
    /// the latest, versions below it, nothing owning it, or bytes its cutoff still covers - so whichever pass
    /// acts last sees its own effect.
    pub(super) async fn reconcile_recheck(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<(), String> {
        let database = self.file_service.database();
        let live = self.reconcile_read(project_id, raw_id).await?;
        let unsettled = match &live {
            Some(latest) => {
                database
                    .raw_owner_traces(project_id, raw_id)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_empty()
                    || !self
                        .deleted_spans_of(project_id, &latest.record)
                        .await?
                        .is_empty()
            }
            None => {
                match database
                    .raw_record_cutoffs(project_id, &[raw_id.to_string()])
                    .await
                    .map_err(|error| error.to_string())?
                    .get(raw_id)
                {
                    None => false,
                    Some(_) => !self
                        .analytics
                        .get_raw_records(project_id, &[raw_id.to_string()])
                        .await
                        .map_err(|error| error.to_string())?
                        .is_empty(),
                }
            }
        };
        if unsettled {
            self.enqueue(project_id, raw_id).await?;
        }
        Ok(())
    }

    /// The arbiter epoch of each of these projects, one read per project.
    pub(super) async fn raw_epochs<'a>(
        &self,
        projects: impl Iterator<Item = &'a str>,
    ) -> Result<HashMap<String, i64>, String> {
        let mut epochs = HashMap::new();
        for project in projects {
            if epochs.contains_key(project) {
                continue;
            }
            let epoch = self
                .file_service
                .database()
                .raw_arbiter_epoch(&ProjectId::from(project))
                .await
                .map_err(|error| error.to_string())?;
            epochs.insert(project.to_string(), epoch);
        }
        Ok(epochs)
    }

    async fn hold_active(&self, project_id: &ProjectId) -> Result<bool, String> {
        match &self.storage_governance {
            Some(governance) => governance
                .current_hold(project_id)
                .await
                .map(|hold| hold.is_some())
                .map_err(|error| error.to_string()),
            None => Ok(false),
        }
    }

    async fn enqueue(&self, project_id: &ProjectId, raw_id: &str) -> Result<(), String> {
        self.analytics
            .enqueue_raw_records(project_id, &[raw_id.to_string()])
            .await
            .map_err(|error| error.to_string())
    }

    /// The model's rewrite: the record without the spans the deletion fences now refuse, one key above the one
    /// read at the current epoch. Returns whether the read held anything to remove.
    async fn rewrite_without_deleted(
        &self,
        project_id: &ProjectId,
        read: &RawRecordRow,
    ) -> Result<bool, String> {
        let removed = self.deleted_spans_of(project_id, &read.record).await?;
        if removed.is_empty() {
            return Ok(false);
        }
        let (content, original, available) =
            self.decode_with_media(project_id, &read.record).await?;
        let request = raw_record::request_of(content, &original)?;
        let keep: HashSet<SpanKey> = raw_record::spans_of(&request)
            .into_iter()
            .filter(|key| !removed.contains(key))
            .collect();
        let rewritten = raw_record::without_spans(request, &keep).encode_to_vec();
        let encoded = if self.file_service.is_enabled() {
            raw_payload::encode(&rewritten, RawContent::Protobuf).record
        } else {
            raw_payload::wrap(&rewritten, RawContent::Protobuf)
        };
        // A medium no store holds could only have belonged to a removed span - an object is owned by every
        // trace whose span carries it, and a surviving span's trace still owns it. Should one be referenced
        // anyway, the rewrite would reference nothing, so it is refused and the entry stays queued.
        let missing: Vec<String> = raw_payload::media_hashes(&encoded)
            .map_err(|error| error.to_string())?
            .iter()
            .filter(|hash| !available.contains(*hash))
            .map(hex::encode)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "a span the rewrite keeps references media no store holds: {}",
                missing.join(", ")
            ));
        }
        let epoch = self
            .file_service
            .database()
            .raw_arbiter_epoch(project_id)
            .await
            .map_err(|error| error.to_string())?;
        let version = sideseat_ports::types::raw_version_above(Some(read.version), epoch, 1)
            .ok_or_else(|| {
                format!(
                    "raw record {} has no version key left in its epoch",
                    read.raw_id
                )
            })?;
        let row = RawRecordRow {
            origin: RawOrigin::Deleted,
            version,
            trace_ids: Vec::new(),
            record: encoded,
            ..read.clone()
        };
        let appended = self
            .analytics
            .append_raw_rewrites(std::slice::from_ref(&row))
            .await
            .map_err(|error| error.to_string())?;
        if !appended.contains(&row.raw_id) {
            // Another reconciler collected the record since it was read: it stays collected.
            return Ok(true);
        }
        // The removed spans' media is no longer referenced by this record; their traces' survivors decide.
        let mut traces: Vec<String> = removed.into_iter().map(|(trace, _)| trace).collect();
        traces.sort_unstable();
        traces.dedup();
        if let Err(error) = self
            .file_service
            .reconcile_trace_survivors(project_id, &traces, self.analytics.as_ref())
            .await
        {
            tracing::warn!(%error, "Could not reconcile files after a raw rewrite; the file sweep retries");
        }
        Ok(true)
    }

    /// The spans of a record the deletion fences refuse now: the same fences, in the same order, as an ingest
    /// of the record would apply. Read from the record's shape; no media is needed to find them.
    async fn deleted_spans_of(
        &self,
        project_id: &ProjectId,
        record: &[u8],
    ) -> Result<HashSet<SpanKey>, String> {
        let (content, shape) =
            raw_payload::decode_shape(record).map_err(|error| error.to_string())?;
        let mut request = raw_record::request_of(content, &shape)?;
        crate::otlp::inject_project_id_traces(&mut request, project_id.as_str());
        crate::traces::strip_unstorable_spans(&mut request);
        let spans = process_request(
            &request,
            &self.pricing,
            false,
            &self.file_cache,
            ExtractionMode::PerCarrier,
            self.rules,
        )
        .map(|(spans, _, _)| spans)
        .unwrap_or_default();
        let before: HashSet<SpanKey> = spans
            .iter()
            .map(|span| (span.trace_id.clone(), span.span_id.clone()))
            .collect();
        let mut kept = spans;
        let unreadable = || "could not read the deletion fences".to_string();
        self.drop_spans_for_deleted_sessions(&mut kept)
            .await
            .map_err(|()| unreadable())?;
        self.drop_spans_for_journalled_deletions(&mut kept)
            .await
            .map_err(|()| unreadable())?;
        self.drop_spans_for_deleted_traces(&mut kept)
            .await
            .map_err(|()| unreadable())?;
        let kept: HashSet<SpanKey> = kept
            .into_iter()
            .map(|span| (span.trace_id, span.span_id))
            .collect();
        Ok(before.difference(&kept).cloned().collect())
    }

    /// The record decoded with the media the file store holds; a medium it does not hold is replaced by blank
    /// text of its length (see [`raw_payload::decode_shape`]). Returns which media were real.
    async fn decode_with_media(
        &self,
        project_id: &ProjectId,
        record: &[u8],
    ) -> Result<(RawContent, Vec<u8>, HashSet<[u8; 32]>), String> {
        let hashes = raw_payload::media_hashes(record).map_err(|error| error.to_string())?;
        let mut objects: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
        if self.file_service.is_enabled() {
            for hash in &hashes {
                if let Ok(file) = self
                    .file_service
                    .get_file(project_id, &hex::encode(hash))
                    .await
                {
                    objects.insert(*hash, file.data.to_vec());
                }
            }
        }
        let available: HashSet<[u8; 32]> = objects.keys().copied().collect();
        let blanks = raw_payload::blank_lengths(record).map_err(|error| error.to_string())?;
        let (content, bytes) = raw_payload::decode(record, |hash| match objects.get(hash) {
            Some(bytes) => Some(Cow::Borrowed(bytes.as_slice())),
            None => blanks.get(hash).map(|len| Cow::Owned(vec![0u8; *len])),
        })
        .map_err(|error| error.to_string())?;
        Ok((content, bytes, available))
    }
}
