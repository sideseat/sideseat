//! The rule asset format, and how assets are found, ordered and digested.
//!
//! Assets are embedded from `server/rules/` as a *directory*, deliberately: adding a framework must be
//! adding a file, with no Rust change. A hand-maintained `include_str!` list would mean the binary
//! still knows which frameworks exist, which is the thing the mandate forbids.

use std::collections::BTreeMap;

use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::Value as JsonValue;

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
    /// Which carriers an ingestion reads on this dialect's spans, and how each is parsed.
    #[serde(default)]
    pub messages: Vec<MessageRule>,
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
    /// Where this rule sits in the ordered sweep - a **migration bridge**, not the target design.
    ///
    /// The accepted design is order-independent: each rule states sufficient conditions, a unique
    /// sufficient candidate wins, and a declared `supersedes` resolves a known overlap. Today's rules do
    /// not state sufficient conditions - they were transcribed from a first-match table whose order is
    /// load-bearing, because the SideSeat SDK defaults `service.name` to one framework's name, so a
    /// service-name signal evaluated early claims every span of every framework using the SDK.
    ///
    /// So the rank reproduces that answer while the overlaps are *collected and reported* rather than
    /// silently resolved (`overlapping_candidates`). It goes when the predicates are narrow enough that
    /// no span has two candidates - and until then, naming it `legacy_rank` is the honest description of
    /// what it is.
    #[serde(rename = "legacy_rank")]
    pub legacy_rank: i32,
    /// Rule ids this rule beats where both match. Declared, so a genuine overlap is owned rather than
    /// resolved by a number.
    #[serde(default)]
    pub supersedes: Vec<String>,
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
    /// A *span* attribute contains this substring.
    ///
    /// Generic on purpose. This replaced a `metadata_contains` dimension whose key the engine supplied,
    /// which made one framework's attribute name look like part of OpenTelemetry: only one rule ever used
    /// it. `service.name` stays a structural dimension because it really is OTel's own resource
    /// attribute; `metadata` is a framework's, so the key belongs in the asset beside the value.
    #[serde(default)]
    pub span_attr_contains: Vec<KeyValue>,
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
    /// Clause ids this clause beats where their languages overlap and neither contains the other.
    ///
    /// The escape hatch for a genuine overlap between predicates that cannot be ordered by subsumption -
    /// stated in data, by name, rather than resolved by load order or by a number. Without it such a pair
    /// fails compilation, which is the point: somebody has to own the decision.
    #[serde(default)]
    pub supersedes: Vec<String>,
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
    /// Instrumentation scope *version* prefix.
    ///
    /// A prefix rather than a comparison, deliberately: versions here are not reliably semantic, so
    /// `>=` would have to invent an ordering for strings that have none. A prefix states exactly what it
    /// checks. Present because a producer can change a carrier's meaning between releases, which is the
    /// one thing no other dimension can express.
    #[serde(default)]
    pub scope_version_prefix: Option<String>,
}

impl MatchSpec {
    /// How many of the three carrier fields this clause names. Exactly one is required: a clause naming
    /// none would match every observation of a span, and one naming several was silently reduced to
    /// whichever the indexer looked at first, quietly ignoring the rest.
    pub fn primary_key_count(&self) -> usize {
        usize::from(self.event.is_some())
            + usize::from(self.attribute.is_some())
            + usize::from(self.attribute_prefix.is_some())
    }

    /// The carrier this clause keys on, for indexing.
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

    /// Does this clause's match language *contain* the other's - is the other at least as specific?
    ///
    /// Subsumption, not a score. A numeric score imposes an order on predicates that have none:
    /// `observation_type = agent` and `span_name_prefix = invoke_` constrain different things and
    /// neither implies the other, so any number assigned to them is arbitrary and decides real cases by
    /// arithmetic. Specificity is only meaningful where one clause's language is a strict subset of the
    /// other's, and where it is not, the ruleset is ambiguous and must say which wins.
    ///
    /// Returns true when every observation matching `other` also matches `self`.
    pub fn contains_language_of(&self, other: &Self) -> bool {
        // Carrier: an exact name is inside a prefix that covers it; a longer prefix is inside a shorter.
        let carrier_contains = match (self.primary_key(), other.primary_key()) {
            (Some(PrimaryKey::Event(a)), Some(PrimaryKey::Event(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::Attribute(b))) => a == b,
            (Some(PrimaryKey::AttributePrefix(p)), Some(PrimaryKey::Attribute(a))) => {
                a.starts_with(p)
            }
            (Some(PrimaryKey::AttributePrefix(a)), Some(PrimaryKey::AttributePrefix(b))) => {
                b.starts_with(a)
            }
            _ => false,
        };
        if !carrier_contains {
            return false;
        }
        // Qualifiers: an unconstrained dimension contains any constraint on it.
        let types = self.observation_type.is_empty()
            || (!other.observation_type.is_empty()
                && other
                    .observation_type
                    .iter()
                    .all(|t| self.observation_type.contains(t)));
        let span_names = match (&self.span_name_prefix, &other.span_name_prefix) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b.starts_with(a.as_str()),
        };
        // A longer needle is the more specific claim only when it *contains* the shorter one: a scope
        // holding `foo` is not necessarily one holding `bar`, but one holding `foobar` does hold `oob`.
        let scopes = match (&self.scope_name_contains, &other.scope_name_contains) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b.contains(a.as_str()),
        };
        let versions = match (&self.scope_version_prefix, &other.scope_version_prefix) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b.starts_with(a.as_str()),
        };
        types && span_names && scopes && versions
    }

    /// Could one observation satisfy both clauses?
    ///
    /// Deliberately conservative: where it cannot be shown that no observation satisfies both, this says
    /// they overlap, so the compiler asks for an explicit ordering rather than assuming independence.
    /// Two `scope_name_contains` needles are the case that matters - a scope name can hold both `foo`
    /// and `bar`, so treating unequal needles as disjoint let two clauses both match with nothing
    /// choosing between them.
    pub fn can_both_match(&self, other: &Self) -> bool {
        let carriers_overlap = match (self.primary_key(), other.primary_key()) {
            (Some(PrimaryKey::Event(a)), Some(PrimaryKey::Event(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::Attribute(b))) => a == b,
            (Some(PrimaryKey::Attribute(a)), Some(PrimaryKey::AttributePrefix(p)))
            | (Some(PrimaryKey::AttributePrefix(p)), Some(PrimaryKey::Attribute(a))) => {
                a.starts_with(p)
            }
            (Some(PrimaryKey::AttributePrefix(a)), Some(PrimaryKey::AttributePrefix(b))) => {
                // Either can extend the other, and some key beginning with the longer satisfies both.
                a.starts_with(b) || b.starts_with(a)
            }
            _ => false,
        };
        if !carriers_overlap {
            return false;
        }
        let types = self.observation_type.is_empty()
            || other.observation_type.is_empty()
            || self
                .observation_type
                .iter()
                .any(|t| other.observation_type.contains(t));
        // One span name cannot start with two prefixes unless one extends the other.
        let span_names = match (&self.span_name_prefix, &other.span_name_prefix) {
            (Some(a), Some(b)) => a.starts_with(b.as_str()) || b.starts_with(a.as_str()),
            _ => true,
        };
        // Two substrings are always jointly satisfiable: concatenate them.
        let versions = match (&self.scope_version_prefix, &other.scope_version_prefix) {
            (Some(a), Some(b)) => a.starts_with(b.as_str()) || b.starts_with(a.as_str()),
            _ => true,
        };
        types && span_names && versions
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

/// One message-extraction rule: a carrier to read, how to parse it, and what to emit.
///
/// Deliberately small. Extraction stores the payload raw and normalisation happens at query time, so a
/// rule that needs more than this is a rule whose *transform* is not yet expressible - and the honest
/// response is to leave that extractor in Rust and count it, not to grow this type until it is a
/// programming language.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The carrier to read. Absent for a `compose` rule, which has many sources rather than one.
    #[serde(default)]
    pub read: ReadSpec,
    /// Assemble one message from several attributes, rather than wrapping one read value.
    ///
    /// The dual of `wrap`, and needed because a dialect writes one response across many keys - the text
    /// here, the tool calls there, a structured object beside them, and any other member of the same
    /// family swept up. There is no single carrier to read, so there is no single value to wrap.
    #[serde(default)]
    pub compose: Option<ComposeSpec>,
    /// How to turn its raw string into a value. Absent for an indexed family, which has no single
    /// string to parse - each member is read on its own.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Wrap the parsed value in a message envelope with this role.
    ///
    /// Some carriers hold a bare payload rather than a message - a tool's arguments, say - and the role
    /// that payload represents is a fact about the carrier, so it is declared beside it.
    #[serde(default)]
    pub wrap: Option<WrapSpec>,
    /// Whether the observation is a message or a tool definition.
    pub emit: EmitTarget,
    /// A gate on the span, in the detection vocabulary: the rule is consulted only where this holds.
    ///
    /// Several extractors refuse to read a carrier whose name they share with other dialects unless the
    /// span also carries their own marker - `gen_ai.prompt` is the generic conventions' key and also
    /// where one exporter writes a whole request, so reading it unconditionally would claim another
    /// dialect's payload.
    #[serde(default)]
    pub when: Option<DetectMatch>,
    /// Ordered readings of the parsed value, tried until one yields an observation.
    ///
    /// An ordered coalesce, not a program: a payload has more than one documented shape and the rule
    /// says which to try first. Empty means "emit the parsed value as it stands", which is what the
    /// three dialects migrated first needed.
    #[serde(default)]
    pub alternatives: Vec<Alternative>,
    /// Tag the observation with this carrier, whatever alternative was read.
    ///
    /// Normally the key found is the tag, so two spellings of a payload stay distinguishable. One dialect
    /// deliberately does the opposite: it reads a renamed key but reports the canonical one, so every
    /// span's prompt is tagged alike whichever spelling it used. Declared, because it is the *reverse* of
    /// the default and a reader would otherwise assume the default.
    #[serde(default)]
    pub tag_as: Option<String>,
    /// May this rule read a *tool execution* span?
    ///
    /// A tool span reads the conventions and nothing else. The reason is that a tool span carries the
    /// call and its result under the conventions' own keys, while a dialect's broader carriers on the same
    /// span hold the enclosing agent's state - read there they duplicate the turn. That was a hardcoded
    /// exemption for one extractor by name; it is a property of a rule now.
    #[serde(default)]
    pub reads_tool_spans: bool,
    /// Split a text carrier into tagged sections and route each by its tag.
    ///
    /// A general text-carrier capability, and one dialect needs it: a single attribute holds either side
    /// of a conversation, distinguished by a bracketed tag, and several sections joined by a separator -
    /// one per parallel tool call. Each is read on its own so every result keeps its own id.
    #[serde(default)]
    pub sections: Option<SectionsSpec>,
    /// Reject a carrier whose value is blank once trimmed.
    ///
    /// Distinct from `require_non_empty`, which rejects only the empty string: one dialect treats
    /// whitespace as absence and another does not, and collapsing the two would change both.
    #[serde(default)]
    pub require_non_blank: bool,
    /// Skip a carrier whose value is empty.
    ///
    /// An attribute present and empty is not evidence of a message, and wrapping it produces a turn with
    /// nothing in it - which the no-empty-content invariant then rejects downstream.
    #[serde(default)]
    pub require_non_empty: bool,
    /// A negative gate: the rule is skipped where this holds.
    ///
    /// Symmetric to `when`, and needed for a genuine either/or - a response is read from its text when it
    /// has text, and from its tool calls only when it does not, or one response would be emitted twice.
    #[serde(default)]
    pub unless: Option<DetectMatch>,
    /// What an indexed entry must carry to count as one.
    ///
    /// An index exists as soon as *any* key mentions it, and a family legitimately holds keys that are not
    /// messages - so without this a request's settings each become a turn.
    ///
    /// Each member declares how its presence is decided, because the dialects genuinely differ and the
    /// difference is observable: one writes `content` directly *and* `content.0.text`, so either proves it,
    /// while another writes `contents.0.type` and never a bare `contents`, so only a nested key does. A
    /// single rule for all of them would either miss entries or invent them.
    #[serde(default)]
    pub require_members: Option<MemberRequirements>,
    /// Position in the consulted order. See `MessagePlan` for why it is `legacy_`.
    #[serde(rename = "legacy_rank")]
    pub legacy_rank: i32,
}

/// The carrier a message rule reads: exactly one of the two, checked at compile time.
///
/// Two optional fields rather than a tagged enum, for the same reason the carrier match spec uses them:
/// an externally-tagged enum needs `{"attribute": {"attribute": "k"}}` in JSON, which is the shape
/// nobody writes and serde rejects silently at the file level. Requiring exactly one is the check that
/// makes this equivalent while staying readable.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ReadSpec {
    #[serde(default)]
    pub attribute: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    /// Ordered carrier alternatives: the first of these the span carries is read, and the observation is
    /// tagged with **that** key.
    ///
    /// A dialect that renamed a key keeps accepting the old one, and the tag has to be the key actually
    /// found or two spans carrying different spellings would be indistinguishable downstream.
    #[serde(default)]
    pub attribute_any_of: Vec<String>,
    /// An *indexed attribute family*: `<prefix>.0.role`, `<prefix>.0.content`, `<prefix>.1.role`, ...
    ///
    /// One entry per index, each assembled from every key under it with the prefix stripped. This is an
    /// OpenTelemetry encoding - a list of objects flattened into dotted keys because attributes are a
    /// flat map - so reading it is a generic capability, not a producer's policy. The carrier each entry
    /// is tagged with is `<prefix>.<index>`, which is what makes two turns of one family distinguishable
    /// downstream.
    #[serde(default)]
    pub indexed_family: Option<String>,
    /// A sub-level of each indexed entry whose members are read at the top of the object.
    ///
    /// One dialect nests the message inside the entry - `<prefix>.0.message.role` - while also putting
    /// entry-level members beside it. Both are collected, the sub-level's names unprefixed and the rest as
    /// they stand, and the observation is tagged with `<prefix>.<index>.<member>` because that is the
    /// payload it came from.
    #[serde(default)]
    pub entry_member: Option<String>,
}

impl ReadSpec {
    /// How many carriers this names. Exactly one is required.
    pub fn named_count(&self) -> usize {
        usize::from(self.attribute.is_some())
            + usize::from(self.event.is_some())
            + usize::from(self.indexed_family.is_some())
            + usize::from(!self.attribute_any_of.is_empty())
    }
}

/// How a raw attribute string becomes a value.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParseMode {
    /// Parse as JSON; skip the carrier entirely if it does not parse.
    Json,
    /// Parse as JSON, keeping the raw text as a string if it does not parse.
    JsonOrString,
    /// Keep the raw text. Some carriers hold prose, and parsing it would turn a bare word into a
    /// non-string or an accidental number into a number.
    Text,
}

/// The envelope a bare payload is wrapped in.
///
/// Some carriers hold a payload rather than a message - a tool's arguments, an instruction, a response's
/// text - and what that payload *is* is a fact about the carrier, so the envelope is declared beside it.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WrapSpec {
    pub role: String,
    /// The member the read value becomes. Defaults to `content`.
    ///
    /// Not always content: a response carrying only tool calls has no content, and putting the calls
    /// under `content` would render them as the assistant's prose.
    #[serde(default)]
    pub content_as: Option<String>,
    /// Literal members added to the envelope.
    #[serde(default)]
    pub members: BTreeMap<String, JsonValue>,
    /// Members taken from *other* attributes of the same span.
    ///
    /// A dialect writes one logical message across several attributes - the arguments here, the tool's
    /// name and call id there - and which attribute holds which part is exactly the knowledge that
    /// belongs in an asset.
    #[serde(default)]
    pub attach: Vec<AttachSpec>,
    /// Wrap the value in a *content block* first, and make that block the message's only content.
    ///
    /// A tool call is not a bare object under a role: it is a `tool_use` block, and the block shape is
    /// what carries the name and the id through normalisation. Emitting the arguments without them
    /// produced a nameless call the pipeline then discarded - extracted and *then* dropped, which is
    /// worse than not reading it, because every layer looked fine.
    #[serde(default)]
    pub block: Option<BlockSpec>,
}

/// A content block built around the read value.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BlockSpec {
    /// The block's `type` member - `tool_use`, `tool_result`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The member the read value becomes inside the block. Defaults to `content`.
    #[serde(default)]
    pub content_as: Option<String>,
    /// Members taken from sibling attributes, as on the envelope.
    #[serde(default)]
    pub attach: Vec<AttachSpec>,
}

/// One member taken from a sibling attribute.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AttachSpec {
    /// The attribute to read.
    pub from: String,
    /// The member it becomes.
    #[serde(rename = "as")]
    pub as_member: String,
    /// How to read it. Defaults to text.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Attach only when the source attribute equals this exactly.
    ///
    /// How a boolean flag arrives: an attribute whose string is `"true"`. Without the comparison the
    /// literal `"false"` would attach as a truthy value.
    #[serde(default)]
    pub when_equals: Option<String>,
    /// The literal to attach instead of the attribute's value, for a flag.
    #[serde(default)]
    pub value: Option<JsonValue>,
    /// Treat a blank value as absent, so the fallbacks below apply.
    #[serde(default)]
    pub blank_is_absent: bool,
    /// Remove a leading `[TAG]\n` marker before parsing.
    ///
    /// One dialect tags a structured payload with the tool it belongs to and then writes the JSON beneath
    /// it; parsing without stripping fails, and the member would silently fall back to its default.
    #[serde(default)]
    pub strip_bracket_tag: bool,
    /// Fall back to the span name with this prefix removed, trimmed, when the attribute is absent.
    ///
    /// The conventions prescribe `execute_tool {name}` as a tool span's name, so a producer that omits
    /// the attribute still names the tool - and an unnamed call is unusable downstream.
    #[serde(default)]
    pub or_span_name_after: Option<String>,
    /// Attach this literal when nothing else supplied a value.
    ///
    /// Distinct from omitting the member: a block whose shape *requires* a name carries an empty one
    /// rather than none, and the two are different values to anything hashing the payload.
    #[serde(default)]
    pub default: Option<JsonValue>,
    /// Place this member *after* the content member rather than before it.
    ///
    /// Member order is declared because it is *observable*: this map preserves insertion order and the
    /// message is stored as serialised JSON, so moving a member changes the persisted bytes and with them
    /// the reconstruction cache digest. It does **not** change the normalised content hash - the feed sorts
    /// object keys before hashing - so this is about reproducing what was stored, not about identity. The
    /// orders here are what the extractors emitted, which is why they are stated rather than chosen.
    #[serde(default)]
    pub after_content: bool,
}

/// What an emitted observation is.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmitTarget {
    Message,
    ToolDefinitions,
}

/// One documented shape of a payload: where to look, what to require, and what to carry down.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    #[serde(default)]
    pub doc: Option<String>,
    /// A member of the parsed object to read. Absent means the parsed value itself.
    #[serde(default)]
    pub select: Option<String>,
    /// Treat the selected value as a list and read each element.
    #[serde(default)]
    pub each: bool,
    /// After selecting an element, descend to this member - `choices[].message`.
    #[serde(default)]
    pub descend: Option<String>,
    /// Copy these members from the element into the descended value before emitting.
    ///
    /// A provider puts the reason a turn stopped beside the message rather than inside it, and dropping
    /// it loses the only record that a response was truncated.
    #[serde(default)]
    pub lift: Vec<String>,
    /// The shape an observation must have to be emitted.
    ///
    /// A predicate set, so "has a role and content", "is an object" and "is a non-empty string" are one
    /// vocabulary rather than three fields that grew one at a time.
    #[serde(default)]
    pub require: PredicateSet,
}

/// What an emitted value must look like. Both forms exist because the extractors use both, and the
/// difference is real: a message needs a role *and* content to be a message, while a *response* may
/// legitimately carry content with no role.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ShapeRequirement {
    /// Every one of these members must be present.
    #[serde(default)]
    pub all_of: Vec<String>,
    /// At least one of these members must be present.
    #[serde(default)]
    pub any_of: Vec<String>,
    /// The value must be an object.
    ///
    /// Its own fact, because a member requirement cannot express it: a scalar has no members, so an
    /// empty requirement admits it. One dialect's prompt array legitimately holds non-objects, and
    /// emitting one as a message produces a turn with no role and no content.
    #[serde(default)]
    pub is_object: bool,
}

/// Which members an indexed entry must carry.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct MemberRequirements {
    /// Every one of these must be present.
    #[serde(default)]
    pub all_of: Vec<MemberRequirement>,
    /// At least one of these must be present.
    #[serde(default)]
    pub any_of: Vec<MemberRequirement>,
}

/// One member, and how its presence is decided.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MemberRequirement {
    pub name: String,
    #[serde(default)]
    pub presence: MemberPresence,
}

/// How a member's presence is established.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberPresence {
    /// The member's own key exists.
    #[default]
    Exact,
    /// Some key nested under it exists - the member is an array or object flattened into dotted keys.
    Nested,
    /// Either.
    Either,
}

/// A message assembled from several attributes of one span.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ComposeSpec {
    /// The carrier the assembled message is tagged with.
    pub tag: String,
    /// The members, in the order they are inserted - which is observable, since content identity is
    /// hashed from the payload.
    pub members: Vec<ComposeMember>,
    /// Literal members added *after* every source member.
    ///
    /// Position matters and this is why it is a separate field: the code being replaced inserts the role
    /// last, after everything it collected, so a payload built role-first would be stored with different
    /// bytes - which changes the reconstruction cache digest, though not the normalised content hash.
    #[serde(default)]
    pub trailing: BTreeMap<String, JsonValue>,
}

/// One member of a composed message: a named source, or a sweep of a prefix.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ComposeMember {
    /// The member's name. Absent for a sweep, which takes its names from the keys it finds.
    #[serde(rename = "as", default)]
    pub as_member: Option<String>,
    /// Ordered sources; the first the span carries wins.
    #[serde(default)]
    pub from_any_of: Vec<String>,
    /// How to read it. Defaults to text.
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// A last-resort source, used only where the gate holds.
    ///
    /// Separate from `from_any_of` because it is *conditional*: this key is not the dialect's own, so
    /// reading it unguarded would claim a generic carrier that belongs to whatever wrote it.
    #[serde(default)]
    pub fallback: Option<ComposeFallback>,
    /// Collect every attribute under this prefix, keyed by the remainder.
    #[serde(default)]
    pub sweep_prefix: Option<String>,
    /// Names the sweep skips, because a named member above already read them.
    #[serde(default)]
    pub except: Vec<String>,
}

/// A conditional last-resort source.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ComposeFallback {
    pub from: String,
    /// The evidence required before the fallback is read.
    pub when: DetectMatch,
    #[serde(default)]
    pub parse: Option<ParseMode>,
}

/// A text carrier read as tagged sections.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SectionsSpec {
    /// The separator between sections.
    pub split_on: String,
    /// Routes, tried in order; the first whose tag matches wins, and a route with no `tag_prefix` is the
    /// default.
    pub routes: Vec<SectionRoute>,
}

/// What to do with a section whose tag matches.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SectionRoute {
    #[serde(default)]
    pub doc: Option<String>,
    /// The tag prefix this route claims. Absent means "any section not claimed above".
    #[serde(default)]
    pub tag_prefix: Option<String>,
    /// The role the emitted message carries.
    pub role: String,
    /// Build a content block instead of putting the body under `content`.
    #[serde(default)]
    pub block: Option<SectionBlock>,
    /// Drop the section entirely when every one of these holds, tested against
    /// `{"capture": <tag remainder>, "body": <section body>}`.
    ///
    /// Predicates rather than a fused pair. It was `capture_lacks_prefix` **and** `body_starts_with`,
    /// which is one producer's policy in the shape of a field - the weakest feature in the vocabulary by
    /// its own test. What it expresses is unchanged and still narrow on purpose: a dialect writes each
    /// tool result twice, once as the text the model saw tagged with the call id and once as raw
    /// structured telemetry tagged with the tool's name, and emitting both shows every result twice. The
    /// conditions stay conjunctive so that if the id prefix ever changes, an unrecognised section reaches
    /// the feed unlinked rather than vanishing from it.
    #[serde(default)]
    pub skip_when: PredicateSet,
}

/// A block built from a section, carrying what the tag captured.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SectionBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    /// The member the tag's remainder becomes - an id that pairs this section with a call.
    #[serde(default)]
    pub capture_as: Option<String>,
    /// The member the body becomes. Defaults to `content`.
    #[serde(default)]
    pub content_as: Option<String>,
}

// ============================================================================
// VALUE PREDICATES
// ============================================================================

/// A condition on a JSON value, or on a member of it.
///
/// One vocabulary for every question the rules ask *about a value*: whether an alternative's shape holds,
/// whether a source is eligible, whether a section is dropped. Before this there were three bespoke
/// spellings - a member-name list, an `is_object` flag, and a fused "capture lacks prefix and body starts
/// with" pair - and a fourth was about to be added for a dialect that needs "this member is an object, or
/// that one is a non-empty string". Three narrow predicates are harder to reason about than one, and the
/// fused pair was producer policy wearing a generic name.
///
/// Deliberately *not* used for attribute-key presence (`MemberRequirements`): that asks about a flat map of
/// dotted keys, where "nested" means "some other key starts with this one". Same word, different domain -
/// and one type spanning both would have to mean different things depending on where it was used.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ValuePredicate {
    /// The member to test. Absent means the value itself.
    #[serde(default)]
    pub path: Option<String>,
    /// The member must be present. Implied when the predicate names nothing else.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The value's JSON kind.
    #[serde(default)]
    pub kind: Option<ValueKind>,
    /// A string, array or object must not be empty. Meaningless for other kinds, and refused there.
    #[serde(default)]
    pub non_empty: Option<bool>,
    /// A string must start with this.
    #[serde(default)]
    pub starts_with: Option<String>,
    /// A string must *not* start with this.
    #[serde(default)]
    pub lacks_prefix: Option<String>,
}

/// A JSON kind, for `ValuePredicate::kind`.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

/// A set of value predicates, combined.
///
/// `all` and `any` both, because the dialects need both and the difference is real: a request's message
/// needs a role *and* content, while a response may carry either a structured message *or* streamed text.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct PredicateSet {
    #[serde(default)]
    pub all: Vec<ValuePredicate>,
    #[serde(default)]
    pub any: Vec<ValuePredicate>,
}

impl PredicateSet {
    /// Nothing to check.
    pub fn is_empty(&self) -> bool {
        self.all.is_empty() && self.any.is_empty()
    }
}
