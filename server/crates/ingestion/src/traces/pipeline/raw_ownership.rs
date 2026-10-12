//! An ingest's claim on its raw records, after its rows commit: the model's `Confirm`, `Rekey` and `Check`
//! (`server/specs/RawRecordOwnership.tla`).
//!
//! The rows are written; their traces now claim the records they name, through the arbiter's confirm fence in
//! the transactional store. A claim rests on a version keyed above every collection decision of its record; a
//! record with no live version is keyed afresh before it is claimed (`Rekey`). Only after the claim is granted
//! is the record checked against the rows (repaired if it does not hold them), and only after that may the
//! payload settle.
//!
//! A claim the fence refuses, or answers with a pending decision, was overtaken by a collection decided after
//! the ingest's live read - and that collection's purge removes the rows no trace owns, possibly this ingest's.
//! The batch then fails rather than re-keying in place, and its redelivery writes the rows again and keys above
//! the decision. The payload is registered as the record's pending writer until it settles, and no decision
//! is made on a record with one, so only an execution that outlived its payload's settlement, or an inline
//! ingest that was never registered, can be overtaken; the inline ingest answers failure before acknowledging.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use sideseat_ports::types::{RawConfirmOutcome, RawOwnerClaim, RawRecordRow, raw_version_epoch};

use super::raw_lifecycle::WrittenRecord;
use super::*;

/// Late versions already counted and logged, so repeated reads of one do not count it again. Bounded: an
/// evicted version seen again counts again, which over-reports only after this many distinct late versions.
const LATE_LANDINGS_REMEMBERED: usize = 1024;

/// Distinct versions this process found at or below their record's high-water mark - keyed under a decision
/// whose purge had already completed, so they landed after it: a landing that beat the landing horizon, which
/// only an exceeded clock-skew or execution bound produces (`docs/engineering/compact-storage.md`). A version
/// under a decision still pending its purge is not counted: that purge is the ordinary way it goes.
static LATE_RAW_LANDINGS: AtomicU64 = AtomicU64::new(0);
static LATE_RAW_LANDINGS_SEEN: Mutex<Vec<(String, String, i64)>> = Mutex::new(Vec::new());

/// How many distinct versions this process has found landed after their record's completed purge.
pub fn late_raw_landings() -> u64 {
    LATE_RAW_LANDINGS.load(Ordering::Relaxed)
}

/// Count and log a version found at or below its record's high-water mark, once per version remembered.
pub(super) fn note_late_landing(project_id: &ProjectId, record: &RawRecordRow, high_water: i64) {
    let key = (
        project_id.to_string(),
        record.raw_id.clone(),
        record.version,
    );
    let first = match LATE_RAW_LANDINGS_SEEN.lock() {
        Ok(mut seen) => {
            if seen.contains(&key) {
                false
            } else {
                if seen.len() >= LATE_LANDINGS_REMEMBERED {
                    seen.remove(0);
                }
                seen.push(key);
                true
            }
        }
        Err(_) => true,
    };
    if !first {
        return;
    }
    LATE_RAW_LANDINGS.fetch_add(1, Ordering::Relaxed);
    tracing::warn!(
        project_id = %project_id,
        raw_id = %record.raw_id,
        version_epoch = raw_version_epoch(record.version),
        high_water,
        epochs_late = high_water - raw_version_epoch(record.version),
        received_secs_ago = (Utc::now() - record.received_at).num_seconds(),
        "A raw version landed at or below its record's completed collection: it is invisible and reclaimed on \
         the record's next pass, but the landing beat the landing horizon - check the clocks and execution \
         bounds of the analytics store"
    );
}

/// The latest version of each record that readers may see: the stored latest, unless it is keyed at or below
/// its record's cutoff - then every version is (versions order by epoch first), and the record reads as absent.
pub(super) async fn live_raw_records(
    pipeline: &TracePipeline,
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<HashMap<String, RawRecordRow>, String> {
    let cutoffs = pipeline
        .file_service
        .database()
        .raw_record_cutoffs(project_id, raw_ids)
        .await
        .map_err(|error| error.to_string())?;
    let stored = pipeline
        .analytics
        .get_raw_records(project_id, raw_ids)
        .await
        .map_err(|error| error.to_string())?;
    let mut live = HashMap::new();
    for record in stored {
        match cutoffs.get(&record.raw_id) {
            Some(cutoff) if raw_version_epoch(record.version) <= cutoff.epoch() => {
                if raw_version_epoch(record.version) <= cutoff.high_water {
                    note_late_landing(project_id, &record, cutoff.high_water);
                }
            }
            _ => {
                live.insert(record.raw_id.clone(), record);
            }
        }
    }
    Ok(live)
}

impl TracePipeline {
    /// After the rows are written: claim each written record through the confirm fence - keying a fresh
    /// version first when there is no live one, or when the fence refuses - and then check the record holds
    /// the rows. A failure fails the batch; the payload does not settle.
    pub(super) async fn own_raw_records(
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
            let mut live = live_raw_records(self, &project_id, &ids).await?;
            let epoch = self.raw_epoch(&project_id).await?;

            // A record with no live version - collected since the insert looked, or never inserted because a
            // stale doomed copy looked present - is keyed afresh before it is claimed (`Rekey`).
            for item in &items {
                if !live.contains_key(item.draft.raw_id()) {
                    let fresh = self.append_raw_key(item, None, epoch).await?;
                    live.insert(fresh.raw_id.clone(), fresh);
                }
            }

            let claims: Vec<RawOwnerClaim> = items
                .iter()
                .map(|item| RawOwnerClaim {
                    raw_id: item.draft.raw_id().to_string(),
                    version_epoch: item.claim_epoch,
                    trace_ids: traces_of(item),
                })
                .collect();
            let outcomes = self
                .file_service
                .database()
                .confirm_raw_owners(&project_id, &claims)
                .await
                .map_err(|error| error.to_string())?;
            // A refusal or a pending decision means the claim rests on an epoch some collection bounds, which the
            // fence will never grant on. The batch fails, and its redelivery reads the epoch afresh, keys its record
            // above the decision and claims it.
            let contested: Vec<String> = items
                .iter()
                .zip(&outcomes)
                .filter(|(_, outcome)| !matches!(outcome, RawConfirmOutcome::Granted))
                .map(|(item, _)| item.draft.raw_id().to_string())
                .collect();
            if !contested.is_empty() {
                // Queued, so a pass looks at the version this batch may have keyed: a redelivery claims the record
                // again, and an execution whose payload already settled leaves only that version for a pass to
                // collect. Its rows stay, as every unacknowledged write's do, until their trace's retention.
                if let Err(error) = self
                    .analytics
                    .enqueue_raw_records(&project_id, &contested)
                    .await
                {
                    tracing::warn!(%error, "Could not queue the records a refused claim leaves rows for");
                }
                let contested = contested.len();
                return Err(format!(
                    "{contested} raw record(s) were collected while this batch wrote them; the batch is \
                     refused and delivered again"
                ));
            }

            // The check, once the rows own the record (`Check`): a repair above the latest at the epoch now.
            let epoch = self.raw_epoch(&project_id).await?;
            for item in &items {
                let current = live.get(item.draft.raw_id());
                if let Some(current) = current
                    && item.draft.covers(current, &item.written)
                {
                    continue;
                }
                let repair = self.append_raw_key(item, current, epoch).await?;
                tracing::debug!(
                    raw_id = %repair.raw_id,
                    "The latest raw record did not hold this ingest's rows; appended the union"
                );
            }
        }
        Ok(())
    }

    async fn raw_epoch(&self, project_id: &ProjectId) -> Result<i64, String> {
        self.file_service
            .database()
            .raw_arbiter_epoch(project_id)
            .await
            .map_err(|error| error.to_string())
    }

    /// Append the union of `latest`'s content and what this ingest kept, keyed above `latest` at `epoch`, and
    /// queue it (`append_raw_records`). Returns the version written.
    async fn append_raw_key(
        &self,
        item: &WrittenRecord<'_>,
        latest: Option<&RawRecordRow>,
        epoch: i64,
    ) -> Result<RawRecordRow, String> {
        let version = item.draft.repair_row(
            item.request,
            latest,
            &item.kept,
            item.draft.received_at(),
            item.hold_until,
            epoch,
        )?;
        self.analytics
            .append_raw_records(std::slice::from_ref(&version))
            .await
            .map_err(|error| error.to_string())?;
        Ok(version)
    }
}

/// The distinct traces of an ingest's written spans: what its claim takes ownership through.
fn traces_of(item: &WrittenRecord<'_>) -> Vec<String> {
    let mut traces: Vec<String> = item
        .written
        .iter()
        .map(|(trace_id, _)| trace_id.clone())
        .collect();
    traces.sort_unstable();
    traces.dedup();
    traces
}
