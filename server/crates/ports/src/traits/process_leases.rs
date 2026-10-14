//! The leases processes hold on a transactional store, which keep a restore repair and serving apart.

use async_trait::async_trait;

use crate::error::DataError;

/// What a process holds a store's lease for.
///
/// A restore repair needs every other process stopped: one that outlives the restore acts on state the restore
/// took back - a collector paused before it deletes, when it wakes, the bytes of a row the restore brought back
/// (`server/specs/MediaIncarnation.tla`, `CollectorsStopped`). A server's lease excludes a repair's; a repair's
/// excludes both - two repairs at once advance the same sequences and replay the same journal against each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum ProcessRole {
    Server,
    RestoreRepair,
}

impl ProcessRole {
    /// The roles whose live leases this one cannot be held beside.
    pub const fn excludes(self) -> &'static [Self] {
        match self {
            Self::Server => &[Self::RestoreRepair],
            Self::RestoreRepair => &[Self::Server, Self::RestoreRepair],
        }
    }

    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[async_trait]
pub trait ProcessLeases: Send + Sync {
    /// Take or renew `process_id`'s lease for `role`, held for `lease_secs` from the store's now.
    ///
    /// The store's clock, not the caller's, both here and in [`ProcessLeases::live_process_leases`]: processes on
    /// different hosts disagree about the time, and a repair whose clock runs ahead would see a server's
    /// continuously renewed lease as run out.
    async fn renew_process_lease(
        &self,
        process_id: &str,
        role: ProcessRole,
        lease_secs: i64,
    ) -> Result<(), DataError>;

    /// Give up `process_id`'s lease; one not held is not an error.
    async fn end_process_lease(&self, process_id: &str) -> Result<(), DataError>;

    /// The processes other than `except` whose lease for `role` has not expired, in order.
    async fn live_process_leases(
        &self,
        role: ProcessRole,
        except: &str,
    ) -> Result<Vec<String>, DataError>;
}
