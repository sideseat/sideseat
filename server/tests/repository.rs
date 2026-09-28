//! Invariants about the **repository**, not about the server.
//!
//! These integration tests enforce file layout, dependency boundaries, CI coverage and committed fixtures.

include!("repository_parts/part_01_tests.rs");
include!("repository_parts/part_02_tests.rs");
include!("repository_parts/part_03_tests.rs");
include!("repository_parts/part_04_tests.rs");
include!("repository_parts/part_05_tests.rs");
