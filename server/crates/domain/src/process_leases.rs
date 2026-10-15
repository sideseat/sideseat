//! A process's lease on the transactional store, held only while no process of the role it excludes holds one.
//!
//! A restore repair must run with every other process stopped, and no server may start while it runs: a process
//! that outlives a restore acts on what the restore took back (`server/specs/MediaIncarnation.tla`,
//! `CollectorsStopped`). Each process takes its own lease before it looks for the other role's, so of two that
//! start together the one that looks second sees the first. A restore takes the lease table back too, and a server
//! still running writes its lease again within one renewal; so a repair looks once more after waiting that long.
//!
//! A server whose lease goes unrenewed keeps serving reads and ingest - a store failover must not take every
//! instance down at once - but closes its [`CollectionFence`] until it holds the lease again: what a repair
//! needs excluded is deletion. The timing makes that hold: a repair looks a second time [`RESTORE_SETTLE`] after
//! its first, a server renews every [`RENEW_EVERY`], which is sooner, and one whose renewals do not land closes
//! its fence [`LOST_AFTER`] after the last that did, which is sooner still. So at the repair's second look every
//! server either holds a lease it sees or has closed its fence; the difference between the two is the margin a
//! collector step already past the fence has to finish in.
//!
//! What a lease cannot stop is a process that paused - stopped, not exited - and takes the step it was about to
//! take when it wakes. The procedure's "stop every process" means exited.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use sideseat_core::utils::collection_fence::{CollectionFence, HeldUntil, Passes};
use sideseat_ports::error::DataError;
use sideseat_ports::traits::{ProcessLeases, ProcessRole};

/// How long a lease lasts from its last renewal.
pub const LEASE_SECS: i64 = 30;

/// How often a held lease is renewed, and a lost one taken again.
pub const RENEW_EVERY: Duration = Duration::from_secs(5);

/// How long a restore repair waits before it looks again: past a renewal of any server still running.
pub const RESTORE_SETTLE: Duration = Duration::from_secs(20);

/// How long a lease may go unrenewed before its holder counts it lost and closes its fence: past two renewals, so
/// one slow write does not lose it, and short of [`RESTORE_SETTLE`] by the margin a collector step already past
/// the fence has to finish in.
pub const LOST_AFTER: Duration = Duration::from_secs(12);

/// What a restore repair's second look leaves a server whose renewals stopped landing: the time between that
/// server closing its fence ([`LOST_AFTER`] past its last renewal that landed) and the second look
/// ([`RESTORE_SETTLE`] past the first). It is a budget, not a bound: it is meant to cover a collector step that
/// entered its pass before the fence closed and is still running - the step the fence cannot stop, only no longer
/// admit - but nothing enforces it. A delete the step had sent can be cancelled at the fence and still be applied
/// by the store later (`CollectionFence::bounded`), after the repair has begun: the open restore residual in
/// `docs/engineering/compact-storage.md`.
pub const COLLECTOR_STEP_MARGIN: Duration = Duration::from_secs(8);

const _: () = assert!(RENEW_EVERY.as_secs() * 2 < LOST_AFTER.as_secs());
const _: () =
    assert!(LOST_AFTER.as_secs() + COLLECTOR_STEP_MARGIN.as_secs() <= RESTORE_SETTLE.as_secs());
const _: () = assert!(RESTORE_SETTLE.as_secs() < LEASE_SECS as u64);
// Each lease statement step's budget fits inside the loss bound; the renewer's own wait, not the backend's,
// enforces the deadline.
const _: () =
    assert!(sideseat_core::constants::PROCESS_LEASE_STATEMENT_TIMEOUT_SECS <= LOST_AFTER.as_secs());

#[derive(Debug, Error)]
pub enum ProcessLeaseError {
    #[error("{role} process(es) {processes:?} hold a live lease on the transactional store")]
    Excluded {
        role: &'static str,
        processes: Vec<String>,
    },
    #[error(transparent)]
    Data(#[from] DataError),
}

/// A held lease, renewed in the background until [`ProcessLease::end`], and taken again if it is lost.
pub struct ProcessLease {
    store: Arc<dyn ProcessLeases>,
    id: String,
    stop: watch::Sender<bool>,
    held_until: watch::Receiver<Option<HeldUntil>>,
    passes: Arc<Passes>,
    renewer: JoinHandle<()>,
}

impl ProcessLease {
    /// Take a lease for `role` under a new process id, refusing while a process of a role it excludes holds a
    /// live one. With a `settle`, it looks again after waiting it, and refuses then too.
    pub async fn acquire(
        store: Arc<dyn ProcessLeases>,
        role: ProcessRole,
        settle: Duration,
    ) -> Result<Self, ProcessLeaseError> {
        let id = format!("{}-{}", role.as_str(), Uuid::new_v4());
        let mut renewed_at = Instant::now();
        store.renew_process_lease(&id, role, LEASE_SECS).await?;
        alone(store.as_ref(), &id, role).await?;
        if !settle.is_zero() {
            tokio::time::sleep(settle).await;
            renewed_at = Instant::now();
            store.renew_process_lease(&id, role, LEASE_SECS).await?;
            alone(store.as_ref(), &id, role).await?;
        }
        let (stop, stopped) = watch::channel(false);
        let (hold, held_until) = watch::channel(Some(HeldUntil::new(renewed_at + LOST_AFTER, 0)));
        let passes = Arc::new(Passes::default());
        let renewer = tokio::spawn(renew(
            Renewal {
                store: Arc::clone(&store),
                id: id.clone(),
                role,
                every: RENEW_EVERY,
                lost_after: LOST_AFTER,
                passes: Arc::clone(&passes),
            },
            renewed_at,
            stopped,
            hold,
        ));
        Ok(Self {
            store,
            id,
            stop,
            held_until,
            passes,
            renewer,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Until when the lease is held: [`LOST_AFTER`] past its last renewal that landed, and none once that has
    /// passed, until it is taken again.
    pub fn held_until(&self) -> watch::Receiver<Option<HeldUntil>> {
        self.held_until.clone()
    }

    /// The fence every collector of this process checks: open while the lease is held, and, once it is taken
    /// again after a loss, not before every pass entered before the loss has left.
    pub fn collection_fence(&self) -> CollectionFence {
        CollectionFence::following_passes(self.held_until.clone(), Arc::clone(&self.passes))
    }

    /// Stop renewing and give the lease up. A failure to delete it is logged: the lease then runs out.
    pub async fn end(self) {
        let _ = self.stop.send(true);
        let _ = self.renewer.await;
        if let Err(error) = self.store.end_process_lease(&self.id).await {
            tracing::warn!(%error, process = %self.id, "Could not end the process lease; it runs out instead");
        }
    }
}

/// Refuse - giving up the lease just taken - while a process of a role `role` excludes holds a live one.
async fn alone(
    store: &dyn ProcessLeases,
    id: &str,
    role: ProcessRole,
) -> Result<(), ProcessLeaseError> {
    for &excluded in role.excludes() {
        let processes = store.live_process_leases(excluded, id).await?;
        if processes.is_empty() {
            continue;
        }
        if let Err(error) = store.end_process_lease(id).await {
            tracing::warn!(%error, process = %id, "Could not end a refused process lease; it runs out instead");
        }
        return Err(ProcessLeaseError::Excluded {
            role: excluded.as_str(),
            processes,
        });
    }
    Ok(())
}

/// How long a lease taken again waits for the passes entered before its loss before it says so.
const DRAIN_WARN_AFTER: Duration = Duration::from_secs(30);

struct Renewal {
    store: Arc<dyn ProcessLeases>,
    id: String,
    role: ProcessRole,
    every: Duration,
    lost_after: Duration,
    passes: Arc<Passes>,
}

/// Where the renewer is: holding the lease with its fence open; lost; or taken again with the fence still closed,
/// waiting since an instant for the passes entered before the loss to leave.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Open,
    Lost,
    Draining { since: Instant, warned: bool },
}

/// Renew every `every` until stopped. A renewal is timed from when it was sent - the store's lease runs from no
/// earlier - and moves `hold` to `lost_after` past that. Once that passes without one that succeeded - a renewal
/// still in flight then is abandoned - the lease is lost: `hold` reads none, and each later attempt takes it again
/// as [`ProcessLease::acquire`] does, refused while a process of a role it excludes holds one. Taken again, the
/// lease is renewed as before but `hold` stays none until no pass entered before the loss is running - while it
/// reads none no pass can enter - and then reads the lease's next term.
async fn renew(
    renewal: Renewal,
    mut renewed_at: Instant,
    mut stopped: watch::Receiver<bool>,
    hold: watch::Sender<Option<HeldUntil>>,
) {
    let Renewal {
        store,
        id,
        role,
        every,
        lost_after,
        passes,
    } = renewal;
    let mut state = Held::Open;
    let mut term = 0;
    let mut interval = tokio::time::interval_at(Instant::now() + every, every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let deadline = renewed_at + lost_after;
        let renewing = state != Held::Lost;
        tokio::select! {
            biased;
            changed = stopped.changed() => {
                if changed.is_err() || *stopped.borrow() {
                    return;
                }
            }
            () = tokio::time::sleep_until(deadline), if renewing => {
                state = Held::Lost;
                lose(&hold, &id);
            }
            _ = interval.tick() => {
                let sent = Instant::now();
                if renewing {
                    // Waited out to the loss deadline, not to the next tick: a renewal that lands late still
                    // renews from when it was sent, which is no later than the store's own start of it, so it
                    // moves nothing the fence's timing rests on - and abandoning it at the tick lost the lease of
                    // a server whose store merely answered slowly under load.
                    let until = deadline;
                    match tokio::time::timeout_at(until, store.renew_process_lease(&id, role, LEASE_SECS)).await {
                        Ok(Ok(())) => {
                            renewed_at = sent;
                            if state == Held::Open {
                                let _ = hold.send(Some(HeldUntil::new(sent + lost_after, term)));
                            }
                        }
                        Ok(Err(error)) => tracing::warn!(%error, process = %id, "Could not renew the process lease"),
                        Err(_) => tracing::warn!(process = %id, "Renewing the process lease timed out"),
                    }
                    if renewed_at + lost_after <= Instant::now() {
                        state = Held::Lost;
                        lose(&hold, &id);
                        continue;
                    }
                } else {
                    // Bounded by the loss bound, not the interval: the renewal and the exclusion look are two
                    // statements, and on a loaded store each can take most of an interval.
                    let retaken = tokio::time::timeout(lost_after, async {
                        store.renew_process_lease(&id, role, LEASE_SECS).await?;
                        alone(store.as_ref(), &id, role).await
                    })
                    .await;
                    match retaken {
                        Ok(Ok(())) => {
                            renewed_at = sent;
                            state = Held::Draining { since: sent, warned: false };
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, process = %id, "Could not take the lost process lease again")
                        }
                        Err(_) => tracing::warn!(process = %id, "Taking the lost process lease again timed out"),
                    }
                }
                if let Held::Draining { since, warned } = state {
                    if passes.is_empty() {
                        term += 1;
                        state = Held::Open;
                        tracing::info!(process = %id, "Took the lost process lease again; collection resumes");
                        let _ = hold.send(Some(HeldUntil::new(renewed_at + lost_after, term)));
                    } else if !warned && since.elapsed() >= DRAIN_WARN_AFTER {
                        state = Held::Draining { since, warned: true };
                        let running: Vec<String> = passes
                            .running_passes()
                            .iter()
                            .map(|pass| format!("{} for {}s", pass.name, pass.running_for.as_secs()))
                            .collect();
                        tracing::warn!(
                            process = %id,
                            waited_secs = since.elapsed().as_secs(),
                            ?running,
                            "The lease is held again, but collection stays stopped until the passes begun before \
                             its loss finish"
                        );
                    }
                }
            }
        }
    }
}

fn lose(hold: &watch::Sender<Option<HeldUntil>>, id: &str) {
    tracing::error!(
        process = %id,
        "The process lease went unrenewed too long; deletion and collection stop until it is taken again"
    );
    let _ = hold.send(None);
}

#[cfg(test)]
#[path = "process_leases_tests.rs"]
mod tests;
