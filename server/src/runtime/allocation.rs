//! Allocation counters used by the footprint gates.
//!
//! Growth gates use live allocations rather than RSS because allocators may retain pages after the program
//! frees them. RSS remains useful operational context and is reported separately. Counting in this wrapper,
//! rather than reading allocator-specific statistics, keeps live-byte measurements comparable across
//! supported platforms and allocator backends.

#![allow(unsafe_code)]
// `GlobalAlloc` is an unsafe trait. Every pointer operation is delegated unchanged to the backend; this
// wrapper only updates counters after successful allocations and before valid deallocations.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Bytes handed out by [`CountingAllocator`] since the process started. Monotone; the churn figure.
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// Bytes allocated and not yet freed.
///
/// A single atomic avoids inconsistent snapshots from separate allocated and freed counters. The signed
/// representation tolerates transiently negative observations, which readers clamp to zero.
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
        // Preserve the backend's in-place growth support. A failed realloc leaves the old block live, so
        // counters change only after success.
        let new_ptr = unsafe { self.backend.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // Apply the live-size delta atomically so readers cannot observe an intermediate free or alloc.
            ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
            LIVE.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        }
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // Preserve the backend's optimized zeroed-allocation path.
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

    /// Churn is monotone, so concurrent frees cannot hide this test's allocation.
    ///
    /// Live growth is only reported here because the counter is process-global. Footprint tests serialize
    /// measurements before asserting growth.
    #[test]
    fn an_allocation_passes_through_the_counting_allocator() {
        const BLOCK: usize = 8 * 1024 * 1024;

        let before = AllocationSnapshot::now();
        let block: Vec<u8> = vec![0u8; BLOCK];
        let growth = AllocationSnapshot::now().growth_since(&before);
        drop(block);

        let after = AllocationSnapshot::now();
        assert!(
            after.churn_since(&before) >= BLOCK as u64,
            "an {BLOCK}-byte vector must show in the churn, which no concurrent free can reduce; got {} bytes",
            after.churn_since(&before)
        );
        eprintln!(
            "allocation: churn {} bytes, live growth at peak {growth} bytes (reported, not asserted - the \
             counter is process-global)",
            after.churn_since(&before)
        );
    }

    /// Source inspection enforces the single-atomic design and coverage of every allocation path.
    #[test]
    fn the_live_figure_is_a_single_atomic() {
        let source: String = include_str!("allocation.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| !line.trim_start().starts_with("///"))
            .collect::<Vec<_>>()
            .join("\n");

        // Assemble needles so this source-reading test does not match its own assertions.
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
