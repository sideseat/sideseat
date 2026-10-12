//! Stored raw telemetry: the authority every derived row is rebuilt from.

use chrono::{DateTime, Utc};

use super::{ProjectId, StagedSignal};

/// What a stored record's bytes are, relative to the body that was received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawOrigin {
    /// Exactly the received body, media held by hash: decoding reproduces it byte for byte.
    Received,
    /// Re-encoded at ingest without the spans the deletion fences refused (a deleted trace, session or
    /// span, a project being deleted, a timestamp no store can hold). Not the received bytes.
    Fenced,
    /// Rewritten after a deletion removed spans it held. Not the received bytes.
    Deleted,
}

impl RawOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Fenced => "fenced",
            Self::Deleted => "deleted",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "received" => Some(Self::Received),
            "fenced" => Some(Self::Fenced),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// One version of a stored export: an SSR1 record (see `sideseat_domain::raw_payload`) and where it came from.
///
/// A record has versions: the first insert, an ingest's repair, a rewrite after a deletion. Readers take the
/// latest by `version`, which is what both backends answer - DuckDB by ordering, ClickHouse because
/// `ReplacingMergeTree(version)` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecordRow {
    pub project_id: ProjectId,
    /// Hex BLAKE3 of the project and the received body. Stable across versions, so the rows that name a record
    /// keep naming it after a deletion rewrites it.
    pub raw_id: String,
    pub signal: StagedSignal,
    pub received_at: DateTime<Utc>,
    pub origin: RawOrigin,
    /// Orders the versions of one record; see the raw lifecycle in `docs/engineering/compact-storage.md`.
    pub version: i64,
    /// The latest signal time the record carries, the instant its rows' retention is measured from - so a
    /// time-to-live on the record can be the latest of theirs.
    pub signal_until: DateTime<Utc>,
    /// The project's legal hold when the version was written, as the rows derived from it carry it.
    pub hold_until: Option<DateTime<Utc>>,
    /// The traces this version holds spans of. Written to the trace index (`otel_raw_traces`) with the record,
    /// which is how a deletion finds a record when no row names it any more - the row expired, or was a
    /// superseded revision a merge removed. Written, not read back: reads return it empty, and the record itself
    /// is the answer to what it holds.
    pub trace_ids: Vec<String>,
    pub record: Vec<u8>,
}

/// A record whose rows changed - deleted, expired, or written by a repair - and that must be reconciled
/// with them. `token` identifies the entry, so a reconciler clears exactly the entries it read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawPending {
    pub project_id: ProjectId,
    pub raw_id: String,
    pub token: String,
}

/// One record's ownership claim an ingest confirms after its rows commit: the record, the epoch of the version
/// the claim rests on, and the traces whose rows were written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawOwnerClaim {
    pub raw_id: String,
    /// The arbiter epoch under which the version this claim rests on was keyed - the one the writer read before
    /// its insert, or the latest version it observed (`RawRecordOwnership.tla`, `vep`).
    pub version_epoch: i64,
    pub trace_ids: Vec<String>,
}

/// The confirm fence's verdict for one claim (`RawRecordOwnership.tla`, `Confirm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawConfirmOutcome {
    /// The claim's version is above every collection decision, pending or completed; its traces own the record
    /// durably, except those the deletion journal took meanwhile.
    Granted,
    /// The claim's version is at or below a decision some purge is entitled to delete: nothing was granted, and
    /// the writer must re-key its version above `fresh_epoch` and confirm again (`Rekey`).
    Refused { fresh_epoch: i64 },
}

/// A collection decision pending its purge (`RawRecordOwnership.tla`, `acoll`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCollection {
    pub project_id: ProjectId,
    pub raw_id: String,
    /// The project epoch the decision was recorded at; the purge deletes versions keyed at or below it.
    pub epoch: i64,
    pub decided_at: DateTime<Utc>,
}

/// The bits of a stored version below its epoch: a version number is `(epoch << 32) | n`, so versions order by
/// epoch first and by `n` within it - what `RawRecordOwnership.tla`'s `Below` orders keys by, and what DuckDB's
/// ordering and ClickHouse's `ReplacingMergeTree(version_key)` compare as one integer.
pub const RAW_VERSION_EPOCH_SHIFT: u32 = 32;

/// The largest epoch a version can carry: the key stays a non-negative `i64`, which both stores order as the
/// model orders keys. Two billion collection decisions in one project - none is ever expected.
pub const RAW_VERSION_MAX_EPOCH: i64 = (1 << 31) - 1;

/// The largest `n` within an epoch.
pub const RAW_VERSION_MAX_N: i64 = (1 << RAW_VERSION_EPOCH_SHIFT) - 1;

/// The stored version for the `n`-th key in `epoch`, or `None` outside the representable domain - an epoch
/// out of `0..=RAW_VERSION_MAX_EPOCH` or an `n` out of `0..=RAW_VERSION_MAX_N`. Never wrapped: a wrapped key
/// would sort below a purge's bound it should be above.
#[must_use]
pub fn raw_version(epoch: i64, n: i64) -> Option<i64> {
    ((0..=RAW_VERSION_MAX_EPOCH).contains(&epoch) && (0..=RAW_VERSION_MAX_N).contains(&n))
        .then_some((epoch << RAW_VERSION_EPOCH_SHIFT) | n)
}

/// The epoch a stored version was keyed under.
#[must_use]
pub fn raw_version_epoch(version: i64) -> i64 {
    version >> RAW_VERSION_EPOCH_SHIFT
}

/// The key above `latest`, keyed no earlier than `epoch` (`RawRecordOwnership.tla`, `Above`): `step` further
/// within the latest's epoch when it is not behind, else the first key of `epoch`. `None` when the latest's
/// epoch has no room left for `step` more: the key cannot move to another epoch on its own - the epoch is the
/// arbiter's fencing token - so the write is refused rather than given a key that does not order above.
#[must_use]
pub fn raw_version_above(latest: Option<i64>, epoch: i64, step: i64) -> Option<i64> {
    match latest {
        Some(latest) if raw_version_epoch(latest) >= epoch => {
            let n = (latest & RAW_VERSION_MAX_N).checked_add(step)?;
            raw_version(raw_version_epoch(latest), n)
        }
        _ => raw_version(epoch, 1),
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    fn key(epoch: i64, n: i64) -> i64 {
        raw_version(epoch, n).expect("in domain")
    }

    #[test]
    fn versions_order_by_epoch_first_and_n_within_it() {
        assert!(key(1, 1) > key(0, RAW_VERSION_MAX_N));
        assert!(key(2, 3) > key(2, 2));
        assert_eq!(raw_version_epoch(key(7, 5)), 7);
        assert_eq!(raw_version_epoch(key(0, 1)), 0);
        assert!(
            key(RAW_VERSION_MAX_EPOCH, RAW_VERSION_MAX_N) > 0,
            "the largest key stays positive"
        );
    }

    #[test]
    fn a_key_outside_the_domain_is_refused_rather_than_wrapped() {
        assert_eq!(raw_version(RAW_VERSION_MAX_EPOCH + 1, 1), None);
        assert_eq!(raw_version(-1, 1), None);
        assert_eq!(raw_version(0, RAW_VERSION_MAX_N + 1), None);
        assert_eq!(raw_version(0, -1), None);
    }

    #[test]
    fn the_key_above_follows_the_model_s_above() {
        assert_eq!(raw_version_above(None, 3, 2), Some(key(3, 1)));
        assert_eq!(raw_version_above(Some(key(3, 1)), 3, 2), Some(key(3, 3)));
        // A latest behind the epoch is superseded by the epoch's first key.
        assert_eq!(raw_version_above(Some(key(1, 9)), 3, 1), Some(key(3, 1)));
        // A latest ahead of a stale epoch stays ahead.
        assert_eq!(raw_version_above(Some(key(5, 1)), 3, 1), Some(key(5, 2)));
        // An exhausted epoch refuses rather than spilling into the next.
        assert_eq!(
            raw_version_above(Some(key(5, RAW_VERSION_MAX_N)), 3, 1),
            None
        );
    }
}
