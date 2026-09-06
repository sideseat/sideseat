//! The rule asset format, and how assets are found, ordered and digested.
//!
//! Assets are embedded from `server/rules/` as a *directory*, deliberately: adding a framework must be
//! adding a file, with no Rust change. A hand-maintained `include_str!` list would mean the binary
//! still knows which frameworks exist, which is the thing the mandate forbids.

use std::collections::BTreeMap;

use rust_embed::RustEmbed;
use serde::Deserialize;

/// The embedded rule assets.
#[derive(RustEmbed)]
#[folder = "rules/"]
struct RuleAssets;

/// One rule file's parsed contents.
///
/// Every section is optional: a framework that only needs to declare its carriers says nothing about
/// messages, and a shared dialect fragment may declare carriers alone.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// Stable id for diagnostics and explain traces. Not a framework identity anything branches on.
    pub id: String,
    /// What this file is for, in prose. Surfaced by the explain trace, which is why documentation is a
    /// field rather than a comment.
    #[serde(default)]
    pub doc: Option<String>,
    /// Carrier semantics declarations.
    #[serde(default)]
    pub carriers: Vec<CarrierRule>,
}

/// One carrier declaration: what to match, and what the matched carrier is evidence of.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierRule {
    /// Stable clause id, reported by the explain trace.
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(rename = "match")]
    pub match_spec: MatchSpec,
    /// The preset this clause resolves to, optionally with named overrides.
    pub facts: Facts,
    /// The ordering family this carrier belongs to, when it is a *fragmented ordered input*: several
    /// attribute keys that are one array (`llm.input_messages.0.message` and `.1.message`).
    ///
    /// The seventh carrier fact. It was hardcoded in the order resolver, which is exactly the shape of
    /// defect this engine exists to remove: a semantic fact about one framework's carrier, written in
    /// Rust, that no rule file could state.
    #[serde(default)]
    pub ordering_family: Option<String>,
}

/// What an observation must look like for a clause to apply. Every field is optional and all present
/// fields must hold, so a clause constraining more dimensions is strictly more specific.
///
/// The dimensions are a fixed, finite set of orthogonal scalar constraints - which is what makes
/// specificity well-defined here, unlike the tree-shape predicates of the content chain, where
/// ordering has to be declared by name instead.
#[derive(Debug, Default, Deserialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    /// Exact OTel event name the observation was read from.
    #[serde(default)]
    pub event: Option<String>,
    /// Exact attribute key.
    #[serde(default)]
    pub attribute: Option<String>,
    /// Attribute key prefix, for indexed families (`llm.input_messages.0.message`).
    #[serde(default)]
    pub attribute_prefix: Option<String>,
    /// Any of these observation types. This is the dimension the span-blind lookup lacked: the same
    /// carrier name means different things on a generation span and on an aggregator.
    #[serde(default)]
    pub observation_type: Vec<String>,
    /// Span name prefix.
    #[serde(default)]
    pub span_name_prefix: Option<String>,
    /// Instrumentation scope name substring.
    ///
    /// Narrowing evidence only, never an exclusive key: historical rows may carry no scope, several
    /// frameworks share instrumentation packages, and versions are not reliably semantic.
    #[serde(default)]
    pub scope_name_contains: Option<String>,
}

impl MatchSpec {
    /// How specific this clause is: how many dimensions it constrains, with an exact attribute
    /// counting for more than a prefix.
    ///
    /// Used only to let a qualified clause beat a generic one. Two clauses that could match the same
    /// observation at *equal* specificity are a compile error, not a coin toss.
    pub fn specificity(&self) -> u32 {
        let mut score = 0;
        if self.event.is_some() {
            score += 2;
        }
        if self.attribute.is_some() {
            score += 2;
        }
        if self.attribute_prefix.is_some() {
            score += 1;
        }
        if !self.observation_type.is_empty() {
            score += 2;
        }
        if self.span_name_prefix.is_some() {
            score += 1;
        }
        if self.scope_name_contains.is_some() {
            score += 1;
        }
        score
    }

    /// The carrier name this clause keys on, for indexing: the exact event, the exact attribute, or the
    /// attribute prefix. A clause naming none of the three is malformed - it would match every
    /// observation of a span.
    pub fn primary_key(&self) -> Option<PrimaryKey<'_>> {
        if let Some(event) = &self.event {
            return Some(PrimaryKey::Event(event));
        }
        if let Some(attribute) = &self.attribute {
            return Some(PrimaryKey::Attribute(attribute));
        }
        self.attribute_prefix
            .as_deref()
            .map(PrimaryKey::AttributePrefix)
    }

    /// Whether two clauses' qualifier sets can both hold for one observation. Only meaningful for
    /// clauses that share a primary key; used to decide whether equal specificity is a real collision.
    pub fn qualifiers_can_overlap(&self, other: &Self) -> bool {
        let types_overlap = self.observation_type.is_empty()
            || other.observation_type.is_empty()
            || self
                .observation_type
                .iter()
                .any(|t| other.observation_type.contains(t));
        let prefixes_overlap = match (&self.span_name_prefix, &other.span_name_prefix) {
            (Some(a), Some(b)) => a.starts_with(b.as_str()) || b.starts_with(a.as_str()),
            _ => true,
        };
        let scopes_overlap = match (&self.scope_name_contains, &other.scope_name_contains) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        types_overlap && prefixes_overlap && scopes_overlap
    }
}

/// What a clause keys on, for the lookup index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimaryKey<'a> {
    Event(&'a str),
    Attribute(&'a str),
    AttributePrefix(&'a str),
}

/// The six carrier facts, named by preset with optional per-field overrides.
///
/// A preset plus overrides rather than six booleans spelled out per clause: the presets are the
/// vocabulary the model is stated in, and a clause that writes them all out invites one being wrong in
/// a way no reader notices.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    /// `emission`, `snapshot` or `accumulated_state`.
    pub preset: String,
    #[serde(default)]
    pub position_proves_distinct_occurrence: Option<bool>,
    #[serde(default)]
    pub position_provides_sequence_order: Option<bool>,
    #[serde(default)]
    pub carrier_is_atomic_emission: Option<bool>,
    #[serde(default)]
    pub carrier_may_contain_history_or_state: Option<bool>,
    #[serde(default)]
    pub carrier_holds_span_output: Option<bool>,
    #[serde(default)]
    pub carrier_is_detached_request_frame: Option<bool>,
}

/// Every embedded asset, keyed by path so the order is deterministic.
///
/// A `BTreeMap` rather than the embed crate's iteration order: the compile walks these, and a
/// collision diagnostic that named a different pair of files per build would be untraceable.
pub fn embedded_sources() -> BTreeMap<String, Vec<u8>> {
    RuleAssets::iter()
        .filter(|path| path.ends_with(".json"))
        .filter_map(|path| {
            RuleAssets::get(&path).map(|file| (path.to_string(), file.data.into_owned()))
        })
        .collect()
}

/// BLAKE3 over the asset paths and bytes, hex-encoded.
///
/// Paths are hashed too, and length-prefixed, so moving a declaration between files changes the digest
/// and two files cannot be concatenated into the same hash as one.
pub fn digest_of(sources: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, bytes) in sources {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
}
