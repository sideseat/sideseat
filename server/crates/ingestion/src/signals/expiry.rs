//! Records older than the analytics backend keeps.
//!
//! ClickHouse's tables drop a row older than their TTL by its own timestamp in a merge soon after the row is written, unless a legal
//! hold stamped on it keeps it. Such records are written as any others - the raw record holds the export as it
//! arrived - and the store simply never has their rows. The answer says so: they are counted as rejected, under
//! the horizon's name, rather than the export being retried for rows that will never appear
//! (`StagingService::disposition` settles it).

use super::{Signal, SignalContext, SignalExportError, SignalRejection, storable};

/// What an export loses: records outside the storable range, removed before it is staged, and records older than
/// the backend keeps, which it drops once it has written them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Early {
    pub(super) records: usize,
    pub(super) reason: Option<SignalRejection>,
    /// Of `records`, those still in the request - written, and dropped by the backend.
    expired: usize,
}

impl Early {
    pub(super) fn of(unstorable: usize, expired: Option<(usize, i64)>) -> Self {
        let expired_records = expired.map_or(0, |(records, _)| records);
        let reason = match (unstorable > 0, expired) {
            (true, Some((records, _))) if records > 0 => Some(SignalRejection::Mixed),
            (false, Some((records, days))) if records > 0 => Some(SignalRejection::Expired(days)),
            (true, _) => Some(SignalRejection::Unstorable),
            (false, _) => None,
        };
        Self {
            records: unstorable + expired_records,
            reason,
            expired: expired_records,
        }
    }

    /// These rejections and the expired records `expired` counts, once the export is written.
    pub(super) fn with(self, expired: Option<(usize, i64)>) -> Self {
        Self::of(self.records - self.expired, expired)
    }

    /// The records rejected with `dropped` more the store refused: expired records are still in the request, and
    /// one a fence also dropped - a deleted trace's - is counted once, so the two counts overlap and the larger
    /// stands for both. A lower bound where they do not.
    pub(super) fn combined_with(self, dropped: usize) -> usize {
        (self.records - self.expired) + self.expired.max(dropped)
    }

    /// The partial success it answers with, once the `remaining` records left in the request were written: some
    /// were stored unless every one of them was expired.
    pub(super) fn answer(self, remaining: usize) -> Option<(usize, SignalRejection, bool)> {
        self.reason
            .map(|reason| (self.records, reason, self.expired < remaining))
    }
}

/// How many of `request`'s records are older than the backend keeps, with the horizon in days - none where the
/// backend has no horizon, or the project holds a legal hold, under which the backend keeps them.
///
/// Counted when the export arrives; a record that crosses the horizon while the export is written is dropped
/// too, and settles as deliberately absent, but is not counted.
pub(super) async fn expired<S: Signal>(
    signal: &S,
    request: &S::Request,
    context: &SignalContext<'_>,
) -> Result<Option<(usize, i64)>, SignalExportError> {
    let Some(horizon) = context.staging.expiry_horizon() else {
        return Ok(None);
    };
    let cutoff = storable::horizon_nanos(context.clock.now(), horizon);
    let count = signal.expired(request, cutoff);
    if count == 0 {
        return Ok(None);
    }
    let held = context
        .storage_governance
        .current_hold(&sideseat_ports::types::ProjectId::from(context.project_id))
        .await
        .map_err(|_| SignalExportError::StoreUnavailable)?
        .is_some();
    if held {
        return Ok(None);
    }
    tracing::debug!(
        project_id = context.project_id,
        count,
        "Records older than the backend's expiry horizon arrived; it drops them once written"
    );
    Ok(Some((count, horizon.num_days())))
}
