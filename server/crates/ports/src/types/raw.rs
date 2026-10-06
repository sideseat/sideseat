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
