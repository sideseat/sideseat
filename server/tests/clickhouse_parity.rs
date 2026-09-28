//! ClickHouse/DuckDB read-path parity.
//!
//! The two analytics backends implement the same `AnalyticsRepository` over hand-written SQL in
//! two dialects. Nothing but review kept them agreeing, and review does not catch a ClickHouse
//! expression that is merely *accepted* while returning something different: an arbitrary span's
//! tags instead of the union, a null trace name where DuckDB falls back to the earliest named
//! span, a `max()` over a Nullable that stays Nullable. Some do not even parse, and that only
//! shows up against a real server - `JSONExtract` on a `Nullable(String)` inside an array fails
//! with "Nested type Array(String) cannot be inside Nullable type" and takes the whole trace
//! list with it.
//!
//! So: insert one span set into both backends, call the analytics reads on both, and require the
//! answers to match. DuckDB is the reference because it is the default backend and its behaviour
//! is what the goldens and the UI were built against.
//!
//! Covered: trace list and single trace, span list, spans for a trace, single span, events, links,
//! bulk span counts, session list and single session, traces and trace ids for a session, message
//! rows for span/trace/session, the project feed's span and message pages, filter options for all
//! three scopes, tag options, project span counts, project stats, the four delete paths, and -
//! per filter variant, since each is rendered by its own arm - pagination, sorting, time bounds
//! and the advanced filters on the trace, span and session lists.
//!
//! Not covered, and worth knowing before trusting a green run:
//!
//! - metric ingestion and reads.
//! - anything that only appears at scale or with data this fixture does not have: a trace of
//!   thousands of spans, top-N truncation in stats, several models or frameworks in one project.
//! - re-ingested or duplicated spans, and spans sharing a timestamp exactly, where the two
//!   dialects' `argMin`/`FIRST` tie-breaking could differ.
//! - sub-second timestamp handling: the fixture uses whole seconds.
//! - rows old enough for the ClickHouse TTL to reap, distributed (sharded) mode, and async
//!   inserts - all three are configurations this single-node test cannot enter.
//! - timezone and DST behaviour in the stats bucketing.
//! - `ingested_at`, which is the server clock at write time and so differs by design.
//!
//! Needs a live ClickHouse. Skips with a message when `SIDESEAT_TEST_CLICKHOUSE_URL` is unset,
//! so `cargo test` stays green on a checkout with no container:
//!
//! ```bash
//! make test-clickhouse     # starts a container, runs this, removes it
//! ```

include!("clickhouse_parity_parts/part_01_tests.rs");
include!("clickhouse_parity_parts/part_02_tests.rs");
include!("clickhouse_parity_parts/part_03_tests.rs");
include!("clickhouse_parity_parts/part_04_tests.rs");
include!("clickhouse_parity_parts/part_05_tests.rs");
include!("clickhouse_parity_parts/part_06_tests.rs");
include!("clickhouse_parity_parts/part_07_tests.rs");
include!("clickhouse_parity_parts/part_08_tests.rs");
