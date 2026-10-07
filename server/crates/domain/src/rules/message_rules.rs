//! Declarative message extraction and carrier parsing.
//!
//! This is the part of the mandate that matters most, and the part the architecture already made
//! tractable. Extraction here does **not** interpret a conversation - normalisation happens at query
//! time, on the stored raw payload - so an extractor's whole job is to *claim a carrier and keep what it
//! held*. Several of them are already nothing but that, expressed as Rust:
//!
//! ```text
//! if let Some(parsed) = extract_json(attrs, "input.value") {
//!     messages.push(RawMessage::from_attr("input.value", timestamp, parsed));
//! }
//! ```
//!
//! Which is a declaration with a function around it. This module is the declaration without one.
//!
//! Every shape this list once excepted is now covered, which is why the list is gone: the bounded state
//! walk, the typed-message decision table, the positional join against a serialised state member, and the
//! `repr` grammar that is not JSON (sealed in `tool_repr`, reached through a declared vocabulary). Messages,
//! tool definitions, tool names and *events* are all declared, and `messages.rs` names no framework.
//!
//! Selection is RFC 9535 **JSONPath** (`serde_json_path`), not JMESPath: a JSONPath query returns *borrowed*
//! references into the original value, so member order survives, where JMESPath re-materialises through a
//! sorted map and alphabetised every provider payload it touched. That was measured, not assumed - it
//! reordered goldens. What stays in this module is the structural algebra a query language cannot express:
//! claiming, stages and precedence, bounded traversal, positional joins, grouping, and the canonical
//! constructors. See `docs/engineering/framework-rules-engine.md`.
//!
//! The engine emits *values*, not `RawMessage`s: the ingestion types live in `domain::traces`, and the
//! engine having to know them would point the dependency the wrong way for no benefit.

use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

use super::detect_rules::CompiledDetect;
use super::schema::{
    Alternative, AttachSpec, BlockSpec, ComposeMember, ComposeSpec, DetectMatch, ElementsSpec,
    EmitTarget, KeyValue, MemberPresence, MemberRequirements, MessageRule, OverlaySpec, ParseMode,
    PredicateSet, ReadSpec, SectionsSpec, SingleToolCallSpec, ToolCallsSpec, ToolReprSpec,
    ValueKind, ValuePredicate, WrapSpec,
};
use super::{detect_rules, expr, refusal, schema, tool_repr};

/// One reading of a payload: the value **as constructed** and its target where it differs from the rule's.
///
/// The envelope used to travel here and be applied by the caller, which made construction failure invisible to
/// the coalesce: an alternative that selected something produced a candidate, `readings()` returned it as *the*
/// answer, and the caller then found the envelope unbuildable and dropped it - so the alternatives after it and
/// the rule's `fallback` were never tried, and the rule emitted nothing where a later shape would have worked.
///
/// Building inside the coalesce makes "could not be built" mean "this alternative produced nothing", which is
/// the same answer as "this shape does not match" and the only one the coalesce can act on.
type Reading = (JsonValue, Option<EmitTarget>, Vec<String>);

/// One node's readings, and which clauses **recognised** it.
///
/// Two outputs from one pass. They answer different questions and are needed together: the built readings are
/// the observations, while `recognised` says which declarations matched the payload's shape - whether or not
/// their envelope could then be made. The walk's stop reads the second, so a clause whose construction failed
/// still means "this node is that shape", and one evaluation serves both (it used to take two, which on a
/// deep payload is the difference between linear and quadratic work).
struct Selection {
    built: Vec<Reading>,
    recognised: Vec<String>,
}

impl Selection {
    fn absorb(&mut self, other: Selection) {
        self.built.extend(other.built);
        for id in other.recognised {
            if !self.recognised.contains(&id) {
                self.recognised.push(id);
            }
        }
    }
}

/// What building a message needs, threaded into the coalesce so construction happens before it commits.
///
/// `None` at the aggregate path: there the entries become one array and the rule's envelope wraps *that*, once,
/// rather than each entry.
#[derive(Clone, Copy)]
struct Construction<'a> {
    /// The rule's envelope, used where a reading declares none of its own.
    rule_wrap: Option<&'a WrapSpec>,
    ctx: &'a MessageContext<'a>,
    /// The whole payload, which an envelope may read members from.
    root: &'a JsonValue,
}

/// What an ingestion knows when it asks which carriers to read.
#[derive(Debug, Clone, Copy)]
pub struct MessageContext<'a> {
    pub span_name: &'a str,
    pub scope_name: Option<&'a str>,
    /// The map a rule's `read` draws from - a span's attributes, or an event's when reading one.
    pub span_attrs: &'a HashMap<String, String>,
    /// The map a rule's `when`/`unless` asks about, which is always the **span's**.
    ///
    /// Separate from the read source because an event's attributes are not a span's: with one field, an
    /// event rule's `attr_exists` silently asked about the event's own map, and a `span_name` gate compiled
    /// against an empty name and could only ever fail.
    pub gate_attrs: &'a HashMap<String, String>,
    /// Whether this is a tool execution span, which only some rules may read.
    pub is_tool_span: bool,
}

impl<'a> MessageContext<'a> {
    /// A span read as itself: the gates and the reads see the same map.
    pub fn for_span(
        span_name: &'a str,
        span_attrs: &'a HashMap<String, String>,
        is_tool_span: bool,
    ) -> Self {
        Self {
            span_name,
            scope_name: None,
            span_attrs,
            gate_attrs: span_attrs,
            is_tool_span,
        }
    }

    /// A span read with the instrumentation scope carried by its `ScopeSpans` envelope.
    pub fn for_scoped_span(
        span_name: &'a str,
        scope_name: Option<&'a str>,
        span_attrs: &'a HashMap<String, String>,
        is_tool_span: bool,
    ) -> Self {
        Self {
            span_name,
            scope_name,
            span_attrs,
            gate_attrs: span_attrs,
            is_tool_span,
        }
    }

    /// One of a span's events: read from the event, gated on the span that carries it.
    pub fn for_event(
        span_name: &'a str,
        scope_name: Option<&'a str>,
        span_attrs: &'a HashMap<String, String>,
        event_attrs: &'a HashMap<String, String>,
        is_tool_span: bool,
    ) -> Self {
        Self {
            span_name,
            scope_name,
            span_attrs: event_attrs,
            gate_attrs: span_attrs,
            is_tool_span,
        }
    }
}

/// Where an emitted observation came from, in the vocabulary the ingestion types use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmittedCarrier<'a> {
    Attribute(&'a str),
    Event(&'a str),
    /// A carrier name assembled at read time - one entry of an indexed family, `<prefix>.<index>`.
    Owned(String),
    /// An assembled name that is an *event* rather than an attribute. Kept apart because carrier semantics
    /// are looked up by kind, so reporting an event as an attribute changes what it is read as evidence of.
    OwnedEvent(String),
}

impl EmittedCarrier<'_> {
    /// Whether this carrier is an event rather than an attribute.
    pub fn is_event(&self) -> bool {
        matches!(self, Self::Event(_) | Self::OwnedEvent(_))
    }

    /// The carrier's name, whichever form it took.
    pub fn name(&self) -> &str {
        match self {
            Self::Attribute(name) | Self::Event(name) => name,
            Self::Owned(name) | Self::OwnedEvent(name) => name.as_str(),
        }
    }
}

/// What the declared rules made of one event.
///
/// A struct rather than `(Vec<Emission>, bool)`, because the boolean answered two different questions with one
/// value: "is this event a container" and "was it read". Conflated, a container whose reads all failed
/// suppressed its own raw form and produced nothing, so the event disappeared with no record anywhere.
#[derive(Debug)]
pub struct EventReading<'a> {
    pub emissions: Vec<Emission<'a>>,
    /// Suppress the event's raw form: it is a declared container **and** something read it.
    pub replaces_raw: bool,
    /// It is a declared container and **nothing** read it. The raw form is kept - the alternative is silent
    /// loss - and the caller reports it, since an unreadable container and an ordinary one are different
    /// diagnoses.
    pub unhandled_container: bool,
    /// The event attributes a reading owns. The raw form is the fallback for what nothing read, so it never
    /// repeats them: one owner per carrier holds for an event's attributes as it does for a span's.
    pub owned_attributes: Vec<String>,
}

/// One observation a rule produced.
#[derive(Debug, Clone)]
pub struct Emission<'a> {
    /// The rule that produced it, for the explain trace.
    pub rule_id: &'a str,
    /// Every declaration that produced it: the rule, and the clauses inside it, outermost first.
    ///
    /// An **`EvidenceSet`**, not one path, because an emission can have several contributing clauses and a
    /// single path cannot say so honestly: an aggregate is built from many readings, and a grouped element run
    /// is built from every element whose case matched - two cases deriving one key are legitimate aliases, so
    /// the run has two witnesses rather than a choice between them.
    ///
    /// Owned rather than borrowed, because a fragment's cases are compiled per call: `readings()` clones each
    /// case spec, so a `&'a str` into one would not outlive the recursion that produced it.
    pub evidence: super::expr::EvidenceSet,
    pub carrier: EmittedCarrier<'a>,
    /// The carriers this emission *read*, which is what it owns - separate from the tag above.
    ///
    /// A **set**, because a `compose` reads several: owning only its synthetic tag left every attribute it
    /// consumed free for another rule to read as well, which is reachable today - one dialect composes from
    /// `output.value` and another reads that carrier directly.
    ///
    /// A rule with `tag_as` reads one key and reports another, and claiming the report would leave the key
    /// it actually read free for a second rule to read as well. The kind travels with the name because an
    /// attribute and an event of the same name are different carriers, which the retired implementation
    /// said with `attr:` and `event:` prefixes.
    pub owns: Vec<OwnedCarrier>,
    pub target: EmitTarget,
    pub value: JsonValue,
}

/// The carrier an emission read: its kind and its key.
///
/// Ordered so an owned *set* can be normalised - a family's members are collected in attribute-map order,
/// which is randomised per process, and two runs must agree about what an emission owns.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnedCarrier {
    pub is_event: bool,
    pub name: String,
}

impl OwnedCarrier {
    fn attribute(name: &str) -> Self {
        Self {
            is_event: false,
            name: name.to_string(),
        }
    }

    /// One attribute, as the single-carrier set an ordinary reading owns.
    fn just(name: &str) -> Vec<Self> {
        vec![Self::attribute(name)]
    }
}

/// Which entry point runs a rule, resolved at compile time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledSource {
    /// The span path, at this stage.
    Span(super::schema::MessageStage),
    /// The event path, for these event names. Non-empty by construction.
    Event(Vec<String>),
}

impl CompiledSource {
    /// The events this rule reads, empty for a span rule.
    pub fn event_names(&self) -> &[String] {
        match self {
            Self::Span(_) => &[],
            Self::Event(names) => names,
        }
    }

    /// The stage a span rule runs at, `None` for an event rule - which the event path runs whenever the
    /// event appears, with no stage of its own.
    pub fn stage(&self) -> Option<super::schema::MessageStage> {
        match self {
            Self::Span(stage) => Some(*stage),
            Self::Event(_) => None,
        }
    }
}

/// A compiled message rule.
#[derive(Debug, Clone)]
pub struct CompiledMessageRule {
    /// Tool definitions written as a language's `repr`; the grammar is sealed, its vocabulary declared.
    tool_repr: Option<ToolReprSpec>,
    pub rule_file: String,
    pub rule_id: String,
    pub doc: Option<String>,
    pub read: ReadSpec,
    pub compose: Option<CompiledCompose>,
    pub parse: Option<ParseMode>,
    pub require_members: Option<MemberRequirements>,
    pub wrap: Option<WrapSpec>,
    pub target: EmitTarget,
    pub aggregate_into_array: bool,
    pub when: Option<CompiledDetect>,
    pub unless: Option<CompiledDetect>,
    pub instrumentation_scope: Option<super::schema::InstrumentationScopeMatch>,
    pub require_non_empty: bool,
    pub require_non_blank: bool,
    pub branch_set: Option<CompiledBranchSet>,
    /// Where this rule reads: a span's attributes at a stage, or a named event's.
    ///
    /// One value, not a stage beside a possibly-empty event list. As two, the entry points disagreed about
    /// which they honour - the event path ignored the stage entirely, so two event rules at different stages
    /// counted as different ordering arenas (where a shared rank is legal) and were then both run with
    /// ownership decided by comparing their ids.
    pub source: CompiledSource,
    pub elements: Option<ElementsSpec>,
    pub walk: Option<super::schema::WalkSpec>,
    pub sections: Option<SectionsSpec>,
    pub reads_tool_spans: bool,
    pub tag_as: Option<String>,
    pub alternatives: Vec<CompiledReading>,
    pub also: Vec<CompiledReading>,
    pub fallback: Vec<CompiledReading>,
    pub priority: i32,
}

/// Message rules in the order they are consulted.
///
/// Order is `priority`, for the same reason detection's is: these were transcribed from a list whose
/// position decided which extractor claimed a contested carrier. Where no two rules read the same
/// carrier unconditionally - checked at compile time - the order decides only between conditional claims.
#[derive(Debug, Default)]
pub struct MessagePlan {
    /// Indices of the rules that can produce a tool definition or a name list.
    ///
    /// Precomputed because the metadata path would otherwise evaluate every rule on every span - sixty
    /// evaluations where thirteen can contribute, and a message rule's evaluation is not cheap: a bounded
    /// state walk, JSONPath over a parsed payload, a `repr` grammar. `emit_rule` is pure, so this is cost
    /// rather than correctness, and it is cost paid twice per span on the same payloads.
    metadata_candidates: Vec<usize>,
    rules: Vec<CompiledMessageRule>,
    /// Each declared message event's raw form, from the assets this plan was compiled from.
    ///
    /// Carried rather than read from `ruleset()`: `from_event` acts on this policy, and reaching for a global
    /// meant a plan compiled in a test could not state it - so the one decision that entry point makes was
    /// untestable outside the embedded corpus.
    raw_forms: std::collections::BTreeMap<String, super::schema::RawEventForm>,
}

/// Why a message ruleset would not compile.
#[derive(Debug)]
pub enum MessageCompileError {
    /// A rule naming no carrier, or naming both an attribute and an event.
    NotExactlyOneCarrier {
        rule: String,
    },
    DuplicateRuleId {
        rule: String,
    },
    EmptyCarrier {
        rule: String,
    },
    /// A reading references a fragment nobody declares.
    UnknownFragment {
        fragment: String,
    },
    /// A construct the engine accepts but cannot execute, or a field it would silently ignore.
    ///
    /// Both are the same defect from a reader's point of view: the asset says something and nothing
    /// happens. An engine that accepts a no-op is worse than one that refuses it, because the rule looks
    /// live.
    Inexpressible {
        rule: String,
        detail: &'static str,
    },
    /// A lower-ranked rule takes part of an all-or-nothing reading, so the reading is lost **whole** and the
    /// carriers the taker never wanted reach nobody. Distinct from `ContestedCarrier`, which is two rules
    /// wanting one carrier: here the loss is of carriers nothing contested.
    StarvedReading {
        starved: String,
        taker: String,
        carrier: String,
    },
    /// Two rules read the same carrier. One of them would never be reached, because the first claim of a
    /// carrier wins - so this is a rule that silently does nothing, not a precedence to resolve.
    ContestedCarrier {
        first: String,
        second: String,
        carrier: String,
    },
}

impl std::fmt::Display for MessageCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotExactlyOneCarrier { rule } => write!(
                f,
                "message rule `{rule}` must name exactly one of `attribute` or `event`"
            ),
            Self::UnknownFragment { fragment } => {
                write!(f, "no asset declares the fragment `{fragment}`")
            }
            Self::Inexpressible { rule, detail } => {
                write!(f, "message rule `{rule}`: {detail}")
            }
            Self::DuplicateRuleId { rule } => {
                write!(f, "message rule id `{rule}` is declared more than once")
            }
            Self::StarvedReading {
                starved,
                taker,
                carrier,
            } => write!(
                f,
                "message rule `{starved}` reads several carriers as one observation, and `{taker}` reads \
                 `{carrier}` at an earlier rank - so the whole reading is dropped and its other carriers \
                 reach nobody. Rank `{starved}` above `{taker}`; a second spelling does not help, because the \
                 taken one is the one selected"
            ),
            Self::EmptyCarrier { rule } => {
                write!(f, "message rule `{rule}` names an empty carrier")
            }
            Self::ContestedCarrier {
                first,
                second,
                carrier,
            } => write!(
                f,
                "message rules `{first}` and `{second}` both read `{carrier}`: the first claim of a \
                 carrier wins, so one of them can never emit anything"
            ),
        }
    }
}

/// A reading with its fragment's cases inlined.
///
/// Resolved at compile time, so there is no runtime indirection and no recursion to bound: a fragment's own
/// cases may not reference a fragment, which the compiler checks.
#[derive(Debug, Clone)]
pub struct CompiledReading {
    pub spec: Alternative,
    pub fragment_cases: Vec<Alternative>,
}

/// A branch set with every sub-reading compiled.
#[derive(Debug, Clone)]
pub struct CompiledBranchSet {
    pub primary: Vec<CompiledMessageRule>,
    pub fallback: Vec<CompiledMessageRule>,
    pub always: Vec<CompiledMessageRule>,
}

/// A composed message with its gates compiled.
///
/// Mirrors the asset form so the *gate* is built once rather than per observation - the same reason the
/// rule's own gates are compiled.
#[derive(Debug, Clone)]
pub struct CompiledCompose {
    /// A condition on the assembled object, checked before it is emitted.
    pub require: PredicateSet,
    /// The assembled members are one canonical tool definition, not a message.
    pub as_tool_definition: bool,
    pub tag: String,
    pub members: Vec<CompiledComposeMember>,
    pub trailing: std::collections::BTreeMap<String, JsonValue>,
}

/// One member of a composed message, with any conditional source's gate compiled.
#[derive(Debug, Clone)]
pub struct CompiledComposeMember {
    pub spec: ComposeMember,
    pub fallback_gate: Option<CompiledDetect>,
}

fn compile_compose(compose: &ComposeSpec) -> CompiledCompose {
    CompiledCompose {
        require: compose.require.clone(),
        as_tool_definition: compose.as_tool_definition,
        tag: compose.tag.clone(),
        members: compose
            .members
            .iter()
            .map(|member| CompiledComposeMember {
                spec: member.clone(),
                fallback_gate: member
                    .fallback
                    .as_ref()
                    .map(|f| super::detect_rules::compile_signals(&f.when)),
            })
            .collect(),
        trailing: compose.trailing.clone(),
    }
}

/// A carrier pattern: an exact name, or everything beneath a prefix.
///
/// Two sets are compiled per rule - what it *consumes* and what it *emits* - because they are different
/// questions and conflating them missed real conflicts. A rule can emit a carrier another rule consumes
/// (`compose` reads `ai.response.text` and emits `ai.response`), a rule can emit a name it never read
/// (`tag_as`), and an indexed family consumes a whole prefix while emitting one name per index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarrierPattern {
    Exact(String),
    Prefix(String),
}

impl CarrierPattern {
    /// Could these two patterns cover a common carrier?
    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(name), Self::Prefix(prefix))
            | (Self::Prefix(prefix), Self::Exact(name)) => name.starts_with(prefix.as_str()),
            (Self::Prefix(a), Self::Prefix(b)) => {
                a.starts_with(b.as_str()) || b.starts_with(a.as_str())
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Exact(name) => name.clone(),
            Self::Prefix(prefix) => format!("{prefix}*"),
        }
    }
}

mod build;
mod compile_plan;
mod compile_rule;
mod emit;
mod plan;
mod predicates;
mod reading;
mod validation;
mod walk;

use build::*;
pub use compile_plan::compile;
use compile_rule::*;
use emit::*;
use plan::*;
use predicates::element_passes;
#[cfg(test)]
pub(crate) use predicates::predicate_holds_for_test;
pub(super) use predicates::predicates_hold;
use reading::*;
pub(super) use reading::{parse_value, query};
pub(super) use validation::predicate_defect;
use validation::*;
use walk::*;
