//! The durable ingestion queue, against a real Redis.
//!
//! # Why this suite exists
//!
//! The Redis stream backend is what makes an asynchronous 200 honest: a payload is acknowledged only
//! after it has been written, and an unacknowledged one is redelivered. Every claim in that sentence is
//! about Redis behaviour: consumer groups, pending entries, durability and consumed-only trimming. These
//! integration tests exercise that contract against a real server.
//!
//! Skips with a message when `SIDESEAT_TEST_REDIS_URL` is unset, so `make check` stays green without
//! Docker; `make test-redis` starts a pinned container and sets it.

include!("redis_stream_tests_parts/part_01_tests.rs");
include!("redis_stream_tests_parts/part_02_tests.rs");
