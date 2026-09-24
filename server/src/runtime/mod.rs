//! Process-wide runtime concerns owned by the executable composition layer.

/// The pinned allocator and the live-byte counters the footprint gates read. See the module for why the
/// return-to-baseline gate is on live allocations rather than on RSS.
pub mod allocation;
pub mod clock;
pub(crate) mod shutdown;
