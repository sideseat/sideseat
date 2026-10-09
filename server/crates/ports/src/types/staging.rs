//! Durable staged-payload metadata shared by the domain and transactional adapters.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ProjectId;

/// The signal encoded by a staged blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedSignal {
    Traces,
    Metrics,
    Logs,
}

impl StagedSignal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Metrics => "metrics",
            Self::Logs => "logs",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "traces" => Some(Self::Traces),
            "metrics" => Some(Self::Metrics),
            "logs" => Some(Self::Logs),
            _ => None,
        }
    }
}

/// One row a delivery must either confirm or prove deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StagedRecord {
    Span {
        trace_id: String,
        span_id: String,
        content_digest: String,
        timestamp: DateTime<Utc>,
    },
    Metric {
        datapoint_id: String,
        content_digest: String,
        timestamp: DateTime<Utc>,
    },
    Log {
        log_digest: String,
        ordinal: u32,
        timestamp: DateTime<Utc>,
        trace_id: Option<String>,
        span_id: Option<String>,
        /// Whether `timestamp` is the record's own instant - its time or observed time, both part of its
        /// digest - rather than the receipt time a record carrying neither is given, which differs per delivery.
        /// Only an own instant may bound a read for the stored row.
        #[serde(default)]
        own_instant: bool,
    },
}

impl StagedRecord {
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::Span { timestamp, .. }
            | Self::Metric { timestamp, .. }
            | Self::Log { timestamp, .. } => *timestamp,
        }
    }
}

/// What the store holds as a span's or a datapoint's current revision, as settling a staged export compares it:
/// the revision's content digest and the instant it was received at, which orders it among the identity's
/// revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinningRevision {
    pub content_digest: String,
    pub ingested_at: DateTime<Utc>,
}

/// Durable registry row pointing at one blob-store object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedPayload {
    pub id: String,
    pub project_id: ProjectId,
    pub signal: StagedSignal,
    pub blob_hash: String,
    pub byte_len: u64,
    /// When the export was received: every write of it stores its rows at this instant, to the microsecond.
    pub created_at: DateTime<Utc>,
    pub redrive_attempts: u32,
    pub unconfirmed: bool,
    pub records: Vec<StagedRecord>,
}

/// Where the registry's sequence stands, for classifying a queue reference whose row is missing.
///
/// Every registration takes the next value of a durable sequence, committed with its row, and its queue
/// reference carries that value. A row lost to a rolled-back commit then shows as a sequence above the
/// high-water mark, or as that sequence now held by a different payload once later registrations reuse it.
/// The protocol, and the one double fault it cannot see, are in `server/specs/StagingRetirement.tla`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSequenceState {
    /// The highest sequence the registry has committed.
    pub high_water: i64,
    /// The payload whose live row holds the sequence asked about, if any.
    pub holder: Option<String>,
}
