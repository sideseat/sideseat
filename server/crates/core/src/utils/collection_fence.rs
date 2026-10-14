//! Whether this process may delete or reclaim stored state right now.
//!
//! A server holds a lease on the transactional store while it runs, and a restore repair refuses while one is
//! live. A server that cannot renew its lease - the store failing over, a slow disk - keeps serving reads, but a
//! repair may start while it is cut off, so everything that deletes or reclaims stops at this fence until the
//! lease is held again: a collector that outlives a restore deletes what the restore brought back
//! (`server/specs/MediaIncarnation.tla`, `Lease`).
//!
//! A destructive pass - a sweep, a settlement, an export - runs inside a [`Pass`] it enters at its start, and
//! checks the fence again immediately before each statement that deletes. A lease lost and taken again opens its
//! fence only once every pass entered before the loss has left (`Drain`): a decision a pass took before the
//! restore never acts after it, and a pass that outlives the loss finds the fence closed at its next check and
//! stops. What no fence closes is one statement already in flight.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

/// The fence a step that deletes or reclaims checks, and is bounded by.
///
/// A lease's fence is open until a deadline its holder moves on with each renewal that lands, and compares that
/// deadline with the clock itself: it closes on time even when the task renewing the lease is late to run.
#[derive(Debug, Clone)]
pub struct CollectionFence {
    held: Option<watch::Receiver<Option<HeldUntil>>>,
    passes: Arc<Passes>,
}

/// A held lease as its fence sees it: until when, and in which term - a lease lost and taken again is a new term,
/// so a step that began in the old one can tell it did not hold the lease throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldUntil {
    pub deadline: Instant,
    pub term: u64,
}

impl HeldUntil {
    pub fn new(deadline: Instant, term: u64) -> Self {
        Self { deadline, term }
    }
}

/// The passes running under one fence, which a lease taken again waits for before it opens it.
#[derive(Debug, Default)]
pub struct Passes {
    next: AtomicU64,
    running: Mutex<BTreeMap<u64, (&'static str, Instant)>>,
}

impl Passes {
    pub fn is_empty(&self) -> bool {
        self.running().is_empty()
    }

    /// Every pass running, by name, with how long it has run, longest first.
    pub fn running_passes(&self) -> Vec<RunningPass> {
        let now = Instant::now();
        let mut running: Vec<RunningPass> = self
            .running()
            .values()
            .map(|(name, since)| RunningPass {
                name,
                running_for: now.saturating_duration_since(*since),
            })
            .collect();
        running.sort_by_key(|pass| std::cmp::Reverse(pass.running_for));
        running
    }

    fn running(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, (&'static str, Instant)>> {
        // A panic while the map is held cannot leave it half-changed: each change is one insert or remove.
        self.running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// One pass in [`Passes`], as the status reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningPass {
    pub name: &'static str,
    pub running_for: Duration,
}

/// A destructive pass entered under a fence: running until dropped, on every way out, a panic's included.
#[derive(Debug)]
pub struct Pass {
    passes: Arc<Passes>,
    id: u64,
}

impl Drop for Pass {
    fn drop(&mut self) {
        self.passes.running().remove(&self.id);
    }
}

/// What a fence reports: whether it is open, and the passes running under it - which, while it is closed after a
/// lease was taken again, are what it waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceStatus {
    pub open: bool,
    pub running: Vec<RunningPass>,
}

impl CollectionFence {
    /// A fence that is always open: a process that holds no lease it could lose, and tests.
    pub fn open() -> Self {
        Self {
            held: None,
            passes: Arc::default(),
        }
    }

    /// A fence open until the deadline `held_until` reads, and closed while it reads none.
    pub fn following(held_until: watch::Receiver<Option<HeldUntil>>) -> Self {
        Self::following_passes(held_until, Arc::default())
    }

    /// [`CollectionFence::following`], its passes counted in `passes`: what the lease's holder drains.
    pub fn following_passes(
        held_until: watch::Receiver<Option<HeldUntil>>,
        passes: Arc<Passes>,
    ) -> Self {
        Self {
            held: Some(held_until),
            passes,
        }
    }

    /// The lease as held now, if the fence is open; `Some(None)` for a fence that is always open.
    fn open_until(&self) -> Option<Option<HeldUntil>> {
        match &self.held {
            None => Some(None),
            Some(held_until) => {
                let held = (*held_until.borrow())?;
                (Instant::now() < held.deadline).then_some(Some(held))
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open_until().is_some()
    }

    /// The term of the lease held now, if the fence is open: what [`CollectionFence::held_through`] is asked of.
    pub fn term(&self) -> Option<u64> {
        self.open_until()
            .map(|held| held.map_or(0, |held| held.term))
    }

    /// Whether the fence is open now in `term`: the lease was held from when `term` was read until now, never
    /// lost in between - a loss and a retake would be a later term.
    pub fn held_through(&self, term: u64) -> bool {
        self.term() == Some(term)
    }

    /// Enter a destructive pass called `name`, if the fence is open. A lease lost while it runs is not opened
    /// again until it has left.
    pub fn enter(&self, name: &'static str) -> Option<Pass> {
        if !self.is_open() {
            return None;
        }
        let id = self.passes.next.fetch_add(1, Ordering::Relaxed);
        self.passes.running().insert(id, (name, Instant::now()));
        let pass = Pass {
            passes: Arc::clone(&self.passes),
            id,
        };
        // Closed in between: the holder may already have found no pass running, so this one must not run.
        self.is_open().then_some(pass)
    }

    pub fn status(&self) -> FenceStatus {
        FenceStatus {
            open: self.is_open(),
            running: self.passes.running_passes(),
        }
    }

    /// Run `step` if the fence is open, abandoning it once the fence closes or its lease's term changes. A
    /// renewal that lands while it runs moves the deadline it is held to. None when the fence was closed or the
    /// step was abandoned - the client side of it: a request a remote store received may still be applied.
    pub async fn bounded<F: Future>(&self, step: F) -> Option<F::Output> {
        let first = self.open_until()?;
        let Some(first) = first else {
            return Some(step.await);
        };
        tokio::pin!(step);
        let mut deadline = first.deadline;
        loop {
            match tokio::time::timeout_at(deadline, &mut step).await {
                Ok(output) => return Some(output),
                Err(_) => match self.open_until() {
                    Some(Some(held)) if held.term == first.term && held.deadline > deadline => {
                        deadline = held.deadline;
                    }
                    _ => return None,
                },
            }
        }
    }
}

#[cfg(test)]
#[path = "collection_fence_tests.rs"]
mod tests;
