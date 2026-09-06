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
    /// Detection signals. Produce a **label** and nothing else: no behaviour reads it.
    #[serde(default)]
    pub detect: Vec<DetectRule>,
    /// The slugs an SDK may write into `sideseat.framework` for this framework, and the label they
    /// resolve to.
    ///
    /// Separate from `detect` because a declaration is evidence about the *process*, not about a span,
    /// and is consulted only after every signal has failed. Provider slugs (`bedrock`, `openai`) belong
    /// to no framework file, which is how they keep resolving to nothing.
    #[serde(default)]
    pub sdk_slugs: Vec<SdkSlug>,
}

/// One detection rule: signals that identify a producer, and the label they yield.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The label written to the span's `framework` column. A display and filtering value.
    pub label: String,
    /// Where this rule sits in the ordered sweep.
    ///
    /// Ordering is *policy* here and cannot be derived: the signals genuinely overlap, and the current
    /// answer depends on the order. Specific attribute rules must precede service-name fallbacks
    /// because the SideSeat SDK defaults `service.name` to one framework's name, so a service-name rule
    /// evaluated early would claim every span of every framework using the SDK. An explicit rank keeps
    /// that visible and reviewable instead of resting on where a line sits in a file.
    pub rank: i32,
    #[serde(rename = "match")]
    pub match_spec: DetectMatch,
}

/// One SDK-declared slug and the label it resolves to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkSlug {
    pub slug: String,
    pub label: String,
}

/// A pair of strings - an attribute key and the value or substring it must hold.
#[derive(Debug, Deserialize, Clone)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// A case-insensitive text search over named sources.
///
/// The one signal that is neither a prefix nor an equality: a framework whose spans are identified by a
/// phrase appearing somewhere in a name, in any of several spellings.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextContains {
    /// `span_name`, or `attr:<key>`.
    pub sources: Vec<String>,
    /// Any of these, matched case-insensitively.
    pub needles: Vec<String>,
}

/// The signals a detection rule may use. **Any** satisfied signal matches the rule.
///
/// Disjunctive, which is the existing behaviour and worth naming: each dimension is independently
/// sufficient. That is why a rule listing a broad `service_name` beside a narrow `attr_prefix` is not
/// "narrow" at all, and why rank matters.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DetectMatch {
    /// Span name equals, or starts with, any of these.
    #[serde(default)]
    pub span_name: Vec<String>,
    /// Any span attribute key starts with any of these.
    #[serde(default)]
    pub attr_prefix: Vec<String>,
    /// A span attribute equals this value exactly.
    #[serde(default)]
    pub attr_equals: Vec<KeyValue>,
    /// Any of these span attribute keys exists.
    #[serde(default)]
    pub attr_exists: Vec<String>,
    /// The resource's `service.name` equals or *contains* any of these.
    ///
    /// A substring test, which is why no rule may identify a framework by a short common word: `agno`
    /// would also match a service called `diagnostics`.
    #[serde(default)]
    pub service_name: Vec<String>,
    /// The span's `metadata` attribute contains any of these substrings.
    #[serde(default)]
    pub metadata_contains: Vec<String>,
    /// A resource attribute contains this substring - how an instrumentation library identifies itself
    /// through `telemetry.sdk.name`.
    #[serde(default)]
    pub resource_attr_contains: Vec<KeyValue>,
    /// A case-insensitive phrase search over the span name or a named attribute.
    #[serde(default)]
    pub text_contains: Option<TextContains>,
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
