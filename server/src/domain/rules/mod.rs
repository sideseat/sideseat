//! The framework rules engine: framework knowledge as data, interpreted generically.
//!
//! Every fact about a *specific* framework or provider lives in a rule asset under `server/rules/`,
//! not in this module. The engine knows how to match and how to compose; it knows no producer names,
//! carrier keys, tags or type mappings. The design and its acceptance are recorded in
//! `server/docs/framework-rules-engine.md`.
//!
//! Two properties make that checkable rather than merely intended:
//!
//! - **No behavioural API accepts a framework label.** Not "contains no framework literals" - a
//!   generic-looking `String` parameter can smuggle one, so the gate is a dataflow check. Detection
//!   produces a label for provenance, display and filtering; nothing that decides behaviour reads it.
//! - **Rules compile once, to a typed plan.** Matching a carrier must not parse JSON, resolve a path
//!   string or dispatch on a string per observation - the read path has a p95 ceiling measured in
//!   milliseconds.
//!
//! What is deliberately *not* here: concrete provider connectors. `providers/test_connection.rs`
//! builds real clients with real auth flows, which is executable adapter code; calling it data would
//! be dishonest.

pub mod carrier_rules;
pub mod schema;

#[cfg(test)]
mod carrier_rules_tests;

pub use carrier_rules::CarrierContext;

use std::sync::OnceLock;

/// The compiled ruleset, built once from the embedded assets.
///
/// One `OnceLock` rather than a lazy per-lookup build: a compile that happened per span would put
/// path resolution and table construction on the read path, which is what the typed plan exists to
/// keep off it.
pub struct Ruleset {
    /// Carrier semantics, indexed for lookup by exact name and by prefix.
    pub carriers: carrier_rules::CarrierPlan,
    /// BLAKE3 of the asset bytes that produced this plan, hex-encoded.
    ///
    /// Joins the reconstruction cache key. That cache is a memo over a pure function of the rows, and
    /// once rules can change they are part of the function - without this, a dev hot-load would serve
    /// answers built by a different ruleset from rows that had not changed.
    pub digest: String,
}

static RULESET: OnceLock<Ruleset> = OnceLock::new();

/// The compiled ruleset. Panics only if an *embedded* asset is malformed, which is a build defect: the
/// assets ship inside the binary, so there is no runtime input that can reach this.
pub fn ruleset() -> &'static Ruleset {
    RULESET.get_or_init(|| {
        let sources = schema::embedded_sources();
        let digest = schema::digest_of(&sources);
        let carriers = carrier_rules::compile(&sources)
            .unwrap_or_else(|e| panic!("embedded rule assets are malformed: {e}"));
        Ruleset { carriers, digest }
    })
}

/// The ruleset digest, for the reconstruction cache key.
pub fn ruleset_digest() -> &'static str {
    &ruleset().digest
}
