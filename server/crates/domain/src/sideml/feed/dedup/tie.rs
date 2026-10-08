//! Which copy survives a dedup tie, as a switch the tests can flip.

// Test-only: prefer the *later* of two tied copies instead of the earlier.
//
// Which copy survives a quality tie is decided by arrival order, and it is not something a caller can
// vary - reversing the row order does not reach it, because rows are re-sorted by timestamp before
// dedup ever sees them. So the property "the *order* of the answer does not depend on which copy
// survived" had no test, which is the central claim of the ordering redesign.
//
// Flipping this changes which copy survives, and content differs between copies - so the test asserts
// the shape is unchanged *and* that the content moved somewhere, since a perturbation that reaches
// nothing proves nothing.
//
// Thread-local, not a global: the suite runs in parallel, and a process-wide flag changed what every
// other test was measuring at the same time. The read path runs on its caller's thread, so a
// thread-local reaches exactly the pipeline under test.
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    #[doc(hidden)]
    pub static PREFER_LATER_ON_TIE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn prefer_later_on_tie() -> bool {
    PREFER_LATER_ON_TIE.with(|flag| flag.get())
}

#[cfg(not(any(test, feature = "test-support")))]
pub(super) fn prefer_later_on_tie() -> bool {
    false
}
