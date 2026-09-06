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
pub mod detect_rules;
pub mod schema;

#[cfg(test)]
mod carrier_rules_tests;
#[cfg(test)]
mod detect_rules_tests;

pub use carrier_rules::CarrierContext;
pub use detect_rules::DetectContext;

use std::sync::OnceLock;

/// The two resource/span attribute keys the engine reads *structurally* rather than as producer
/// vocabulary.
///
/// `service.name` and `metadata` are OpenTelemetry's own names, not any framework's: a rule says which
/// *value* identifies a producer, and the key it looks in is part of the dimension's definition. Held
/// here so a rule file cannot redefine where "the service name" lives, which would make two assets
/// disagree about what the dimension means.
pub(crate) const SERVICE_NAME_KEY: &str = "service.name";
pub(crate) const METADATA_KEY: &str = "metadata";

/// The label for a span no detection rule claimed and no declaration resolved.
///
/// The engine's own vocabulary for the *absence* of an answer, not a framework - which is why it lives
/// here while every real label lives in an asset.
pub const UNCLAIMED_LABEL: &str = "Unknown";

/// The compiled ruleset, built once from the embedded assets.
///
/// One `OnceLock` rather than a lazy per-lookup build: a compile that happened per span would put
/// path resolution and table construction on the read path, which is what the typed plan exists to
/// keep off it.
pub struct Ruleset {
    /// Carrier semantics, indexed for lookup by exact name and by prefix.
    pub carriers: carrier_rules::CarrierPlan,
    /// Detection signals in rank order, and the SDK-declaration fallback.
    pub detect: detect_rules::DetectPlan,
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
            .unwrap_or_else(|e| panic!("embedded carrier rules are malformed: {e}"));
        let detect = detect_rules::compile(&sources)
            .unwrap_or_else(|e| panic!("embedded detection rules are malformed: {e}"));
        Ruleset {
            carriers,
            detect,
            digest,
        }
    })
}

/// The ruleset digest, for the reconstruction cache key.
pub fn ruleset_digest() -> &'static str {
    &ruleset().digest
}
