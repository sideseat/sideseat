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
    /// The arbiter epoch the claim rests on: the one the writer read before storing its record and writing its
    /// rows (`RawRecordOwnership.tla`, `rowEp`). The fence grants only above every decision of the record.
    pub version_epoch: i64,
    pub trace_ids: Vec<String>,
}

/// The confirm fence's verdict for one claim (`RawRecordOwnership.tla`, `Confirm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawConfirmOutcome {
    /// The claim's epoch is above every collection decision, and none is pending; its traces own the record
    /// durably, except those the deletion journal took meanwhile.
    Granted,
    /// The claim's epoch is at or below a decision some purge is entitled to delete: nothing was granted. The
    /// writer fails its batch, and its redelivery reads the epoch afresh - above `fresh_epoch` - and claims again.
    Refused { fresh_epoch: i64 },
    /// The record has a collection decision pending its purge: nothing was granted, and the writer fails its
    /// batch as for a refusal. No grant is made while a purge is pending, so no owner appears under a decision
    /// whose purge is still to run (`RawRecordOwnership.tla`, `Grantable`).
    Collecting,
}

/// A collected record's state as readers and the fence see it: the highest decision ever purged, and the
/// decision pending its purge, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RawCutoff {
    pub high_water: i64,
    pub pending: Option<i64>,
}

impl RawCutoff {
    /// The highest epoch any purge of the record - pending or completed - is entitled to delete.
    #[must_use]
    pub fn epoch(&self) -> i64 {
        self.pending
            .map_or(self.high_water, |pending| pending.max(self.high_water))
    }
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

/// A rewritten version of a raw record, and the version it was prepared from: what decides whether it may land.
/// A rewrite lands only while the version it was prepared from is still the record's latest - one prepared from
/// older content, whatever key its epoch gives it, would bring back spans and media a newer version no longer
/// holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRewrite {
    pub row: RawRecordRow,
    /// The latest version this rewrite was prepared from.
    pub based_on: RawRecordRow,
}

impl RawRewrite {
    /// Whether `latest` is the version this rewrite was prepared from: its key and its bytes. Two versions can
    /// share a key - a repair and a rewrite keyed above one latest at one epoch - so the key alone does not tell
    /// them apart, while two with one key and the same bytes are the same version to every reader.
    pub fn based_on_latest(&self, latest: &RawRecordRow) -> bool {
        latest.version == self.based_on.version && latest.record == self.based_on.record
    }
}

/// The raw arbiter's integer key for a record: the first eight bytes of its id, which is itself a BLAKE3 digest
/// of the project and the received body - so the key is already project-scoped and uniform. Owner rows carry
/// integers rather than the 64-character id, which is what keeps them small (`raw_owners`).
#[must_use]
pub fn raw_record_key(raw_id: &str) -> i64 {
    let mut bytes = [0u8; 8];
    for (slot, pair) in bytes.iter_mut().zip(raw_id.as_bytes().chunks(2)) {
        let digit = |byte: u8| (byte as char).to_digit(16).unwrap_or(0) as u8;
        *slot = (digit(pair[0]) << 4) | pair.get(1).map_or(0, |byte| digit(*byte));
    }
    i64::from_be_bytes(bytes)
}

/// The raw arbiter's key for a trace, exact rather than hashed, because a key collision here would let one
/// trace's deletion release another trace's ownership. An OpenTelemetry trace id as the rows store it - 32
/// lowercase hex digits - is its 16 bytes. Any other id is `0xFF` and its text, with a second `0xFF` when that
/// would be 16 bytes long: so every 16-byte key is a decoded trace id, and no other key can equal one; and since
/// UTF-8 text never contains `0xFF`, the marker cannot be confused with the text after it.
#[must_use]
pub fn raw_trace_key(trace_id: &str) -> Vec<u8> {
    const TRACE_ID_BYTES: usize = 16;
    let bytes = trace_id.as_bytes();
    let standard = bytes.len() == 2 * TRACE_ID_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    if standard {
        let digit = |byte: u8| (byte as char).to_digit(16).unwrap_or(0) as u8;
        return bytes
            .chunks(2)
            .map(|pair| (digit(pair[0]) << 4) | digit(pair[1]))
            .collect();
    }
    let mut key = Vec::with_capacity(bytes.len() + 2);
    key.push(0xFF);
    if bytes.len() + 1 == TRACE_ID_BYTES {
        key.push(0xFF);
    }
    key.extend_from_slice(bytes);
    key
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
    fn owner_keys_are_stable_and_distinct() {
        let project = ProjectId::from("p");
        assert_eq!(raw_record_key("00000000000000ff"), 0xff);
        assert_eq!(raw_record_key("8000000000000000ffff"), i64::MIN);
        let _ = project;
        let standard = "0aff".repeat(8);
        let decoded: Vec<u8> = [0x0a, 0xff].repeat(8);
        assert_eq!(raw_trace_key(&standard), decoded);
        assert_ne!(
            raw_trace_key(&"0a".repeat(16)),
            raw_trace_key(&"0b".repeat(16))
        );
        assert_eq!(
            raw_trace_key("zz"),
            vec![0xFF, b'z', b'z'],
            "a non-standard id keeps its text behind the marker"
        );
        // Ids that once collided: hex that decodes to the marker and a text, and a text that would be 16 bytes.
        assert_ne!(raw_trace_key("ff7a7a"), raw_trace_key("zz"));
        let fifteen = "z".repeat(15);
        let mut lookalike = vec![0xFF];
        lookalike.extend_from_slice(fifteen.as_bytes());
        assert_eq!(raw_trace_key(&fifteen).len(), 17);
        assert_ne!(raw_trace_key(&fifteen), lookalike);
        assert_ne!(
            raw_trace_key(&"ff".repeat(16)),
            raw_trace_key(&"F".repeat(15))
        );
        assert_ne!(
            raw_trace_key(&"AB".repeat(16)),
            raw_trace_key(&"ab".repeat(16)),
            "upper-case hex is not the stored spelling, and keeps its text"
        );
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
