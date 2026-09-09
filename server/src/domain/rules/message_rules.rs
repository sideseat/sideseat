//! Message extraction as data: which carriers an ingestion reads, and how each is parsed.
//!
//! This is the part of the mandate that matters most, and the part the architecture already made
//! tractable. Extraction here does **not** interpret a conversation - normalisation happens at query
//! time, on the stored raw payload - so an extractor's whole job is to *claim a carrier and keep what it
//! held*. Several of them are already nothing but that, expressed as Rust:
//!
//! ```text
//! if let Some(parsed) = extract_json(attrs, "traceloop.entity.input") {
//!     messages.push(RawMessage::from_attr("traceloop.entity.input", timestamp, parsed));
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
//! constructors. See `server/docs/framework-rules-engine.md`.
//!
//! The engine emits *values*, not `RawMessage`s: the ingestion types live in `domain::traces`, and the
//! engine having to know them would point the dependency the wrong way for no benefit.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

use super::detect_rules::CompiledDetect;
use super::schema::{
    Alternative, AttachSpec, BlockSpec, ComposeMember, ComposeSpec, DetectMatch, ElementsSpec,
    EmitTarget, KeyValue, MemberPresence, MemberRequirements, MessageRule, OverlaySpec, ParseMode,
    PredicateSet, ReadSpec, RuleFile, SectionsSpec, SingleToolCallSpec, ToolCallsSpec,
    ToolReprSpec, ValueKind, ValuePredicate, WrapSpec,
};

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
            span_attrs,
            gate_attrs: span_attrs,
            is_tool_span,
        }
    }

    /// One of a span's events: read from the event, gated on the span that carries it.
    pub fn for_event(
        span_name: &'a str,
        span_attrs: &'a HashMap<String, String>,
        event_attrs: &'a HashMap<String, String>,
        is_tool_span: bool,
    ) -> Self {
        Self {
            span_name,
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
    pub legacy_rank: i32,
}

/// Message rules in the order they are consulted.
///
/// Order is `legacy_rank`, for the same reason detection's is: these were transcribed from a list whose
/// position decided which extractor claimed a contested carrier. Where no two rules read the same
/// carrier - which is true of every rule here, checked at compile time - the order changes nothing, and
/// that is the state the rank exists to be retired from.
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
    Parse {
        path: String,
        message: String,
    },
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
            Self::Parse { path, message } => write!(f, "{path}: {message}"),
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

/// Compile one rule, with every validation the engine performs.
///
/// Extracted so a branch set's sub-readings get exactly the same checks as a top-level rule - a sub-reading
/// that skipped them would be the one place a no-op could still hide.
fn compile_rule(
    file_id: &str,
    rule: &MessageRule,
    fragments: &HashMap<String, Vec<Alternative>>,
) -> Result<CompiledMessageRule, MessageCompileError> {
    let MessageRule {
        id,
        doc,
        source,
        tool_repr,
        read,
        compose,
        parse,
        wrap,
        emit,
        aggregate_into_array,
        alternatives,
        also,
        fallback,
        require_members,
        require_non_empty,
        require_non_blank,
        branch_set,
        elements,
        walk,
        sections,
        reads_tool_spans,
        tag_as,
        unless,
        when,
        legacy_rank,
    } = rule;
    // The values every check below reads, resolved once. The *declarations* above keep their presence, which
    // is a separate question and is asked only by the branch-seam refusals: a field written out with its
    // default value is still a statement the engine does not read where it was written.
    let emit_target = emit.unwrap_or(EmitTarget::Message);
    let aggregate = aggregate_into_array.unwrap_or(false);
    let non_empty = require_non_empty.unwrap_or(false);
    let non_blank = require_non_blank.unwrap_or(false);
    let tool_spans = reads_tool_spans.unwrap_or(false);
    if compose.is_none() && branch_set.is_none() && read.named_count() != 1 {
        return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
    }
    // Every combination the runner would silently ignore is refused here instead. Each of these
    // was accepted and did nothing, which reads as a rule that works.
    let inexpressible = |detail: &'static str| MessageCompileError::Inexpressible {
        rule: id.clone(),
        detail,
    };
    // Every gate a message rule can carry, through **one** validator. Three call sites grew the checks
    // separately - the detection compiler, the field-source compiler, and this one - so the same declaration was
    // refused in two places and compiled in a third. A compose member's conditional fallback is a gate too, read
    // only where it holds, so an undeclarable one makes that member's last resort dead while reading as though
    // it has one.
    let gates = [when.as_ref(), unless.as_ref()]
        .into_iter()
        .flatten()
        .chain(
            compose
                .iter()
                .flat_map(|compose| &compose.members)
                .filter_map(|member| member.fallback.as_ref())
                .map(|fallback| &fallback.when),
        );
    for gate in gates {
        if let Some(detail) = message_gate_defect(gate) {
            return Err(MessageCompileError::Inexpressible {
                rule: id.clone(),
                detail,
            });
        }
    }
    if compose.is_some()
        && (wrap.is_some()
            || sections.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || parse.is_some()
            // `tag_as` is the dangerous one: a compose always emits `compose.tag`, while the static
            // conflict analysis models `tag_as` - so a compose declaring both emitted one carrier and was
            // checked for collisions against another. The rest are ignored the same way the list above is.
            || tag_as.is_some()
            || aggregate_into_array.is_some()
            || elements.is_some()
            || walk.is_some()
            || require_non_empty.is_some()
            || require_non_blank.is_some()
            || require_members.is_some())
    {
        return Err(inexpressible(
            "`compose` builds the whole message and tags it with its own `tag`, so `wrap`, \
                 `sections`, `alternatives`, `parse`, `tag_as` and the reading requirements would be \
                 ignored",
        ));
    }
    // `each` reads *every* listed key the span carries; only `tool_repr` iterates its carriers. Declared
    // anywhere else it would be silently read as `first_present` - which is precisely the conflation the two
    // members exist to end, so the wrong pairing is a refusal rather than a quiet reinterpretation.
    if !read.each.is_empty() && tool_repr.is_none() {
        return Err(inexpressible(
            "`each` reads every listed key as its own observation, and only `tool_repr` iterates its \
                 carriers - elsewhere it would be read as `first_present`, which is the ambiguity the two \
                 members replace. Use `first_present` for ordered alternatives",
        ));
    }
    // A repeated key can never mean what it says: under `first_present` the second occurrence is
    // unreachable, and under `each` it would read one attribute as two observations of it. An *empty* list
    // needs no rule of its own - it names no carrier, so `named_count` already refuses a rule whose only
    // source it is. A **single**-key list is deliberately allowed: `single_carrier_of` reads it as the exact
    // carrier it is, which is what lets a renamed key be declared alongside nothing else.
    for keys in [&read.first_present, &read.each] {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if keys.iter().any(|key| !seen.insert(key.as_str())) {
            return Err(inexpressible(
                "a carrier list names one key twice - under `first_present` the repeat is unreachable, and \
                 under `each` it would read one attribute as two observations",
            ));
        }
    }
    // **Two declarations must not write the same output member.** A wrap builds its object by inserting in a
    // fixed order - role, literal members, pre-content attachments, the content, post-content attachments -
    // and every insert *overwrites*. So `{"role": "user", "members": {"role": "assistant"},
    // "content_as": "role"}` compiled and produced a message whose role is its content, with the two
    // declarations before it silently discarded. Attachments could overwrite literals, the content and each
    // other, and `compose.trailing` could overwrite a named or swept member.
    //
    // Refused rather than ordered, because the order is not the point: two declarations writing one name is
    // a rule that states two things about one member, and only one of them is true. If replacement is ever
    // wanted it needs a name of its own, not an insertion order a reader has to know.
    if let Some(wrap) = wrap {
        let mut names: Vec<String> = Vec::new();
        if wrap.role.is_some() || wrap.role_from.is_some() {
            names.push("role".to_string());
        }
        names.extend(wrap.members.keys().cloned());
        // The content member is written unless a tool-call list replaces it, which is stated at that branch.
        if wrap.tool_calls_from.is_none() {
            names.push(
                wrap.content_as
                    .clone()
                    .unwrap_or_else(|| "content".to_string()),
            );
        }
        names.extend(wrap.attach.iter().map(|a| a.as_member.clone()));
        if let Some(spec) = &wrap.tool_calls_from {
            names.push(
                spec.as_member
                    .clone()
                    .unwrap_or_else(|| "tool_calls".to_string()),
            );
        }
        if let Some(spec) = &wrap.tool_call_from {
            names.push(
                spec.as_member
                    .clone()
                    .unwrap_or_else(|| "tool_call".to_string()),
            );
        }
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if names.iter().any(|name| !seen.insert(name.as_str())) {
            return Err(inexpressible(
                "two declarations of one envelope write the same output member, so one of them is silently \
                     discarded - a rule stating two things about one member states one thing that is false",
            ));
        }
    }
    // The same question for a compose: its `trailing` literals are inserted after the members, so a trailing
    // name that a member also writes overwrites it. A **sweep** member's names are only known at read time, so
    // the sweep has to exclude every fixed output name - which is a declaration it can make (`except`) and
    // which `vercel-ai.response` did not, leaving `ai.response.role` swept and then overwritten by
    // `trailing.role`.
    if let Some(compose) = compose {
        let named: Vec<&str> = compose
            .members
            .iter()
            .filter_map(|member| member.as_member.as_deref())
            .collect();
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if named.iter().any(|name| !seen.insert(*name)) {
            return Err(inexpressible(
                "two compose members write the same output member, so one is silently discarded",
            ));
        }
        for trailing in compose.trailing.keys() {
            if seen.contains(trailing.as_str()) {
                return Err(inexpressible(
                    "a `trailing` literal writes an output member a compose member also writes, and \
                         trailing is inserted last - so the member's value is silently discarded",
                ));
            }
        }
        for member in &compose.members {
            let Some(prefix) = &member.sweep_prefix else {
                continue;
            };
            // A swept name is `<prefix><rest>`, so the fixed names it could collide with are the ones whose
            // spelling a producer could write under that prefix.
            for fixed in seen
                .iter()
                .copied()
                .chain(compose.trailing.keys().map(String::as_str))
            {
                if !member.except.iter().any(|except| except == fixed) {
                    return Err(inexpressible(
                        "a sweep member does not exclude an output member the rule also writes, so whichever \
                             is inserted last silently discards the other - add it to `except`",
                    ));
                }
                let _ = prefix;
            }
        }
    }
    // **`parse` is required wherever the reading parses a raw scalar of its own.** Omitted, it meant three
    // incompatible things depending on what sat beside it: text for a compose, JSON for an ordinary read or a
    // `tool_repr`, JSON-or-string for a named family. So one absent declaration was three different decisions,
    // and which one applied was a property of a *sibling* member - the same defect `attribute_any_of` had.
    //
    // The exemptions are stated rather than implicit, and each is a reading that parses no scalar itself:
    //
    // - a **compose** reads through its members, each of which declares its own mode;
    // - a **sweep** member takes whatever a prefix holds, so sniffing is what it is for;
    // - an **indexed family** assembles entries from keys rather than parsing one string.
    //
    // Corpus-neutral: every shipped reading that parses a scalar already declares it, which is what makes this
    // a gate rather than a migration.
    let parses_a_scalar = read.attribute.is_some()
        || !read.first_present.is_empty()
        || !read.each.is_empty()
        || read.attribute_family.is_some();
    if parse.is_none() && compose.is_none() && parses_a_scalar {
        return Err(inexpressible(
            "reads a raw attribute and does not declare `parse`, which means text, JSON or JSON-or-string \
                 depending on what sits beside it - state the mode",
        ));
    }
    if let Some(compose) = compose {
        for member in &compose.members {
            // A member naming carriers reads one of their strings; a sweep member takes whatever a prefix
            // holds, which is the one place sniffing is the point.
            if member.parse.is_none() && !member.from_any_of.is_empty() {
                return Err(inexpressible(
                    "a compose member names carriers and does not declare `parse` - each member reads its \
                         own string, so the mode is the member's",
                ));
            }
        }
    }
    // `require_non_empty` / `require_non_blank` ask about a **raw carrier string**, and an indexed family has
    // none: its entries are assembled from many keys, so there is nothing for the check to be about. The branch
    // reading a family returns before these checks run, so such a declaration was read from nowhere - refused
    // rather than silently ignored, because the asset would otherwise state a filter it does not have.
    // (`require_members` is the entry-level filter that family reads *do* honour.)
    if read.indexed_family.is_some() && (non_empty || non_blank) {
        return Err(inexpressible(
            "an indexed family assembles each entry from several keys, so there is no raw string for \
                 `require_non_empty` or `require_non_blank` to ask about - use `require_members`",
        ));
    }
    // A wrap is meaningful on an *aggregated* family: the entries become one array, and one array needs an
    // envelope saying what it is - a result set is one observation, not one message per document.
    if read.indexed_family.is_some()
        && ((wrap.is_some() && !aggregate) || !alternatives.is_empty() || !also.is_empty())
    {
        return Err(inexpressible(
            "an indexed family assembles each entry itself, so `wrap` and `alternatives` would \
                 be ignored",
        ));
    }
    // `elements` reads an array element by element and derives each element's own tag, so a `tag_as` beside
    // it is a name the rule never emits - and the static conflict analysis modelled that name while the
    // runtime emitted the derived one. The rest of the list is what the elements branch returns before
    // reaching: it parses the carrier itself and emits each element, with no envelope and no readings.
    if elements.is_some()
        && (tag_as.is_some()
            || wrap.is_some()
            || sections.is_some()
            || walk.is_some()
            || aggregate_into_array.is_some()
            || require_members.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty())
    {
        return Err(inexpressible(
            "`elements` derives each element's own tag and emits it directly, so `tag_as`, an envelope, \
                 a walk, an aggregate or a reading would be ignored",
        ));
    }
    // `sections` returns before the walk, the aggregate, the readings and the fallback, so each of those was
    // accepted and dead - the list was incomplete rather than absent, which is the harder kind to notice.
    if sections.is_some()
        && (wrap.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || walk.is_some()
            || aggregate_into_array.is_some()
            || tag_as.is_some())
    {
        return Err(inexpressible(
            "`sections` builds each section's message and emits it directly, so `wrap`, `alternatives`, a \
                 `fallback`, a walk, an aggregate or a `tag_as` would be ignored",
        ));
    }
    // **An aggregate wraps the array once, so a per-reading envelope beside it is dead.** The runtime built
    // one observation from the entries and discarded every reading's envelope *and* the rule's - while the
    // indexed-family aggregate applies the rule's envelope once, so identical syntax meant different things by
    // read form. The rule's envelope now applies to the assembled array in both, and a per-reading envelope is
    // refused: "construct each, then aggregate" is a different operation and nothing declares it.
    if *aggregate_into_array == Some(true) {
        let declares_wrap = |readings: &[Alternative]| {
            readings.iter().any(|reading| {
                reading.wrap.is_some() || reading.extra_cases.iter().any(|c| c.wrap.is_some())
            })
        };
        if declares_wrap(alternatives) || declares_wrap(also) || declares_wrap(fallback) {
            return Err(inexpressible(
                "an aggregate builds one observation from every reading, so a per-reading envelope would be \
                     discarded - declare the envelope on the rule, which wraps the assembled array",
            ));
        }
    }
    // **A `tool_repr` states literals that must be able to match something.** The typed structure accepted
    // every one of these and each is a declaration that cannot mean what it says:
    //
    // | Declared | What it does |
    // | --- | --- |
    // | no candidates | the entries are found and nothing is tried as a tool |
    // | an empty field, label or marker | matches at position zero of every string, so every entry is a repr |
    // | a case-folded duplicate source type | the map is compared case-insensitively, so the second is unreachable |
    // | a target or default outside JSON Schema's primitives | a `type` member no validator acts on |
    if let Some(repr) = tool_repr {
        if repr.candidates.is_empty() {
            return Err(inexpressible(
                "declares a `tool_repr` with no candidates, so the entries are found and nothing is tried as \
                     a tool",
            ));
        }
        let literals = [
            ("name_field", &repr.name_field),
            ("description_field", &repr.description_field),
            ("name_label", &repr.name_label),
            ("description_label", &repr.description_label),
            ("arguments_label", &repr.arguments_label),
        ];
        if literals.iter().any(|(_, value)| value.trim().is_empty())
            || repr.repr_markers.iter().any(|m| m.trim().is_empty())
            || repr.parameter_members.iter().any(|m| m.trim().is_empty())
            || repr.field_terminators.iter().any(|t| t.trim().is_empty())
            || repr
                .type_map
                .iter()
                .any(|(source, target)| source.trim().is_empty() || target.trim().is_empty())
        {
            return Err(inexpressible(
                "declares an empty field, label, marker or type name in its `tool_repr` - an empty token \
                     matches at position zero of every string, so it matches everything",
            ));
        }
        let mut folded: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if repr
            .type_map
            .iter()
            .any(|(source, _)| !folded.insert(source.to_lowercase()))
        {
            return Err(inexpressible(
                "maps one source type twice in its `tool_repr`; the map is compared case-insensitively, so \
                     the second mapping is unreachable",
            ));
        }
        let primitive = |name: &String| {
            super::schema::UnknownType::PRIMITIVES
                .iter()
                .any(|allowed| allowed == name)
        };
        let default_target = match &repr.type_default {
            super::schema::UnknownType::Unconstrained => None,
            super::schema::UnknownType::MapTo(target) => Some(target),
        };
        if repr.type_map.iter().any(|(_, target)| !primitive(target))
            || default_target.is_some_and(|target| !primitive(target))
        {
            return Err(inexpressible(
                "maps a type to something that is not a JSON Schema primitive, which is a `type` member no \
                     reader acts on",
            ));
        }
    }
    // **A constructor and its target have to be about the same thing.** `EmitTarget` is a filing destination
    // and nothing checked that the reading filed there was the shape that destination holds, so
    // `{"wrap": {"role": "user"}, "emit": "tool_names"}` compiled and filed `{"role":"user","content":"hello"}`
    // as a tool *name*.
    //
    // Not a full typing of the constructor/target pairs - that belongs to the discriminated grammar, and
    // pretending this matrix is it would be worse than the gap. These are the pairs that are provably
    // incoherent today:
    //
    // | Constructor | Requires | Why |
    // | --- | --- | --- |
    // | `wrap`, `sections`, `elements` | a message target | each builds a message: a role, a content member, blocks |
    // | `tool_repr` | `tool_definitions` | it reads a language's `repr` of tool schemas |
    // | any constructor | not `claim` | a claim is recognition - it takes a payload off the table and emits nothing, so a constructed value is discarded |
    let targets = {
        let mut targets = vec![emit.unwrap_or(EmitTarget::Message)];
        for reading in alternatives
            .iter()
            .chain(also.iter())
            .chain(fallback.iter())
        {
            targets.extend(reading.emit);
        }
        targets
    };
    let builds_a_message = wrap.is_some() || sections.is_some() || elements.is_some();
    if builds_a_message && !targets.contains(&EmitTarget::Message) {
        return Err(inexpressible(
            "builds a message - an envelope, sections or element passes - and files it under a target that \
                 does not hold messages",
        ));
    }
    if tool_repr.is_some() && !targets.contains(&EmitTarget::ToolDefinitions) {
        return Err(inexpressible(
            "reads a language's `repr` of tool schemas and does not file them as tool definitions",
        ));
    }
    if targets == vec![EmitTarget::Claim]
        && (builds_a_message
            || compose.is_some()
            || tool_repr.is_some()
            || *aggregate_into_array == Some(true))
    {
        return Err(inexpressible(
            "constructs a value and emits a `claim`, which takes a payload off the table and emits nothing - \
                 so whatever was built is discarded",
        ));
    }
    // **A walk's `stop_on` names clauses of its own rule.** A name that matches nothing can never stop the
    // descent, so the walk silently runs to `max_depth` - and an id that is merely misspelled looks exactly like
    // a deliberate "never stop".
    if let Some(walk) = walk {
        let local: std::collections::BTreeSet<&str> = alternatives
            .iter()
            .chain(also.iter())
            .chain(fallback.iter())
            .map(|reading| reading.id.as_str())
            .collect();
        if walk.stop_on.iter().any(|id| !local.contains(id.as_str())) {
            return Err(inexpressible(
                "a walk's `stop_on` names a clause this rule does not declare, so it could never stop the \
                     descent - which is indistinguishable from meaning never to stop",
            ));
        }
    }
    // **An element pass states exactly one action, and its decision table is total.** Five shapes were
    // accepted and each did something other than what it said:
    //
    // | Shape | What happened |
    // | --- | --- |
    // | no passes at all | the rule reads its array and emits nothing |
    // | a pass with neither `tag_from` nor `group` | the pass matches elements and drops them |
    // | a pass with **both** | `group` silently wins and `tag_from` is dead |
    // | an empty `group.by` | no element derives a key, so every run is empty |
    // | a derived value missing from `tag_by_key` | the whole run is dropped, silently |
    //
    // The last is the one to notice: a decision table that answers for an element and then has no tag for the
    // answer discards content the rule matched *on purpose*. Logfire's data already satisfies all five.
    if let Some(elements) = elements {
        if elements.passes.is_empty() {
            return Err(inexpressible(
                "`elements` declares no passes, so the rule reads its array and emits nothing",
            ));
        }
        for pass in &elements.passes {
            match (&pass.tag_from, &pass.group) {
                (None, None) => {
                    return Err(inexpressible(
                        "an element pass declares neither `tag_from` nor `group`, so it matches elements and \
                             drops them",
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(inexpressible(
                        "an element pass declares both `tag_from` and `group`; `group` wins and `tag_from` \
                             would be ignored",
                    ));
                }
                _ => {}
            }
            let Some(group) = &pass.group else { continue };
            if group.by.is_empty() {
                return Err(inexpressible(
                    "a grouped element pass declares no cases, so no element derives a key and every run is \
                         empty",
                ));
            }
            for case in &group.by {
                if !group.tag_by_key.contains_key(&case.value) {
                    return Err(inexpressible(
                        "a grouped element pass derives a value its `tag_by_key` has no tag for, so a run \
                             the rule matched on purpose is silently discarded",
                    ));
                }
            }
        }
    }
    // The same for an indexed family, whose branch also returns before the walk, the readings and the
    // fallback. Its existing refusal covered `wrap`, `alternatives` and `also` only.
    if read.indexed_family.is_some()
        && (!fallback.is_empty() || walk.is_some() || sections.is_some())
    {
        return Err(inexpressible(
            "an indexed family assembles each entry itself, so a `fallback`, a walk or `sections` would be \
                 ignored",
        ));
    }
    // And a named family, which is the newest of the three and returns at the same point.
    if read.attribute_family.is_some()
        && (!alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || walk.is_some()
            || sections.is_some()
            || elements.is_some()
            || aggregate_into_array.is_some())
    {
        return Err(inexpressible(
            "a named family emits one observation per member, so `alternatives`, a `fallback`, a walk, \
                 `sections`, `elements` or an aggregate would be ignored",
        ));
    }
    if read.entry_member.is_some() && read.indexed_family.is_none() {
        return Err(inexpressible(
            "`entry_member` is a sub-level of an indexed entry and means nothing without \
                 `indexed_family`",
        ));
    }
    if require_members.is_some() && read.indexed_family.is_none() {
        return Err(inexpressible(
            "`require_members` is checked per indexed entry and means nothing without \
                 `indexed_family`",
        ));
    }
    // The requirement's own literals, which nothing checked. An empty member name makes the evaluator look
    // for `<entry>.` or `<entry>..`, so the rule is dead - and an explicitly empty group holds
    // unconditionally, which is the opposite of "at least one of these".
    if let Some(members) = require_members {
        if members
            .all_of
            .iter()
            .chain(&members.any_of)
            .any(|requirement| requirement.name.is_empty())
        {
            return Err(inexpressible(
                "a `require_members` entry names an empty member, so it asks for `<entry>.` and can \
                 never hold",
            ));
        }
        if members.all_of.is_empty() && members.any_of.is_empty() {
            return Err(inexpressible(
                "`require_members` is declared with no requirement, which holds for every entry - leave \
                 it out to require nothing",
            ));
        }
    }
    // Combinations the evaluator silently ignores. Each of these compiled and did nothing, which is worse
    // than a refusal: the rule reads as a statement the engine never makes.
    if read.entry_value.is_none() && read.entry_value_parse.is_some() {
        return Err(inexpressible(
            "`entry_value_parse` says how the *projected* value is read, so it means nothing without \
                 `entry_value`",
        ));
    }
    if read.indexed_family.is_none()
        && (read.overlay.is_some()
            || !read.numeric_members.is_empty()
            || read.entry_value.is_some()
            || read.entry_value_parse.is_some())
    {
        return Err(inexpressible(
            "`overlay`, `numeric_members` and `entry_value` describe an indexed family's entries and \
                 are read only for one",
        ));
    }
    if aggregate
        && alternatives
            .iter()
            .chain(also)
            .chain(fallback)
            .any(|a| a.emit.is_some())
    {
        return Err(inexpressible(
            "an aggregate is one observation, so its target is the rule's; a per-reading `emit` beside \
                 `aggregate_into_array` would be ignored",
        ));
    }
    // `only_plain_data` answers "is this already a message" and returns the value untouched when it is, so
    // anything that would have built a block or a call list is skipped. Silently: the declaration reads as
    // though the block is built. Refused rather than reordered, because the two say contradictory things -
    // one that the value may already be a message, the other that it is a payload to wrap.
    if let Some(wrap) = wrap
        && wrap.only_plain_data
        && (wrap.block.is_some()
            || wrap.prepend_block.is_some()
            || wrap.tool_calls_from.is_some()
            || wrap.tool_call_from.is_some())
    {
        return Err(inexpressible(
            "`only_plain_data` passes an already-message-shaped value through untouched, so a block or a \
                 tool-call constructor beside it would be skipped for exactly the values it exists to \
                 recognise",
        ));
    }
    // A canonical tool definition is a tool definition. Emitted on the message axis it would be a message
    // shaped like one, which no reader expects.
    if compose.as_ref().is_some_and(|c| c.as_tool_definition)
        && emit_target != EmitTarget::ToolDefinitions
    {
        return Err(inexpressible(
            "`as_tool_definition` builds a canonical tool definition, so the rule's target must be \
                 `tool_definitions`",
        ));
    }
    if let Some(overlay) = &read.overlay {
        if overlay.from.is_empty()
            || overlay.when_member_prefix.is_empty()
            || overlay.as_member.is_empty()
        {
            return Err(inexpressible(
                "an overlay names a carrier, the flattened members it replaces and the member they \
                     become; an empty one of those is not a name - and an empty prefix matches every \
                     member, so the overlay would delete the whole entry",
            ));
        }
        if overlay.select_any_of.is_empty() || overlay.content_any_of.is_empty() {
            return Err(inexpressible(
                "an overlay with no path to its counterpart list, or none to that counterpart's \
                     content, can never find anything",
            ));
        }
    }
    if tool_repr.is_some()
        && (emit_target != EmitTarget::ToolDefinitions
            || read.indexed_family.is_some()
            || aggregate
            || non_empty
            || non_blank
            || wrap.is_some()
            || compose.is_some()
            || sections.is_some()
            || elements.is_some()
            || walk.is_some()
            || branch_set.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty())
    {
        return Err(inexpressible(
            "a `repr` grammar assembles tool definitions itself from attribute carriers, so an indexed \
                 family, an aggregate, a content requirement, a reading, an envelope, or any target but \
                 `tool_definitions` would be ignored",
        ));
    }
    // `alternatives` and `also` may coexist: the first list is the ordered question "which shape is
    // this", the second is "and read this as well, always". One carrier really does need both - a dialect's
    // response member holds the reply *and* the inner turns that produced it - and refusing the pair forced
    // that into two rules claiming one carrier, which the ownership check rightly refuses.
    if let Some(sections) = sections {
        if sections.split_on.is_empty() {
            return Err(inexpressible("`sections.split_on` is empty"));
        }
        // A default route consumes every section, so anything after it is dead.
        if let Some(position) = sections
            .routes
            .iter()
            .position(|route| route.tag_prefix.is_none())
            && position + 1 < sections.routes.len()
        {
            return Err(inexpressible(
                "a route with no `tag_prefix` claims every section, so the routes after it can \
                     never match",
            ));
        }
    }
    if let Some(compose) = compose {
        for member in &compose.members {
            if member.sweep_prefix.is_some()
                && (member.as_member.is_some() || !member.from_any_of.is_empty())
            {
                return Err(inexpressible(
                    "a compose member is either a sweep or a named source, not both - the sweep \
                         would silently win",
                ));
            }
            if member.sweep_prefix.as_deref() == Some("") {
                return Err(inexpressible(
                    "a compose member has an empty `sweep_prefix`",
                ));
            }
            if member.sweep_prefix.is_none() && member.as_member.is_none() {
                return Err(inexpressible(
                    "a compose member names neither a member nor a sweep",
                ));
            }
        }
    }
    if let Some(compose) = compose {
        if read.named_count() != 0 {
            return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
        }
        if compose.tag.is_empty() || compose.members.is_empty() {
            return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
        }
    } else {
        // Every name the rule could read or tag with must be non-empty: an empty prefix
        // matches every attribute of every span.
        let named = [
            read.attribute.as_deref(),
            read.indexed_family.as_deref(),
            tag_as.as_deref(),
        ];
        if named.iter().flatten().any(|name| name.is_empty())
            || read.first_present.iter().any(String::is_empty)
            || read.each.iter().any(String::is_empty)
        {
            return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
        }
    }
    // The source, resolved once. An event rule naming nothing is refused: it reads no event, and under the
    // previous spelling `when_event: []` silently made the rule an ordinary span rule instead - a different
    // entry point from the one it was written for.
    let compiled_source = match source {
        None => CompiledSource::Span(super::schema::MessageStage::default()),
        Some(super::schema::MessageSource::Span(span)) => CompiledSource::Span(span.stage),
        Some(super::schema::MessageSource::Event(event)) => {
            if event.names.is_empty() {
                return Err(inexpressible(
                    "an event source naming no event reads nothing - remove the `source` to read a span's \
                     attributes, or name the events",
                ));
            }
            let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            if event.names.iter().any(|name| name.is_empty()) {
                return Err(inexpressible("an event source names an empty event"));
            }
            if event.names.iter().any(|name| !seen.insert(name.as_str())) {
                return Err(inexpressible(
                    "an event source names one event twice, which would read it twice",
                ));
            }
            CompiledSource::Event(event.names.clone())
        }
    };

    let compiled_branch_set = match branch_set {
        Some(set) => {
            let compile_group =
                    |group: &Vec<MessageRule>| -> Result<Vec<CompiledMessageRule>, MessageCompileError> {
                        group
                                .iter()
                                .map(|sub| {
                                    // The mirror of the parent's no-dead-fields rule. A leaf is reached
                                    // through its parent, so the fields the *entry points* consult are read
                                    // from the parent alone: `source` selects which entry point runs the
                                    // rule at all, and a branch's order is positional, so a leaf's rank
                                    // orders nothing. Each compiled silently and stated something the
                                    // engine never reads.
                                    if sub.source.is_some() || sub.legacy_rank.is_some() {
                                        return Err(inexpressible(
                                            "a branch leaf is reached through its parent, so `source` and \
                                             `legacy_rank` are read from the parent and would be ignored \
                                             here - declare them on the rule that owns the branch set",
                                        ));
                                    }
                                    compile_rule(file_id, sub, fragments)
                                })
                                .collect()
                    };
            if set
                .primary
                .iter()
                .chain(&set.fallback_if_primary_empty)
                .chain(&set.always)
                .any(|sub| sub.branch_set.is_some())
            {
                return Err(inexpressible(
                    "a branch set inside a branch set would be a control structure rather than a \
                         declaration",
                ));
            }
            if set.primary.is_empty() {
                return Err(inexpressible("a branch set declares no `primary` reading"));
            }
            // A branch parent is a *seam*: `emit_rule` delegates to its leaves immediately, so anything it
            // declares about reading or emitting is dead. Only its id, rank, documentation, gates and the
            // branch configuration are read - and a dead declaration is worse than a refused one, because it
            // reads as a statement the engine never makes.
            if read.named_count() != 0
                || compose.is_some()
                || wrap.is_some()
                || sections.is_some()
                || elements.is_some()
                || walk.is_some()
                || tool_repr.is_some()
                || aggregate_into_array.is_some()
                || !alternatives.is_empty()
                || !also.is_empty()
                || !fallback.is_empty()
                || emit.is_some()
                // Each of these was accepted and ignored. `reads_tool_spans` is the observable one: the
                // permission is read from the *leaves* (`reads_tool_spans_anywhere`), so a parent granting it
                // over ordinary leaves made the whole branch silently skipped on a tool span. `parse`,
                // `tag_as` and the two emptiness requirements describe a reading the parent does not perform.
                || parse.is_some()
                || tag_as.is_some()
                || require_non_empty.is_some()
                || require_non_blank.is_some()
                || reads_tool_spans.is_some()
                || require_members.is_some()
            {
                return Err(inexpressible(
                    "a branch set delegates to its leaves, so a carrier, an envelope, a reading, a \
                         requirement, a tool-span permission or a target on the parent would be ignored - \
                         declare it on the leaf that means it",
                ));
            }
            Some(CompiledBranchSet {
                primary: compile_group(&set.primary)?,
                fallback: compile_group(&set.fallback_if_primary_empty)?,
                always: compile_group(&set.always)?,
            })
        }
        None => None,
    };
    Ok(CompiledMessageRule {
        tool_repr: tool_repr.clone(),
        rule_file: file_id.to_string(),
        rule_id: id.clone(),
        doc: doc.clone(),
        read: read.clone(),
        compose: compose.as_ref().map(compile_compose),
        parse: *parse,
        require_members: require_members.clone(),
        wrap: wrap.clone(),
        target: emit_target,
        aggregate_into_array: aggregate,
        when: when.as_ref().map(super::detect_rules::compile_signals),
        unless: unless.as_ref().map(super::detect_rules::compile_signals),
        require_non_empty: non_empty,
        require_non_blank: non_blank,
        branch_set: compiled_branch_set,
        source: compiled_source,
        elements: elements.clone(),
        walk: walk.clone(),
        sections: sections.clone(),
        reads_tool_spans: tool_spans,
        tag_as: tag_as.clone(),
        alternatives: inline_fragments(alternatives, fragments)?,
        also: inline_fragments(also, fragments)?,
        fallback: inline_fragments(fallback, fragments)?,
        legacy_rank: legacy_rank.unwrap_or(0),
    })
}

/// Compile every asset's message rules into one plan.
pub fn compile(sources: &BTreeMap<String, Vec<u8>>) -> Result<MessagePlan, MessageCompileError> {
    let mut rules: Vec<CompiledMessageRule> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();

    // Fragments first, across every asset: a rule may reference one defined in another file, which is what
    // makes a shared dialect table shared. Recognised events are collected in the same pass, for the same
    // reason - a rule may name an event another file recognises.
    let mut fragments: HashMap<String, Vec<Alternative>> = HashMap::new();
    let mut recognised_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut raw_forms: std::collections::BTreeMap<String, super::schema::RawEventForm> =
        std::collections::BTreeMap::new();
    for (path, bytes) in sources {
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|e| MessageCompileError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;
        if file.message_events.iter().any(|e| e.name.is_empty()) {
            return Err(MessageCompileError::Inexpressible {
                rule: format!("{}.message_events", file.id),
                detail: "declares an event with no name, which would make every unnamed event a message \
                         event",
            });
        }
        recognised_events.extend(file.message_events.iter().map(|e| e.name.clone()));
        // The raw form travels **with the plan**, from the same declarations the recognition set comes from.
        // `from_event` used to read `ruleset().message_events`, a global, which made the one policy this entry
        // point acts on unreachable from a probe plan - so a claim-only reading of a container event could not
        // be tested at all. A disagreement between the two is refused by `compile_message_events`; here the
        // last write wins for a *repeat*, which that refusal has already excluded.
        for event in &file.message_events {
            raw_forms.insert(event.name.clone(), event.raw.unwrap_or_default());
        }
        for (name, fragment) in &file.fragments {
            if fragment.cases.is_empty() {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment declares no cases",
                });
            }
            // One level, no recursion: a fragment's cases may not reference a fragment.
            if fragment.cases.iter().any(|c| c.then_fragment.is_some()) {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment's own cases may not reference a fragment. One level, deliberately: \
                             composition would be a compile-time expansion - splice the named fragment's \
                             cases in place, refuse a cycle, bound the depth - and no asset needs it, so \
                             building it now would add a capability nothing exercises, which is the defect \
                             this engine refuses everywhere else. Lift it when a second level is what an \
                             asset actually wants to say",
                });
            }
            if fragments
                .insert(format!("{}.{name}", file.id), fragment.cases.clone())
                .is_some()
            {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment of this name is already declared",
                });
            }
        }
    }

    for (path, bytes) in sources {
        // Parsed, not skipped. Skipping is what hid a schema mistake that made *every* message rule
        // silently vanish: the assets were well-formed JSON that did not match the type, the file was
        // discarded, and the plan compiled clean with nothing in it. A ruleset that cannot be read is an
        // error, never an empty answer.
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|e| MessageCompileError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;
        for rule in &file.messages {
            // Branch leaves too, not only top-level rules. A leaf's id is what `keep_unclaimed` uses to
            // tell "this rule reading its own carrier again" from "a second rule reading it", so two leaves
            // sharing an id would make that check silently pass a double read.
            let ids = std::iter::once(&rule.id).chain(
                rule.branch_set
                    .iter()
                    .flat_map(|set| {
                        set.primary
                            .iter()
                            .chain(&set.fallback_if_primary_empty)
                            .chain(&set.always)
                    })
                    .map(|sub| &sub.id),
            );
            for id in ids {
                if seen_ids.insert(id.clone(), ()).is_some() {
                    return Err(MessageCompileError::DuplicateRuleId { rule: id.clone() });
                }
            }
            // A top-level rule's position among the others is policy somebody owns, so it is stated.
            if rule.legacy_rank.is_none() {
                return Err(MessageCompileError::Inexpressible {
                    rule: rule.id.clone(),
                    detail: "a top-level rule must declare `legacy_rank`, which is its position among \
                             the others",
                });
            }
            rules.push(compile_rule(&file.id, rule, &fragments)?);
        }
    }

    // Predicates, checked once over every place a compiled rule holds one - after fragments and extra
    // cases are inlined, which is the only point at which they are all visible.
    for rule in &rules {
        if let Some(detail) = predicate_sets(rule).into_iter().find_map(predicate_defect) {
            return Err(MessageCompileError::Inexpressible {
                rule: rule.rule_id.clone(),
                detail,
            });
        }
    }

    // An event name no asset recognises makes a rule *dead*: recognition rejects the event before the plan
    // is asked, so the rule compiles and never runs. That is the failure declaring recognition was meant to
    // remove, so it is refused rather than left to be discovered.
    for rule in &rules {
        if rule
            .source
            .event_names()
            .iter()
            .any(|name| !recognised_events.contains(name))
        {
            return Err(MessageCompileError::Inexpressible {
                rule: rule.rule_id.clone(),
                detail: "names an event no asset recognises, so it would never run - declare it in a \
                         `message_events` list",
            });
        }
    }

    rules.sort_by(|a, b| {
        a.legacy_rank
            .cmp(&b.legacy_rank)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    // A shared rank is refused **within an ordering arena**, because the tie-break above is the rule *id*:
    // renaming a rule would change which of two contenders reads a carrier, and a rule id must not be a
    // control-flow primitive. Classification and detection already refuse a shared rank outright; this path
    // did not, and five pairs in the shipped assets share one.
    //
    // Not refused globally, because those five pairs are legitimate: each puts a *message* rule beside a
    // *metadata* rule, and their orders are independent - a tool definition is not a reading of the
    // conversation and the two paths never contend. So an arena is a set of rules whose relative order is
    // observable: the same stage, an overlapping output axis, and either both reading a span's attributes or
    // both reading events whose names intersect.
    for (index, rule) in rules.iter().enumerate() {
        for other in rules.iter().skip(index + 1) {
            if rule.legacy_rank != other.legacy_rank || !share_an_arena(rule, other) {
                continue;
            }
            return Err(MessageCompileError::Inexpressible {
                rule: format!("{} and {}", rule.rule_id, other.rule_id),
                detail: "share a rank in one ordering arena, so which of them reads a contested carrier is \
                         decided by comparing their *ids* - renaming a rule would change the answer",
            });
        }
    }

    // Two rules must not claim one carrier, in either direction.
    //
    // Previously this compared "the carrier a rule reads" and missed four real conflicts: a `tag_as` that
    // emits a name the rule never read, an indexed family against an exact key it generates, a `compose`
    // that consumes a carrier another rule emits, and a sweep overlapping an exact source. Comparing the
    // *consumed* and *emitted* sets asks the question that matters - who owns this carrier - rather than
    // one convenient projection of it.
    // Claiming is a message-axis mechanism: `run` claims carriers, `tool_definitions` does not. So two
    // rules contend only when both can emit on the message axis - a message rule and a pure
    // tool-definition rule reading the same carrier is the co-located case (one conversation, one tool
    // list) that routing-by-emission handles, not a conflict.
    // `possible_targets` already looks everywhere a target can be declared - a branch set's leaves, a
    // fragment's cases, a selection point's extra cases - so this is one question with one answer, and the
    // metadata side asks it of the same function.
    // **Per output axis.** Two rules reading one carrier contend on the axis they both emit on, and only that
    // one: a dialect stating its tools on the carrier another rule reads as a conversation is two true
    // statements, which is why the check was written for the message axis. But it was written for the message
    // axis *alone*, so two rules reading one carrier and both emitting tool definitions compiled - and the
    // metadata path does no claiming, so both survived and their rank silently became precedence somewhere
    // downstream.
    //
    // `tool_repr` is excluded from the message axis because a `repr` grammar reads tool schemas, never a
    // conversation - it is a metadata reader whatever else the rule declares.
    /// Whether a rule can emit on one output axis.
    type EmitsOnAxis = fn(&CompiledMessageRule) -> bool;
    let axes: [EmitsOnAxis; 3] = [
        |rule: &CompiledMessageRule| {
            rule.tool_repr.is_none()
                && possible_targets(rule)
                    .into_iter()
                    .any(|target| matches!(target, EmitTarget::Message | EmitTarget::Claim))
        },
        |rule: &CompiledMessageRule| {
            rule.tool_repr.is_some()
                || possible_targets(rule).contains(&EmitTarget::ToolDefinitions)
        },
        |rule: &CompiledMessageRule| possible_targets(rule).contains(&EmitTarget::ToolNames),
    ];
    for (i, a) in rules.iter().enumerate() {
        for b in &rules[i + 1..] {
            if !axes.iter().any(|emits| emits(a) && emits(b)) {
                continue;
            }
            // An event rule reads an *event's* attributes; a span rule reads the span's. Two different maps,
            // so a key appearing in both is two different carriers - `gen_ai.input.messages` is a span
            // attribute for one convention and an attribute *of* the inference-details event for another.
            if a.source.event_names().is_empty() != b.source.event_names().is_empty() {
                continue;
            }
            // Two event rules contend only if they can apply to the same event.
            if !a.source.event_names().is_empty()
                && !a
                    .source
                    .event_names()
                    .iter()
                    .any(|name| b.source.event_names().contains(name))
            {
                continue;
            }
            // **Starvation**, which the conflict check above deliberately excuses and must not.
            //
            // Every excuse it makes rests on "the two take turns and the ranks decide": a conditional claim
            // yields on spans its condition excludes, so the lower rank simply goes first. That is sound when
            // the loser's emission owns the one carrier it lost. It is false for an **all-or-nothing** reading,
            // where an emission is accepted only if its whole ownership set is free - the loser is dropped
            // *whole*, and the carriers the taker never wanted end up owned by nobody.
            //
            // Directional: `a` holds the earlier rank, so it is `a` that can starve `b`. Asked of every
            // multi-owner reading, not only `compose`: an indexed family's entry owns its own members, an
            // aggregate owns every entry, and an overlay owns both sides of the join. Codex's demonstration is
            // the family case - a conditional rank-1 rule reading `family.0.role` leaves the entry unable to
            // take `family.0.content`, which then reaches nobody.
            //
            // Asked **before** the stage exemption below, deliberately. That exemption is sound for the
            // question it guards - two stages sharing a carrier are safe because the fallback *inherits* what
            // the dialect stage claimed - and inheritance is exactly what makes cross-stage starvation real: a
            // multi-owner reading at the fallback stage arrives with the dialect's claims already in its claim
            // set, so a dialect rule that took one of its keys drops the whole reading.
            for owned in owned_all_or_nothing(b) {
                if let Some(taken) = consumed_carriers(a)
                    .iter()
                    .chain(emitted_carriers(a).iter())
                    .find(|consumed| consumed.pattern.overlaps(&owned))
                {
                    return Err(MessageCompileError::StarvedReading {
                        starved: b.rule_id.clone(),
                        taker: a.rule_id.clone(),
                        carrier: taken.pattern.describe(),
                    });
                }
            }
            // Two stages may share a carrier, and the reason is *not* that they never run together - a
            // generation span whose answer is unaccounted for reads the fallback after a dialect produced
            // something, which is exactly when they do. What makes the pair safe is that the fallback
            // **inherits** what the dialect stage read, including a carrier it only *claimed*: `fallback`
            // takes those carriers and starts its claim set from them. So the guarantee is enforced at
            // evaluation, not assumed here.
            if a.source.stage() != b.source.stage() {
                continue;
            }
            // A conditional claim is not a dead rule: it yields on spans its condition excludes, and the
            // ranks decide which is tried first. Only two *unconditional* claims on one carrier are a defect.
            //
            // Asked per carrier, not per rule. A rule that reads an indexed family unconditionally and joins
            // against a side payload only where a witness holds is conditional about the payload and not
            // about the family - and as a rule-wide flag it waived every conflict the rule was in, so a
            // second rule reading one of the family's own keys was accepted while being permanently dead.
            // What runtime ownership is *over*: the carrier a rule read (`Emission::owns`). So a conditional
            // read excuses a read collision - the two take turns and the ranks decide - while for an
            // **emitted** collision the question is different. Two rules tagging one carrier are separated at
            // runtime only when they also read one, because that is what ownership resolves. A gated rule
            // reading `x` and an ungated one reading `y`, both tagging `shared`, own different carriers, so
            // both emissions survive and nothing downstream tells them apart: the tag is what carrier
            // semantics, identity and ordering all key on.
            //
            // Read-conditionality is therefore *not* an excuse for an emitted collision; shared physical
            // ownership is. That is what makes one dialect's claim on `input.value` coexist with another's
            // reading of it - the same key, resolved by whichever rank comes first.
            let a_reads = consumed_carriers(a);
            let b_reads = consumed_carriers(b);
            // The exemption for a tag collision has to be about the carriers the colliding *emissions*
            // necessarily own, not any carrier either rule might read. A rule reading
            // `first_present: ["first", "second"]` beside one reading `second` overlaps statically and
            // owns `first` at runtime when both are present - so both emissions survive under one tag, which
            // is the defect the exemption was meant to exclude. Same for an unused compose fallback or an
            // unrelated branch leaf.
            let shared_ownership = match (necessarily_owned(a), necessarily_owned(b)) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            };
            // For the **both emit** comparison only: two rules tagging one carrier are separated at runtime
            // when ownership provably resolves them, and read-conditionality is not that proof. The cross
            // comparisons below keep each emission's own condition, because "A emits what B reads" is a
            // question about whether A emits at all.
            let emitted_tags = |rule: &CompiledMessageRule| {
                emitted_patterns(rule)
                    .into_iter()
                    .map(|mut emitted| {
                        emitted.condition = Condition {
                            gate: None,
                            // Ownership resolving them is the only thing that makes a shared tag safe;
                            // nothing else separates two emissions under one name.
                            narrowed: shared_ownership,
                        };
                        emitted
                    })
                    .collect::<Vec<_>>()
            };
            let conflict = [
                (a_reads.clone(), b_reads.clone(), "both read"),
                (emitted_tags(a), emitted_tags(b), "both emit"),
                (
                    emitted_carriers(a),
                    b_reads,
                    "one emits what the other reads",
                ),
                (
                    a_reads,
                    emitted_carriers(b),
                    "one reads what the other emits",
                ),
            ]
            .into_iter()
            // Directional: `a` is the earlier rank, so the question is whether it suppresses `b`.
            .find_map(|(left, right, how)| {
                left.iter()
                    .find_map(|l| {
                        right
                            .iter()
                            .find(|r| {
                                l.condition.suppresses(&r.condition)
                                    && l.pattern.overlaps(&r.pattern)
                            })
                            .map(|_| l.pattern.describe())
                    })
                    .map(|carrier| (carrier, how))
            });
            if let Some((carrier, how)) = conflict {
                return Err(MessageCompileError::ContestedCarrier {
                    first: a.rule_id.clone(),
                    second: b.rule_id.clone(),
                    carrier: format!("{carrier} ({how})"),
                });
            }
        }
    }

    let metadata_candidates = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| can_emit_metadata(rule))
        .map(|(index, _)| index)
        .collect();
    Ok(MessagePlan {
        rules,
        metadata_candidates,
        raw_forms,
    })
}

/// A **selected** set of predicate defects, refused like any other no-op.
///
/// Not a satisfiability decision procedure, and the distinction is load-bearing: it proves chosen defects at
/// the *root*, and accepts everything else - including a genuine contradiction on a singular member path.
/// Under-refusing is the right failure, because an over-refusal deletes a working rule, and every
/// over-refusal in this area has been one of mine.
///
/// One function, applied by one recursive pass over every predicate-bearing place a *compiled* rule has -
/// after fragments and extra cases are inlined. Checking the direct alternatives only left the same
/// contradiction reachable through `require_parent`, an attachment, an overlay, a prepended block, or any
/// case a fragment contributed: the pass that ran before inlining could not see those at all.
pub(super) fn predicate_defect(set: &PredicateSet) -> Option<&'static str> {
    // Defects between *members* of a set, which no per-predicate check can see - and restricted to the
    // **root**, because only there is the reasoning sound. My first version compared any two members on the
    // same path and was wrong three ways at once:
    //
    // - On a *member* path, `not_null: true` beside `not_null: false` is not a tautology: when the member is
    //   absent both fail, so the pair means "the member exists".
    // - A *plural* path makes each predicate existential, so `kind: string` and `kind: number` can both hold
    //   against different matches of `$.*` - an ordinary statement the check refused.
    // - Two spellings of one path (`$.v`, `$['v']`) render differently, so an equivalent-path contradiction
    //   escaped while a satisfiable same-path pair was refused.
    //
    // At the root all three disappear: the value always exists, there is exactly one of it, and the path is
    // either absent or `$`. That is also where the defect that prompted this lives - a content-block rule
    // whose `require` holds for every block it is offered.
    let on_root = |p: &ValuePredicate| p.path.as_ref().is_none_or(|path| path.to_string() == "$");
    // How many conditions a predicate asserts. A complement pair is a tautology only when *neither* side
    // narrows any further: `{kind: string, not_null: true}` beside `{not_null: false}` is false for a
    // non-null number, so the extra `kind` makes the pair an ordinary statement.
    let sole = |p: &ValuePredicate| -> bool {
        usize::from(p.exists.is_some())
            + usize::from(p.kind.is_some())
            + usize::from(p.non_empty.is_some())
            + usize::from(p.not_null.is_some())
            + usize::from(p.identifier_like.is_some())
            + usize::from(p.starts_with.is_some())
            + usize::from(p.lacks_prefix.is_some())
            + usize::from(!p.one_of.is_empty())
            + usize::from(!p.none_of.is_empty())
            == 1
    };
    let complements = |a: &ValuePredicate, b: &ValuePredicate| -> bool {
        sole(a)
            && sole(b)
            && ((a.not_null == Some(true) && b.not_null == Some(false))
                || (a.not_null == Some(false) && b.not_null == Some(true))
                || (a.non_empty == Some(true) && b.non_empty == Some(false))
                || (a.non_empty == Some(false) && b.non_empty == Some(true))
                || (a.identifier_like == Some(true) && b.identifier_like == Some(false))
                || (a.identifier_like == Some(false) && b.identifier_like == Some(true))
                // A prefix and its negation: every string has it or lacks it, and a non-string lacks it.
                || (a.starts_with.is_some() && a.starts_with == b.lacks_prefix)
                || (b.starts_with.is_some() && b.starts_with == a.lacks_prefix))
    };
    // `exists` is the one complement that is a tautology on **any** path, and it is exactly why: it is the
    // predicate that decides presence, so "present" beside "absent" covers every value there is. On a
    // singular path one branch or the other always holds; on a plural one a match either exists (the first
    // branch) or there are none and the singular branch answers `exists: false`. So this pair is checked
    // wherever it appears, unlike the value complements above, which a *member* path makes satisfiable.
    //
    // Load-bearing beyond being a no-op: `rule_is_wholly_conditional` reads "every reading carries a
    // `require`" as evidence that a rule yields its carrier on some span. A tautological `require` makes that
    // a false statement, and the second rule reading the same carrier is then permanently dead.
    let same_path = |a: &ValuePredicate, b: &ValuePredicate| match (&a.path, &b.path) {
        (Some(a), Some(b)) => canonical_path(a) == canonical_path(b),
        (None, None) => true,
        _ => false,
    };
    // A tautology needs every forbidden value to be required by the other branch: with
    // `none_of: ["a", "b"]` beside `one_of: ["a"]`, the value `"b"` satisfies neither.
    let covers = |required: &ValuePredicate, forbidden: &ValuePredicate| {
        sole(required)
            && sole(forbidden)
            && !forbidden.none_of.is_empty()
            && forbidden
                .none_of
                .iter()
                .all(|value| required.one_of.contains(value))
    };
    for (i, left) in set.any.iter().enumerate() {
        for right in &set.any[i + 1..] {
            if !same_path(left, right) {
                continue;
            }
            if sole(left)
                && sole(right)
                && ((left.exists == Some(true) && right.exists == Some(false))
                    || (left.exists == Some(false) && right.exists == Some(true)))
            {
                return Some(
                    "an `any` set requires a value to exist and to be absent, which holds of every value",
                );
            }
            if covers(left, right) || covers(right, left) {
                return Some(
                    "an `any` set forbids only values another member requires, and a sole `none_of` \
                     also holds of an absent value - so it holds for every value",
                );
            }
        }
    }
    let any_root: Vec<&ValuePredicate> = set.any.iter().filter(|p| on_root(p)).collect();
    let all_root: Vec<&ValuePredicate> = set.all.iter().filter(|p| on_root(p)).collect();
    for (i, left) in any_root.iter().enumerate() {
        for right in &any_root[i + 1..] {
            if complements(left, right) {
                return Some(
                    "an `any` set holds a root condition and its negation, so it holds for every value",
                );
            }
        }
    }
    let all_root_kinds: Vec<ValueKind> = all_root.iter().filter_map(|p| p.kind).collect();
    if let Some(first) = all_root_kinds.first()
        && all_root_kinds.iter().any(|kind| kind != first)
    {
        return Some("an `all` set names two kinds for the root, so it holds for nothing");
    }
    if let Some(required) = all_root_kinds.first()
        && !any_root.is_empty()
        && any_root
            .iter()
            .all(|p| p.kind.is_some_and(|kind| kind != *required))
    {
        return Some(
            "an `all` set names one root kind and every `any` member names a different one, so it \
                 holds for nothing",
        );
    }
    // A kind that is not null cannot also be null. Reached across the branches, since the `all` side is
    // required and every `any` member must hold something compatible with it.
    if let Some(required) = all_root_kinds.first()
        && *required != ValueKind::Null
        && (all_root.iter().any(|p| p.not_null == Some(false))
            || (!any_root.is_empty() && any_root.iter().all(|p| p.not_null == Some(false))))
    {
        return Some(
            "an `all` set requires a root kind that is not null while a required branch asserts the \
                 root is null, so it holds for nothing",
        );
    }
    for (i, left) in all_root.iter().enumerate() {
        for right in &all_root[i + 1..] {
            if complements(left, right) {
                return Some(
                    "an `all` set holds a root condition and its negation, so it holds for nothing",
                );
            }
        }
    }

    for predicate in set.all.iter().chain(set.any.iter()) {
        if predicate.non_empty.is_some()
            && matches!(
                predicate.kind,
                Some(ValueKind::Number | ValueKind::Bool | ValueKind::Null)
            )
        {
            return Some(
                "`non_empty` is meaningless for a number, boolean or null - only strings, \
                 arrays and objects can be empty",
            );
        }
        if predicate.exists == Some(false)
            && (predicate.kind.is_some()
                || predicate.non_empty.is_some()
                || predicate.not_null.is_some()
                || predicate.identifier_like.is_some()
                || predicate.starts_with.is_some()
                || predicate.lacks_prefix.is_some()
                // `one_of` needs a value to be one of them, so it cannot hold on an absent member - and the
                // absent branch returns before consulting it, so it was silently ignored. `none_of` is
                // deliberately not here: its documented reading accepts absence, which is how a dialect's
                // unnamed events fall through to the reading that handles them.
                || !predicate.one_of.is_empty())
        {
            return Some(
                "`exists: false` asserts the member is absent, so no other condition on it \
                 can hold",
            );
        }
        // Only *overlapping* prefixes contradict. `starts_with: "ab"` with `lacks_prefix: "a"` cannot hold,
        // and so can `lacks_prefix: "ab"` with `starts_with: "a"` - but `starts_with: "a"` beside
        // `lacks_prefix: "b"` is an ordinary, satisfiable statement, and refusing it refused a real rule.
        // Only when the required prefix *already begins with* the forbidden one: `starts_with: "ab"` with
        // `lacks_prefix: "a"` cannot hold. The reverse is satisfiable - `starts_with: "a"` beside
        // `lacks_prefix: "ab"` is met by `"ac"` - and refusing it refused a real rule.
        if let (Some(starts), Some(lacks)) = (&predicate.starts_with, &predicate.lacks_prefix)
            && starts.starts_with(lacks.as_str())
        {
            return Some(
                "`starts_with` begins with the prefix `lacks_prefix` forbids, so no value satisfies both",
            );
        }
        // A predicate on the **root** that asserts nothing beyond presence is a tautology: the value being
        // tested always exists. `{}`, `{"path": "$"}` and `{"exists": true}` are the same statement, and each
        // makes a rule that requires it recognise everything. A member *path* with no conditions is
        // different and stays legal - it asserts the member is there.
        let on_root = predicate
            .path
            .as_ref()
            .is_none_or(|path| path.to_string() == "$");
        // The root always exists, so asserting its absence can never hold. Judged *before* the
        // presence-only rule below, which any other condition - `none_of`, say - would otherwise mask.
        if on_root && predicate.exists == Some(false) {
            return Some(
                "`exists: false` on the root can never hold - the value being tested is always there",
            );
        }
        let asserts_only_presence = predicate.kind.is_none()
            && predicate.non_empty.is_none()
            && predicate.not_null.is_none()
            && predicate.identifier_like.is_none()
            && predicate.starts_with.is_none()
            && predicate.lacks_prefix.is_none()
            && predicate.one_of.is_empty()
            && predicate.none_of.is_empty();
        if matches!(predicate.kind, Some(ValueKind::Null)) && predicate.not_null == Some(true) {
            return Some("`kind: null` and `not_null: true` on one predicate");
        }
        if predicate.kind.is_some()
            && !matches!(predicate.kind, Some(ValueKind::Null))
            && predicate.not_null == Some(false)
        {
            return Some("a kind that is not null, beside `not_null: false`");
        }
        if let Some(both) = predicate
            .one_of
            .iter()
            .find(|value| predicate.none_of.contains(value))
        {
            let _ = both;
            return Some("a value named by both `one_of` and `none_of`");
        }
        if on_root && asserts_only_presence {
            return Some(
                "a predicate on the root that asserts nothing beyond presence is a tautology - the \
                     value being tested always exists, so the condition recognises everything",
            );
        }
        // Every text condition needs a string. Declared beside a kind that is not one, it can never hold -
        // and a predicate that can never hold is the same defect as one that asserts nothing.
        if (predicate.identifier_like.is_some()
            || predicate.starts_with.is_some()
            || predicate.lacks_prefix.is_some()
            || !predicate.one_of.is_empty())
            && matches!(
                predicate.kind,
                Some(
                    ValueKind::Number
                        | ValueKind::Bool
                        | ValueKind::Null
                        | ValueKind::Array
                        | ValueKind::Object
                )
            )
        {
            return Some(
                "a text condition - `identifier_like`, `starts_with`, `lacks_prefix`, `one_of` - needs \
                     a string, so beside a kind that is not one it can never hold",
            );
        }
    }
    None
}

/// Every predicate set a compiled rule holds, wherever the declaration put it.
fn predicate_sets(rule: &CompiledMessageRule) -> Vec<&PredicateSet> {
    let mut out = Vec::new();
    if let Some(set) = &rule.branch_set {
        for sub in set.primary.iter().chain(&set.fallback).chain(&set.always) {
            out.extend(predicate_sets(sub));
        }
    }
    if let Some(compose) = &rule.compose {
        out.push(&compose.require);
    }
    if let Some(sections) = &rule.sections {
        out.extend(sections.routes.iter().map(|route| &route.skip_when));
    }
    // An array read pass by pass has predicates at two levels - the pass's own, and each derived case of its
    // grouping - and both are evaluated. This is a live path, not a latent one.
    if let Some(elements) = &rule.elements {
        for pass in &elements.passes {
            out.push(&pass.when);
            if let Some(group) = &pass.group {
                out.extend(group.by.iter().map(|case| &case.when));
            }
        }
    }
    if let Some(overlay) = &rule.read.overlay {
        out.push(&overlay.witness);
        out.push(&overlay.require);
    }
    for reading in rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
    {
        // The reading's own sets, and every case a fragment or this selection point contributed - which the
        // pre-inlining pass could not reach.
        for spec in std::iter::once(&reading.spec).chain(reading.fragment_cases.iter()) {
            out.push(&spec.require);
            out.push(&spec.require_parent);
            if let Some(wrap) = &spec.wrap {
                out.extend(wrap_predicate_sets(wrap));
            }
        }
    }
    if let Some(wrap) = &rule.wrap {
        out.extend(wrap_predicate_sets(wrap));
    }
    out
}

/// Every predicate set an envelope holds.
fn wrap_predicate_sets(wrap: &WrapSpec) -> Vec<&PredicateSet> {
    let mut out = vec![&wrap.require_after];
    out.extend(wrap.attach.iter().map(|attach| &attach.require));
    if let Some(block) = &wrap.prepend_block {
        out.push(&block.require);
    }
    for block in wrap
        .block
        .iter()
        .chain(wrap.prepend_block.as_ref().map(|p| &p.block))
    {
        out.extend(block.attach.iter().map(|attach| &attach.require));
    }
    out
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

/// Why a message-rule gate could never hold.
///
/// One definition for `when`, `unless` and a compose member's fallback: the dimensions a message gate is not
/// given, plus every way a gate is undeclarable whoever asks it.
fn message_gate_defect(gate: &DetectMatch) -> Option<&'static str> {
    if super::detect_rules::unavailable_gate_dimension(gate).is_some() {
        return Some(
            "uses a resource dimension, and a message gate is given no resource attributes - it could never \
             hold",
        );
    }
    super::detect_rules::gate_defect(gate)
}

/// Whether a rule's claim on a carrier is *conditional* - on the span, or on nothing else having
/// supplied the value.
///
/// This is what separates a genuine conflict from a shared carrier. The check exists to catch a rule that
/// can never emit, and a rule whose claim is conditional is not that: it fires on the spans its condition
/// admits and yields to the other elsewhere, with `legacy_rank` deciding which is tried first. Two
/// *unconditional* rules on one carrier really are a defect - the second could never run.
///
/// Reachable in practice: the generic `output.value` is read by one dialect as a gated last resort and by
/// another as its own output, and they are told apart by the span they are on.
fn rule_condition(rule: &CompiledMessageRule) -> Condition {
    // Both facets, not one label. A rule can be gated *and* payload-narrowed, and folding them into one
    // value made "same gate, mutually exclusive payloads" look like a dead pair while it is a working one.
    let gate = rule.when.as_ref().map(|g| g.match_spec.clone());
    // `unless` narrows in the opposite direction: this rule runs where the gate does *not* hold, and nothing
    // here can relate that to another rule's positive gate. Treated as an incomparable narrowing.
    let opaque = rule.unless.is_some();
    if opaque {
        return Condition {
            gate,
            narrowed: true,
        };
    }
    // Every reading this rule can produce is gated on the payload itself, so it claims nothing on a span
    // whose payload no reading recognises. Asked of **all three** lists, because `all_readings` emits
    // through `also` and through `fallback` as well - checking `alternatives` alone let a rule with one
    // required alternative and an unconditional fallback claim its carrier on every span while counting as
    // conditional, which is a permanently dead second rule.
    let readings = rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback);
    let mut any = false;
    let mut every_reading_narrowed = true;
    for reading in readings {
        any = true;
        // `require_parent` narrows a reading exactly as `require` does - it was absent here, so a rule
        // conditional only through it counted as unconditional and could falsely convict a valid fallback.
        if reading.spec.require.is_empty() && reading.spec.require_parent.is_empty() {
            every_reading_narrowed = false;
        }
    }
    Condition {
        gate,
        narrowed: any && every_reading_narrowed,
    }
}

/// A JSONPath rendered so two spellings of one path compare equal.
///
/// `$.v` and `$['v']` select the same member and render differently, so a same-path check on the rendered
/// string missed a contradiction written the second way. Only the identifier case is folded, because that is
/// the whole of the ambiguity: a bracket segment holding anything else has no dot spelling.
fn canonical_path(path: &super::schema::JsonPath) -> String {
    let rendered = path.to_string();
    // A filter's string literal can contain anything, including `['v']` and `.v`, and a textual fold cannot
    // see that it is inside one - it equated `$[?@.x == "['v']"]` with `$[?@.x == ".v"]`, which are different
    // conditions. So a path holding a filter, or any escape, is left exactly as rendered: two spellings then
    // compare unequal and the tautology check does not fire, which under-refuses rather than convicting a
    // real condition. Every fold given up here is on a shape no asset writes.
    if rendered.contains('?') || rendered.contains('\\') {
        return rendered;
    }
    let mut out = String::with_capacity(rendered.len());
    let mut rest = rendered.as_str();
    while let Some(open) = rest.find("['") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("']") else { break };
        let name = &after[..close];
        // `is_alphanumeric`, not the ASCII form: `$.é` and `$['é']` select the same member, and an
        // ASCII-only fold left them different.
        let foldable = !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_numeric());
        out.push_str(&rest[..open]);
        if foldable {
            out.push('.');
            out.push_str(name);
        } else {
            out.push_str(&rest[open..open + 2 + close + 2]);
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// The one carrier every emission of this rule owns, where the rule leaves no choice about it.
///
/// Deliberately narrow: a single named attribute, and nothing else that could contribute another carrier or
/// choose between several. Anything looser is not a *proof* that two emissions are resolved by ownership, and
/// an unproven exemption is how two messages end up under one carrier tag with nothing to tell them apart.
fn necessarily_owned(rule: &CompiledMessageRule) -> Option<&str> {
    if rule.branch_set.is_some() || rule.read.overlay.is_some() {
        return None;
    }
    // **A compose owns its members' carriers and not its own tag.** The exemption here said the opposite -
    // that two composes sharing a tag are resolved by ownership whatever their sources are - and the runtime
    // agreed by pushing the synthetic tag into `owns`. So two composes assembling the same canonical shape from
    // *different* physical carriers suppressed each other: the earlier one claimed a name no producer wrote,
    // and the later one's members went unread. Ownership is over what a span carried; a synthetic tag is what
    // the engine calls the result.
    if rule.compose.is_some() {
        return None;
    }
    if rule.read.indexed_family.is_some() || rule.read.attribute_family.is_some() {
        return None;
    }
    // `each` names several carriers that are all read, so there is no single one to answer with - unlike
    // `first_present`, where exactly one spelling is read per span.
    if !rule.read.each.is_empty() {
        return None;
    }
    match rule.read.first_present.as_slice() {
        // One spelling is not a choice.
        [only] if rule.read.attribute.is_none() => Some(only.as_str()),
        [] => rule.read.attribute.as_deref(),
        _ => None,
    }
}

/// The carriers a rule's emission owns **together**, where losing one loses the whole emission.
///
/// Empty for an ordinary reading, whose emission owns the one carrier it read - there, a lower-ranked rule
/// taking that carrier means the two take turns, which is what ranks are for. Non-empty for a reading that is
/// *all or nothing*, where an emission is accepted only if its whole ownership set is free:
///
/// | Reading | Owned together |
/// | --- | --- |
/// | `compose` with several members | every spelling of every member - `composed()` selects the **first present** with `find_map` and never retries a backup |
/// | `indexed_family` | the family's keys: each entry owns its own members, and an aggregate owns every entry |
/// | `overlay` | the base carrier and the overlay's, which are joined into one observation |
///
/// The distinction matters because the *conditional* excuse - "a gated rule and an ungated one take turns, and
/// the ranks decide" - is sound for a single-carrier reading and false here. A rule that takes one member of a
/// composed reading does not merely go first: the composed emission is dropped whole, so the carriers the
/// taker never wanted end up owned by **nobody** and their content disappears from the feed. Silently, and
/// neither rule looks wrong on its own.
fn owned_all_or_nothing(rule: &CompiledMessageRule) -> Vec<CarrierPattern> {
    if let Some(set) = &rule.branch_set {
        // A branch leaf is a rule of its own, and each leaf's reading is all-or-nothing on its own terms.
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .flat_map(owned_all_or_nothing)
            .collect();
    }
    let mut out: Vec<CarrierPattern> = Vec::new();
    if let Some(compose) = &rule.compose {
        // Only a *several*-member compose: one member reading several spellings takes exactly one of them, so
        // there is nothing another rule can take half of.
        if compose.members.len() > 1 {
            for member in &compose.members {
                out.extend(
                    member
                        .spec
                        .from_any_of
                        .iter()
                        .map(|key| CarrierPattern::Exact(key.clone())),
                );
            }
        }
    }
    if let Some(family) = &rule.read.indexed_family {
        out.push(CarrierPattern::Prefix(format!("{family}.")));
    }
    if let Some(overlay) = &rule.read.overlay {
        out.push(CarrierPattern::Exact(overlay.from.clone()));
        if let Some(attribute) = &rule.read.attribute {
            out.push(CarrierPattern::Exact(attribute.clone()));
        }
    }
    out
}

/// What narrows a claim on a carrier: the span it runs on, and whether the payload narrows it further.
///
/// Two independent facets, because a rule can have both and they answer different questions. The gate says
/// *which spans* the rule runs on and can be related to another rule's gate; `narrowed` says the rule may
/// read nothing even where it runs, for a reason nothing here can compare with another rule's.
#[derive(Clone, Default)]
struct Condition {
    /// The spans this runs on. `None` means every span.
    gate: Option<DetectMatch>,
    /// Narrowed by the payload, by a parent, or by which spelling of the carrier a producer used.
    narrowed: bool,
}

impl Condition {
    /// Whether a claim under `self` **suppresses** one under `other`: it runs wherever the other does, and
    /// where it runs it always claims.
    ///
    /// Directional and rank-aware - the caller passes the earlier rule as `self` - because that is the actual
    /// question: is the later rule dead? Equality was the wrong relation. Gates are *disjunctions* of
    /// signals, so `attr_exists: ["a", "b"]` holds everywhere `attr_exists: ["a"]` does and suppresses it,
    /// while their declared forms differ.
    ///
    /// `narrowed` on the earlier side is what makes the pair safe: it may read nothing on a span it runs on,
    /// leaving the carrier for the later rule. Two *identical* payload requirements are therefore accepted -
    /// comparing payload predicates would be a satisfiability decision this deliberately is not, and
    /// `no_declared_rule_is_dead_across_the_corpus` is what measures that case instead.
    fn suppresses(&self, other: &Self) -> bool {
        if self.narrowed {
            return false;
        }
        match (&self.gate, &other.gate) {
            // Ungated: runs on every span, so it runs wherever anything else does.
            (None, _) => true,
            // Gated against ungated: the other runs on spans this one does not.
            (Some(_), None) => false,
            (Some(mine), Some(theirs)) => gate_covers(mine, theirs),
        }
    }
}

/// Whether every span `narrower` admits is also admitted by `wider`.
///
/// Signal by signal, and only where one literally subsumes another - a longer span-name prefix, the same
/// attribute key, a shorter `contains` needle. Sound and deliberately incomplete: a missed subsumption
/// under-refuses, which leaves a dead rule to the corpus measurement, while a wrong one deletes a working
/// rule at startup.
fn gate_covers(wider: &DetectMatch, narrower: &DetectMatch) -> bool {
    // Dimensions this cannot relate at all. Present on either side, nothing is provable - and saying so is
    // what keeps a missed subsumption an under-refusal rather than a deleted rule.
    if narrower.text_contains.is_some()
        || wider.text_contains.is_some()
        || !narrower.attr_equals_ignore_case.is_empty()
        || !wider.attr_equals_ignore_case.is_empty()
    {
        return false;
    }
    let all_covered = |them: &[String], us: &[String], subsumes: fn(&str, &str) -> bool| {
        them.iter()
            .all(|theirs| us.iter().any(|ours| subsumes(ours, theirs)))
    };
    let pairs_covered = |them: &[KeyValue], us: &[KeyValue], subsumes: fn(&str, &str) -> bool| {
        them.iter().all(|theirs| {
            us.iter()
                .any(|ours| ours.key == theirs.key && subsumes(&ours.value, &theirs.value))
        })
    };
    // A span-name signal matches by equality *or* prefix, so a longer needle is covered by a shorter one.
    let prefix_of = |ours: &str, theirs: &str| theirs.starts_with(ours);
    // A `contains` needle covers any needle that contains it.
    let inside = |ours: &str, theirs: &str| theirs.contains(ours);

    let any_signal = !narrower.span_name.is_empty()
        || !narrower.attr_prefix.is_empty()
        || !narrower.attr_equals.is_empty()
        || !narrower.attr_exists.is_empty()
        || !narrower.service_name.is_empty()
        || !narrower.span_attr_contains.is_empty()
        || !narrower.resource_attr_contains.is_empty();

    // Across dimensions where one runtime signal *implies* another. `attr_equals(k, v)` reads the key, so it
    // cannot hold unless `attr_exists(k)` does; and `attr_prefix(p)` holds of any key starting with `p`, so an
    // exact key that starts with `p` implies it. Same-dimension comparison alone let a wide `attr_exists`
    // rule sit ahead of a narrow `attr_equals` one on the same carrier, where the second can never own it.
    let key_covered = |key: &str| {
        wider.attr_exists.iter().any(|ours| ours == key)
            || wider
                .attr_prefix
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
    };
    let equals_covered = narrower.attr_equals.iter().all(|theirs| {
        key_covered(&theirs.key)
            || wider
                .attr_equals
                .iter()
                .any(|ours| ours.key == theirs.key && ours.value == theirs.value)
    });
    let exists_covered = narrower
        .attr_exists
        .iter()
        .all(|theirs| key_covered(theirs));

    any_signal
        && all_covered(&narrower.span_name, &wider.span_name, prefix_of)
        && all_covered(&narrower.attr_prefix, &wider.attr_prefix, prefix_of)
        && exists_covered
        && all_covered(&narrower.service_name, &wider.service_name, inside)
        && equals_covered
        && pairs_covered(
            &narrower.span_attr_contains,
            &wider.span_attr_contains,
            inside,
        )
        && pairs_covered(
            &narrower.resource_attr_contains,
            &wider.resource_attr_contains,
            inside,
        )
}

/// One carrier a rule reads, and whether *that* claim is conditional.
///
/// Per pattern rather than per rule, because a rule can hold both kinds at once: an indexed family it always
/// reads, beside a side payload it reads only where a witness holds.
#[derive(Clone)]
struct Consumed {
    pattern: CarrierPattern,
    condition: Condition,
}

/// What a rule reads, each carrier paired with what narrows the claim on it.
fn consumed_carriers(rule: &CompiledMessageRule) -> Vec<Consumed> {
    narrow_with(consumed_patterns(rule), &rule_condition(rule))
}

/// Apply a rule-wide condition to patterns that do not already carry a narrower one of their own.
///
/// A **branch leaf keeps its own gate**. Flattening replaced every child condition with the parent's, so an
/// ungated parent made a `when`-gated leaf look unconditional and the leaf could then convict a rule that is
/// live on spans its gate excludes. A leaf's gate is at least as narrow as its parent's - the parent's gate is
/// checked first and then the leaf's - so combining them means keeping the leaf's where it has one.
fn narrow_with(patterns: Vec<Consumed>, wholly: &Condition) -> Vec<Consumed> {
    patterns
        .into_iter()
        .map(|mut consumed| {
            if consumed.condition.narrowed {
                // Already the narrowest answer available: this carrier is read only sometimes, for a reason
                // nothing here can relate to another rule's. A rule-wide condition cannot widen that.
                return consumed;
            }
            match (&consumed.condition.gate, &wholly.gate) {
                // Nothing of its own: the rule-wide condition is the whole answer.
                (None, _) => consumed.condition = wholly.clone(),
                // A gate of its own and none above it: keep it, and inherit any payload narrowing.
                (Some(_), None) => consumed.condition.narrowed |= wholly.narrowed,
                // **Both**, which runtime evaluates as a conjunction - the parent's gate is checked and then
                // the leaf's. Keeping the leaf's alone claimed the rule runs wherever that gate holds, which
                // is false where the parent's does not, and it convicted a rule live on exactly those spans.
                // Where one gate provably covers the other the conjunction *is* the narrower of the two;
                // otherwise nothing here can express it, so the claim is opaque.
                (Some(leaf), Some(parent)) => {
                    if gate_covers(parent, leaf) {
                        // The parent admits every span the leaf does, so the leaf's gate is the conjunction.
                        consumed.condition.narrowed |= wholly.narrowed;
                    } else if gate_covers(leaf, parent) {
                        consumed.condition = wholly.clone();
                    } else {
                        consumed.condition.narrowed = true;
                    }
                }
            }
            consumed
        })
        .collect()
}

fn consumed_patterns(rule: &CompiledMessageRule) -> Vec<Consumed> {
    let mut out: Vec<Consumed> = Vec::new();
    let always = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition::default(),
    };
    let only_sometimes = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition {
            gate: None,
            narrowed: true,
        },
    };
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // Only the *first* alternative is claimed unconditionally: the rest are read where no earlier spelling
    // was present, so a rule reading a later one yields whenever an earlier one is there.
    for (position, key) in rule.read.first_present.iter().enumerate() {
        let pattern = CarrierPattern::Exact(key.clone());
        out.push(if position == 0 {
            always(pattern)
        } else {
            only_sometimes(pattern)
        });
    }
    // `each` is the opposite: every listed key the span carries is read, so every one is claimed
    // unconditionally. Under the conflated member this analysis said `only_sometimes` for all but the first,
    // which understated what the one shipped `each` rule owns.
    for key in &rule.read.each {
        out.push(always(CarrierPattern::Exact(key.clone())));
    }
    // A named family reads every key under its root, unconditionally - a member's value *is* the payload, so
    // there is nothing per-member that could make the read conditional.
    if let Some(family) = &rule.read.attribute_family {
        out.push(always(CarrierPattern::Prefix(format!("{}.", family.root))));
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // Every key beneath the family, since each index's members are read.
        //
        // Conditional where members are required: an entry lacking them contributes nothing, so a second rule
        // reading one of the family's keys is live on a span whose entries this rule rejects.
        let pattern = CarrierPattern::Prefix(format!("{family}."));
        out.push(if rule.require_members.is_some() {
            only_sometimes(pattern)
        } else {
            always(pattern)
        });
    }
    if let Some(overlay) = &rule.read.overlay {
        // The payload a positional overlay joins against is read too, and it is not beneath the family.
        //
        // Conditional where the join is witnessed: the rule consumes this payload only on spans whose
        // counterpart list is this dialect's own serialisation, and yields it elsewhere. The *family* above
        // stays unconditional, which is the distinction a rule-wide flag could not express.
        // `require` narrows the same way a witness does: the joined content has to satisfy it, and where it
        // does not the payload is not consumed.
        let pattern = CarrierPattern::Exact(overlay.from.clone());
        out.push(
            if overlay.witness.is_empty() && overlay.require.is_empty() {
                always(pattern)
            } else {
                only_sometimes(pattern)
            },
        );
    }
    // A branch set's subrules read carriers of their own, and they were invisible here - so two dialects
    // could contend for one carrier as long as the collision was inside a branch set.
    if let Some(set) = &rule.branch_set {
        for sub in set.primary.iter().chain(&set.always) {
            out.extend(consumed_carriers(sub));
        }
        // A `fallback_if_primary_empty` leaf reads only where every primary reading came up empty - a
        // condition on the payload that nothing here can relate to another rule's, so it is narrowed.
        for sub in &set.fallback {
            out.extend(consumed_carriers(sub).into_iter().map(|mut c| {
                c.condition.narrowed = true;
                c
            }));
        }
    }
    if let Some(compose) = &rule.compose {
        for member in compose.members.iter().map(|m| &m.spec) {
            // The first spelling is the one this member always takes; a later one is read only where the
            // earlier is absent.
            for (position, key) in member.from_any_of.iter().enumerate() {
                let pattern = CarrierPattern::Exact(key.clone());
                out.push(if position == 0 {
                    always(pattern)
                } else {
                    only_sometimes(pattern)
                });
            }
            if let Some(fallback) = &member.fallback {
                // Read only where the member's own gate holds, which is what makes a dialect's stand-in for
                // the generic pair coexist with the dialect that owns it.
                out.push(only_sometimes(CarrierPattern::Exact(fallback.from.clone())));
            }
            if let Some(prefix) = &member.sweep_prefix {
                out.push(always(CarrierPattern::Prefix(prefix.clone())));
            }
        }
    }
    out
}

/// What a rule tags its observations with.
fn emitted_carriers(rule: &CompiledMessageRule) -> Vec<Consumed> {
    narrow_with(emitted_patterns(rule), &rule_condition(rule))
}

/// The tags themselves, each with whatever narrows *that* emission.
fn emitted_patterns(rule: &CompiledMessageRule) -> Vec<Consumed> {
    let always = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition::default(),
    };
    // A branch set emits what its sub-rules emit - each is a rule in its own right, and one with `tag_as`
    // emits a carrier this rule never names. Invisible here, two dialects could both emit one carrier from
    // inside their branch sets.
    if let Some(set) = &rule.branch_set {
        let mut out: Vec<Consumed> = set
            .primary
            .iter()
            .chain(&set.always)
            .flat_map(emitted_carriers)
            .collect();
        out.extend(set.fallback.iter().flat_map(emitted_carriers).map(|mut c| {
            c.condition.narrowed = true;
            c
        }));
        return out;
    }
    if let Some(tag) = &rule.tag_as {
        // Overrides every read form: whatever was read, this is the tag.
        return vec![always(CarrierPattern::Exact(tag.clone()))];
    }
    if let Some(compose) = &rule.compose {
        return vec![always(CarrierPattern::Exact(compose.tag.clone()))];
    }
    let mut out = Vec::new();
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // A tag per spelling, and only the first is emitted whatever the span carries.
    for key in &rule.read.each {
        out.push(always(CarrierPattern::Exact(key.clone())));
    }
    for (position, key) in rule.read.first_present.iter().enumerate() {
        let pattern = CarrierPattern::Exact(key.clone());
        out.push(if position == 0 {
            always(pattern)
        } else {
            Consumed {
                pattern,
                condition: Condition {
                    gate: None,
                    narrowed: true,
                },
            }
        });
    }
    // A named family tags each member with the member's own key, so the emitted set is the same prefix.
    if let Some(family) = &rule.read.attribute_family {
        out.push(always(CarrierPattern::Prefix(format!("{}.", family.root))));
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // One tag per index, and per sub-level where there is one - a prefix covers them all. Emitted only
        // for entries that satisfy the required members, which is why the condition mirrors the read side.
        let pattern = CarrierPattern::Prefix(format!("{family}."));
        out.push(if rule.require_members.is_some() {
            Consumed {
                pattern,
                condition: Condition {
                    gate: None,
                    narrowed: true,
                },
            }
        } else {
            always(pattern)
        });
    }
    out
}

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    /// Tool definitions this span declares, from every rule that reads a `repr` grammar.
    ///
    /// Separate from `run`, and claimed **per axis**. A tool *definition* is not a message, so a framework
    /// stating its tools on the carrier another rule reads as a conversation is two true statements - which is
    /// why this path is not subject to the message axis's claims. But two rules reading one carrier and both
    /// emitting *definitions* do contend, and nothing resolved that: both survived and their rank became
    /// precedence somewhere downstream, which is a rule id deciding an answer.
    ///
    /// Three arenas, so a definition and a name list read from one carrier both stand while two definition
    /// readings of it do not.
    pub fn tool_definitions<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        // Every rule, filtered to the *metadata* emissions - a definition or a name list. Not gated on the
        // tool-span check: a tool definition is metadata about a span, and the path reading it has always
        // run on every span. Routing by the emission's own target rather than the rule's is what lets one
        // carrier hold both a conversation and the tools it was offered - the conversation goes to `run`,
        // the tools come here, from the same rule.
        let produced = self
            .metadata_candidates
            .iter()
            .flat_map(|&index| emit_rule(&self.rules[index], ctx))
            .filter(|emission| {
                matches!(
                    emission.target,
                    EmitTarget::ToolDefinitions | EmitTarget::ToolNames
                )
            })
            .filter_map(Self::validated_metadata);

        // Claimed by `(carrier, axis)`. The same carrier may yield one definition list and one name list, and
        // one rule legitimately emits several observations from one carrier - so a claim refuses a *different
        // rule* on the *same axis*, exactly as the message path's does.
        let mut claimed: std::collections::HashSet<(OwnedCarrier, EmitTarget)> =
            std::collections::HashSet::new();
        let mut owner: std::collections::HashMap<(OwnedCarrier, EmitTarget), &str> =
            std::collections::HashMap::new();
        let mut kept: Vec<Emission<'p>> = Vec::new();
        for emission in produced {
            let keys: Vec<(OwnedCarrier, EmitTarget)> = emission
                .owns
                .iter()
                .map(|owned| (owned.clone(), emission.target))
                .collect();
            if keys.iter().any(|key| {
                claimed.contains(key)
                    && owner
                        .get(key)
                        .is_some_and(|first| *first != emission.rule_id)
            }) {
                continue;
            }
            for key in keys {
                owner.entry(key.clone()).or_insert(emission.rule_id);
                claimed.insert(key);
            }
            kept.push(emission);
        }
        kept
    }

    /// A metadata emission with its unusable items removed, or `None` where nothing usable is left.
    ///
    /// **Per item**, which is the point. A tool-name list holding `["search", 7]` was persisted as written, and
    /// the read side deserialises the whole column as `Vec<String>` - so the number failed that and took the
    /// valid `"search"` with it. A malformed item must not poison its siblings, and the place to stop it is
    /// where the item is produced.
    ///
    /// **Tool names only**, and that limit is the point rather than an omission. A tool name is a non-blank
    /// string by contract, whatever produced it, so the check is shape-independent and belongs here.
    ///
    /// A tool **definition** cannot be checked here yet: at emission it is still the *producer's* shape -
    /// Bedrock writes `{"toolSpec": {"name": …}}` - and the canonical `{"function": {"name": …}}` appears only
    /// at query-time normalisation, which recognises a closed list of provider shapes in Rust. Asking "does
    /// this have a usable name" here would mean knowing every one of those shapes, which is exactly what cycle
    /// 13's finding 3 says must move into the assets. Written as a check over `function.name`, it dropped
    /// `bedrock/converse`'s perfectly good `get_weather`.
    fn validated_metadata(emission: Emission<'_>) -> Option<Emission<'_>> {
        if emission.target != EmitTarget::ToolNames {
            return Some(emission);
        }
        let usable = |item: &JsonValue| item.as_str().is_some_and(|name| !name.trim().is_empty());
        let Some(items) = emission.value.as_array() else {
            // A single value rather than a list: the same question, one item.
            return usable(&emission.value).then_some(emission);
        };
        let kept: Vec<JsonValue> = items.iter().filter(|item| usable(item)).cloned().collect();
        let dropped = items.len() - kept.len();
        if dropped > 0 {
            tracing::debug!(
                target: "sideseat::rules",
                rule = %emission.rule_id,
                dropped,
                "tool names were not non-blank strings, so they name nothing"
            );
        }
        if kept.is_empty() {
            return None;
        }
        Some(Emission {
            value: JsonValue::Array(kept),
            ..emission
        })
    }

    /// Every carrier this span carries that a rule reads, read by **one** rule each.
    ///
    /// The first rule to produce from a carrier owns it, and the ranks decide who is first. That is not a
    /// tie-break bolted on: it is what one extractor claiming a carrier meant, and consolidating sixteen
    /// extractors into one entry would otherwise have turned "the earlier one won" into "both emit". Two
    /// dialects really do read the same key - `message` is read by two - and the ownership check permits
    /// the collision precisely because a condition and a rank separate them.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        self.stage(ctx, super::schema::MessageStage::Dialect)
    }

    /// What this *event* declares, and what to do with the event's own raw form.
    ///
    /// An event's attributes are read exactly as a span's are - the same envelopes, the same predicates -
    /// because they are the same kind of thing: a flat map a producer wrote. Only where they are found
    /// differs, which is why this is an entry point rather than a new vocabulary.
    pub fn from_event<'p>(
        &'p self,
        event_name: &str,
        event_attrs: &HashMap<String, String>,
        span_name: &str,
        span_attrs: &HashMap<String, String>,
        is_tool_span: bool,
    ) -> EventReading<'p> {
        let ctx = MessageContext::for_event(span_name, span_attrs, event_attrs, is_tool_span);
        let mut out = Vec::new();
        let mut handled = false;
        let mut carrier_present = false;
        let mut claimed: std::collections::HashSet<OwnedCarrier> = std::collections::HashSet::new();
        // The event's own declaration, asked once. It used to be ORed together from every reading that
        // matched and whose gates held, which meant the policy was stated twice with nothing keeping the
        // two statements consistent - a `true` beside a `false` compiled, and `true` silently won.
        let replaces =
            self.raw_forms.get(event_name) == Some(&super::schema::RawEventForm::Replace);
        for rule in self.rules.iter().filter(|rule| {
            rule.source
                .event_names()
                .iter()
                .any(|name| name == event_name)
        }) {
            if is_tool_span && !rule.reads_tool_spans {
                continue;
            }
            // A rule whose condition fails says nothing about the event, so it must not suppress the raw
            // form either. Asked before `replaces` is set, where it used to be set first.
            if !gates_allow(rule, &ctx) {
                continue;
            }
            // The same routing and ownership as a span: only message emissions, one rule per carrier. An
            // event's attributes are a flat map a producer wrote, so nothing about them earns an exemption
            // from either - and without this a `Claim` became a message and two rules could double-read one
            // of the event's attributes.
            // `Message | Claim` through ownership, as a span does - a claim on an event's attribute means
            // the same thing it means on a span's, and filtering it out beforehand left it with no effect at
            // all. The claims are dropped from the *observations* afterwards, since a claim is not a message.
            let readings: Vec<Emission<'p>> = emit_rule(rule, &ctx)
                .into_iter()
                .filter(|e| matches!(e.target, EmitTarget::Message | EmitTarget::Claim))
                .collect();
            let mut kept = Vec::new();
            keep_unclaimed(readings, &mut claimed, &mut kept);
            // A **claim** counts as handling the event even though it is not a message: that is what a claim
            // means - this payload is framework internals, taken off the table deliberately. Recorded before
            // the message filter below, which drops claims from the observations.
            handled |= !kept.is_empty();
            // Whether the rule's carrier was **there**, asked whatever the reading produced. This is what
            // separates "the container was unreadable" from "the container held nothing this rule wanted",
            // which the two cases below need to answer differently.
            carrier_present |= resolve_attribute(&rule.read, ctx.span_attrs).is_some();
            out.extend(kept.into_iter().filter(|e| e.target == EmitTarget::Message));
        }
        // **Replacement depends on something having read the event**, not on the declaration alone. A
        // container whose declared reads all fail - `gen_ai.input.messages = "{"` on the inference-details
        // event - produced no messages *and* suppressed the raw form, so the event vanished: indistinguishable
        // from it never having been emitted, on the ingest path, with nothing recorded anywhere.
        //
        // Not a declarable policy. Suppressing a container whose payload was *there and unreadable* is a loss
        // with no upside, so there is no second behaviour for an asset to choose between - and a policy member
        // with one sensible value is how a format acquires a setting nobody can reason about.
        //
        // **"Present" is the question, not "read".** A container carrying nothing a rule names is an ordinary
        // empty container, and keeping its raw form would put a message in the feed whose content is whatever
        // unrelated attributes the producer attached - noise a user sees, to protect against a loss that did
        // not happen. A container whose declared carrier *is* present and produced nothing is the malformed
        // case, and there the raw form is the only remaining evidence the payload existed. Distinguishing them
        // properly needs the `Absent | Empty | Malformed | Value` algebra the message path still lacks;
        // carrier presence is the approximation available today, and it is right for both shapes the corpus
        // and the review name.
        let unreadable = replaces && !handled && carrier_present;
        EventReading {
            replaces_raw: replaces && !unreadable,
            unhandled_container: unreadable,
            emissions: out,
        }
    }

    /// The last-resort carriers: the generic input/output pair and the dialect stand-ins for it.
    ///
    /// A stage rather than a rule asking about other rules. *When* it runs is the caller's policy - nothing
    /// recognised the span, or a generation span's answer is still unaccounted for - and that policy is
    /// generic, being a function of the observation type. Which carriers it reads is this plan's business.
    pub fn fallback<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        already_read: &std::collections::HashSet<OwnedCarrier>,
    ) -> Vec<Emission<'p>> {
        // The carriers a dialect already read, **typed**, because the fallback is not only reached when the
        // dialect stage produced nothing: a generation span whose answer is unaccounted for reads it
        // afterwards. Without this the two stages had independent claim sets, so a dialect claim on
        // `output.value` and the fallback's reading of it both survived.
        //
        // Taken as `Emission::owns` rather than rebuilt from the emitted carrier: a rule with `tag_as` reads
        // one key and reports another, so reconstructing ownership from the report leaves the key it
        // actually read unclaimed - the same defect the `owns` field exists to remove.
        self.stage_with(
            ctx,
            super::schema::MessageStage::Fallback,
            already_read.clone(),
        )
    }

    fn stage<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        stage: super::schema::MessageStage,
    ) -> Vec<Emission<'p>> {
        self.stage_with(ctx, stage, std::collections::HashSet::new())
    }

    fn stage_with<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        stage: super::schema::MessageStage,
        mut claimed: std::collections::HashSet<OwnedCarrier>,
    ) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        for rule in self
            .rules
            .iter()
            .filter(|rule| rule.source == CompiledSource::Span(stage))
        {
            // The tool-span gate is a message-axis question - "may this rule read such a span *as a
            // conversation*" - so it lives here, not in `emit_rule`, which the metadata path also calls.
            //
            // Asked of every rule that could read, which for a branch set is its sub-rules: the parent
            // declares no carrier of its own, so consulting only the parent let a permitted sub-rule be
            // skipped and a forbidden one run.
            if ctx.is_tool_span && !reads_tool_spans_anywhere(rule) {
                continue;
            }
            // Only the message emissions belong to `run`: a definition or a name list is metadata that
            // `tool_definitions` reads, on every span. Filtered by the emission's own target, so one rule
            // reading a carrier that holds both a conversation and a tool list contributes to both paths.
            // Branch-set expansion is inside `emit_rule`, so both paths see the same readings and the
            // primary-empty decision is made on all emissions, not on one axis's slice of them.
            let messages = emit_rule(rule, ctx)
                .into_iter()
                .filter(|e| matches!(e.target, EmitTarget::Message | EmitTarget::Claim));
            keep_unclaimed(messages.collect(), &mut claimed, &mut out);
        }
        out
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> impl Iterator<Item = &CompiledMessageRule> {
        self.rules.iter()
    }
}

/// Every target this rule could name, wherever the declaration puts it.
///
/// Direct readings, a *fragment's* cases, the extra cases at one selection point, and a branch set's leaves.
/// Asked in one place because two questions depend on it - which axis a rule reads, and whether the metadata
/// path must evaluate it - and a target visible to one but not the other loses an emission on both axes: the
/// message path filters it out and the metadata path never ran the rule.
fn possible_targets(rule: &CompiledMessageRule) -> Vec<EmitTarget> {
    if let Some(set) = &rule.branch_set {
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .flat_map(possible_targets)
            .collect();
    }
    let mut out = vec![rule.target];
    for reading in rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
    {
        out.extend(reading.spec.emit);
        // A fragment's cases and this point's extra cases can each override the target.
        out.extend(reading.fragment_cases.iter().filter_map(|case| case.emit));
        out.extend(reading.spec.extra_cases.iter().filter_map(|case| case.emit));
    }
    out
}

/// Whether two rules' relative order is observable.
///
/// Three conditions, and each excludes a pair that legitimately shares a rank:
///
/// - **The same stage.** A fallback-stage rule runs only where the dialect stage produced nothing, so its rank
///   relative to a dialect rule orders nothing.
/// - **An overlapping output axis.** A message and a tool definition are not read by the same path: the
///   metadata path filters messages out and the message path filters metadata out. All five shared ranks in the
///   shipped assets are of this kind.
/// - **The same input domain.** An event rule is selected by event name, so two such rules contend only
///   where their name sets intersect; a span rule reads a span's attributes at one stage.
///
/// The stage comparison is now *inside* the domain question rather than beside it, and that is the defect
/// this function had: two **event** rules declaring different stages compared unequal here - so they were
/// held to be different arenas, where a shared rank is legal - while the event path ignores the stage
/// entirely and ran both, leaving ownership to be decided by comparing their ids.
fn share_an_arena(a: &CompiledMessageRule, b: &CompiledMessageRule) -> bool {
    let axis = |rule: &CompiledMessageRule| {
        let targets = possible_targets(rule);
        let message = targets
            .iter()
            .any(|target| matches!(target, EmitTarget::Message | EmitTarget::Claim));
        let metadata = targets
            .iter()
            .any(|target| matches!(target, EmitTarget::ToolDefinitions | EmitTarget::ToolNames));
        (message, metadata)
    };
    let (a_message, a_metadata) = axis(a);
    let (b_message, b_metadata) = axis(b);
    if !((a_message && b_message) || (a_metadata && b_metadata)) {
        return false;
    }
    match (&a.source, &b.source) {
        // Both read a span's attributes - at the same stage, or they never run together.
        (CompiledSource::Span(one), CompiledSource::Span(other)) => one == other,
        // One is selected by event name and the other is not, so they are never candidates together.
        (CompiledSource::Span(_), CompiledSource::Event(_))
        | (CompiledSource::Event(_), CompiledSource::Span(_)) => false,
        // Both are, and they contend wherever an event name is in both sets - **whatever stage they
        // declare**, because an event rule has no stage to declare.
        (CompiledSource::Event(one), CompiledSource::Event(other)) => {
            one.iter().any(|name| other.contains(name))
        }
    }
}

/// Which rules the metadata path needs to evaluate at all.
///
/// A rule qualifies if it, or any of its readings, or any sub-rule of its branch set, can name a metadata
/// target. Also read by the tool-span gate, which must exempt metadata: a tool definition is not a reading
/// of the span's conversation, so it is not what that gate is about.
fn can_emit_metadata(rule: &CompiledMessageRule) -> bool {
    if let Some(set) = &rule.branch_set {
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .any(can_emit_metadata);
    }
    rule.tool_repr.is_some()
        || possible_targets(rule)
            .into_iter()
            .any(|target| matches!(target, EmitTarget::ToolDefinitions | EmitTarget::ToolNames))
}

/// Whether this rule, or any sub-rule of its branch set, may read a tool span as a conversation.
///
/// A branch-set parent declares no carrier, so its own flag says nothing about what its sub-rules read.
fn reads_tool_spans_anywhere(rule: &CompiledMessageRule) -> bool {
    match &rule.branch_set {
        Some(set) => set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .any(reads_tool_spans_anywhere),
        None => rule.reads_tool_spans,
    }
}

/// Keep one rule's emissions, unless a rule before it already owns their carrier.
///
/// Per *rule*, not per emission: one rule legitimately emits many observations from one carrier - a list of
/// turns is one carrier and many messages - so a rule that owns a carrier keeps everything it read from it,
/// and the next rule reading that carrier keeps nothing.
fn keep_unclaimed<'p>(
    produced: Vec<Emission<'p>>,
    claimed: &mut std::collections::HashSet<OwnedCarrier>,
    out: &mut Vec<Emission<'p>>,
) {
    // Claimed within this batch as well as before it. A branch set's sub-rules are separate rules whose
    // emissions arrive flattened into one vector, so two of them reading one carrier would both survive a
    // check that only consulted earlier *top-level* claims.
    //
    // Per rule still, not per emission: one rule legitimately emits many observations from one carrier, so a
    // carrier this batch has already claimed is only refused to a *different* rule within it.
    let mut mine: std::collections::HashSet<OwnedCarrier> = std::collections::HashSet::new();
    let mut owner: std::collections::HashMap<OwnedCarrier, &str> = std::collections::HashMap::new();
    for emission in produced {
        // A tool definition is metadata about the span, not a reading of its conversation, so it neither
        // claims a carrier nor is blocked by one: a dialect legitimately states its tools on a carrier
        // another rule reads as a conversation, and both statements are true.
        if emission.target == EmitTarget::ToolDefinitions {
            out.push(emission);
            continue;
        }
        if emission.owns.iter().any(|owned| claimed.contains(owned)) {
            continue;
        }
        if emission.owns.iter().any(|owned| {
            owner
                .get(owned)
                .is_some_and(|first| *first != emission.rule_id)
        }) {
            // A different rule in this batch already owns one of them.
            continue;
        }
        for owned in &emission.owns {
            owner.insert(owned.clone(), emission.rule_id);
            mine.insert(owned.clone());
        }
        out.push(emission);
    }
    claimed.extend(mine);
}

/// Parse a raw attribute value in the declared mode.
///
/// Each mode reproduces one thing the extractors do, and the difference between the first two is
/// load-bearing: `Json` *skips* a value it cannot parse, while `JsonOrString` keeps it as a string. An
/// extractor that used one where the other was meant would either drop a plain-text payload or store a
/// quoted fragment of JSON as prose.
fn parse_value(raw: &str, mode: ParseMode) -> Option<JsonValue> {
    match mode {
        ParseMode::Json => serde_json::from_str(raw).ok(),
        // The elements are each serialised, because an OTLP array attribute cannot nest. **An array**, which
        // the mode names: a top-level object used to be returned unchanged, so a rule declaring this mode
        // silently accepted a shape its own declaration rules out.
        ParseMode::StringifiedArray => {
            let (value, unparsed) = crate::utils::json::parse_stringified_array_elements(
                serde_json::from_str(raw).ok()?,
            )?;
            if unparsed > 0 {
                // Kept, not dropped - dropping shortens the list silently and refusing the carrier loses the
                // elements that did parse - and reported, because a retained element is a producer defect that
                // used to be recorded nowhere at all.
                tracing::debug!(
                    target: "sideseat::rules",
                    unparsed,
                    "elements of a stringified array did not parse and are kept as strings"
                );
            }
            Some(value)
        }
        ParseMode::JsonOrString => Some(serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))),
        // Prose. Parsing it would turn a bare word into a non-string and an accidental digit string
        // into a number.
        ParseMode::Text => Some(json!(raw)),
    }
}

/// Everything one payload yields: the first-winning alternatives, every cumulative reading, and a
/// fallback used only when neither produced anything.
///
/// The distinction between "first wins" and "all contribute" is not stylistic. One dialect writes a turn's
/// history under one member and the answer itself under another, so reading them as alternatives dropped
/// the assistant output of every run that carried history.
fn all_readings(
    parsed: &JsonValue,
    rule: &CompiledMessageRule,
    build: Option<Construction<'_>>,
) -> Selection {
    let mut out = Selection {
        built: Vec::new(),
        recognised: Vec::new(),
    };
    if !rule.alternatives.is_empty() {
        out.absorb(readings(parsed, &rule.alternatives, build));
    }
    for alternative in &rule.also {
        out.absorb(readings(parsed, std::slice::from_ref(alternative), build));
    }
    if out.built.is_empty() {
        if rule.alternatives.is_empty() && rule.also.is_empty() && rule.fallback.is_empty() {
            // No readings declared at all: the payload is the observation - still built, since the rule's own
            // envelope applies to it.
            return Selection {
                built: match built(parsed.clone(), None, build) {
                    Some(value) => vec![(value, None, Vec::new())],
                    None => Vec::new(),
                },
                recognised: Vec::new(),
            };
        }
        for alternative in &rule.fallback {
            out.absorb(readings(parsed, std::slice::from_ref(alternative), build));
        }
    }
    out
}

/// Query a value with a compiled JSONPath.
///
/// One definition, so every path in the vocabulary is the same language. It replaced a hand-rolled resolver
/// whose dotted-step syntax could not distinguish a member *named* `event.name` from a nested `event` ->
/// `name` - a real defect, and the sort of thing a standard defines away (`$['event.name']`).
///
/// The nodes are **borrowed from the value queried**, which is the property the whole choice rests on:
/// cloning one out keeps the provider's member order. See
/// `the_selection_language_behaves_as_the_engine_assumes`.
pub(super) fn query<'v>(
    value: &'v JsonValue,
    path: &serde_json_path::JsonPath,
) -> Vec<&'v JsonValue> {
    path.query(value).into_iter().collect()
}

/// The observations one payload yields, under the first alternative that produces any.
///
/// An ordered coalesce over documented shapes. With no alternatives the payload is emitted as it stands,
/// which is what a carrier holding exactly one message needs. "The first that produces any" is the whole
/// control flow, and it is deliberately all there is: a shape that yields nothing is not an error, it is
/// evidence the payload is in a different one of its documented forms.
fn readings(
    parsed: &JsonValue,
    alternatives: &[CompiledReading],
    build: Option<Construction<'_>>,
) -> Selection {
    if alternatives.is_empty() {
        return Selection {
            built: match built(parsed.clone(), None, build) {
                Some(value) => vec![(value, None, Vec::new())],
                None => Vec::new(),
            },
            recognised: Vec::new(),
        };
    }
    // Accumulated across **every** alternative, not per alternative: a clause that recognised the node and
    // could not build says so whether or not a later clause then answered.
    let mut recognised: Vec<String> = Vec::new();
    for reading in alternatives {
        let alternative = &reading.spec;
        // Asked of the enclosing value, before anything is selected out of it: the discriminator for a
        // batch of results is the type of the message holding them.
        if !predicates_hold(parsed, &alternative.require_parent) {
            continue;
        }
        let selected: Vec<&JsonValue> = match &alternative.select {
            Some(path) => query(parsed, path),
            None => vec![parsed],
        };
        if selected.is_empty() {
            continue;
        }
        let elements: Vec<&JsonValue> = if alternative.each {
            let mut items = Vec::new();
            let mut all_arrays = true;
            for value in &selected {
                match value.as_array() {
                    Some(inner) => items.extend(inner.iter()),
                    // Declared as a list and is not one: this is not the shape, so try the next.
                    None => all_arrays = false,
                }
            }
            if !all_arrays {
                continue;
            }
            items
        } else {
            selected
        };

        let mut produced = Vec::new();
        for element in elements {
            // For each element, the first of these sub-paths that resolves - decided per element, because
            // one dialect's groups each either wrap their contents under one of two spellings or are the
            // content themselves, and choosing once for the whole array would drop the odd one out.
            let element: Vec<&JsonValue> = if !alternative.then_present_any_of.is_empty() {
                // The first path that resolves *at all*. A present-but-empty member has declared nothing,
                // and yields nothing - it does not fall through to the element itself.
                match alternative
                    .then_present_any_of
                    .iter()
                    .find_map(|path| query(element, path).into_iter().next())
                    // A wrapper *is* a list. A present member that is not one has not declared its
                    // contents, so the element is not this shape - the same answer as the member being
                    // absent, which is what the retired code did by requiring the member to be an array.
                    .and_then(JsonValue::as_array)
                {
                    Some(items) => items.iter().collect(),
                    None if alternative.else_element => vec![element],
                    None => continue,
                }
            } else if alternative.then_any_of.is_empty() {
                vec![element]
            } else {
                let found = alternative
                    .then_any_of
                    .iter()
                    .map(|path| query(element, path))
                    .find(|found| !found.is_empty());
                match found {
                    Some(found) => found,
                    None if alternative.else_element => vec![element],
                    None => continue,
                }
            };
            for element in element {
                // Descend where declared, then lift - **one** copy step whose conflict policy is declared,
                // where there were two members with opposite unstated ones.
                let mut candidate = match &alternative.descend {
                    Some(member) => {
                        let Some(inner) = element.get(member.as_str()) else {
                            continue;
                        };
                        inner.clone()
                    }
                    None => element.clone(),
                };
                if !alternative.lift.is_empty()
                    && let Some(object) = candidate.as_object_mut()
                {
                    for lift in &alternative.lift {
                        // `element` is the value the selection landed on, `parent` the value it came out of.
                        let source = match lift.from {
                            super::schema::LiftSource::Element => element,
                            super::schema::LiftSource::Parent => parsed,
                        };
                        for member in &lift.members {
                            if object.contains_key(member.as_str())
                                && lift.on_conflict == super::schema::LiftConflict::KeepTarget
                            {
                                continue;
                            }
                            if let Some(value) = source.get(member.as_str()) {
                                object.insert(member.clone(), value.clone());
                            }
                        }
                    }
                }
                let candidate = candidate;
                // Trim declared per reading, because trimming a payload meant to be verbatim would change it.
                let candidate = match (alternative.trim, candidate.as_str()) {
                    (true, Some(text)) => json!(text.trim()),
                    _ => candidate,
                };
                if !predicates_hold(&candidate, &alternative.require) {
                    continue;
                }
                // The fragment decides what the element *is*; this reading decided where to look. Splitting
                // them is why one dialect can recognise its message shapes at four selection points with one
                // table.
                if !reading.fragment_cases.is_empty() {
                    let cases: Vec<CompiledReading> = reading
                        .fragment_cases
                        .iter()
                        .map(|spec| CompiledReading {
                            spec: spec.clone(),
                            fragment_cases: Vec::new(),
                        })
                        .collect();
                    // The selection point and then the case that answered: the fragment decides what the
                    // element *is*, this reading decided where to look, and a diagnostic needs both. Nested
                    // fragment cases lost the selection-point id entirely before this.
                    let inner = readings(&candidate, &cases, build);
                    // A fragment case matching means the *selection point* recognised this node, which is what
                    // a walk stop is about - the case says what the value is, the selection point says the
                    // rule's clause found it here.
                    if !inner.recognised.is_empty() && !recognised.contains(&alternative.id) {
                        recognised.push(alternative.id.clone());
                    }
                    produced.extend(inner.built.into_iter().map(|(value, target, mut steps)| {
                        let mut path = vec![alternative.id.clone()];
                        path.append(&mut steps);
                        (value, target, path)
                    }));
                    continue;
                }
                // Recognition is recorded **before** construction: the candidate passed every predicate this
                // clause states, so the payload is this shape whatever happens to the envelope.
                if !recognised.contains(&alternative.id) {
                    recognised.push(alternative.id.clone());
                }
                // **Built here**, so a candidate whose envelope cannot be made counts as this alternative
                // producing nothing - and the coalesce moves on to the next shape and then to the fallback.
                let Some(value) = built(candidate, alternative.wrap.as_ref(), build) else {
                    continue;
                };
                produced.push((value, alternative.emit, vec![alternative.id.clone()]));
            }
        }
        if !produced.is_empty() {
            return Selection {
                built: produced,
                recognised,
            };
        }
    }
    // Recognised but unbuildable: nothing was produced, and the recognition still travels out - a walk must not
    // descend into a node a clause identified as a message merely because its envelope failed.
    Selection {
        built: Vec::new(),
        recognised,
    }
}

/// One emission's evidence: the rule, plus the clauses inside it that produced this observation.
///
/// A single witness for the ordinary case. Where several clauses contributed - an aggregate's readings, a
/// grouped run's cases - the caller passes them all and each becomes its own path, because "these three
/// declarations produced this" is the true statement and picking one of them is not.
fn rule_evidence(rule: &CompiledMessageRule, paths: &[Vec<String>]) -> super::expr::EvidenceSet {
    let root = || super::expr::ClausePath::root(rule.rule_id.clone());
    // Each element is a **path**, not a step: a grouped element run's witnesses share the pass and differ in
    // the case, so `rule -> pass -> case` twice is the true statement while `rule -> pass` beside `rule -> case`
    // is two paths neither of which exists.
    let witnesses: Vec<super::expr::ClausePath> = paths
        .iter()
        .map(|steps| {
            steps
                .iter()
                .fold(root(), |path, step| path.then(step.clone()))
        })
        .collect();
    super::expr::EvidenceSet::of(witnesses).unwrap_or_else(|| super::expr::EvidenceSet::one(root()))
}

/// A reading's value once its envelope is applied, or `None` when the envelope cannot be built.
///
/// A reading's own envelope wins over the rule's, which is what `or` says. With no construction context - the
/// aggregate path - the value is returned as it stands, and compilation refuses a per-reading envelope there so
/// nothing can be silently dropped.
fn built(
    value: JsonValue,
    reading_wrap: Option<&WrapSpec>,
    build: Option<Construction<'_>>,
) -> Option<JsonValue> {
    let Some(build) = build else {
        return Some(value);
    };
    match reading_wrap.or(build.rule_wrap) {
        Some(wrap) => wrapped(value, wrap, build.ctx, Some(build.root)),
        None => Some(value),
    }
}

/// One entry per index of a dotted attribute family, assembled from the keys under it.
///
/// The convention flattens a list of objects into `<prefix>.<index>.<member>`, so this is the inverse:
/// gather every key of an index, strip the prefix, and keep the remainder as the member name - nested
/// members included, so `content.0.text` stays `content.0.text` rather than being lost or guessed at.
///
/// Ordered by index, because a `BTreeMap` key is the number: reading the attribute map directly would
/// order turns by hash.
fn indexed_entries(
    attrs: &HashMap<String, String>,
    family: &str,
    read: &ReadSpec,
    require: Option<&MemberRequirements>,
) -> Vec<IndexedEntry> {
    // Every other parameter was a facet of the same `ReadSpec`, and threading them one by one meant a new
    // facet was a new argument at every call site.
    let entry_member = read.entry_member.as_deref();
    let numeric = &read.numeric_members;
    let overlay = read.overlay.as_ref();
    let entry_value = read.entry_value.as_ref();
    let entry_value_parse = read.entry_value_parse;
    // Parsed once for the whole family: the counterpart list describes every entry, so parsing it per
    // entry would re-parse one payload as many times as there are messages.
    let counterparts = overlay.and_then(|overlay| counterpart_list(attrs, overlay));
    // **Bucketed in one pass**, keyed by index and holding the member name after `family.<index>.`. The
    // discovery pass and then a full scan of the attribute map *per index* is quadratic in the family's size:
    // a hundred-turn conversation flattened into a family means a hundred walks over every attribute the span
    // carries, twice, plus one per `require`d member. A `BTreeMap` because the entries are read in index order,
    // which is what keeps a turn sequence from being a hash order.
    let family_dot = format!("{family}.");
    let mut buckets: std::collections::BTreeMap<usize, Vec<(&str, &String)>> =
        std::collections::BTreeMap::new();
    for (key, value) in attrs {
        if let Some(rest) = key.strip_prefix(&family_dot)
            && let Some(index) = rest.split('.').next()
            && let Ok(parsed) = index.parse::<usize>()
        {
            // The remainder past `family.<index>` - empty where the key *is* the index, which a producer can
            // write and which belongs to no member.
            let member = rest[index.len()..].strip_prefix('.').unwrap_or("");
            buckets.entry(parsed).or_default().push((member, value));
        }
    }
    // Sorted once per bucket rather than once per read of it: the attribute map's order is randomised per
    // process and these objects are persisted with their insertion order.
    for members in buckets.values_mut() {
        members.sort_unstable_by_key(|(member, _)| *member);
    }

    let mut out: Vec<IndexedEntry> = Vec::new();
    for (index, members) in &buckets {
        let index = *index;
        // The physical keys this entry read. An entry's *tag* is `family.N`, which is a name for the entry
        // and not a key any producer wrote - so owning the tag left `family.N.content` free for another
        // rule, and the overlay's own attribute free for the dialect that reads it as a whole payload.
        let mut consumed: Vec<String> = Vec::new();
        let entry_prefix = format!("{family}.{index}");
        // Where the message sits: the entry itself, or a sub-level of it.
        let subject_prefix = match entry_member {
            Some(member) => format!("{entry_prefix}.{member}"),
            None => entry_prefix.clone(),
        };

        // An index exists as soon as any key mentions it, and a family holds keys that are not messages.
        // Asked of this index's bucket, so a requirement costs the bucket rather than the whole span.
        if let Some(require) = require
            && !bucket_members_present(members, entry_member, require)
        {
            continue;
        }

        let mut object = serde_json::Map::new();
        // The subject's own members, unprefixed. From this index's bucket, already sorted.
        let subject_dot = format!("{subject_prefix}.");
        let within = match entry_member {
            Some(nested) => format!("{nested}."),
            None => String::new(),
        };
        for (member, value) in members
            .iter()
            .filter_map(|(member, value)| match entry_member {
                Some(_) => member.strip_prefix(within.as_str()).map(|m| (m, *value)),
                None => Some((*member, *value)),
            })
            .filter(|(member, _)| !member.is_empty())
        {
            consumed.push(format!("{subject_dot}{member}"));
            object.insert(member.to_string(), member_value(member, value, numeric));
        }
        // Where the message is nested, the entry's *other* members come too: they belong to the same
        // observation, and the sub-level's own keys are already in, so they are skipped here.
        if entry_member.is_some() {
            let entry_dot = format!("{entry_prefix}.");
            for (member, value) in members
                .iter()
                .filter(|(member, _)| !member.is_empty() && !member.starts_with(within.as_str()))
            {
                consumed.push(format!("{entry_dot}{member}"));
                object.insert(member.to_string(), member_value(member, value, numeric));
            }
        }
        if let Some(overlay) = overlay
            && let Some(content) = counterpart_content(overlay, counterparts.as_ref(), index)
            && object
                .keys()
                .any(|member| member.starts_with(overlay.when_member_prefix.as_str()))
        {
            let flattened: Vec<String> = object
                .keys()
                .filter(|member| member.starts_with(overlay.when_member_prefix.as_str()))
                .cloned()
                .collect();
            for member in flattened {
                object.remove(&member);
            }
            // The overlay's payload was read, so this entry owns it: a dialect joining its flattened family
            // against a serialised copy has consumed that copy, and leaving it unclaimed let the dialect
            // that reads it whole emit the same turn again.
            consumed.push(overlay.from.clone());
            object.insert(overlay.as_member.clone(), content);
        }
        // A projection reads one value out of the entry: the entry is a wrapper around a single payload,
        // and the payload is the datum. An entry the projection does not find contributes nothing - it is
        // not this shape - rather than contributing the wrapper.
        match entry_value {
            Some(path) => {
                let assembled = JsonValue::Object(object);
                if let Some(found) = query(&assembled, path).into_iter().next() {
                    // A declared parse mode decides what a malformed payload means. Without it the member
                    // has already been sniffed to a string, and emitting that string as a tool definition
                    // reports junk where the retired code reported nothing.
                    let value = match (entry_value_parse, found.as_str()) {
                        (Some(mode), Some(text)) => parse_value(text, mode),
                        (Some(_), None) => Some(found.clone()),
                        (None, _) => Some(found.clone()),
                    };
                    if let Some(value) = value {
                        out.push(IndexedEntry {
                            carrier: subject_prefix,
                            value,
                            consumed,
                        });
                    }
                }
            }
            None => out.push(IndexedEntry {
                carrier: subject_prefix,
                value: JsonValue::Object(object),
                consumed,
            }),
        }
    }
    out
}

/// One entry of an indexed family: its tag, its assembled payload, and the physical keys it read.
///
/// The keys are separate from the tag because they are different kinds of name. `llm.input_messages.0.message`
/// is assembled here to identify the entry; `llm.input_messages.0.message.role` is a key a producer wrote,
/// and only the second is something another rule could also read.
struct IndexedEntry {
    carrier: String,
    value: JsonValue,
    consumed: Vec<String>,
}

/// The counterpart list a positional overlay joins against, where the span carries one this dialect wrote.
fn counterpart_list(
    attrs: &HashMap<String, String>,
    overlay: &OverlaySpec,
) -> Option<Vec<JsonValue>> {
    let raw = attrs.get(overlay.from.as_str())?;
    let parsed = parse_value(raw, overlay.parse.unwrap_or(ParseMode::Json))?;
    let list = overlay
        .select_any_of
        .iter()
        .find_map(|path| query(&parsed, path).into_iter().next()?.as_array())?;
    // A batch of exactly one conversation: its single member is the list of messages. Not a mixed or
    // longer list - two batches are two conversations, and the witness below decides whether whatever is
    // left is this dialect's own serialisation.
    let list = match (overlay.unwrap_single_element_list, list.as_slice()) {
        (true, [only]) => only.as_array().unwrap_or(list),
        _ => list,
    };
    if list.is_empty() {
        return None;
    }
    let witness = JsonValue::Array(list.clone());
    if !predicates_hold(&witness, &overlay.witness) {
        return None;
    }
    Some(list.clone())
}

/// The content the counterpart at this position holds, where it is an improvement on the flattened form.
fn counterpart_content(
    overlay: &OverlaySpec,
    counterparts: Option<&Vec<JsonValue>>,
    index: usize,
) -> Option<JsonValue> {
    let found = overlay
        .content_any_of
        .iter()
        .find_map(|path| query(counterparts?.get(index)?, path).into_iter().next())?;
    predicates_hold(found, &overlay.require).then(|| found.clone())
}

/// A named member read as a number where its text is one, otherwise the ordinary sniff.
fn member_value(member: &str, raw: &str, numeric: &[String]) -> JsonValue {
    if numeric.iter().any(|name| name == member)
        && let Ok(number) = raw.parse::<f64>()
        && let Some(number) = serde_json::Number::from_f64(number)
    {
        return JsonValue::Number(number);
    }
    sniffed_value(raw)
}

/// A member of an indexed family: JSON where it looks like JSON, the text otherwise.
///
/// The sniff matters. Parsing everything would turn `true`, `42` and a bare word into non-strings, and
/// parsing nothing would store an object as prose - so the test is whether the value *opens* as JSON,
/// with the raw text kept when it opens that way and does not parse.
fn sniffed_value(raw: &str) -> JsonValue {
    if raw.starts_with('{') || raw.starts_with('[') {
        serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))
    } else {
        json!(raw)
    }
}

/// Both gates, in one place so every read form is subject to them.
fn gates_allow(rule: &CompiledMessageRule, ctx: &MessageContext<'_>) -> bool {
    if let Some(gate) = &rule.when
        && !super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.gate_attrs)
    {
        return false;
    }
    if let Some(gate) = &rule.unless
        && super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.gate_attrs)
    {
        return false;
    }
    true
}

/// The attribute a rule reads and its raw value: the named one, or the first of its alternatives the
/// span carries.
///
/// Returns the key *found*, not the key asked for, because that key becomes the carrier tag and two
/// spellings of one payload must stay distinguishable.
/// Every carrier this rule names that the span carries, in declared order.
///
/// Not the first, unlike an ordinary read: a framework may write the same tools under several keys at
/// different richness, and each is its own observation - so all of them are read and the best copy per
/// name wins downstream, rather than the richest being hidden behind whichever key was declared first.
fn carrier_texts<'p, 's>(
    rule: &'p CompiledMessageRule,
    ctx: &MessageContext<'s>,
) -> Vec<(&'p str, &'s str)> {
    rule.read
        .attribute
        .as_deref()
        .into_iter()
        .chain(rule.read.each.iter().map(String::as_str))
        .filter_map(|key| ctx.span_attrs.get(key).map(|raw| (key, raw.as_str())))
        .collect()
}

fn resolve_attribute<'p, 's>(
    read: &'p ReadSpec,
    attrs: &'s HashMap<String, String>,
) -> Option<(&'p str, &'s str)> {
    if let Some(attribute) = read.attribute.as_deref() {
        return attrs.get(attribute).map(|raw| (attribute, raw.as_str()));
    }
    read.first_present
        .iter()
        .find_map(|key| attrs.get(key).map(|raw| (key.as_str(), raw.as_str())))
}

/// Build the declared envelope around a read value.
fn wrapped(
    value: JsonValue,
    wrap: &WrapSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
) -> Option<JsonValue> {
    // The reading as it arrived, before a content member was selected out of it: an attachment may name a
    // sibling of the content, which is gone once the content replaces the value.
    let subject = value.clone();
    let subject = Some(&subject);
    // A content block, when the carrier holds one part of a block rather than a whole message.
    // The role, and the part of the reading that is the content, may both come from the payload.
    let role = wrap
        .role_from
        .as_ref()
        .and_then(|path| query(&value, path).into_iter().next())
        .and_then(JsonValue::as_str)
        .and_then(|found| match wrap.role_map.get(found) {
            Some(mapped) => Some(mapped.clone()),
            // Closed: the member names a speaker rather than a role, so an unlisted value is not one.
            None if wrap.role_map_is_closed => None,
            None => Some(found.to_string()),
        })
        .or_else(|| wrap.role.clone());
    let content_paths: Vec<&serde_json_path::JsonPath> = wrap.content_from_any_of.iter().collect();
    let value = if content_paths.is_empty() {
        value
    } else {
        // The first path that resolves: one dialect serialises a message three ways and the content sits in a
        // different member each time.
        match content_paths
            .iter()
            .find_map(|path| query(&value, path).into_iter().next())
        {
            Some(found) => found.clone(),
            None => match &wrap.content_default {
                Some(default) => default.clone(),
                None => return None,
            },
        }
    };
    // Wrap only where the value is not already message-shaped: a generic carrier holds either.
    if wrap.only_plain_data && !crate::domain::sideml::is_plain_data_or_content(&value) {
        return Some(value);
    }
    let value = match &wrap.block {
        Some(block) => JsonValue::Array(vec![built_block(value, block, ctx, payload, subject)]),
        None => value,
    };
    // A block built from another member, placed before the content: one dialect reports a model's reasoning
    // beside its reply, and the canonical form is a thinking block ahead of the text.
    let value = match &wrap.prepend_block {
        Some(spec) => {
            match subject
                .and_then(|s| query(s, &spec.from).into_iter().next())
                .filter(|found| predicates_hold(found, &spec.require))
            {
                Some(found) => {
                    let prefix = built_block(found.clone(), &spec.block, ctx, payload, subject);
                    match value {
                        JsonValue::Array(items) => {
                            let mut all = vec![prefix];
                            all.extend(items);
                            JsonValue::Array(all)
                        }
                        other => JsonValue::Array(vec![prefix, other]),
                    }
                }
                None => value,
            }
        }
        None => value,
    };
    let mut object = serde_json::Map::new();
    if let Some(role) = role {
        object.insert("role".to_string(), json!(role));
    }
    for (member, literal) in &wrap.members {
        object.insert(member.clone(), literal.clone());
    }
    let content_member = wrap.content_as.as_deref().unwrap_or("content");
    // Before the content member, then the content, then after - because the order is observable: see
    // `AttachSpec::after_content`.
    for attach in wrap.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    // The canonical tool-call list replaces the content: a message that carries calls carries no text, and
    // an empty list means the reading found nothing usable, which `require_after` is what refuses.
    match &wrap.tool_calls_from {
        Some(spec) => {
            let member = spec.as_member.as_deref().unwrap_or("tool_calls");
            object.insert(
                member.to_string(),
                JsonValue::Array(canonical_tool_calls(subject, spec)?),
            );
        }
        None => {
            object.insert(content_member.to_string(), value);
        }
    }
    if let Some(spec) = &wrap.tool_call_from
        && let Some(call) = single_tool_call(subject, spec)
    {
        object.insert(
            spec.as_member.as_deref().unwrap_or("tool_call").to_string(),
            call,
        );
    }
    for attach in wrap.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    let message = JsonValue::Object(object);
    // Some shapes can only be judged once assembled - a tool result is worth keeping if it ended up with a
    // name, a call id or content, and the call id may have come from the element or from its parent.
    if !predicates_hold(&message, &wrap.require_after) {
        return None;
    }
    Some(message)
}

/// One tool call as `{name, arguments}`, the convention `sideml/tools.rs` unwraps.
fn single_tool_call(subject: Option<&JsonValue>, spec: &SingleToolCallSpec) -> Option<JsonValue> {
    let subject = subject?;
    // Only a string names a tool. A number or an object here is not a name, and the declared default is
    // what the retired path used - reporting the structure as a name builds an unusable canonical call.
    let name = query(subject, &spec.name)
        .into_iter()
        .next()
        .filter(|found| found.is_string())
        .cloned()
        .or_else(|| spec.name_default.clone())?;
    let arguments = query(subject, &spec.arguments)
        .into_iter()
        .next()
        .cloned()
        .or_else(|| spec.arguments_default.clone())
        .unwrap_or(json!({}));
    Some(json!({"name": name, "arguments": arguments}))
}

/// The canonical tool-call list: `{id, type: "function", function: {name, arguments}}` per call.
///
/// A call with no id or no name is dropped - the id is what pairs a result with its call, and a nameless
/// call names nothing to run. Arguments arrive as a serialised JSON string as often as an object, and are
/// parsed here so nothing downstream has to know that one member is encoded twice.
fn canonical_tool_calls(
    subject: Option<&JsonValue>,
    spec: &ToolCallsSpec,
) -> Option<Vec<JsonValue>> {
    let Some(subject) = subject else {
        return Some(Vec::new());
    };
    let mut calls = Vec::new();
    let mut invalid = 0;
    for call in query(subject, &spec.select) {
        let built = (|| {
            let id = query(call, &spec.id).into_iter().next()?.as_str()?;
            let name = query(call, &spec.name).into_iter().next()?.as_str()?;
            let arguments = match query(call, &spec.arguments).into_iter().next() {
                Some(found) => match found.as_str() {
                    Some(text) => serde_json::from_str(text).unwrap_or(json!(text)),
                    None => found.clone(),
                },
                None => json!({}),
            };
            Some(json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": arguments},
            }))
        })();
        match built {
            Some(call) => calls.push(call),
            None => invalid += 1,
        }
    }
    // **Reported under either policy.** A call with no id or no name is a producer defect, and the loop that
    // read it silently absorbing that is how a response that called two tools came to show one.
    if invalid > 0 {
        tracing::debug!(
            target: "sideseat::rules",
            invalid,
            policy = ?spec.on_invalid_item,
            "tool calls in a list could not be built - they carry no id or no name"
        );
        if spec.on_invalid_item == super::schema::InvalidItem::FailMessage {
            // The construction is malformed, so the coalesce moves on and the rule's `fallback` gets its turn -
            // which is the difference between "the message is incomplete" and "this shape did not apply".
            return None;
        }
    }
    Some(calls)
}

/// The block a rule builds around its read value.
fn built_block(
    value: JsonValue,
    block: &BlockSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
    subject: Option<&JsonValue>,
) -> JsonValue {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(block.block_type));
    for attach in block.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    object.insert(
        block.content_as.as_deref().unwrap_or("content").to_string(),
        value,
    );
    for attach in block.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    JsonValue::Object(object)
}

/// One attachment's value, or `None` where nothing supplied one.
fn attached_value(
    attach: &AttachSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
    subject: Option<&JsonValue>,
) -> Option<JsonValue> {
    // A literal on its own, naming no source: the member is part of the shape rather than something read.
    // A content block's unsigned `signature` is exactly this, and its value is `null`.
    if attach.from.is_none() && attach.from_value_any_of.is_empty() && attach.from_path.is_none() {
        return attach.value.clone();
    }
    // Ordered paths into the value being wrapped, for a member that may sit at the top level or under the
    // wrapper a serialiser added.
    if !attach.from_value_any_of.is_empty() {
        let found = subject.and_then(|subject| {
            attach
                .from_value_any_of
                .iter()
                .find_map(|path| query(subject, path).into_iter().next())
        });
        if let Some(found) = found {
            if !predicates_hold(found, &attach.require) {
                return None;
            }
            let value = match attach.parse {
                Some(mode) => match found.as_str() {
                    // A member holding serialised JSON: parsed here, because leaving it a string means
                    // whoever reads it later has to know that this one member is encoded twice.
                    Some(text) => parse_value(text, mode)?,
                    None => found.clone(),
                },
                None => found.clone(),
            };
            return Some(value);
        }
        // Nothing in the value: fall through to the payload path below, which is how "the element's own, else
        // its parent's" is one member rather than two that overwrite each other.
    }
    // A member of the rule's own payload, where the dialect reports it beside the content rather than
    // inside it.
    if let Some(path) = &attach.from_path {
        // **Falls through**, like `from_value_any_of` above. This was `?`, which returned from the whole
        // function - so a payload path that resolved to nothing skipped the sibling `from` attribute, the
        // span-name fallback *and* the `default`, while an absent `from_value_any_of` fell through to exactly
        // those. One member, two source forms, two different answers to "nothing here": the asymmetry was in
        // the code rather than in anything declared.
        if let Some(found) = payload.and_then(|payload| query(payload, path).into_iter().next()) {
            if !predicates_hold(found, &attach.require) {
                return None;
            }
            let value = match (attach.lowercase, found.as_str()) {
                (true, Some(text)) => json!(text.to_lowercase()),
                _ => found.clone(),
            };
            return Some(value);
        }
    }
    if let Some(raw) = attach
        .from
        .as_ref()
        .and_then(|key| ctx.span_attrs.get(key))
        .filter(|raw| !(attach.blank_is_absent && raw.trim().is_empty()))
    {
        let raw = if attach.strip_bracket_tag {
            split_bracket_tag(raw).1
        } else {
            raw.as_str()
        };
        if let Some(expected) = &attach.when_equals {
            if raw != expected {
                return None;
            }
            // A flag: the literal is the point, not the string that proved it.
            return Some(attach.value.clone().unwrap_or(json!(true)));
        }
        // A value that will not parse falls through to the default below, which is what an unparseable
        // structured member should do: the member exists in the shape, so it carries its empty form.
        if let Some(parsed) = parse_value(raw, attach.parse.unwrap_or(ParseMode::Text)) {
            // `require` asked here too, where it used to apply to `from_value_any_of` alone - a modifier that
            // silently means nothing beside one source form is the same defect as a source form that means two
            // things.
            if !predicates_hold(&parsed, &attach.require) {
                return None;
            }
            let parsed = match (attach.lowercase, parsed.as_str()) {
                (true, Some(text)) => json!(text.to_lowercase()),
                _ => parsed,
            };
            return Some(parsed);
        }
    }
    // The span name, where the conventions put the same fact.
    if let Some(prefix) = &attach.or_span_name_after
        && let Some(rest) = ctx.span_name.strip_prefix(prefix.as_str())
    {
        let trimmed = rest.trim();
        if !trimmed.is_empty() {
            return Some(json!(trimmed));
        }
    }
    attach.default.clone()
}

/// Whether an indexed entry's bucket holds the members a rule requires.
///
/// The bucket rather than the span's whole attribute map: asked per index, a full scan is quadratic in the
/// family's size, and every answer is a property of this index's keys alone.
fn bucket_members_present<'a>(
    members: &[(&'a str, &String)],
    entry_member: Option<&str>,
    require: &MemberRequirements,
) -> bool {
    // Names are relative to the *subject*, which is the entry or a sub-level of it.
    let within = entry_member.map(|nested| format!("{nested}."));
    let relative = |member: &'a str| -> Option<&'a str> {
        match &within {
            Some(prefix) => member.strip_prefix(prefix.as_str()),
            None => Some(member),
        }
    };
    let present = |requirement: &super::schema::MemberRequirement| {
        let nested = format!("{}.", requirement.name);
        let exact = members
            .iter()
            .any(|(member, _)| relative(member) == Some(requirement.name.as_str()));
        let under = || {
            members
                .iter()
                .any(|(member, _)| relative(member).is_some_and(|m| m.starts_with(nested.as_str())))
        };
        match requirement.presence {
            MemberPresence::Exact => exact,
            MemberPresence::Nested => under(),
            MemberPresence::Either => exact || under(),
        }
    };
    (require.all_of.is_empty() || require.all_of.iter().all(present))
        && (require.any_of.is_empty() || require.any_of.iter().any(present))
}

/// Assemble a composed message, or `None` where the span supplied no member.
///
/// Emitted only when at least one *source* member was filled - the trailing literals are not evidence of
/// anything, so a rule whose sources all missed would otherwise emit a message consisting of a role.
fn composed(
    compose: &CompiledCompose,
    ctx: &MessageContext<'_>,
    read: &mut Vec<OwnedCarrier>,
) -> Option<JsonValue> {
    let attrs = ctx.span_attrs;
    let mut object = serde_json::Map::new();

    for compiled in &compose.members {
        let member = &compiled.spec;
        if let Some(prefix) = &member.sweep_prefix {
            // Sorted before insertion. The attribute map's iteration order is randomised per process, and
            // this object is serialised with insertion order preserved and *persisted* - so an unsorted
            // sweep writes different bytes for the same span on different runs. The equivalence oracle
            // cannot see it: both implementations walk the same map in the same process.
            let mut swept: Vec<(&str, &String)> = attrs
                .iter()
                .filter_map(|(key, value)| {
                    key.strip_prefix(prefix.as_str())
                        .filter(|suffix| !member.except.iter().any(|skip| skip == suffix))
                        .map(|suffix| (suffix, value))
                })
                .collect();
            swept.sort_unstable_by_key(|(suffix, _)| *suffix);
            for (suffix, value) in swept {
                read.push(OwnedCarrier::attribute(&format!("{prefix}{suffix}")));
                object.insert(suffix.to_string(), sniffed_value(value));
            }
            continue;
        }
        let Some(name) = &member.as_member else {
            continue;
        };
        let direct = member
            .from_any_of
            .iter()
            .find_map(|key| attrs.get(key).map(|raw| (key, raw)))
            .and_then(|(key, raw)| {
                // The carrier this member actually read - recorded **after** the parse, so a member whose
                // payload does not parse neither contributes nor takes the carrier off the table. Recorded
                // before, a malformed compose member was owned while the *same* malformed carrier read by an
                // ordinary rule was not: one asymmetry, and the compose silently suppressed another dialect's
                // reading of junk it could not read either. A rule that means to take a payload away without
                // emitting has `claim` for it.
                let value = parse_value(raw, member.parse.unwrap_or(ParseMode::Text))?;
                read.push(OwnedCarrier::attribute(key));
                Some(value)
            });
        let value = direct.or_else(|| {
            // The conditional last resort: a key that is not this dialect's own, read only on evidence
            // that the span is one of its spans.
            let fallback = member.fallback.as_ref()?;
            let gate = compiled.fallback_gate.as_ref()?;
            if !super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.gate_attrs) {
                return None;
            }
            let raw = attrs.get(&fallback.from)?;
            let value = parse_value(raw, fallback.parse.unwrap_or(ParseMode::Text))?;
            read.push(OwnedCarrier::attribute(&fallback.from));
            Some(value)
        });
        if let Some(value) = value {
            object.insert(name.clone(), value);
        }
    }

    if object.is_empty() {
        return None;
    }
    for (member, literal) in &compose.trailing {
        object.insert(member.clone(), literal.clone());
    }
    Some(JsonValue::Object(object))
}

/// A leading `[TAG]\n` marker split from its body, both trimmed.
///
/// The tag is recognised only with the newline: a body that merely opens with a bracket is not a tagged
/// section, and treating it as one would swallow its first line.
fn split_bracket_tag(value: &str) -> (Option<&str>, &str) {
    match value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n"))
    {
        Some((tag, body)) => (Some(tag.trim()), body.trim()),
        None => (None, value.trim()),
    }
}

/// The messages a tagged text carrier yields, one per section.
/// Each section's message, with **which route** built it.
///
/// `SectionRoute.id` is a required declaration and was discarded here: two routes of one rule produced
/// emissions carrying identical evidence, so a diagnostic could name the rule and not the route - which is
/// exactly what a reader needs when `claude-agent-sdk.new_context`'s `tool_result` and `as_user` routes
/// disagree.
fn sectioned(raw: &str, spec: &SectionsSpec) -> Vec<(String, JsonValue)> {
    let mut out = Vec::new();
    for section in raw.split(spec.split_on.as_str()) {
        let (tag, body) = split_bracket_tag(section);
        if body.is_empty() {
            continue;
        }
        // The first route whose prefix the tag carries, else the default.
        let matched = spec
            .routes
            .iter()
            .find_map(|route| match &route.tag_prefix {
                Some(prefix) => tag
                    .and_then(|t| t.strip_prefix(prefix.as_str()))
                    .map(|rest| (route, Some(rest.trim()))),
                None => Some((route, None)),
            });
        let Some((route, capture)) = matched else {
            continue;
        };
        if !route.skip_when.is_empty() {
            // The section as a value, so one predicate vocabulary answers this too.
            let subject = json!({
                "capture": capture.map(JsonValue::from).unwrap_or(JsonValue::Null),
                "body": body,
            });
            if predicates_hold(&subject, &route.skip_when) {
                continue;
            }
        }
        let mut message = serde_json::Map::new();
        message.insert("role".to_string(), json!(route.role));
        match &route.block {
            Some(block) => {
                let mut object = serde_json::Map::new();
                object.insert("type".to_string(), json!(block.block_type));
                if let Some(member) = &block.capture_as
                    && let Some(captured) = capture
                {
                    object.insert(member.clone(), json!(captured));
                }
                object.insert(
                    block.content_as.as_deref().unwrap_or("content").to_string(),
                    json!(body),
                );
                message.insert(
                    "content".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(object)]),
                );
            }
            None => {
                message.insert("content".to_string(), json!(body));
            }
        }
        out.push((route.id.clone(), JsonValue::Object(message)));
    }
    out
}

/// The observations one rule finds on a span.
///
/// Separate from the plan's loop so a branch set's sub-readings run through exactly the same path as a
/// top-level rule - a second evaluator would be a second place for the two to drift.
fn emit_rule<'p>(rule: &'p CompiledMessageRule, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
    // A branch set is several readings with a local order between them: the primaries, then the fallbacks
    // only if those found nothing, then the unconditional ones. Evaluated here, not in `run`, so the
    // metadata path sees it too and the primary-empty decision is made on *all* emissions - a primary
    // whose only output is a tool list still counts as matched, which is what stops a tools-only request
    // from wrongly taking the fallback.
    if let Some(set) = &rule.branch_set {
        let mut out = Vec::new();
        if !gates_allow(rule, ctx) {
            return out;
        }
        // Per sub-rule and **per axis**, because the parent declares no carrier and its own flag says
        // nothing about what a sub-rule reads. A leaf that may not read a tool span as a conversation may
        // still state that span's tools, so its metadata emissions are kept and its message emissions are
        // not - a whole-leaf boolean let a mixed-axis leaf through on the strength of its metadata and `run`
        // then retained its messages.
        let from = |sub: &'p CompiledMessageRule| -> Vec<Emission<'p>> {
            let forbidden = ctx.is_tool_span && !sub.reads_tool_spans;
            emit_rule(sub, ctx)
                .into_iter()
                .filter(|emission| {
                    !forbidden
                        || matches!(
                            emission.target,
                            EmitTarget::ToolDefinitions | EmitTarget::ToolNames
                        )
                })
                .collect()
        };
        for sub in &set.primary {
            out.extend(from(sub));
        }
        // "Every primary reading came up empty" is asked **per axis**, not over every emission.
        //
        // A branch set may hold leaves that answer different questions about the same carrier: one dialect's
        // primary reads its serialised request as the conversation *and* reads the tools it was offered out of
        // the same attribute. Judged over all emissions, a request carrying tools and no messages made the
        // branch non-empty, so the fallback did not run - and the message path then filtered the tool
        // definition out, so the span reported **no message at all** and the tool call's arguments were lost.
        //
        // So a fallback leaf runs when nothing of *its* kind was produced. That is what the declaration says:
        // read this instead, if the primaries found none of what you are asking for.
        let produced: std::collections::BTreeSet<EmitTarget> =
            out.iter().map(|emission| emission.target).collect();
        for sub in &set.fallback {
            if possible_targets(sub)
                .into_iter()
                .any(|target| produced.contains(&target))
            {
                continue;
            }
            out.extend(from(sub));
        }
        for sub in &set.always {
            out.extend(from(sub));
        }
        return out;
    }
    let mut out = Vec::new();
    if !gates_allow(rule, ctx) {
        return out;
    }
    if let Some(compose) = &rule.compose {
        // Every physical attribute the compose read, so the emission owns them all. Owning only the
        // synthetic tag left each consumed attribute free for another dialect to read as conversation.
        let mut read_carriers = Vec::new();
        if let Some(value) = composed(compose, ctx, &mut read_carriers).filter(|value| {
            // Judged once the members are together: a name a dialect reported may not be a tool anyone can
            // call, and only the assembled object shows it.
            predicates_hold(value, &compose.require)
        }) {
            // The canonical tool-definition shape, where the assembled members are one tool rather than a
            // message. Wrapped here because the shape is ours and the members are the dialect's.
            let value = if compose.as_tool_definition {
                json!([{"type": "function", "function": value}])
            } else {
                value
            };
            // The synthetic tag is **not** owned: `owns` is what the span carried and this is the name the
            // engine gives the assembled result. Claiming it let one compose suppress another that read
            // entirely different carriers.
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                carrier: EmittedCarrier::Attribute(compose.tag.as_str()),
                owns: read_carriers,
                target: rule.target,
                value,
            });
        }
        return out;
    }
    // Tool definitions written as a language's `repr`: the grammar is sealed, its vocabulary declared.
    if let Some(spec) = &rule.tool_repr {
        for (attribute, raw) in carrier_texts(rule, ctx) {
            if let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json))
                && let Some(tools) = super::tool_repr::tools_from_carrier(&parsed, spec)
            {
                out.push(Emission {
                    rule_id: &rule.rule_id,
                    evidence: rule_evidence(rule, &[]),
                    carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
                    owns: OwnedCarrier::just(attribute),
                    target: rule.target,
                    value: JsonValue::Array(tools),
                });
            }
        }
        return out;
    }
    // A **named** family: every member under the root is its own observation, in the declared order. Unlike an
    // indexed family there are no sub-members to assemble - a member's value *is* the payload - so this is one
    // read per key rather than an entry built from several.
    if let Some(family) = &rule.read.attribute_family {
        let root_dot = format!("{}.", family.root);
        let mut members: Vec<(&String, &String)> = ctx
            .span_attrs
            .iter()
            .filter(|(key, _)| key.starts_with(&root_dot))
            .collect();
        match family.order {
            super::schema::AttributeFamilyOrder::MemberName => members.sort_by(|a, b| a.0.cmp(b.0)),
        }
        for (key, raw) in members {
            // **Per member**, because a member is the observation here - the family as a whole is not one
            // payload. This branch returns before the rule-wide checks further down, so without this a
            // `require_non_blank` on a named family was a declaration read from nowhere: a blank member was
            // emitted and the asset said it would not be.
            if rule.require_non_empty && raw.is_empty() {
                continue;
            }
            if rule.require_non_blank && raw.trim().is_empty() {
                continue;
            }
            let Some(value) = parse_value(raw, rule.parse.unwrap_or(ParseMode::JsonOrString))
            else {
                continue;
            };
            let value = match &rule.wrap {
                Some(wrap) => match wrapped(value, wrap, ctx, None) {
                    Some(built) => built,
                    // An envelope that cannot be built is not an observation: the same rule the other
                    // readings follow, rather than emitting a bare payload under a message tag.
                    None => continue,
                },
                None => value,
            };
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                // Tagged with the **member's own key**, not the root: two members are two carriers, and one
                // tag for the family would make them indistinguishable to carrier semantics and identity.
                // `Owned`, because the key comes from the span rather than from the rule.
                carrier: EmittedCarrier::Owned(key.clone()),
                owns: OwnedCarrier::just(key),
                target: rule.target,
                value,
            });
        }
        return out;
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        let entries = indexed_entries(
            ctx.span_attrs,
            family,
            &rule.read,
            rule.require_members.as_ref(),
        );
        // A result set is one observation. Its entries are the array, and the envelope says what the array
        // is - so the whole family is tagged once rather than one carrier per document.
        if rule.aggregate_into_array {
            if entries.is_empty() {
                return out;
            }
            // Every key the aggregate consumed, plus each entry's tag and the family's own name. Owning the
            // family name alone left every physical member free for another rule, and the entry tags are
            // names this engine assembled rather than keys a producer wrote.
            let mut owns: Vec<OwnedCarrier> = Vec::new();
            for entry in &entries {
                owns.push(OwnedCarrier::attribute(&entry.carrier));
                owns.extend(
                    entry
                        .consumed
                        .iter()
                        .map(|key| OwnedCarrier::attribute(key)),
                );
            }
            owns.push(OwnedCarrier::attribute(family));
            owns.sort_unstable();
            owns.dedup();
            let array = JsonValue::Array(entries.into_iter().map(|entry| entry.value).collect());
            let value = match &rule.wrap {
                Some(wrap) => match wrapped(array, wrap, ctx, None) {
                    Some(value) => value,
                    None => return out,
                },
                None => array,
            };
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(family)),
                owns,
                target: rule.target,
                value,
            });
            return out;
        }
        for entry in entries {
            // The entry's tag *and* every key it read. The tag alone is a name for the entry, not a key, so
            // it conflicted with nothing a second rule could reach.
            let mut owns = OwnedCarrier::just(&entry.carrier);
            owns.extend(
                entry
                    .consumed
                    .iter()
                    .map(|key| OwnedCarrier::attribute(key)),
            );
            owns.sort_unstable();
            owns.dedup();
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                owns,
                carrier: EmittedCarrier::Owned(entry.carrier),
                target: rule.target,
                value: entry.value,
            });
        }
        return out;
    }
    // Event carriers are read from a span's events, not its attributes; no declared rule needs
    // one yet, and probing the attribute map for an event name would silently match nothing.
    let Some((attribute, raw)) = resolve_attribute(&rule.read, ctx.span_attrs) else {
        return out;
    };
    if rule.require_non_empty && raw.is_empty() {
        return out;
    }
    if rule.require_non_blank && raw.trim().is_empty() {
        return out;
    }
    // An array-valued carrier read element by element, in declared passes.
    if let Some(elements) = &rule.elements {
        let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
            return out;
        };
        for (carrier, value, clause) in element_passes(&parsed, elements) {
            // An event carrier is kept as one: carrier semantics are looked up by kind, so reporting an
            // event as an attribute changes what the pipeline reads it as evidence of.
            let tagged = if elements.tags_are_events {
                EmittedCarrier::OwnedEvent(carrier)
            } else {
                EmittedCarrier::Owned(carrier)
            };
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &clause),
                // The array attribute is what was read; each element's tag is a name for one of its parts.
                owns: OwnedCarrier::just(attribute),
                carrier: tagged,
                target: rule.target,
                value,
            });
        }
        return out;
    }
    // A text carrier read as tagged sections, each emitted on its own.
    if let Some(sections) = &rule.sections {
        for (route, value) in sectioned(raw, sections) {
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[vec![route]]),
                carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
                owns: OwnedCarrier::just(attribute),
                target: rule.target,
                value,
            });
        }
        return out;
    }
    let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
        return out;
    };
    // A state object its nodes write into: the readings are applied at every node of a bounded walk, so a
    // conversation nested a level or two down is found without trawling the payload for anything
    // message-shaped.
    // The aggregate path passes **no** construction: its entries become one array and the rule's envelope
    // wraps that array, once. Compilation refuses a per-reading envelope beside an aggregate, so nothing can be
    // lost by not building here.
    let build = (!rule.aggregate_into_array).then_some(Construction {
        rule_wrap: rule.wrap.as_ref(),
        ctx,
        root: &parsed,
    });
    let readings = match &rule.walk {
        Some(walk) => walked_readings(&parsed, rule, walk, build),
        None => all_readings(&parsed, rule, build).built,
    };
    // A tool list is a set, not a sequence of messages: the whole list is one observation, and emitting one
    // per tool would make each look like a separate declaration.
    if rule.aggregate_into_array {
        if readings.is_empty() {
            return out;
        }
        // The aggregate is built from **every** reading, so its evidence is every clause that contributed -
        // which is why an emission carries a set rather than one path.
        let mut contributing: Vec<Vec<String>> = Vec::new();
        let mut values = Vec::new();
        for (value, _, path) in readings {
            if !contributing.contains(&path) {
                contributing.push(path);
            }
            values.push(value);
        }
        let assembled = JsonValue::Array(values);
        // **The rule's envelope wraps the assembled array, once.** It used to be discarded here while the
        // indexed-family aggregate applied it - so the same two declarations meant different things depending on
        // the read form. Per-reading envelopes are refused beside an aggregate, so this is the only one there
        // can be, and an envelope that cannot be built is no observation rather than a bare array under a
        // message tag.
        let value = match &rule.wrap {
            Some(wrap) => match wrapped(assembled, wrap, ctx, Some(&parsed)) {
                Some(built) => built,
                None => return out,
            },
            None => assembled,
        };
        out.push(Emission {
            rule_id: &rule.rule_id,
            evidence: rule_evidence(rule, &contributing),
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            owns: OwnedCarrier::just(attribute),
            target: rule.target,
            value,
        });
        return out;
    }
    // Already built: an envelope that could not be made was a reading that produced nothing, decided inside
    // the coalesce so the alternatives after it and the rule's `fallback` still got their turn.
    for (value, per_reading_target, clause) in readings {
        out.push(Emission {
            rule_id: &rule.rule_id,
            evidence: rule_evidence(rule, std::slice::from_ref(&clause)),
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            owns: OwnedCarrier::just(attribute),
            target: per_reading_target.unwrap_or(rule.target),
            value,
        });
    }

    out
}

/// Whether one predicate holds of a value.
/// One value predicate, reachable from the test that pins what `non_empty` means for a scalar.
#[cfg(test)]
pub(crate) fn predicate_holds_for_test(predicate: &ValuePredicate, value: &JsonValue) -> bool {
    predicate_holds(value, predicate)
}

fn predicate_holds(value: &JsonValue, predicate: &ValuePredicate) -> bool {
    // A path that can match more than once is asked **existentially**: some match satisfies the condition.
    // Answering only about the first is a silent narrowing, and it would disagree with `exists`, which on a
    // filter path already means "at least one".
    if let Some(path) = &predicate.path {
        let matches = query(value, path);
        if matches.len() > 1 {
            return matches
                .into_iter()
                .any(|subject| condition_holds(subject, predicate));
        }
    }
    let Some(subject) = (match &predicate.path {
        Some(path) => query(value, path).into_iter().next(),
        None => Some(value),
    }) else {
        // Absent. Only a predicate asserting absence is satisfied - or one asserting the value is not in a
        // set, which is true when there is no value, and is how a dialect's unnamed events fall through to
        // the reading that handles them.
        return predicate.exists == Some(false)
            || (!predicate.none_of.is_empty()
                && predicate.exists.is_none()
                && predicate.kind.is_none()
                && predicate.non_empty.is_none()
                && predicate.not_null.is_none()
                && predicate.identifier_like.is_none()
                && predicate.starts_with.is_none()
                && predicate.lacks_prefix.is_none()
                && predicate.one_of.is_empty());
    };
    condition_holds(subject, predicate)
}

/// The conditions a *present* value must satisfy.
fn condition_holds(subject: &JsonValue, predicate: &ValuePredicate) -> bool {
    if predicate.exists == Some(false) {
        return false;
    }
    if let Some(kind) = predicate.kind
        && !matches_kind(subject, kind)
    {
        return false;
    }
    if let Some(want_identifier) = predicate.identifier_like {
        let looks_like_one = subject
            .as_str()
            .is_some_and(|text| text.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        if looks_like_one != want_identifier {
            return false;
        }
    }
    if let Some(want_not_null) = predicate.not_null
        && subject.is_null() == want_not_null
    {
        return false;
    }
    if let Some(want_non_empty) = predicate.non_empty {
        let filled = match subject {
            JsonValue::String(text) => !text.is_empty(),
            JsonValue::Array(items) => !items.is_empty(),
            JsonValue::Object(map) => !map.is_empty(),
            // A scalar is neither empty **nor** non-empty in this vocabulary, so it fails the predicate
            // whichever way it was asked. `true` here made `{"path": "$.content", "non_empty": true}` hold
            // for `{"content": 0}` - a number reported as filled content - and the compiler's refusal does
            // not cover it, because the refusal is about the *declaration* and this is about the value.
            _ => return false,
        };
        if filled != want_non_empty {
            return false;
        }
    }
    if let Some(prefix) = &predicate.starts_with
        && !subject
            .as_str()
            .is_some_and(|text| text.starts_with(prefix.as_str()))
    {
        return false;
    }
    if let Some(prefix) = &predicate.lacks_prefix
        && subject
            .as_str()
            .is_some_and(|text| text.starts_with(prefix.as_str()))
    {
        return false;
    }
    if !predicate.one_of.is_empty()
        && !subject
            .as_str()
            .is_some_and(|text| predicate.one_of.iter().any(|want| want == text))
    {
        return false;
    }
    if !predicate.none_of.is_empty()
        && subject
            .as_str()
            .is_some_and(|text| predicate.none_of.iter().any(|reject| reject == text))
    {
        return false;
    }
    true
}

fn matches_kind(value: &JsonValue, kind: ValueKind) -> bool {
    match kind {
        ValueKind::Object => value.is_object(),
        ValueKind::Array => value.is_array(),
        ValueKind::String => value.is_string(),
        ValueKind::Number => value.is_number(),
        ValueKind::Bool => value.is_boolean(),
        ValueKind::Null => value.is_null(),
    }
}

/// Whether a predicate set holds of a value. An empty set holds.
pub(super) fn predicates_hold(value: &JsonValue, set: &PredicateSet) -> bool {
    (set.all.is_empty() || set.all.iter().all(|p| predicate_holds(value, p)))
        && (set.any.is_empty() || set.any.iter().any(|p| predicate_holds(value, p)))
}

/// The observations an array-valued carrier yields, pass by pass.
///
/// Each pass scans every element. That is the shape of the code being replaced and the order is
/// observable - one dialect emits every recognised event before any grouped block - so it is declared
/// rather than left to how a loop happens to be written.
/// Each element pass's observations, with **which clauses produced them**.
///
/// The path is the pass, and for a grouped pass the derived case whose predicate matched - both are required
/// declarations that this function used to discard, leaving two routes of one rule indistinguishable in a
/// diagnostic.
fn element_passes(
    parsed: &JsonValue,
    spec: &ElementsSpec,
) -> Vec<(String, JsonValue, Vec<Vec<String>>)> {
    let array = match &spec.select {
        Some(path) => query(parsed, path).into_iter().next(),
        None => Some(parsed),
    };
    let Some(items) = array.and_then(JsonValue::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for pass in &spec.passes {
        let matching = items
            .iter()
            .filter(|element| predicates_hold(element, &pass.when));

        match &pass.group {
            // Runs of consecutive elements sharing a derived key.
            Some(group) => {
                let mut run_key: Option<String> = None;
                let mut collected: Vec<JsonValue> = Vec::new();
                // The case that produced the run travels with it: a run is keyed by a *derived value*, and
                // several cases may derive the same one, so the key alone does not name the clause.
                let flush = |key: Option<String>,
                             cases: Vec<String>,
                             blocks: Vec<JsonValue>,
                             out: &mut Vec<_>| {
                    let Some(key) = key else { return };
                    if blocks.is_empty() {
                        return;
                    }
                    let Some(tag) = group.tag_by_key.get(&key) else {
                        return;
                    };
                    // One path per contributing case, each under this pass.
                    let paths: Vec<Vec<String>> = if cases.is_empty() {
                        vec![vec![pass.id.clone()]]
                    } else {
                        cases
                            .into_iter()
                            .map(|case| vec![pass.id.clone(), case])
                            .collect()
                    };
                    out.push((
                        tag.clone(),
                        json!({
                            group.key_as.clone(): key,
                            "content": JsonValue::Array(blocks),
                        }),
                        paths,
                    ));
                };
                // **Every** contributing case, not the first: two cases deriving one key are legitimate
                // aliases, so a run built from both has two witnesses and naming one of them claims it
                // produced blocks it did not match.
                let mut run_cases: Vec<String> = Vec::new();
                // **The whole array**, not the pass's matches: "consecutive" is a fact about the array a
                // producer wrote, and filtering first made it a fact about the filtered view. Logfire's shape -
                // two content blocks with a named assistant event between them - collapsed into one "run" of the
                // two, with a message lying between them that the run claims is not there. So an element that
                // fails the pass, derives no case, or has nothing to collect **ends** the current run.
                for element in items {
                    let Some(matched) = Some(element)
                        .filter(|element| predicates_hold(element, &pass.when))
                        .and_then(|element| {
                            group
                                .by
                                .iter()
                                .find(|case| predicates_hold(element, &case.when))
                        })
                    else {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        continue;
                    };
                    let key = matched.value.clone();
                    let Some(part) = query(element, &group.collect).into_iter().next() else {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        continue;
                    };
                    if run_key.as_ref() != Some(&key) {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        run_key = Some(key);
                    }
                    if !run_cases.iter().any(|seen| seen == &matched.id) {
                        run_cases.push(matched.id.clone());
                    }
                    collected.push(part.clone());
                }
                flush(run_key, run_cases, collected, &mut out);
            }
            // Each element emitted as it stands, tagged by what it carries.
            None => {
                let Some(tag_path) = &pass.tag_from else {
                    continue;
                };
                for element in matching {
                    let Some(tag) = query(element, tag_path)
                        .into_iter()
                        .next()
                        .and_then(JsonValue::as_str)
                    else {
                        continue;
                    };
                    out.push((
                        tag.to_string(),
                        element.clone(),
                        vec![vec![pass.id.clone()]],
                    ));
                }
            }
        }
    }
    out
}

/// Inline each reading's fragment reference, so the runtime holds cases rather than a name.
fn inline_fragments(
    readings: &[Alternative],
    fragments: &HashMap<String, Vec<Alternative>>,
) -> Result<Vec<CompiledReading>, MessageCompileError> {
    readings
        .iter()
        .map(|spec| {
            let mut fragment_cases = match &spec.then_fragment {
                Some(name) => match fragments.get(name) {
                    Some(cases) => cases.clone(),
                    None => {
                        return Err(MessageCompileError::UnknownFragment {
                            fragment: name.clone(),
                        });
                    }
                },
                None => Vec::new(),
            };
            // After the shared table, never before: the table is the dialect's own answer and this is one
            // place that accepts one more shape.
            fragment_cases.extend(spec.extra_cases.iter().cloned());
            // A case is a *leaf*. `Alternative` is one type, so it structurally permits a nested
            // `then_fragment` or `extra_cases` - and a leaf's own `fragment_cases` is forced empty when it
            // runs, so such a declaration is silently ignored. Refused rather than left to be discovered,
            // which is the same reason a fragment may not reference a fragment.
            if let Some(nested) = fragment_cases
                .iter()
                .find(|case| case.then_fragment.is_some() || !case.extra_cases.is_empty())
            {
                // Named by the offending *case*, not by the reading that holds it: an `extra_cases` entry on
                // an unnamed reading was reported as "a reading", which does not locate anything.
                return Err(MessageCompileError::Inexpressible {
                    rule: nested
                        .doc
                        .clone()
                        .or_else(|| spec.then_fragment.clone())
                        .unwrap_or_else(|| "an undocumented case".to_string()),
                    detail: "a fragment case or extra case is a leaf, so a `then_fragment` or \
                             `extra_cases` on it would be ignored",
                });
            }
            // **Traversal slots that the pipeline order makes dead.** Each of these compiled and did nothing,
            // which is the same defect as a construction branch accepting a sibling it returns before:
            //
            // | Declared | What happens |
            // | --- | --- |
            // | a lift `from: element` with no `descend` | the element *is* the candidate, so every member is already there |
            // | `then_any_of` beside `then_present_any_of` | presence wins and the yielding form is dead |
            // | `else_element` with neither | it names the fallback for a coalesce that is not there |
            //
            // The third is where the two forms differ: one coalesces by *yielding*, the other by *presence*.
            // That they are one member with two silently-ordered halves is Codex's finding 8 and stays open;
            // this refuses the combination that cannot mean anything.
            // A lift **from the element** copies members that sit beside the value being emitted, which is
            // only a different value when something was descended into: without `descend` the element *is* the
            // candidate, so every member is already there and the lift is a no-op.
            if spec.descend.is_none()
                && spec
                    .lift
                    .iter()
                    .any(|lift| lift.from == super::schema::LiftSource::Element)
            {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "lifts from the element with no `descend`, where the element is already the value \
                             being emitted - so every member is there and the lift copies nothing",
                });
            }
            if !spec.then_any_of.is_empty() && !spec.then_present_any_of.is_empty() {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "declares both `then_any_of` and `then_present_any_of`; presence wins, so the \
                             yielding form would be ignored - they coalesce by different questions",
                });
            }
            if spec.else_element
                && spec.then_any_of.is_empty()
                && spec.then_present_any_of.is_empty()
            {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "declares `else_element` with no `then_any_of` or `then_present_any_of` - it \
                             names the fallback for a coalesce that is not there",
                });
            }
            Ok(CompiledReading {
                spec: spec.clone(),
                fragment_cases,
            })
        })
        .collect()
}

/// The readings of every node of a bounded walk.
///
/// Bounded three ways, all declared: the depth, the members pruned because the node-level readings already
/// took them, and stopping below a node that was itself read as a message - a message's members are its
/// content, so descending into one would read its parts as turns.
fn walked_readings(
    root: &JsonValue,
    rule: &CompiledMessageRule,
    walk: &super::schema::WalkSpec,
    build: Option<Construction<'_>>,
) -> Vec<Reading> {
    let mut out = Vec::new();
    let mut stack = vec![(root, walk.max_depth)];
    while let Some((node, depth)) = stack.pop() {
        // **One pass, two answers.** Whether to descend is a question about the *payload's shape* - "this node
        // is the message, do not read its parts as turns" - while whether an envelope can be built is about the
        // declaration, so a clause whose construction failed still means the node is that shape. Deciding the
        // walk from the built result made construction failure widen the traversal, which the
        // carrier-subsequence invariant caught on `langgraph/image_gen`.
        //
        // It was two evaluations of the node for a while, which is quadratic work on a deep payload; `Selection`
        // returns both from one.
        //
        // And **which** clauses stop it is declared (`stop_on`), because "did anything get selected here" is a
        // wider question than "was this node a message": LangGraph's `also_3` selects every state member, so a
        // node holding a message *beside* more state counted as matched and its siblings were never visited.
        let here = all_readings(node, rule, build);
        let matched = here
            .recognised
            .iter()
            .any(|id| walk.stop_on.iter().any(|stop| stop == id));
        out.extend(here.built);
        if depth == 0 || matched {
            continue;
        }
        match node {
            JsonValue::Object(map) => {
                // A fixed order: the payload's own map order is what a reader sees, and the stack pops in
                // reverse, so members are pushed reversed to visit them as written.
                let members: Vec<&JsonValue> = map
                    .iter()
                    .filter(|(key, value)| {
                        !walk.prune.iter().any(|pruned| pruned == *key)
                            && (value.is_object() || value.is_array())
                    })
                    .map(|(_, value)| value)
                    .collect();
                for value in members.into_iter().rev() {
                    stack.push((value, depth - 1));
                }
            }
            JsonValue::Array(items) => {
                for value in items.iter().rev().filter(|v| v.is_object() || v.is_array()) {
                    stack.push((value, depth - 1));
                }
            }
            _ => {}
        }
    }
    out
}
