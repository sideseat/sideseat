//! Stored raw telemetry: the authority every derived row is rebuilt from.

use chrono::{DateTime, Utc};

use super::{ProjectId, StagedSignal};

/// One received export as stored: an SSR1 record (see `sideseat_domain::raw_payload`) and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecordRow {
    pub project_id: ProjectId,
    /// Hex SHA-256 of the project and the record as first stored. Stable for the life of the row, so the derived
    /// rows that name it keep naming it after a deletion rewrites the record.
    pub raw_id: String,
    pub signal: StagedSignal,
    pub received_at: DateTime<Utc>,
    /// The record no longer holds every record it was received with, because some were deleted.
    pub rewritten: bool,
    pub record: Vec<u8>,
}
