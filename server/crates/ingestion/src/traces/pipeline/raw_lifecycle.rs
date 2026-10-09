//! Keeping raw records and their rows agreeing: the ingest's repair after its write, and the reconciler that
//! removes deleted content and collects records no row names.
//!
//! The protocol, and the interleavings it survives, is `server/specs/RawRecordOwnership.tla`; each step here
//! names the action it implements. In short:
//!
//! - every deletion and expiry of span rows enqueues the records holding those traces, before the rows go;
//! - an ingest whose rows the latest record does not hold appends the union, two versions up, and enqueues;
//! - the reconciler, unless a legal hold is active, deletes a record no row names (restoring it if rows
//!   appear meanwhile) or rewrites it without the spans the deletion fences now refuse, then looks again and
//!   re-enqueues anything still unsettled - so whichever reconciler acts last sees its own effect.
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

/// How often the reconciler looks at its queue. Deleted content leaves the raw store within about this long
/// of the deletion, and an idle queue costs one indexed read per interval.
const RAW_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// One request's record after its rows were written.
pub(super) struct WrittenRecord<'a> {
    pub draft: &'a RawDraft,
    pub request: &'a ExportTraceServiceRequest,
    /// What the deletion fences kept, the content of this ingest's own version.
    pub kept: HashSet<SpanKey>,
    /// What was written as rows.
    pub written: HashSet<SpanKey>,
    pub hold_until: Option<DateTime<Utc>>,
}

/// The version a reconciler appends to restore a record it deleted under rows that appeared meanwhile: what it
/// read, one version up, with its trace index rebuilt.
///
/// A read returns no trace ids - the record holds the answer - and the delete took the record's trace-index rows
/// with it. Appended as read, the restored record was indexed under no trace, so a later deletion of one of its
/// traces, which finds records through that index, left the deleted content in it.
pub(super) fn restored_version(read: &RawRecordRow) -> RawRecordRow {
    let mut restored = read.clone();
    restored.version = read.version.saturating_add(1);
    restored.trace_ids = match raw_record::record_identities(&read.record) {
        Ok(identities) => identities
            .into_iter()
            .map(|(trace_id, _)| trace_id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        Err(error) => {
            tracing::error!(
                raw_id = %read.raw_id,
                %error,
                "A restored raw record is unreadable, so it cannot be indexed under its traces"
            );
            Vec::new()
        }
    };
    restored
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
    /// The model's `Check`: after the rows are written, the latest record must hold them.
    ///
    /// Almost always one indexed read: the record this ingest inserted, or the received body another inserted,
    /// holds everything. Only a record a deletion rewrote, or one deleted meanwhile, needs the repair.
    pub(super) async fn repair_raw_records(
        &self,
        written: &[WrittenRecord<'_>],
    ) -> Result<(), String> {
        let mut by_project: BTreeMap<&str, Vec<&WrittenRecord<'_>>> = BTreeMap::new();
        for item in written.iter().filter(|item| !item.written.is_empty()) {
            by_project
                .entry(item.draft.project_id())
                .or_default()
                .push(item);
        }
        for (project, items) in by_project {
            let project_id = ProjectId::from(project);
            let mut ids: Vec<String> = items
                .iter()
                .map(|item| item.draft.raw_id().to_string())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            let latest: HashMap<String, RawRecordRow> = self
                .analytics
                .get_raw_records(&project_id, &ids)
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|row| (row.raw_id.clone(), row))
                .collect();
            for item in items {
                let current = latest.get(item.draft.raw_id());
                if let Some(current) = current
                    && item.draft.covers(current, &item.written)
                {
                    continue;
                }
                let repair = item.draft.repair_row(
                    item.request,
                    current,
                    &item.kept,
                    Utc::now(),
                    item.hold_until,
                )?;
                self.analytics
                    .append_raw_records(std::slice::from_ref(&repair))
                    .await
                    .map_err(|error| error.to_string())?;
                self.analytics
                    .enqueue_raw_records(&project_id, std::slice::from_ref(&repair.raw_id))
                    .await
                    .map_err(|error| error.to_string())?;
                tracing::debug!(
                    raw_id = %repair.raw_id,
                    "The latest raw record did not hold this ingest's rows; appended the union"
                );
            }
        }
        Ok(())
    }

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
                // Drain what is queued, a batch at a time, without starving shutdown.
                loop {
                    match self.reconcile_raw_records(RAW_RECONCILE_BATCH).await {
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

    /// One pass: read up to `limit` entries, reconcile each record once, and clear the entries of every record
    /// that settled. Returns how many entries were cleared.
    pub async fn reconcile_raw_records(&self, limit: usize) -> Result<usize, DataError> {
        let entries = self.analytics.pending_raw_records(limit).await?;
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

    /// Reconcile one record with its rows: the model's `RRead`, `RAct` and `RRecheck`.
    pub async fn reconcile_raw_record(
        &self,
        project_id: &ProjectId,
        raw_id: &str,
    ) -> Result<Reconciled, String> {
        let ids = [raw_id.to_string()];
        if self.hold_active(project_id).await? {
            return Ok(Reconciled::Held);
        }
        let read = self
            .analytics
            .get_raw_records(project_id, &ids)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .next();
        let named = self.named(project_id, raw_id).await?;
        if let Some(read) = &read
            && read.hold_until.is_some_and(|until| until > Utc::now())
        {
            return Ok(Reconciled::Held);
        }

        // What this reconciler leaves as the latest version, for the re-check.
        let left: Option<i64> = match (&read, named) {
            (None, _) => None,
            (Some(_), false) => {
                self.analytics
                    .delete_raw_records(project_id, &ids)
                    .await
                    .map_err(|error| error.to_string())?;
                None
            }
            (Some(read), true) => self.rewrite_without_deleted(project_id, read).await?,
        };

        // The re-check.
        let named_now = self.named(project_id, raw_id).await?;
        let latest_now = self
            .analytics
            .get_raw_records(project_id, &ids)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .next();
        if let (Some(read), None, true) = (&read, &latest_now, named_now) {
            // Rows appeared after this reconciler's delete: they get back what it read, which holds them.
            let restored = restored_version(read);
            self.analytics
                .append_raw_records(std::slice::from_ref(&restored))
                .await
                .map_err(|error| error.to_string())?;
            self.enqueue(project_id, raw_id).await?;
        } else {
            let unsettled = (named_now && latest_now.is_none())
                || (!named_now && latest_now.is_some())
                || latest_now.as_ref().map(|row| row.version) != left;
            if unsettled {
                self.enqueue(project_id, raw_id).await?;
            }
        }
        Ok(Reconciled::Settled)
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

    async fn named(&self, project_id: &ProjectId, raw_id: &str) -> Result<bool, String> {
        self.analytics
            .raw_records_named(project_id, &[raw_id.to_string()])
            .await
            .map(|named| named.contains(raw_id))
            .map_err(|error| error.to_string())
    }

    async fn enqueue(&self, project_id: &ProjectId, raw_id: &str) -> Result<(), String> {
        self.analytics
            .enqueue_raw_records(project_id, &[raw_id.to_string()])
            .await
            .map_err(|error| error.to_string())
    }

    /// The model's rewrite: the record without the spans the deletion fences now refuse, one version above the
    /// one read. Returns the version left as the latest.
    async fn rewrite_without_deleted(
        &self,
        project_id: &ProjectId,
        read: &RawRecordRow,
    ) -> Result<Option<i64>, String> {
        let removed = self.deleted_spans_of(project_id, &read.record).await?;
        if removed.is_empty() {
            return Ok(Some(read.version));
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
        let version = read.version.saturating_add(1);
        let row = RawRecordRow {
            origin: RawOrigin::Deleted,
            version,
            trace_ids: Vec::new(),
            record: encoded,
            ..read.clone()
        };
        self.analytics
            .append_raw_records(std::slice::from_ref(&row))
            .await
            .map_err(|error| error.to_string())?;
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
        Ok(Some(version))
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
