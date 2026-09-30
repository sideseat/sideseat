//! Golden tests for message parsing, per framework and per sample.
//!
//! Message count, content, ordering and absence of duplicates are the properties users
//! actually see, and they are easy to break invisibly: a dedup identity tweak or a new
//! extractor can silently drop a tool result or duplicate a turn, and every existing unit
//! test still passes because they each cover one stage in isolation.
//!
//! This harness closes that gap end to end. Each fixture is the exact OTLP payload a real
//! sample sent (captured by `scripts/message-fixtures/record-otlp.py`, see `scripts/message-fixtures/capture.sh`).
//! It is replayed through the real ingestion path — `extract_attributes_batch`,
//! `extract_messages_batch`, SideML conversion, enrichment — and then through each of the
//! four views the API exposes:
//!
//! | View    | Feed entry point                | API endpoint                     |
//! |---------|---------------------------------|----------------------------------|
//! | span    | `process_spans` (1 span)        | `/spans/{trace}/{span}/messages` |
//! | trace   | `process_spans` (1 trace)       | `/traces/{id}/messages`          |
//! | session | `process_spans` (whole session) | `/sessions/{id}/messages`        |
//! | feed    | `process_feed` (every row)      | `/feed/messages`                 |
//!
//! The first three use `process_spans` and differ only in their row set, so each must be built with
//! its own row set - using `process_feed` for a session tested an ordering no session request can
//! return. The feed is the fourth because it is the one view with its own pipeline entry point and
//! its own ordering, and while it was left out it was the only place a duplicate could still
//! surface unchecked. Pagination is not modelled: it is a property of the endpoint rather than of
//! parsing, so the feed view is the whole fixture as one page.
//!
//! The result is compared against a committed expectation file. Regenerate with:
//!
//! ```bash
//! UPDATE_GOLDENS=1 cargo test --locked -p sideseat-server --test message_goldens
//! ```
//!
//! Regenerating is deliberately a separate, explicit step: a golden written straight from
//! current output enshrines whatever bugs exist today, so a regenerated file has to be read
//! before it is committed. The invariant checks below exist precisely because they hold
//! regardless of what the golden says — they catch bugs a blind snapshot would bless.

include!("message_goldens_parts/part_01_tests.rs");
include!("message_goldens_parts/part_02_tests.rs");
include!("message_goldens_parts/part_03_tests.rs");
include!("message_goldens_parts/part_04_tests.rs");
include!("message_goldens_parts/part_05_tests.rs");
include!("message_goldens_parts/part_06_tests.rs");
include!("message_goldens_parts/part_07_tests.rs");
