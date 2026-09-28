//! PostgreSQL/SQLite transactional parity.
//!
//! The two transactional backends implement the same `TransactionalRepository` over hand-written SQL
//! in two dialects. This suite executes both implementations and compares their observable behavior;
//! compilation and review alone cannot catch a query that is accepted while returning different data.
//!
//! The failure modes are the same shape as ClickHouse's, and some are specific to this pair:
//!
//! - `?` versus `$n` placeholders: a miscounted or reordered `$n` binds the wrong column, which
//!   still executes.
//! - SQLite's `INTEGER` is PostgreSQL's `BIGINT`, and its dynamic typing forgives a comparison that
//!   PostgreSQL rejects or coerces.
//! - `INSERT OR REPLACE` versus `ON CONFLICT DO UPDATE`: the SQLite form deletes and reinserts, so
//!   it resets columns the PostgreSQL form leaves alone.
//! - `ON DELETE CASCADE` reaches different rows in the two schemas whenever a foreign key was
//!   declared in one and forgotten in the other.
//! - Concurrency, which single-writer SQLite cannot express at all: PostgreSQL runs two writers, so
//!   a read-then-write that is safe under SQLite's busy error is a lost update here. That is a real
//!   defect this suite found in review and now guards - see [`the_file_fence_holds_against_a_
//!   concurrent_association`].
//!
//! # How parity is stated
//!
//! A scenario is a program run against a repository; its **transcript** is the sequence of
//! observations it made. Parity is transcript equality. Generated ids are replaced by stable labels
//! on first sight, and timestamps by the fact that they exist, so the comparison is about behaviour
//! rather than about clock values or cuid2 output. SQLite is the reference: it is the default
//! backend, and its behaviour is what the goldens and the UI were built against.
//!
//! # Not covered, worth knowing before trusting a green run
//!
//! - Read paths that only differ at scale: pagination past the first page with many rows, and the
//!   ordering of rows sharing a `created_at` second.
//! - The credential secret path (encryption lives above the repository) and OAuth auth-method
//!   lookups.
//! - Connection-pool behaviour: statement timeout, acquire timeout, `max_lifetime` recycling.
//! - Migration *upgrade* paths. The suite runs the current schema; it does not create a v1 database
//!   and walk it forward, so an `ALTER TABLE` arm that is wrong is only caught if the resulting
//!   shape differs from what fresh `SCHEMA` produces.
//! - Anything the transactional repository does not own: analytics rows, file bytes.
//!
//! Needs a live PostgreSQL. Skips with a message when `SIDESEAT_TEST_POSTGRES_URL` is unset, so
//! `cargo test` stays green on a checkout with no container:
//!
//! ```bash
//! make test-postgres     # starts a container, runs this, removes it
//! ```

include!("postgres_parity_parts/part_01_tests.rs");
include!("postgres_parity_parts/part_02_tests.rs");
include!("postgres_parity_parts/part_03_tests.rs");
