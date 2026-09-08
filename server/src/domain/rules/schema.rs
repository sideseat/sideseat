//! The rule asset format, and how assets are found, ordered and digested.
//!
//! Assets are embedded from `server/rules/` as a *directory*, deliberately: adding a framework must be
//! adding a file, with no Rust change. A hand-maintained `include_str!` list would mean the binary
//! still knows which frameworks exist, which is the thing the mandate forbids.

use std::collections::BTreeMap;

use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::Value as JsonValue;
pub use serde_json_path::JsonPath;

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
    /// The events this dialect writes messages on.
    ///
    /// Recognition, not reading: an event named here is read, and one not named by any asset is ignored
    /// entirely. Declared because it is the same kind of fact as a carrier - which key a producer writes -
    /// and as a Rust list it meant a new `when_event` rule was a valid but *dead* declaration until
    /// somebody also edited the list.
    #[serde(default)]
    pub message_events: Vec<MessageEvent>,
    /// Content-block shapes this dialect writes.
    #[serde(default)]
    pub content_blocks: Vec<ContentBlockRule>,
    /// Facts about a *span* this dialect can establish, as opposed to about a carrier.
    ///
    /// "Is this a tool execution" is one question with several answers - an operation name, a span-kind
    /// attribute, a pair of attributes that only appear together - and each dialect knows its own. The
    /// union answers it, so a dialect declares its signal rather than the code carrying a list of them.
    #[serde(default)]
    pub span_facts: Vec<SpanFactRule>,
    /// Named reading tables other rules may apply.
    ///
    /// One dialect's message shapes are recognised at four different selection points - the node itself, a
    /// `messages` list, every member of a state object, and a nested state object - and a table repeated
    /// per point is four places to fix a shape. Referenced as `<file id>.<name>`, resolved at compile time
    /// by inlining, and a fragment's own cases may **not** reference a fragment: one level, no recursion,
    /// nothing to bound at runtime.
    #[serde(default)]
    pub fragments: BTreeMap<String, Fragment>,
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
    #[serde(default)]
    pub carrier_holds_span_input: Option<bool>,
    #[serde(default)]
    pub carrier_holds_expandable_message_array: Option<bool>,
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
/// **No longer small, and that is the finding.** It began as "read a carrier, parse it, emit it" and each
/// dialect added a generic field that prevented a measured defect. Every one is still declarative and
/// non-Turing-complete, but selection, projection, predicates and object construction have been built by
/// hand here - which is what an expression language already standardises, and a hand-built path resolver
/// is where a real bug lived (a literal dotted key read as a nested path).
///
/// So the shaping half of this type is **frozen** and moves to JMESPath, which is a published spec with a
/// parser and quoted identifiers. What stays is the structural half - which carrier, who claims it, in what
/// order, what it emits - because that is ownership and policy rather than a transform.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageRule {
    pub id: String,
    #[serde(default)]
    pub doc: Option<String>,
    /// The *events* this rule applies to. Non-empty makes it an event rule: its reads resolve against the
    /// event's own attributes rather than the span's, and its observations are tagged as events.
    ///
    /// A separate dimension from `when`, because an event name is not a span attribute and a rule gated on
    /// one would otherwise never hold.
    ///
    /// `Option` so an explicit `[]` is distinguishable from silence: a branch leaf may not declare this at
    /// all, and a check comparing against the default accepted the explicit spelling as a no-op.
    #[serde(default)]
    pub when_event: Option<Vec<String>>,
    /// Whether this rule's readings *replace* the event's raw form rather than adding to it.
    ///
    /// One convention event is a container: its own attributes are the two message carriers inside it, and
    /// emitting the container as well would report the conversation twice. Another carries a message *and* a
    /// bundled tool result, where both are wanted. Which it is, is a fact about the event.
    #[serde(default)]
    pub replaces_raw_event: Option<bool>,
    /// When this rule is read: with the dialects, or only if none of them produced a message.
    ///
    /// A *stage*, owned by the engine rather than a rule asking about other rules. Some carriers really are
    /// a last resort - the generic input/output pair, and two dialects' stand-ins for it - and "only if
    /// nothing recognised this span" is what the retired fallback extractor's position in the list meant.
    /// Gating on a sibling carrier's absence instead is measurably broader: a span with a recognised
    /// conversation and an unrelated `response` gains a message it should not have.
    #[serde(default)]
    pub stage: Option<MessageStage>,
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
    #[serde(default)]
    pub tool_repr: Option<ToolReprSpec>,
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
    ///
    /// Defaults to a message, and a branch set's parent declares none: its sub-readings each say what they
    /// emit, so a value here would be unused.
    #[serde(default)]
    pub emit: Option<EmitTarget>,
    /// Emit one observation whose value is the array of everything read, rather than one per reading.
    ///
    /// Tool definitions arrive as a set rather than a sequence of messages, so a dialect's whole tool list
    /// is one observation - emitting one per tool would make each look like a separate declaration.
    #[serde(default)]
    pub aggregate_into_array: Option<bool>,
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
    /// Readings that all contribute, rather than the first that yields.
    ///
    /// A different relation from `alternatives`, and mixing them up loses messages: one dialect writes a
    /// turn's history under one member and *the answer itself* under another, so reading them as
    /// alternatives dropped the assistant output of every run that carried history. Refused together with
    /// `alternatives`, because "first wins" and "all contribute" cannot both be true of one list.
    #[serde(default)]
    pub also: Vec<Alternative>,
    /// A reading used only when nothing else in this rule emitted anything.
    ///
    /// Keeps a span from being silently empty: where a payload matches no documented shape, it is better
    /// to keep it whole than to return nothing and leave no trace that it arrived.
    #[serde(default)]
    pub fallback: Vec<Alternative>,
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
    pub reads_tool_spans: Option<bool>,
    /// Several carrier readings with a *local* order between them.
    ///
    /// One dialect reads a carrier only when nothing else supplied the conversation - a condition about
    /// what *else* was found. Expressing that with rule ids would make ids into control-flow targets and
    /// the interpreter's execution history into something a rule can observe; keeping the readings inside
    /// one rule keeps it a pure function of the span, with no global state and no cycles to worry about.
    #[serde(default)]
    pub branch_set: Option<BranchSet>,
    /// Read an array-valued carrier element by element, in declared passes.
    ///
    /// Passes rather than per-element routing, because the order is observable: one dialect emits every
    /// recognised event *before* any grouped block, so a single interleaved scan would return a different
    /// conversation. Declaring passes makes that ordering a statement rather than an artefact of how the
    /// code happened to loop.
    #[serde(default)]
    pub elements: Option<ElementsSpec>,
    /// Apply this rule's readings at every node of a bounded tree walk.
    ///
    /// One dialect's carrier is a *state object* its nodes write into, so a conversation can sit at the top
    /// level, under one member, or nested a level or two down. Bounded on purpose - an explicit depth, and
    /// members already read at each node are pruned - because the point is to find a state member, not to
    /// trawl a payload for anything message-shaped.
    #[serde(default)]
    pub walk: Option<WalkSpec>,
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
    pub require_non_blank: Option<bool>,
    /// Skip a carrier whose value is empty.
    ///
    /// An attribute present and empty is not evidence of a message, and wrapping it produces a turn with
    /// nothing in it - which the no-empty-content invariant then rejects downstream.
    #[serde(default)]
    pub require_non_empty: Option<bool>,
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
    ///
    /// Required at the top level and **forbidden** inside a branch set: there the local order decides, so a
    /// rank would be a number that looks like it means something and does not.
    #[serde(rename = "legacy_rank", default)]
    pub legacy_rank: Option<i32>,
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
    /// Entry members to read as a number where the text is one.
    ///
    /// OTel attributes are strings, so a relevance score arrives as `"0.9"` - and a score is a number.
    /// Named rather than sniffed, because a version, an id or a postcode is text that happens to parse.
    #[serde(default)]
    pub numeric_members: Vec<String>,
    /// Read one *value* out of each indexed entry, rather than the entry's assembled members.
    ///
    /// A family whose entries each hold a single serialised payload - one tool's JSON schema at
    /// `<prefix>.<n>.tool.json_schema` - is a list of those payloads, not a list of objects with a member
    /// called `tool`. The projection says which leaf is the datum.
    #[serde(default)]
    pub entry_value: Option<JsonPath>,
    /// How the projected value is read. `json` **drops** an entry whose payload does not parse, which is
    /// what a schema that failed to parse always meant - an indexed member is otherwise sniffed, and a
    /// malformed schema would be emitted as the string it is, reported as a tool definition.
    #[serde(default)]
    pub entry_value_parse: Option<ParseMode>,
    /// A richer copy of these same messages, held by another carrier and matched by position.
    #[serde(default)]
    pub overlay: Option<OverlaySpec>,
}

/// One dialect's evidence for a fact about a span.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SpanFactRule {
    pub id: String,
    pub doc: Option<String>,
    /// The fact this is evidence of.
    pub fact: SpanFact,
    /// Any one of these establishes it.
    pub signals: Vec<SpanSignal>,
}

/// A fact about a span that rules and readers ask about by name.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SpanFact {
    /// The span *is a tool running*, so its messages are that tool's input and result rather than a
    /// model's turn. Rules that must not read such a span are gated on it (`reads_tool_spans`).
    ToolExecution,
}

/// One piece of evidence. At least one form, and both together read as a conjunction.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SpanSignal {
    pub doc: Option<String>,
    /// An attribute with this value.
    #[serde(default)]
    pub attr_equals: Option<KeyValue>,
    /// Compare that value case-insensitively. One convention writes its span kind in capitals.
    #[serde(default)]
    pub ignore_case: bool,
    /// **Every** one of these attributes is present. A conjunction, not a choice: a tool name alone sits on
    /// a model span that merely mentions a tool, while the name *and* a call id together are a call
    /// being run.
    #[serde(default)]
    pub attrs_present: Vec<String>,
}

/// Tool definitions a carrier holds as a language's `repr` rather than as JSON.
///
/// The grammar is sealed in `rules::tool_repr` because it is a property of the *language*. Everything a
/// particular framework calls its own - which member holds the tools, which repr fields name them, which
/// labels its embedded documentation uses, how its type names map to JSON Schema's - is here, because
/// that is its vocabulary and not a fact about Python.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolReprSpec {
    pub doc: Option<String>,
    /// The carrier's entries. One entry at a time, so two entries each holding a list interleave as the
    /// payload has them rather than by path - which is what keeps the reported order the framework's own.
    pub entries: JsonPath,
    /// Where an entry holds tools, in order. Each resolved value is tried as a tool definition.
    pub candidates: Vec<JsonPath>,
    /// The repr fields naming a tool and its documentation - `name='search'`.
    pub name_field: String,
    pub description_field: String,
    /// The labels the embedded documentation uses.
    pub name_label: String,
    pub description_label: String,
    pub arguments_label: String,
    /// What makes a string a `repr` rather than a bare tool name. Without these a name containing a space
    /// would be parsed as a repr and yield nothing.
    pub repr_markers: Vec<String>,
    /// Where a tool object states its parameters, in order.
    pub parameter_members: Vec<String>,
    /// The repr fields that may follow a loosely-quoted one. Their appearance is where that value ends.
    ///
    /// A single-quoted value holding a dict repr contains unescaped single quotes, so its closing quote
    /// cannot be found by scanning - the value runs either to the `')` that closes the constructor or to
    /// the next field. Which fields those are is the framework's vocabulary, not the language's.
    #[serde(default)]
    pub field_terminators: Vec<String>,
    /// The language's type names, mapped to JSON Schema's. Compared case-insensitively, and ordered
    /// because the first match wins.
    pub type_map: Vec<(String, String)>,
    /// The type for a name the map does not hold. A tool whose argument type is unrecognised is still a
    /// tool, so the definition is reported with the widest type rather than dropped.
    pub type_default: String,
}

/// Another carrier of the same span describing the same messages at higher fidelity.
///
/// Not an enrichment of what some other reader produced - both carriers are attributes of one span, and
/// the join is positional: entry *n* of the family and member *n* of the other carrier's list are the
/// same message. A flattened family loses whole content blocks and redacts urls, while the serialised copy
/// beside it keeps them, so where both describe one message the richer one is preferred.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct OverlaySpec {
    pub doc: Option<String>,
    /// The attribute holding the richer copy.
    pub from: String,
    #[serde(default)]
    pub parse: Option<ParseMode>,
    /// Ordered paths to the counterpart list; the first that resolves to an array is used.
    pub select_any_of: Vec<JsonPath>,
    /// Unwrap a list of exactly one list. A serialiser that accepts a batch of conversations writes one
    /// conversation as a batch of one, and the members of *that* are the messages.
    ///
    /// Exactly one, deliberately: a batch of two is two conversations, and joining a family by position
    /// against the first of them would attribute one conversation's content to another's messages.
    #[serde(default)]
    pub unwrap_single_element_list: bool,
    /// What the list must look like to be this dialect's own serialisation. Without it, any array of
    /// objects at that path would be treated as the same messages.
    #[serde(default)]
    pub witness: PredicateSet,
    /// Only entries carrying members under this prefix are overlaid - the flattened form of the content
    /// that is known to be lossy.
    pub when_member_prefix: String,
    /// Ordered paths to the counterpart's content.
    pub content_any_of: Vec<JsonPath>,
    /// What that content must be for the overlay to be an improvement.
    #[serde(default)]
    pub require: PredicateSet,
    /// The member the content becomes, replacing every member under `when_member_prefix`.
    pub as_member: String,
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
    /// Parse as JSON, then parse any *string* member of the resulting array as JSON too.
    ///
    /// An OTLP array attribute whose elements are each a serialised object arrives as an array of strings,
    /// because the attribute type has no nesting. Generic: the encoding is OTLP's, not a producer's.
    StringifiedArray,
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
    /// A literal role. One of this and `role_from` is required.
    #[serde(default)]
    pub role: Option<String>,
    /// A JSONPath whose value is the role, relative to the reading being wrapped.
    ///
    /// Several dialects put the role *in* the payload - Gemini's `{parts, role}` is the clearest case - so
    /// a literal here would either be wrong or need one rule per role.
    #[serde(default)]
    pub role_from: Option<JsonPath>,
    /// Rename a role the payload supplied.
    ///
    /// A provider's own vocabulary: one calls the assistant `model`, and normalising that here keeps the
    /// alias beside the dialect that uses it rather than in a shared table nothing points at.
    #[serde(default)]
    pub role_map: BTreeMap<String, String>,
    /// Treat `role_map` as the complete list: a value not in it falls back to `role` rather than being used
    /// as a role itself.
    ///
    /// One dialect names the *speaker* where another names the role - `source: "planner"` means an
    /// assistant, not a role called `planner` - so which of the two a member is has to be declared.
    #[serde(default)]
    pub role_map_is_closed: bool,
    /// A JSONPath whose value becomes the content, relative to the reading being wrapped.
    #[serde(default)]
    pub content_from: Option<JsonPath>,
    /// Ordered paths for the content; the first that resolves wins.
    ///
    /// One dialect serialises a message three ways depending on how it was constructed, and the content sits
    /// in a different member each time - so a single path reads two of the three as empty.
    #[serde(default)]
    pub content_from_any_of: Vec<JsonPath>,
    /// The content when none of the paths above resolve. Absent means the reading is not this shape.
    ///
    /// An explicit `null` is a default of JSON null, not the absence of one: a dialect reports a tool that
    /// returned nothing that way, and the two readings differ.
    #[serde(default, deserialize_with = "explicit_value")]
    pub content_default: Option<JsonValue>,
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
    /// Build a block from another member and put it **before** the content.
    ///
    /// One dialect reports a model's reasoning in a sibling member of its reply, and the canonical form is a
    /// thinking block ahead of the text - so the two become one content list rather than two messages.
    #[serde(default)]
    pub prepend_block: Option<PrependSpec>,
    /// Build the canonical tool-call list from an array of the dialect's own calls.
    ///
    /// A *typed* constructor for a canonical target, not a general object builder: the shape is
    /// `{id, type: "function", function: {name, arguments}}` and only the sources are rule data. Arguments
    /// arrive as a serialised JSON string as often as an object, so parsing them is declared here rather
    /// than left to whoever reads the payload later.
    #[serde(default)]
    pub tool_calls_from: Option<ToolCallsSpec>,
    /// Build a **single** tool call at a named member, as `{name, arguments}`.
    ///
    /// The normaliser already unwraps a `tool_call` member (`sideml/tools.rs`), so this is a canonical
    /// target like the list above rather than a general object builder.
    #[serde(default)]
    pub tool_call_from: Option<SingleToolCallSpec>,
    /// A condition on the **constructed** message, checked after the envelope is built.
    ///
    /// Some shapes can only be judged once assembled: one dialect's tool result is worth keeping if it
    /// ended up with a name, a call id or content, and the call id may have come from the element or from
    /// its parent - so the question cannot be asked of either alone.
    #[serde(default)]
    pub require_after: PredicateSet,
    /// Wrap only where the value is not already message-shaped.
    ///
    /// A generic carrier holds either a message or bare data: `output.value = "the answer"` is the answer,
    /// and `output.value = {"role": …}` is already a message. Wrapping the second buries the conversation a
    /// level down; not wrapping the first loses it entirely, because normalisation looks for `role` on a
    /// string and finds nothing.
    #[serde(default)]
    pub only_plain_data: bool,
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
    /// Why this member is taken from where it is, where that is not obvious. A field rather than a comment,
    /// as everywhere else here, because the explain trace surfaces it.
    #[serde(default)]
    pub doc: Option<String>,
    /// The attribute to read. One of this and `from_path` is required.
    #[serde(default)]
    pub from: Option<String>,
    /// Ordered paths into the value being wrapped; the first that resolves wins.
    ///
    /// The same serialisation variance as the content: a member may sit at the top level or under the
    /// wrapper a serialiser added.
    #[serde(default)]
    pub from_value_any_of: Vec<JsonPath>,
    /// The attached value must satisfy this, or the member is left off.
    ///
    /// An empty list is not a set of tool calls, and attaching one makes a plain reply look like a call.
    #[serde(default)]
    pub require: PredicateSet,
    /// A path into the rule's *own parsed payload*, rather than a sibling attribute.
    ///
    /// Relative to the whole payload, deliberately: a dialect reports why a turn stopped beside the
    /// content rather than inside it, so the member being attached sits outside the part being wrapped.
    #[serde(default)]
    pub from_path: Option<JsonPath>,
    /// Lower-case the attached string.
    ///
    /// A provider writes finish reasons in upper case and the canonical form is lower; declared because
    /// lower-casing a payload that is meant to be verbatim would change it.
    #[serde(default)]
    pub lowercase: bool,
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
    /// The literal to attach instead of the attribute's value, for a flag - or on its own, for a member
    /// that is part of the shape rather than something read.
    ///
    /// An explicit `null` is a value: a content block declares an unsigned signature that way, and the
    /// member has to be present rather than omitted.
    #[serde(default, deserialize_with = "explicit_value")]
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
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmitTarget {
    #[default]
    Message,
    ToolDefinitions,
    /// A list of tool *names*, as opposed to their definitions. A framework that reports only the names
    /// has said which tools were available, not what they take.
    ToolNames,
    /// The carrier is claimed and nothing is read from it.
    ///
    /// A real shape, not a loophole: one dialect's agent spans aggregate what their children already
    /// reported, and their `input.value` is a Python `repr` of framework internals. Claiming says "this is
    /// mine and holds no message", which stops a generic reader from presenting that text as a
    /// conversation - and saying it in a rule is what keeps the decision out of the code.
    Claim,
}

/// One documented shape of a payload: where to look, what to require, and what to carry down.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    #[serde(default)]
    pub doc: Option<String>,
    /// An RFC 9535 JSONPath into the parsed value. Absent means the value itself.
    ///
    /// A standard query rather than a hand-rolled path syntax - `$.tasks_output[*].messages[*]` reads every
    /// turn of every task, and `$['event.name']` reads a member whose name contains a dot, which is where
    /// the hand-built resolver had a bug. Compiled when the asset loads, so a malformed path is a startup
    /// error naming its file rather than a query that silently finds nothing.
    #[serde(default)]
    pub select: Option<JsonPath>,
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
    /// Members taken from the value this selection came from, inserted only where the element lacks them.
    ///
    /// A dialect writes a tool result's call id on the result, and on the message enclosing a batch of
    /// them when the batch shares one - so the enclosing value is a *fallback*, never an override.
    #[serde(default)]
    pub lift_from_parent: Vec<String>,
    /// A condition on the value this selection came from, rather than on the selected element.
    ///
    /// What a batch of tool results *is* is stated on the message enclosing them - its type - while the
    /// reading is one message per element, so the discriminator and the selection sit at different levels.
    #[serde(default)]
    pub require_parent: PredicateSet,
    /// The shape an observation must have to be emitted.
    ///
    /// A predicate set, so "has a role and content", "is an object" and "is a non-empty string" are one
    /// vocabulary rather than three fields that grew one at a time.
    #[serde(default)]
    pub require: PredicateSet,
    /// An envelope for *this* reading only.
    ///
    /// One reading of a payload may be a bare value needing a role while its siblings are already
    /// messages - a dialect's answer sits in a string member beside a list of turns. Overrides the rule's
    /// own `wrap` where present.
    #[serde(default)]
    pub wrap: Option<WrapSpec>,
    /// Trim a string before testing and emitting it.
    ///
    /// Declared rather than always-on: trimming a payload that is meant to be verbatim would change it.
    #[serde(default)]
    pub trim: bool,
    /// Apply this named fragment's cases to each selected element.
    ///
    /// The fragment decides what the element *is*; this reading decides *where to look*. Splitting them is
    /// the point: one dialect's state object holds its messages in four places and recognises them one way.
    #[serde(default)]
    pub then_fragment: Option<String>,
    /// What this reading is, where it differs from the rule's own target.
    ///
    /// A logged model call carries its conversation and the tools it was offered in one carrier, and they
    /// are not the same kind of thing - so the target belongs to the reading, not only to the rule.
    #[serde(default)]
    pub emit: Option<EmitTarget>,
    /// Shapes recognised at *this* selection point only, tried after the shared fragment's own cases.
    ///
    /// A shared table says what a message looks like in a dialect; a particular place that dialect writes
    /// messages may accept one shape more loosely than the rest - a bare `{content}` passed through where
    /// the table would refuse it. Putting that in the table would loosen every other point that reads it.
    #[serde(default)]
    pub extra_cases: Vec<Alternative>,
    /// For each selected element, the first of these paths that resolves.
    ///
    /// Per *element*, which is the point: one dialect's tool groups each either wrap their declarations
    /// under one of two spellings or are a declaration themselves, and deciding once for the whole array
    /// would drop the odd group out.
    #[serde(default)]
    pub then_any_of: Vec<JsonPath>,
    /// Like `then_any_of`, but chosen by the member being **present** rather than by its yielding anything.
    ///
    /// The difference is load-bearing where a wrapper may legitimately be empty: a dialect that writes
    /// `function_declarations: []` has declared no tools, and picking "the first path that yielded
    /// something" skips the present-but-empty member and falls through to emitting the wrapper itself as a
    /// tool. Presence also settles which of two spellings wins when both appear.
    #[serde(default)]
    pub then_present_any_of: Vec<JsonPath>,
    /// Fall back to the element itself when none of `then_any_of` resolved.
    #[serde(default)]
    pub else_element: bool,
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
    /// A condition on the **assembled** object, checked before it is emitted.
    ///
    /// The mirror of `require_after` on an envelope, and needed for the same reason: some shapes can only be
    /// judged once the members are together - whether the name a dialect reported is a tool anyone could
    /// call, for instance.
    #[serde(default)]
    pub require: PredicateSet,
    /// Emit the assembled object as a canonical **tool definition** rather than as a message.
    ///
    /// A dialect that reports one tool per span writes its name, documentation and parameter schema as
    /// three separate attributes. Assembling them is what `compose` does; the shape they become -
    /// `[{type: "function", function: {…}}]` - is a canonical target, so it lives here and only the sources
    /// are rule data.
    #[serde(default)]
    pub as_tool_definition: bool,
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
    /// Why this condition is the right one, where that is not obvious from the condition. A field rather
    /// than a comment, as everywhere else here, because the explain trace surfaces it.
    #[serde(default)]
    pub doc: Option<String>,
    /// A JSONPath to the value under test. Absent means the value itself.
    #[serde(default)]
    pub path: Option<JsonPath>,
    /// The member must be present. Implied when the predicate names nothing else.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The value's JSON kind.
    #[serde(default)]
    pub kind: Option<ValueKind>,
    /// A string, array or object must not be empty. Meaningless for other kinds, and refused there.
    #[serde(default)]
    pub non_empty: Option<bool>,
    /// The value begins like an identifier - a letter, a digit or an underscore.
    ///
    /// Generic in form, and declared per rule rather than folded into the tool-definition constructor: one
    /// dialect reports a synthetic aggregate under a name in parentheses, which is not a tool anyone can
    /// call, while another dialect's carriers have never needed the test. Making it canonical would change
    /// what every other carrier accepts, silently.
    #[serde(default)]
    pub identifier_like: Option<bool>,
    /// The value is not JSON null. Distinct from `exists`, which a null member satisfies, and from
    /// `non_empty`, which is about a string, array or object having contents.
    #[serde(default)]
    pub not_null: Option<bool>,
    /// A string must start with this.
    #[serde(default)]
    pub starts_with: Option<String>,
    /// A string must *not* start with this.
    #[serde(default)]
    pub lacks_prefix: Option<String>,
    /// The value must be one of these strings. An absent member satisfies nothing.
    #[serde(default)]
    pub one_of: Vec<String>,
    /// The value must not be any of these strings.
    ///
    /// An **absent** member satisfies this: "its value is not one of these" is true when there is no value,
    /// which is how a dialect's unnamed events fall through to the reading that handles them.
    #[serde(default)]
    pub none_of: Vec<String>,
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

/// An array-valued carrier read element by element.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ElementsSpec {
    /// The emitted carriers are *events*, not attributes.
    ///
    /// A real distinction, not bookkeeping: carrier semantics are declared per carrier and looked up by
    /// which kind it is, so an event reported as an attribute gets a different reading of what it is
    /// evidence of. These elements *are* events - the dialect packs them into one attribute because
    /// attributes are all it has.
    #[serde(default)]
    pub tags_are_events: bool,
    /// A JSONPath to the array. Absent means the parsed value itself.
    #[serde(default)]
    pub select: Option<JsonPath>,
    /// Passes over the elements, in order. Each scans every element.
    pub passes: Vec<ElementPass>,
}

/// One pass over the elements.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ElementPass {
    #[serde(default)]
    pub doc: Option<String>,
    /// Which elements this pass reads.
    #[serde(default)]
    pub when: PredicateSet,
    /// Emit the element itself, tagged with the value at this path.
    ///
    /// A carrier named by the *data* rather than by the rule: these elements are events, and an event's
    /// name is what downstream keys role derivation and ordering on, so tagging them all alike would erase
    /// the distinction the payload carries.
    #[serde(default)]
    pub tag_from: Option<JsonPath>,
    /// Instead of emitting each element, group runs of them and emit one message per run.
    #[serde(default)]
    pub group: Option<GroupSpec>,
}

/// Runs of consecutive elements collapsed into one message.
///
/// Bounded: one pass, no recursion, and a run ends as soon as the derived key changes.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    /// A decision table deriving the run key from an element - the first matching case wins, and an element
    /// matching none is skipped.
    pub by: Vec<DerivedCase>,
    /// The part of each element collected into the message's content.
    ///
    /// One dialect's blocks carry the real content in a member and a human-readable summary beside it, so
    /// which part is collected is a fact about the payload rather than a default.
    pub collect: JsonPath,
    /// The member the derived key becomes on the emitted message.
    pub key_as: String,
    /// The carrier each derived key is tagged with.
    pub tag_by_key: BTreeMap<String, String>,
}

/// One case of a decision table: a condition, and the value it yields.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DerivedCase {
    #[serde(default)]
    pub doc: Option<String>,
    pub when: PredicateSet,
    pub value: String,
}

/// Several readings of one span with a local order between them.
///
/// Evaluated as: every `primary`; then, only if those produced nothing, every
/// `fallback_if_primary_empty`; then every `always`, whatever happened. Nesting is refused - a branch set
/// inside a branch set would be a control structure rather than a declaration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BranchSet {
    #[serde(default)]
    pub doc: Option<String>,
    /// The readings that normally supply the conversation.
    pub primary: Vec<MessageRule>,
    /// Read only when every `primary` reading came up empty.
    #[serde(default)]
    pub fallback_if_primary_empty: Vec<MessageRule>,
    /// Read whatever the others did.
    ///
    /// The asymmetry is deliberate for at least one dialect: its *answer* must be read even when the
    /// request side was already found, because one gate covering both is what dropped the answer.
    #[serde(default)]
    pub always: Vec<MessageRule>,
}

/// A named table of readings, applied wherever a rule references it.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    #[serde(default)]
    pub doc: Option<String>,
    /// The cases, tried in order; the first that yields wins.
    pub cases: Vec<Alternative>,
}

/// A bounded walk over a state object.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WalkSpec {
    /// How many levels below the carrier to descend. Zero means the carrier itself only.
    pub max_depth: usize,
    /// Members not descended into, because the readings already took them at each node.
    #[serde(default)]
    pub prune: Vec<String>,
    /// Stop descending below a node that was itself read as a message.
    ///
    /// A message's own members are its content, not more state, so descending into one would read its parts
    /// as though they were turns.
    #[serde(default)]
    pub stop_at_match: bool,
}

/// One tool call at a named member, as the normaliser's `{name, arguments}` convention.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SingleToolCallSpec {
    pub name: JsonPath,
    /// The name used when the path resolves to nothing. A call this dialect logged without one still
    /// happened, so it is reported rather than dropped.
    #[serde(default)]
    pub name_default: Option<JsonValue>,
    pub arguments: JsonPath,
    /// The member the call becomes. Defaults to `tool_call`.
    #[serde(default)]
    pub as_member: Option<String>,
    /// The value used when `arguments` resolves to nothing.
    #[serde(default)]
    pub arguments_default: Option<JsonValue>,
}

/// A block built from another member of the same value, placed before the content.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PrependSpec {
    /// Where the block's content is, relative to the value being wrapped. Absent means no block is added,
    /// which is the ordinary case for a dialect that reports reasoning only sometimes.
    pub from: JsonPath,
    /// A condition on the value found there. A dialect writes this member as `null` when there was no
    /// reasoning, and a null is not a thought.
    #[serde(default)]
    pub require: PredicateSet,
    #[serde(flatten)]
    pub block: BlockSpec,
}

/// The canonical tool-call list, built from a dialect's own array of calls.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolCallsSpec {
    /// The array of calls, relative to the value being wrapped.
    pub select: JsonPath,
    /// Where each call's id, name and arguments are. A call missing an id or a name is skipped: the id is
    /// what pairs a result with its call, and a nameless call is unusable downstream.
    pub id: JsonPath,
    pub name: JsonPath,
    pub arguments: JsonPath,
    /// The member the list becomes. Defaults to `tool_calls`.
    #[serde(default)]
    pub as_member: Option<String>,
}

/// A default whose declared value may itself be `null`.
///
/// `Option<JsonValue>` would read an explicit `null` as "no default declared", which is a different
/// statement from "the default is null".
fn explicit_value<'de, D>(deserializer: D) -> Result<Option<JsonValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    JsonValue::deserialize(deserializer).map(Some)
}

/// One content-block shape, and the canonical block it becomes.
///
/// Exactly one target form per rule, checked at compile time. The forms are the canonical SideML blocks,
/// so this is not a general object builder: a rule says *where* a call's name is, never what a tool_use
/// block looks like.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ContentBlockRule {
    pub id: String,
    pub doc: Option<String>,
    /// Where in the normalisation chain this case is tried. Declared, because the chain's order decides
    /// which dialect answers for a shape more than one of them recognises.
    pub at: ChainPosition,
    /// Position among the cases at that point.
    pub legacy_rank: i32,
    /// The shape this case recognises.
    #[serde(default)]
    pub require: PredicateSet,
    #[serde(default)]
    pub tool_use: Option<ToolUseBlock>,
    #[serde(default)]
    pub tool_result: Option<ToolResultBlock>,
    #[serde(default)]
    pub json: Option<JsonDataBlock>,
    #[serde(default)]
    pub text: Option<TextBlock>,
    #[serde(default)]
    pub media: Option<MediaBlock>,
}

/// Where a content-block case sits relative to the provider wire formats.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChainPosition {
    /// Tried before any provider format. For a dialect whose own spelling a provider format would
    /// otherwise claim.
    BeforeProviderFormats,
    /// Tried after them, which is where a dialect's additions to a provider's vocabulary belong.
    AfterProviderFormats,
}

/// A model asking for a tool to be run.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolUseBlock {
    /// Ordered; absent is reported as null, because a provider that omits an id has still made the call.
    #[serde(default)]
    pub id: Vec<JsonPath>,
    /// Required: a nameless call names nothing to run, so the case does not recognise the block.
    pub name: Vec<JsonPath>,
    /// Ordered, and an **empty object counts as absent** - a dialect that renamed this member leaves the
    /// unused one present as `{}`, so "the first that resolves" would always pick the empty one.
    #[serde(default)]
    pub input: Vec<JsonPath>,
}

/// What a tool returned.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ToolResultBlock {
    #[serde(default)]
    pub tool_use_id: Vec<JsonPath>,
    #[serde(default)]
    pub content: Vec<JsonPath>,
    #[serde(default)]
    pub is_error: Vec<JsonPath>,
}

/// Structured data that is not prose.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct JsonDataBlock {
    #[serde(default)]
    pub data: Vec<JsonPath>,
}

/// Prose. Only a string is text.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TextBlock {
    pub text: Vec<JsonPath>,
}

/// Bytes, or a reference to them. The block's kind and whether it is a reference are both *derived*.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MediaBlock {
    pub media_type: Vec<JsonPath>,
    pub data: Vec<JsonPath>,
}

/// When a rule is read.
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageStage {
    /// With the dialects, in rank order. The ordinary case.
    #[default]
    Dialect,
    /// Only if no dialect-stage rule produced a message or a claim.
    Fallback,
}

/// One event that carries messages.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MessageEvent {
    pub name: String,
    pub doc: Option<String>,
}
