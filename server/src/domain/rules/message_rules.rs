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
//! What it does **not** cover, as of now: a state tree walked to a bounded depth (LangGraph), a decision
//! table over ~13 message types (AutoGen), a positional join between an indexed family and a serialised
//! state member (OpenInference's multimodal enrichment), and a serialisation grammar that is not JSON
//! (CrewAI's tool definitions, still read in Rust even though its *messages* are declared). Indexed
//! families and tagged text sections **are** covered - they were on this list and are not any more.
//!
//! The vocabulary here has also grown past what one module should own, which is the acknowledged reason
//! for the next step: selection, projection, predicates and object construction move to **JMESPath**, a
//! published spec with an existing parser, leaving this module the structural algebra that JMESPath
//! cannot express - claiming, precedence, bounded traversal, positional joins, grouping and non-JSON
//! parsing. See `server/docs/framework-rules-engine.md`.
//!
//! The engine emits *values*, not `RawMessage`s: the ingestion types live in `domain::traces`, and the
//! engine having to know them would point the dependency the wrong way for no benefit.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

use super::detect_rules::CompiledDetect;
use super::schema::{
    Alternative, AttachSpec, BlockSpec, ComposeMember, ComposeSpec, ElementsSpec, EmitTarget,
    MemberPresence, MemberRequirements, MessageRule, OverlaySpec, ParseMode, PredicateSet,
    ReadSpec, RuleFile, SectionsSpec, SingleToolCallSpec, ToolCallsSpec, ToolReprSpec, ValueKind,
    ValuePredicate, WrapSpec,
};

/// One reading of a payload: the value, an envelope for this reading alone, and its target where it
/// differs from the rule's.
type Reading = (JsonValue, Option<WrapSpec>, Option<EmitTarget>);

/// What an ingestion knows when it asks which carriers to read.
#[derive(Debug, Clone, Copy)]
pub struct MessageContext<'a> {
    pub span_name: &'a str,
    pub span_attrs: &'a HashMap<String, String>,
    /// Whether this is a tool execution span, which only some rules may read.
    pub is_tool_span: bool,
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

/// One observation a rule produced.
#[derive(Debug, Clone)]
pub struct Emission<'a> {
    /// The clause that produced it, for the explain trace.
    pub rule_id: &'a str,
    pub carrier: EmittedCarrier<'a>,
    pub target: EmitTarget,
    pub value: JsonValue,
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
    rules: Vec<CompiledMessageRule>,
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
    if compose.is_none() && branch_set.is_none() && read.named_count() != 1 {
        return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
    }
    // Every combination the runner would silently ignore is refused here instead. Each of these
    // was accepted and did nothing, which reads as a rule that works.
    let inexpressible = |detail: &'static str| MessageCompileError::Inexpressible {
        rule: id.clone(),
        detail,
    };
    for (label, gate) in [("when", when.as_ref()), ("unless", unless.as_ref())] {
        if let Some(gate) = gate
            && super::detect_rules::unavailable_gate_dimension(gate).is_some()
        {
            return Err(MessageCompileError::Inexpressible {
                rule: id.clone(),
                detail: if label == "when" {
                    "`when` uses a resource dimension, and a message gate is given no resource \
                         attributes - it could never hold"
                } else {
                    "`unless` uses a resource dimension, and a message gate is given no resource \
                         attributes - it could never hold"
                },
            });
        }
    }
    if read.event.is_some() {
        // Accepted by the schema and never executed: events are read from a span's events, and the
        // runner only ever probes the attribute map. Refused until it is implemented.
        return Err(inexpressible(
            "`read.event` is not implemented - events are not routed through the plan",
        ));
    }
    if compose.is_some()
        && (wrap.is_some()
            || sections.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || parse.is_some())
    {
        return Err(inexpressible(
            "`compose` builds the whole message, so `wrap`, `sections`, `alternatives` and \
                 `parse` would be ignored",
        ));
    }
    // A wrap is meaningful on an *aggregated* family: the entries become one array, and one array needs an
    // envelope saying what it is - a result set is one observation, not one message per document.
    if read.indexed_family.is_some()
        && ((wrap.is_some() && !aggregate_into_array)
            || !alternatives.is_empty()
            || !also.is_empty())
    {
        return Err(inexpressible(
            "an indexed family assembles each entry itself, so `wrap` and `alternatives` would \
                 be ignored",
        ));
    }
    if sections.is_some() && (wrap.is_some() || !alternatives.is_empty() || !also.is_empty()) {
        return Err(inexpressible(
            "`sections` builds each section's message, so `wrap` and `alternatives` would be \
                 ignored",
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
    // A predicate that cannot hold, or asserts nothing, is refused like any other no-op.
    let check_predicates = |set: &PredicateSet| -> Option<&'static str> {
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
                    || predicate.starts_with.is_some()
                    || predicate.lacks_prefix.is_some())
            {
                return Some(
                    "`exists: false` asserts the member is absent, so no other condition on it \
                         can hold",
                );
            }
            if predicate.starts_with.is_some() && predicate.lacks_prefix.is_some() {
                return Some("`starts_with` and `lacks_prefix` on one predicate");
            }
        }
        None
    };
    for alternative in alternatives.iter().chain(also).chain(fallback) {
        if let Some(detail) = check_predicates(&alternative.require) {
            return Err(inexpressible(detail));
        }
    }
    // `alternatives` and `also` may coexist: the first list is the ordered question "which shape is
    // this", the second is "and read this as well, always". One carrier really does need both - a dialect's
    // response member holds the reply *and* the inner turns that produced it - and refusing the pair forced
    // that into two rules claiming one carrier, which the ownership check rightly refuses.
    if let Some(sections) = sections {
        for route in &sections.routes {
            if let Some(detail) = check_predicates(&route.skip_when) {
                return Err(inexpressible(detail));
            }
        }
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
            read.event.as_deref(),
            read.indexed_family.as_deref(),
            tag_as.as_deref(),
        ];
        if named.iter().flatten().any(|name| name.is_empty())
            || read.attribute_any_of.iter().any(String::is_empty)
        {
            return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
        }
    }
    let compiled_branch_set = match branch_set {
        Some(set) => {
            let compile_group =
                    |group: &Vec<MessageRule>| -> Result<Vec<CompiledMessageRule>, MessageCompileError> {
                        group
                                .iter()
                                .map(|sub| compile_rule(file_id, sub, fragments))
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
        target: *emit,
        aggregate_into_array: *aggregate_into_array,
        when: when.as_ref().map(super::detect_rules::compile_signals),
        unless: unless.as_ref().map(super::detect_rules::compile_signals),
        require_non_empty: *require_non_empty,
        require_non_blank: *require_non_blank,
        branch_set: compiled_branch_set,
        elements: elements.clone(),
        walk: walk.clone(),
        sections: sections.clone(),
        reads_tool_spans: *reads_tool_spans,
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
    // makes a shared dialect table shared.
    let mut fragments: HashMap<String, Vec<Alternative>> = HashMap::new();
    for (path, bytes) in sources {
        let file: RuleFile =
            serde_json::from_slice(bytes).map_err(|e| MessageCompileError::Parse {
                path: path.clone(),
                message: e.to_string(),
            })?;
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
                    detail: "a fragment's own cases may not reference a fragment - one level, so there is \
                             nothing to bound at runtime",
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
            if seen_ids.insert(rule.id.clone(), ()).is_some() {
                return Err(MessageCompileError::DuplicateRuleId {
                    rule: rule.id.clone(),
                });
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

    rules.sort_by(|a, b| {
        a.legacy_rank
            .cmp(&b.legacy_rank)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    // Two rules must not claim one carrier, in either direction.
    //
    // Previously this compared "the carrier a rule reads" and missed four real conflicts: a `tag_as` that
    // emits a name the rule never read, an indexed family against an exact key it generates, a `compose`
    // that consumes a carrier another rule emits, and a sweep overlapping an exact source. Comparing the
    // *consumed* and *emitted* sets asks the question that matters - who owns this carrier - rather than
    // one convenient projection of it.
    for (i, a) in rules.iter().enumerate() {
        for b in &rules[i + 1..] {
            let conflict = [
                (consumed_carriers(a), consumed_carriers(b), "both read"),
                (emitted_carriers(a), emitted_carriers(b), "both emit"),
                (
                    emitted_carriers(a),
                    consumed_carriers(b),
                    "one emits what the other reads",
                ),
                (
                    consumed_carriers(a),
                    emitted_carriers(b),
                    "one reads what the other emits",
                ),
            ]
            .into_iter()
            .find_map(|(left, right, how)| {
                left.iter()
                    .find_map(|l| right.iter().find(|r| l.overlaps(r)).map(|_| l.describe()))
                    .map(|carrier| (carrier, how))
            });
            // A conditional claim is not a dead rule: it yields on spans its condition excludes, and the
            // ranks decide which is tried first. Only two unconditional rules on one carrier are a defect.
            if claim_is_conditional(a) || claim_is_conditional(b) {
                continue;
            }
            if let Some((carrier, how)) = conflict {
                return Err(MessageCompileError::ContestedCarrier {
                    first: a.rule_id.clone(),
                    second: b.rule_id.clone(),
                    carrier: format!("{carrier} ({how})"),
                });
            }
        }
    }

    Ok(MessagePlan { rules })
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
fn claim_is_conditional(rule: &CompiledMessageRule) -> bool {
    if rule.when.is_some() || rule.unless.is_some() {
        return true;
    }
    // A compose member's fallback is read only where its own gate holds.
    rule.compose.as_ref().is_some_and(|compose| {
        compose
            .members
            .iter()
            .any(|member| member.fallback_gate.is_some())
    })
}

/// What a rule reads.
fn consumed_carriers(rule: &CompiledMessageRule) -> Vec<CarrierPattern> {
    let mut out = Vec::new();
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(CarrierPattern::Exact(attribute.to_string()));
    }
    if let Some(event) = rule.read.event.as_deref() {
        out.push(CarrierPattern::Exact(event.to_string()));
    }
    out.extend(
        rule.read
            .attribute_any_of
            .iter()
            .map(|k| CarrierPattern::Exact(k.clone())),
    );
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // Every key beneath the family, since each index's members are read.
        out.push(CarrierPattern::Prefix(format!("{family}.")));
    }
    if let Some(compose) = &rule.compose {
        for member in compose.members.iter().map(|m| &m.spec) {
            out.extend(
                member
                    .from_any_of
                    .iter()
                    .map(|k| CarrierPattern::Exact(k.clone())),
            );
            if let Some(fallback) = &member.fallback {
                out.push(CarrierPattern::Exact(fallback.from.clone()));
            }
            if let Some(prefix) = &member.sweep_prefix {
                out.push(CarrierPattern::Prefix(prefix.clone()));
            }
        }
    }
    out
}

/// What a rule tags its observations with.
fn emitted_carriers(rule: &CompiledMessageRule) -> Vec<CarrierPattern> {
    if let Some(tag) = &rule.tag_as {
        // Overrides every read form: whatever was read, this is the tag.
        return vec![CarrierPattern::Exact(tag.clone())];
    }
    if let Some(compose) = &rule.compose {
        return vec![CarrierPattern::Exact(compose.tag.clone())];
    }
    let mut out = Vec::new();
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(CarrierPattern::Exact(attribute.to_string()));
    }
    if let Some(event) = rule.read.event.as_deref() {
        out.push(CarrierPattern::Exact(event.to_string()));
    }
    out.extend(
        rule.read
            .attribute_any_of
            .iter()
            .map(|k| CarrierPattern::Exact(k.clone())),
    );
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // One tag per index, and per sub-level where there is one - a prefix covers them all.
        out.push(CarrierPattern::Prefix(format!("{family}.")));
    }
    out
}

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    /// Tool definitions this span declares, from every rule that reads a `repr` grammar.
    ///
    /// Separate from `run`, and deliberately not subject to carrier claiming: a tool *definition* is not a
    /// message, and the tool-definition path has always run on every span. A framework may state its tools
    /// on the same carrier another rule reads as a conversation, and both statements are true.
    pub fn tool_definitions<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        self.rules
            .iter()
            .filter(|rule| rule.tool_repr.is_some())
            .flat_map(|rule| emit_rule(rule, ctx))
            .collect()
    }

    /// Every carrier this span carries that a rule reads, read by **one** rule each.
    ///
    /// The first rule to produce from a carrier owns it, and the ranks decide who is first. That is not a
    /// tie-break bolted on: it is what one extractor claiming a carrier meant, and consolidating sixteen
    /// extractors into one entry would otherwise have turned "the earlier one won" into "both emit". Two
    /// dialects really do read the same key - `message` is read by two - and the ownership check permits
    /// the collision precisely because a condition and a rank separate them.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rule in &self.rules {
            // A `repr` grammar declares tool definitions, which `tool_definitions` reads on every span.
            if rule.tool_repr.is_some() {
                continue;
            }
            // A branch set is several readings with a local order between them: the primaries, then the
            // fallbacks only if those found nothing, then the unconditional ones. No rule ids, no shared
            // state - the order lives inside this rule.
            if let Some(set) = &rule.branch_set {
                if !gates_allow(rule, ctx) {
                    continue;
                }
                let mut produced = Vec::new();
                for sub in &set.primary {
                    produced.extend(emit_rule(sub, ctx));
                }
                if produced.is_empty() {
                    for sub in &set.fallback {
                        produced.extend(emit_rule(sub, ctx));
                    }
                }
                for sub in &set.always {
                    produced.extend(emit_rule(sub, ctx));
                }
                keep_unclaimed(produced, &mut claimed, &mut out);
                continue;
            }
            keep_unclaimed(emit_rule(rule, ctx), &mut claimed, &mut out);
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

/// Keep one rule's emissions, unless a rule before it already owns their carrier.
///
/// Per *rule*, not per emission: one rule legitimately emits many observations from one carrier - a list of
/// turns is one carrier and many messages - so a rule that owns a carrier keeps everything it read from it,
/// and the next rule reading that carrier keeps nothing.
fn keep_unclaimed<'p>(
    produced: Vec<Emission<'p>>,
    claimed: &mut std::collections::HashSet<String>,
    out: &mut Vec<Emission<'p>>,
) {
    let mut mine: std::collections::HashSet<String> = std::collections::HashSet::new();
    for emission in produced {
        let carrier = emission.carrier.name();
        if claimed.contains(carrier) {
            continue;
        }
        mine.insert(carrier.to_string());
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
fn all_readings(parsed: &JsonValue, rule: &CompiledMessageRule) -> Vec<Reading> {
    let mut out = Vec::new();
    if !rule.alternatives.is_empty() {
        out.extend(readings(parsed, &rule.alternatives));
    }
    for alternative in &rule.also {
        out.extend(readings(parsed, std::slice::from_ref(alternative)));
    }
    if out.is_empty() {
        if rule.alternatives.is_empty() && rule.also.is_empty() && rule.fallback.is_empty() {
            // No readings declared at all: the payload is the observation.
            return vec![(parsed.clone(), None, None)];
        }
        for alternative in &rule.fallback {
            out.extend(readings(parsed, std::slice::from_ref(alternative)));
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
fn readings(parsed: &JsonValue, alternatives: &[CompiledReading]) -> Vec<Reading> {
    if alternatives.is_empty() {
        return vec![(parsed.clone(), None, None)];
    }
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
            let element: Vec<&JsonValue> = if alternative.then_any_of.is_empty() {
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
                // Descend, carrying down the members the rule says belong with the message.
                let candidate = match &alternative.descend {
                    Some(member) => {
                        let Some(inner) = element.get(member.as_str()) else {
                            continue;
                        };
                        let mut carried = inner.clone();
                        if let Some(object) = carried.as_object_mut() {
                            for lifted in &alternative.lift {
                                if let Some(value) = element.get(lifted.as_str()) {
                                    object.insert(lifted.clone(), value.clone());
                                }
                            }
                        }
                        carried
                    }
                    None => {
                        let mut candidate = element.clone();
                        // The enclosing value as a fallback: inserted only where the element is silent, so
                        // a result that carries its own call id keeps it.
                        if !alternative.lift_from_parent.is_empty()
                            && let Some(object) = candidate.as_object_mut()
                        {
                            for lifted in &alternative.lift_from_parent {
                                if object.contains_key(lifted.as_str()) {
                                    continue;
                                }
                                if let Some(value) = parsed.get(lifted.as_str()) {
                                    object.insert(lifted.clone(), value.clone());
                                }
                            }
                        }
                        candidate
                    }
                };
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
                    produced.extend(readings(&candidate, &cases));
                    continue;
                }
                produced.push((candidate, alternative.wrap.clone(), alternative.emit));
            }
        }
        if !produced.is_empty() {
            return produced;
        }
    }
    Vec::new()
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
    entry_member: Option<&str>,
    require: Option<&MemberRequirements>,
    numeric: &[String],
    overlay: Option<&OverlaySpec>,
) -> Vec<(String, JsonValue)> {
    // Parsed once for the whole family: the counterpart list describes every entry, so parsing it per
    // entry would re-parse one payload as many times as there are messages.
    let counterparts = overlay.and_then(|overlay| counterpart_list(attrs, overlay));
    let mut indices: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let family_dot = format!("{family}.");
    for key in attrs.keys() {
        if let Some(rest) = key.strip_prefix(&family_dot)
            && let Some(index) = rest.split('.').next()
            && let Ok(parsed) = index.parse::<usize>()
        {
            indices.insert(parsed);
        }
    }

    let mut out = Vec::new();
    for index in indices {
        let entry_prefix = format!("{family}.{index}");
        // Where the message sits: the entry itself, or a sub-level of it.
        let subject_prefix = match entry_member {
            Some(member) => format!("{entry_prefix}.{member}"),
            None => entry_prefix.clone(),
        };

        // An index exists as soon as any key mentions it, and a family holds keys that are not messages.
        if let Some(require) = require
            && !members_present(attrs, &subject_prefix, require)
        {
            continue;
        }

        let mut object = serde_json::Map::new();
        // The subject's own members, unprefixed - sorted, because the attribute map's order is randomised
        // per process and this object is persisted with its insertion order.
        let subject_dot = format!("{subject_prefix}.");
        let mut own: Vec<(&str, &String)> = attrs
            .iter()
            .filter_map(|(key, value)| key.strip_prefix(subject_dot.as_str()).map(|m| (m, value)))
            .collect();
        own.sort_unstable_by_key(|(member, _)| *member);
        for (member, value) in own {
            object.insert(member.to_string(), member_value(member, value, numeric));
        }
        // Where the message is nested, the entry's *other* members come too: they belong to the same
        // observation, and the sub-level's own keys are already in, so they are skipped here.
        if let Some(nested_member) = entry_member {
            let entry_dot = format!("{entry_prefix}.");
            let skip = format!("{nested_member}.");
            let mut siblings: Vec<(&str, &String)> = attrs
                .iter()
                .filter_map(|(key, value)| {
                    key.strip_prefix(entry_dot.as_str())
                        .filter(|member| !member.starts_with(skip.as_str()))
                        .map(|member| (member, value))
                })
                .collect();
            siblings.sort_unstable_by_key(|(member, _)| *member);
            for (member, value) in siblings {
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
            object.insert(overlay.as_member.clone(), content);
        }
        out.push((subject_prefix, JsonValue::Object(object)));
    }
    out
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
    // The tool-span gate asks whether a rule may read such a span **as a conversation**: its messages are
    // that tool's input and result rather than a model's turn. That is not a question about tool
    // *definitions*, and the path reading those has always run on every span - so a `repr` grammar is not
    // subject to it, or a framework's tool list vanishes on any span carrying a tool-execution signal.
    if ctx.is_tool_span && !rule.reads_tool_spans && rule.tool_repr.is_none() {
        return false;
    }
    if let Some(gate) = &rule.when
        && !super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.span_attrs)
    {
        return false;
    }
    if let Some(gate) = &rule.unless
        && super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.span_attrs)
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
        .chain(rule.read.attribute_any_of.iter().map(String::as_str))
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
    read.attribute_any_of
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
    let content_paths: Vec<&serde_json_path::JsonPath> = wrap
        .content_from
        .as_ref()
        .into_iter()
        .chain(wrap.content_from_any_of.iter())
        .collect();
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
                JsonValue::Array(canonical_tool_calls(subject, spec)),
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
fn canonical_tool_calls(subject: Option<&JsonValue>, spec: &ToolCallsSpec) -> Vec<JsonValue> {
    let Some(subject) = subject else {
        return Vec::new();
    };
    query(subject, &spec.select)
        .into_iter()
        .filter_map(|call| {
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
        })
        .collect()
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
        let found = payload.and_then(|payload| query(payload, path).into_iter().next())?;
        let value = match (attach.lowercase, found.as_str()) {
            (true, Some(text)) => json!(text.to_lowercase()),
            _ => found.clone(),
        };
        return Some(value);
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

/// Whether the members a rule requires are present under a prefix.
fn members_present(
    attrs: &HashMap<String, String>,
    prefix: &str,
    require: &MemberRequirements,
) -> bool {
    let present = |requirement: &super::schema::MemberRequirement| {
        let exact = format!("{prefix}.{}", requirement.name);
        let nested = format!("{exact}.");
        match requirement.presence {
            MemberPresence::Exact => attrs.contains_key(&exact),
            MemberPresence::Nested => attrs.keys().any(|k| k.starts_with(nested.as_str())),
            MemberPresence::Either => {
                attrs.contains_key(&exact) || attrs.keys().any(|k| k.starts_with(nested.as_str()))
            }
        }
    };
    (require.all_of.is_empty() || require.all_of.iter().all(present))
        && (require.any_of.is_empty() || require.any_of.iter().any(present))
}

/// Assemble a composed message, or `None` where the span supplied no member.
///
/// Emitted only when at least one *source* member was filled - the trailing literals are not evidence of
/// anything, so a rule whose sources all missed would otherwise emit a message consisting of a role.
fn composed(compose: &CompiledCompose, ctx: &MessageContext<'_>) -> Option<JsonValue> {
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
            .find_map(|key| attrs.get(key))
            .and_then(|raw| parse_value(raw, member.parse.unwrap_or(ParseMode::Text)));
        let value = direct.or_else(|| {
            // The conditional last resort: a key that is not this dialect's own, read only on evidence
            // that the span is one of its spans.
            let fallback = member.fallback.as_ref()?;
            let gate = compiled.fallback_gate.as_ref()?;
            if !super::detect_rules::compiled_signals_hold(gate, ctx.span_name, attrs) {
                return None;
            }
            let raw = attrs.get(&fallback.from)?;
            parse_value(raw, fallback.parse.unwrap_or(ParseMode::Text))
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
fn sectioned(raw: &str, spec: &SectionsSpec) -> Vec<JsonValue> {
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
        out.push(JsonValue::Object(message));
    }
    out
}

/// The observations one rule finds on a span.
///
/// Separate from the plan's loop so a branch set's sub-readings run through exactly the same path as a
/// top-level rule - a second evaluator would be a second place for the two to drift.
fn emit_rule<'p>(rule: &'p CompiledMessageRule, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
    let mut out = Vec::new();
    if !gates_allow(rule, ctx) {
        return out;
    }
    if let Some(compose) = &rule.compose {
        if let Some(value) = composed(compose, ctx) {
            out.push(Emission {
                rule_id: &rule.rule_id,
                carrier: EmittedCarrier::Attribute(compose.tag.as_str()),
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
                    carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
                    target: rule.target,
                    value: JsonValue::Array(tools),
                });
            }
        }
        return out;
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        let entries = indexed_entries(
            ctx.span_attrs,
            family,
            rule.read.entry_member.as_deref(),
            rule.require_members.as_ref(),
            &rule.read.numeric_members,
            rule.read.overlay.as_ref(),
        );
        // A result set is one observation. Its entries are the array, and the envelope says what the array
        // is - so the whole family is tagged once rather than one carrier per document.
        if rule.aggregate_into_array {
            if entries.is_empty() {
                return out;
            }
            let array = JsonValue::Array(entries.into_iter().map(|(_, value)| value).collect());
            let value = match &rule.wrap {
                Some(wrap) => match wrapped(array, wrap, ctx, None) {
                    Some(value) => value,
                    None => return out,
                },
                None => array,
            };
            out.push(Emission {
                rule_id: &rule.rule_id,
                carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(family)),
                target: rule.target,
                value,
            });
            return out;
        }
        for (carrier, value) in entries {
            out.push(Emission {
                rule_id: &rule.rule_id,
                carrier: EmittedCarrier::Owned(carrier),
                target: rule.target,
                value,
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
        for (carrier, value) in element_passes(&parsed, elements) {
            out.push(Emission {
                rule_id: &rule.rule_id,
                // An event carrier is kept as one: carrier semantics are looked up by kind, so reporting
                // an event as an attribute changes what the pipeline reads it as evidence of.
                carrier: if elements.tags_are_events {
                    EmittedCarrier::OwnedEvent(carrier)
                } else {
                    EmittedCarrier::Owned(carrier)
                },
                target: rule.target,
                value,
            });
        }
        return out;
    }
    // A text carrier read as tagged sections, each emitted on its own.
    if let Some(sections) = &rule.sections {
        for value in sectioned(raw, sections) {
            out.push(Emission {
                rule_id: &rule.rule_id,
                carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
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
    let readings = match &rule.walk {
        Some(walk) => walked_readings(&parsed, rule, walk),
        None => all_readings(&parsed, rule),
    };
    // A tool list is a set, not a sequence of messages: the whole list is one observation, and emitting one
    // per tool would make each look like a separate declaration.
    if rule.aggregate_into_array {
        if readings.is_empty() {
            return out;
        }
        out.push(Emission {
            rule_id: &rule.rule_id,
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            target: rule.target,
            value: JsonValue::Array(readings.into_iter().map(|(value, _, _)| value).collect()),
        });
        return out;
    }
    for (value, per_reading_wrap, per_reading_target) in readings {
        let wrap = per_reading_wrap.or(rule.wrap.clone());
        let value = match &wrap {
            Some(wrap) => match wrapped(value, wrap, ctx, Some(&parsed)) {
                Some(wrapped) => wrapped,
                None => continue,
            },
            None => value,
        };
        out.push(Emission {
            rule_id: &rule.rule_id,
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            target: per_reading_target.unwrap_or(rule.target),
            value,
        });
    }

    out
}

/// Whether one predicate holds of a value.
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
            // A scalar is neither empty nor non-empty; the compiler refuses the combination, so this arm
            // only guards a ruleset built in-process by a test.
            _ => true,
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
fn predicates_hold(value: &JsonValue, set: &PredicateSet) -> bool {
    (set.all.is_empty() || set.all.iter().all(|p| predicate_holds(value, p)))
        && (set.any.is_empty() || set.any.iter().any(|p| predicate_holds(value, p)))
}

/// The observations an array-valued carrier yields, pass by pass.
///
/// Each pass scans every element. That is the shape of the code being replaced and the order is
/// observable - one dialect emits every recognised event before any grouped block - so it is declared
/// rather than left to how a loop happens to be written.
fn element_passes(parsed: &JsonValue, spec: &ElementsSpec) -> Vec<(String, JsonValue)> {
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
                let flush = |key: Option<String>, blocks: Vec<JsonValue>, out: &mut Vec<_>| {
                    let Some(key) = key else { return };
                    if blocks.is_empty() {
                        return;
                    }
                    let Some(tag) = group.tag_by_key.get(&key) else {
                        return;
                    };
                    out.push((
                        tag.clone(),
                        json!({
                            group.key_as.clone(): key,
                            "content": JsonValue::Array(blocks),
                        }),
                    ));
                };
                for element in matching {
                    // The first matching case wins; an element matching none is not part of any run.
                    let Some(key) = group
                        .by
                        .iter()
                        .find(|case| predicates_hold(element, &case.when))
                        .map(|case| case.value.clone())
                    else {
                        continue;
                    };
                    let Some(part) = query(element, &group.collect).into_iter().next() else {
                        continue;
                    };
                    if run_key.as_ref() != Some(&key) {
                        flush(run_key.take(), std::mem::take(&mut collected), &mut out);
                        run_key = Some(key);
                    }
                    collected.push(part.clone());
                }
                flush(run_key, collected, &mut out);
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
                    out.push((tag.to_string(), element.clone()));
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
) -> Vec<Reading> {
    let mut out = Vec::new();
    let mut stack = vec![(root, walk.max_depth)];
    while let Some((node, depth)) = stack.pop() {
        let here = all_readings(node, rule);
        let matched = !here.is_empty();
        out.extend(here);
        if depth == 0 || (matched && walk.stop_at_match) {
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
