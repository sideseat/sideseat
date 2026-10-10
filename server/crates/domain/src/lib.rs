//! SideSeat's driver-independent domain layer.

// A hash map's iteration order is randomised per process, and in this crate an iteration order can reach
// message order, dedup's choice of survivor, a rank, a tie-break or the bytes of an answer. So every loop
// over a hash map or set either iterates in a defined order or says, in an `expect`, why its order cannot
// matter.
#![deny(clippy::iter_over_hash_type)]

pub mod cleanup;
pub mod dedup;
pub mod files;
pub mod maintenance;
pub mod observations;
pub mod pricing;
pub mod providers;
pub mod rate_limit;
pub mod raw_payload;
pub mod restore;
pub mod rules;
pub mod search;
pub mod sideml;
pub mod storage_governance;
