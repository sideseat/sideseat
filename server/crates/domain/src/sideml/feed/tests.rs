//! Feed-pipeline tests.
//!
//! These tests verify the correctness of the message processing pipeline
//! using the new flattened block structure.

include!("tests_parts/part_01_tests.rs");
include!("tests_parts/part_02_tests.rs");
include!("tests_parts/part_03_tests.rs");
include!("tests_parts/part_04_tests.rs");
include!("tests_parts/part_05_tests.rs");
include!("tests_parts/part_06_tests.rs");
include!("tests_parts/part_07_tests.rs");
include!("tests_parts/part_08_tests.rs");
include!("tests_parts/part_09_tests.rs");
include!("tests_parts/part_10_tests.rs");
include!("tests_parts/part_11_tests.rs");
include!("tests_parts/part_12_tests.rs");
include!("tests_parts/part_13_tests.rs");
include!("tests_parts/part_14_tests.rs");
include!("tests_parts/part_15_tests.rs");
include!("tests_parts/part_16_tests.rs");
include!("tests_parts/part_17_tests.rs");
include!("tests_parts/part_18_tests.rs");
include!("tests_parts/part_19_tests.rs");
include!("tests_parts/pairing_tests.rs");

/// The pairing generator, in its own module so the proptest prelude stays out of the example tests' namespace.
mod pairing_props {
    use super::*;
    use proptest::prelude::*;

    include!("tests_parts/pairing_props_tests.rs");
}
