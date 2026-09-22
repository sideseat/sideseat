//! Runtime concerns that compose layers: shutdown coordination and the process's allocator.
//!
//! Separate from `core` because these depend on the layers *above* it. `ShutdownService` drains the topic
//! service, so leaving it in `core` made the configuration-and-constants layer name an adapter - and a crate
//! boundary at `core` could not then exist. Composition belongs where composition happens.

/// The pinned allocator and the live-byte counters the footprint gates read. See the module for why the
/// return-to-baseline gate is on live allocations rather than on RSS.
pub mod allocation;
pub mod clock;
pub mod shutdown;
