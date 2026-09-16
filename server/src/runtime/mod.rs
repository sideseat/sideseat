//! Runtime concerns that compose layers: shutdown coordination.
//!
//! Separate from `core` because these depend on the layers *above* it. `ShutdownService` drains the topic
//! service, so leaving it in `core` made the configuration-and-constants layer name an adapter - and a crate
//! boundary at `core` could not then exist. Composition belongs where composition happens.

pub mod shutdown;
