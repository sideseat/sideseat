//! SideSeat's innermost layer: configuration, constants, storage layout, the CLI, and pure utilities.
//!
//! Depends on nothing of SideSeat's own. See `Cargo.toml` for why that is enforced by the manifest rather
//! than by a rule someone has to remember.

pub mod banner;
pub mod cli;
pub mod config;
pub mod constants;
pub mod migration;
pub mod storage;
pub mod utils;
