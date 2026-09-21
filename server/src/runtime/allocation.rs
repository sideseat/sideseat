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
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Bytes handed out by [`CountingAllocator`] since the process started. Monotone; the churn figure.
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// Bytes allocated and not yet freed, as **one** atomic.
///
/// Not derived from an `allocated` and a `freed` counter, and that was the first design's mistake. Two separate
/// `Relaxed` atomics cannot be read consistently: statement order in the writer says nothing about the order a
/// reader observes them in, so on weakly ordered hardware a reader can see the newer `freed` beside an older
/// `allocated` and compute a live figure that was never true - understating by the size of whatever was in
/// flight, which is the direction that lets a footprint gate pass during a regression. Reordering the two
/// `fetch_add`s does not fix it; it only fixes the *source*, which was the gap the second attempt left open.
///
/// One counter removes the question. `i64` rather than `u64` because a `dealloc` may be observed before its
/// `alloc` on another thread, so the value can dip below zero transiently - which is reported as zero rather
/// than as a number near `u64::MAX`.
static LIVE: AtomicI64 = AtomicI64::new(0);

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
            LIVE.fetch_add(layout.size() as i64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
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
            // The live delta as **one** atomic operation, which is why there is no ordering question here at
            // all. Two updates - a free of the old size and an allocation of the new - leave a window in which
            // a reader sees one and not the other, and no arrangement of two `Relaxed` atomics closes it: a
            // 100 MiB block grown to 200 MiB could read as 100 MiB *less* live when it is 100 MiB more, and a
            // gate reading that passes during exactly the regression it exists to catch.
            ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
            LIVE.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        }
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // Same reason as `realloc`: the default implementation would route through `alloc` and then zero the
        // block itself, losing the backend's ability to hand back pages the kernel has already zeroed.
        let ptr = unsafe { self.backend.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
            LIVE.fetch_add(layout.size() as i64, Ordering::Relaxed);
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
    live: i64,
}

impl AllocationSnapshot {
    /// Take a reading now.
    ///
    /// `live` is a single atomic load, so it is a value that was actually true at some instant rather than an
    /// arithmetic combination of two counters read at different ones. `allocated` is read separately and is
    /// only used for churn, where a monotone counter needs no consistency with anything.
    pub fn now() -> Self {
        let live = LIVE.load(Ordering::Relaxed);
        let allocated = ALLOCATED.load(Ordering::Relaxed);
        Self { allocated, live }
    }

    /// Bytes allocated and not yet freed.
    ///
    /// Clamped at zero: a `dealloc` can be observed before the `alloc` it matches, so the counter dips negative
    /// transiently, and a footprint gate reporting a preposterous number is one people learn to rerun rather
    /// than read.
    pub fn live(&self) -> u64 {
        self.live.max(0) as u64
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
    /// The margin is wide on purpose, and "assert the exact size" is not available here. `live` is one
    /// process-global counter, so another test in this binary *freeing* between the two snapshots subtracts from
    /// this thread's apparent growth - which it did: an 8 MiB block measured 8 388 274 against an 8 388 608
    /// assertion, 334 bytes short, and the test passed for months only because the previous two-counter design
    /// happened not to expose it. No test can attribute an exact global delta to its own thread.
    ///
    /// So the block is large enough that concurrent noise cannot cover half of it. What is being checked is that
    /// the allocator is *wired to the counter at all*, which a factor-of-two margin establishes as well as an
    /// exact figure would.
    #[test]
    fn a_large_allocation_shows_as_live_and_its_release_is_counted() {
        const BLOCK: usize = 64 * 1024 * 1024;
        const FLOOR: u64 = (BLOCK / 2) as u64;

        let before = AllocationSnapshot::now();
        let block: Vec<u8> = vec![0u8; BLOCK];
        let during = AllocationSnapshot::now();
        assert!(
            during.growth_since(&before) >= FLOOR,
            "a 64 MiB vector must show as live growth of at least 32 MiB, got {} bytes",
            during.growth_since(&before)
        );
        drop(block);

        let after = AllocationSnapshot::now();
        assert!(
            after.churn_since(&before) >= FLOOR,
            "the churn counts the bytes whether or not they were freed"
        );
        // Growth is deliberately not asserted back to zero: concurrent tests in this binary allocate, so the
        // only sound statement is that the release was counted, which the churn and the free above show.
    }

    /// The live figure comes from exactly one atomic, so no read of it can be torn.
    ///
    /// This replaces an ordering argument that did not hold. The first version derived `live` from an
    /// `allocated` and a `freed` counter and claimed the *source order* of the two `fetch_add`s made a torn read
    /// over-estimate - which is false: two `Relaxed` atomics may be observed in either order whatever the source
    /// says, so a reader could see the newer free beside the older allocation and compute a figure that was
    /// never true, understating by whatever was in flight. That is the direction that lets a gate pass during a
    /// regression, and no arrangement of two counters closes it.
    ///
    /// Read as source text because the property is about which atomics exist, and a runtime test cannot
    /// reliably produce the interleaving it would need to fail.
    #[test]
    fn the_live_figure_is_a_single_atomic() {
        let source: String = include_str!("allocation.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| !line.trim_start().starts_with("///"))
            .collect::<Vec<_>>()
            .join("\n");

        // Needles assembled from pieces, because this test's own text is inside the source it reads: written
        // literally, `contains("static FREED")` is satisfied by this very line and the assertion can never
        // fail. It did exactly that on the first run.
        let freed_counter = concat!("static ", "FREED");
        let live_load = concat!("LIVE", ".load");
        assert!(
            !source.contains(freed_counter),
            "a separate freed counter is back, and the live figure cannot be derived from two atomics \
             consistently"
        );
        assert_eq!(
            source.matches(live_load).count(),
            1,
            "the live figure must come from one load, in one place"
        );

        // And every path that hands out or returns memory adjusts it. Missing one would make the counter drift
        // silently in whichever direction that path goes.
        for method in [
            "fn alloc(",
            "fn dealloc(",
            "fn realloc(",
            "fn alloc_zeroed(",
        ] {
            let body = source
                .split_once(method)
                .unwrap_or_else(|| panic!("{method} is defined here"))
                .1;
            let body = &body[..body.find("\n    unsafe fn").unwrap_or(body.len().min(2000))];
            assert!(
                body.contains(concat!("LIVE", ".fetch_add"))
                    || body.contains(concat!("LIVE", ".fetch_sub")),
                "{method} does not adjust the live counter"
            );
        }
    }

    /// A reading never reports negative live bytes, whichever way the counters were caught.
    #[test]
    fn live_bytes_never_underflow() {
        let observed = AllocationSnapshot::now();
        assert_eq!(observed.growth_since(&observed), 0);

        let inverted = AllocationSnapshot {
            allocated: 100,
            live: -200,
        };
        assert_eq!(
            inverted.live(),
            0,
            "a transiently negative counter reads as zero, not as a number near u64::MAX"
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
