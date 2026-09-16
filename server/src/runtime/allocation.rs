//! A pinned allocator that counts, so a footprint claim can be gated rather than asserted.
//!
//! This lands *before* any footprint optimisation, deliberately: an optimisation whose effect nobody can
//! measure is a change nobody can defend, and this repository's own latency table is gated precisely so it
//! cannot drift while every run passes. The four ceilings it feeds are in `server/tests/footprint.rs` and
//! `scripts/footprint-gates.sh`.
//!
//! Two decisions, each of which has a plausible-looking wrong answer.
//!
//! **Live allocations, not RSS**, for the gate that matters. `glibc` and `jemalloc` both retain freed pages
//! rather than returning them to the kernel, so a "returns to baseline" gate written against RSS fails
//! *correct* code, and fails it differently depending on timing and allocator version — which is worse than
//! no gate, because it teaches people to rerun until it passes. What a reader actually wants to know is
//! whether the program is still holding the memory, and that is `allocated - freed`. RSS stays reported
//! beside it and ungated: it is what an operator's monitoring shows, and a large gap between the two is
//! itself informative, since it means the allocator is holding pages the program has released.
//!
//! **The count is the program's, not the allocator's.** jemalloc publishes `stats.allocated`, which would
//! have saved the `unsafe impl` below — and would have made the gate a statement about jemalloc rather than
//! about SideSeat. Counting in the wrapper gives the same number on every platform whatever the backend is,
//! which is what lets "a session read returns to its baseline" mean something about this code.
//!
//! Never feature-gated: project convention rules feature gates out, and a gate that only exists in a special
//! build is a gate that rots. The cost is two relaxed atomics per allocation, on no path where that is
//! measurable against a DuckDB write or an HTTP round trip.

#![allow(unsafe_code)]
// The workspace denies `unsafe_code` because production code has none, and this is the first exception.
// `GlobalAlloc` cannot be implemented safely - the trait is unsafe by definition, since a wrong
// implementation is memory unsafety - so there is no safe spelling of a counting allocator to prefer. The
// two methods below delegate every pointer decision to the backend and touch nothing but two counters,
// which is the smallest surface an accurate live-byte figure can have. The alternative was reading
// jemalloc's own statistics, which needs no `unsafe` and measures the wrong thing (see the module docs).

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU64, Ordering};

/// Bytes handed out by [`CountingAllocator`] since the process started.
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
/// Bytes returned to it.
static FREED: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_os = "windows"))]
type PinnedBackend = tikv_jemallocator::Jemalloc;
#[cfg(not(target_os = "windows"))]
const PINNED_BACKEND: PinnedBackend = tikv_jemallocator::Jemalloc;

#[cfg(target_os = "windows")]
type PinnedBackend = std::alloc::System;
#[cfg(target_os = "windows")]
const PINNED_BACKEND: PinnedBackend = std::alloc::System;

/// Which allocator produced the resident figures, because footprint numbers are comparable only within one.
///
/// Reported by every gate rather than assumed, so a ceiling taken on Linux is never read as a statement
/// about the Windows build, where `tikv-jemalloc-sys` does not build and the system allocator stands in.
pub const ALLOCATOR_NAME: &str = if cfg!(target_os = "windows") {
    "system"
} else {
    "jemalloc"
};

/// Whether the resident-byte ceilings apply to this build.
///
/// False where the allocator is not the pinned one: the live-allocation gate still holds — it counts this
/// program's own bytes — but an absolute RSS ceiling does not, and a gate that passes on a figure it cannot
/// interpret is the failure mode this whole module exists to avoid.
pub const RESIDENT_CEILINGS_APPLY: bool = !cfg!(target_os = "windows");

/// The pinned allocator, counting.
pub struct CountingAllocator<A> {
    backend: A,
}

impl<A> CountingAllocator<A> {
    const fn new(backend: A) -> Self {
        Self { backend }
    }
}

// `Relaxed` throughout: these counters are a measurement, not a synchronisation mechanism. No decision
// depends on observing another thread's increment at a particular moment, and a stronger ordering would put
// a fence on every allocation to make a number in a test report marginally tidier.
unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.backend.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { self.backend.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Delegated rather than left to the trait's default, which would decompose into `alloc` + `copy` +
        // `dealloc` and throw away the backend's in-place growth - so the default would make every `Vec`
        // push that could have extended in place copy instead. Counted as a free of the old size and an
        // allocation of the new, which is what it is; on failure the old block is still live and neither
        // counter moves.
        let new_ptr = unsafe { self.backend.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // `ALLOCATED` **before** `FREED`, and this order is the whole correctness of the pair.
            //
            // The other way round, a snapshot landing between the two atomics sees the old size freed and the
            // new size not yet allocated - so a 100 MiB block grown to 200 MiB reads as 100 MiB *less* live
            // when it is 100 MiB more, an undercount of 200 MiB. A footprint gate reading that passes during
            // exactly the regression it exists to catch. This order makes the transient error an
            // *over*-estimate, which is the direction every measurement in this module is biased towards.
            ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
            FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // Same reason as `realloc`: the default implementation would route through `alloc` and then zero the
        // block itself, losing the backend's ability to hand back pages the kernel has already zeroed.
        let ptr = unsafe { self.backend.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator<PinnedBackend> = CountingAllocator::new(PINNED_BACKEND);

/// A reading of the counters, for comparing two moments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationSnapshot {
    allocated: u64,
    freed: u64,
}

impl AllocationSnapshot {
    /// Take a reading now.
    pub fn now() -> Self {
        // `freed` first, and this order is load-bearing. Read the other way round, a free landing between
        // the two loads is counted while its allocation is not, so `live` underflows - and a footprint gate
        // that occasionally reports a preposterous number is a gate people learn to rerun rather than read.
        // This way the error is in the safe direction: `live` can overstate by whatever was freed in
        // between, never understate.
        let freed = FREED.load(Ordering::Relaxed);
        let allocated = ALLOCATED.load(Ordering::Relaxed);
        Self { allocated, freed }
    }

    /// Bytes allocated and not yet freed.
    pub fn live(&self) -> u64 {
        self.allocated.saturating_sub(self.freed)
    }

    /// How much more is live now than in `earlier`.
    ///
    /// Saturating, so a measurement ending with *less* live memory than it started reads as zero growth
    /// rather than as an enormous one.
    pub fn growth_since(&self, earlier: &Self) -> u64 {
        self.live().saturating_sub(earlier.live())
    }

    /// Bytes allocated between `earlier` and now, whether or not they were freed.
    ///
    /// The churn as distinct from the growth: a path that allocates a gigabyte and frees all of it has zero
    /// growth and is still worth knowing about, because that is where interning or a `Bytes`-backed payload
    /// pays.
    pub fn churn_since(&self, earlier: &Self) -> u64 {
        self.allocated.saturating_sub(earlier.allocated)
    }
}

/// Resident set size in bytes, or `None` where this platform has no implementation here.
///
/// **Reported, never gated on its own**, for the reason in the module docs: the allocator decides when freed
/// pages go back to the kernel, so this number moves for reasons the program does not control.
pub fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb * 1024);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        // `ps` rather than `task_info`: the Mach call needs a `libproc` dependency and an `unsafe` block for
        // a number nothing gates on. If it ever becomes gated, that trade changes.
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// One line naming the allocator, the live bytes and the resident bytes, for a gate's report.
pub fn describe_footprint() -> String {
    let live = AllocationSnapshot::now().live();
    match resident_bytes() {
        Some(rss) => format!(
            "allocator {ALLOCATOR_NAME}: live {:.1} MB, resident {:.1} MB",
            live as f64 / 1_048_576.0,
            rss as f64 / 1_048_576.0
        ),
        None => format!(
            "allocator {ALLOCATOR_NAME}: live {:.1} MB, resident unavailable on this platform",
            live as f64 / 1_048_576.0
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counters see an allocation, and the churn sees it whether or not it was freed.
    ///
    /// Bounds are deliberately one-sided. This is a process-global counter in a test binary whose other
    /// tests allocate concurrently, so an exact figure would be a flake generator; what has to hold is that
    /// this thread's own block is *at least* accounted for, which is what a footprint gate reads.
    #[test]
    fn a_large_allocation_shows_as_live_and_its_release_is_counted() {
        let before = AllocationSnapshot::now();
        let block: Vec<u8> = vec![0u8; 8 * 1024 * 1024];
        let during = AllocationSnapshot::now();
        assert!(
            during.growth_since(&before) >= 8 * 1024 * 1024,
            "an 8 MiB vector must show as live growth, got {} bytes",
            during.growth_since(&before)
        );
        drop(block);

        let after = AllocationSnapshot::now();
        assert!(
            after.churn_since(&before) >= 8 * 1024 * 1024,
            "the churn counts the bytes whether or not they were freed"
        );
        // Growth is deliberately not asserted back to zero: concurrent tests in this binary allocate, so the
        // only sound statement is that the release was counted, which the churn and the free above show.
    }

    /// Every counter update is ordered so a torn read over-estimates rather than under-estimates.
    ///
    /// Read as source text, because the property is about *statement order* inside `unsafe` blocks and no
    /// runtime test can catch the interleaving reliably. `realloc` had it backwards: freeing the old size
    /// before recording the new allocation made a growing block read as *shrinking* between the two atomics,
    /// so a gate could pass during the regression it exists to catch.
    #[test]
    fn every_counter_pair_is_ordered_to_over_estimate() {
        let source = include_str!("allocation.rs");
        let realloc = source
            .split_once("unsafe fn realloc")
            .expect("realloc is defined here")
            .1;
        let body = realloc
            .split_once("unsafe fn alloc_zeroed")
            .map(|(before, _)| before)
            .unwrap_or(realloc);
        let allocated_at = body
            .find("ALLOCATED.fetch_add")
            .expect("realloc records an allocation");
        let freed_at = body
            .find("FREED.fetch_add")
            .expect("realloc records a free");
        assert!(
            allocated_at < freed_at,
            "realloc must add to ALLOCATED before FREED: the other order makes a growing block read as \
             shrinking between the two atomics, which under-reports live bytes by twice the growth"
        );

        // And the snapshot reads them the other way round, for the same reason from the other side: reading
        // `freed` first means concurrent activity can only make `freed` stale-small and `allocated`
        // fresh-large, which over-estimates.
        let snapshot = source
            .split_once("pub fn now() -> Self {")
            .expect("the snapshot constructor is here")
            .1;
        let freed_read = snapshot.find("FREED.load").expect("reads FREED");
        let allocated_read = snapshot.find("ALLOCATED.load").expect("reads ALLOCATED");
        assert!(
            freed_read < allocated_read,
            "AllocationSnapshot::now must read FREED before ALLOCATED, or `live` can underflow"
        );
    }

    /// A reading never reports negative live bytes, whichever way the counters were caught.
    #[test]
    fn live_bytes_never_underflow() {
        let observed = AllocationSnapshot::now();
        assert_eq!(observed.growth_since(&observed), 0);

        let inverted = AllocationSnapshot {
            allocated: 100,
            freed: 200,
        };
        assert_eq!(
            inverted.live(),
            0,
            "more freed than allocated reads as zero, not as a number near u64::MAX"
        );
        assert_eq!(
            inverted.growth_since(&observed),
            0,
            "and comparing it against a larger reading stays at zero too"
        );
    }

    /// The allocator in force is the pinned one wherever the resident ceilings claim to apply.
    ///
    /// The two constants are what a gate reads to decide whether an absolute megabyte figure means anything,
    /// so they must not be able to disagree.
    #[test]
    fn the_pinned_allocator_and_the_ceiling_claim_agree() {
        assert_eq!(
            RESIDENT_CEILINGS_APPLY,
            ALLOCATOR_NAME == "jemalloc",
            "the resident ceilings apply exactly where the allocator is the pinned one"
        );
    }

    /// RSS is readable on the platforms the gates run on, and its absence elsewhere is not an error.
    #[test]
    fn resident_bytes_is_available_here() {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(
            resident_bytes().is_some_and(|b| b > 0),
            "RSS should be readable on this platform"
        );
        assert!(describe_footprint().contains(ALLOCATOR_NAME));
    }
}
