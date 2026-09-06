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
//! What it deliberately does **not** yet cover: the extractors that genuinely transform - an indexed
//! attribute family, a state tree walked to a bounded depth, a serialisation grammar that is not JSON, a
//! tagged text section. Those need primitives the vocabulary does not have, and inventing them from one
//! example is how a "generic" operation ends up being one producer's policy under another name. They
//! stay in Rust, counted, until the primitive that covers each is designed on its own evidence.
//!
//! The engine emits *values*, not `RawMessage`s: the ingestion types live in `domain::traces`, and the
//! engine having to know them would point the dependency the wrong way for no benefit.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

use super::schema::{
    Alternative, AttachSpec, BlockSpec, DetectMatch, EmitTarget, MemberPresence,
    MemberRequirements, MessageRule, ParseMode, ReadSpec, RuleFile, ShapeRequirement, WrapSpec,
};

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
}

impl EmittedCarrier<'_> {
    /// The carrier's name, whichever form it took.
    pub fn name(&self) -> &str {
        match self {
            Self::Attribute(name) | Self::Event(name) => name,
            Self::Owned(name) => name.as_str(),
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
    pub rule_file: String,
    pub rule_id: String,
    pub doc: Option<String>,
    pub read: ReadSpec,
    pub parse: Option<ParseMode>,
    pub require_members: Option<MemberRequirements>,
    pub wrap: Option<WrapSpec>,
    pub target: EmitTarget,
    pub when: Option<DetectMatch>,
    pub unless: Option<DetectMatch>,
    pub require_non_empty: bool,
    pub reads_tool_spans: bool,
    pub alternatives: Vec<Alternative>,
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

/// Compile every asset's message rules into one plan.
pub fn compile(sources: &BTreeMap<String, Vec<u8>>) -> Result<MessagePlan, MessageCompileError> {
    let mut rules: Vec<CompiledMessageRule> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();

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
            let MessageRule {
                id,
                doc,
                read,
                parse,
                wrap,
                emit,
                require_members,
                require_non_empty,
                reads_tool_spans,
                unless,
                when,
                alternatives,
                legacy_rank,
            } = rule;
            if seen_ids.insert(id.clone(), ()).is_some() {
                return Err(MessageCompileError::DuplicateRuleId { rule: id.clone() });
            }
            if read.named_count() != 1 {
                return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
            }
            let carriers = declared_carriers(read);
            if carriers.is_empty() || carriers.iter().any(|name| name.is_empty()) {
                return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
            }
            rules.push(CompiledMessageRule {
                rule_file: file.id.clone(),
                rule_id: id.clone(),
                doc: doc.clone(),
                read: read.clone(),
                parse: *parse,
                require_members: require_members.clone(),
                wrap: wrap.clone(),
                target: *emit,
                when: when.clone(),
                unless: unless.clone(),
                require_non_empty: *require_non_empty,
                reads_tool_spans: *reads_tool_spans,
                alternatives: alternatives.clone(),
                legacy_rank: *legacy_rank,
            });
        }
    }

    rules.sort_by(|a, b| {
        a.legacy_rank
            .cmp(&b.legacy_rank)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    // No two rules may read one carrier. Unlike detection, this is not a precedence question: the
    // ingestion claims a carrier once, so a second rule reading it is dead weight that looks live.
    for (i, a) in rules.iter().enumerate() {
        for b in &rules[i + 1..] {
            let shared = declared_carriers(&a.read)
                .into_iter()
                .find(|name| declared_carriers(&b.read).contains(name));
            if same_kind(&a.read, &b.read)
                && let Some(carrier) = shared
            {
                return Err(MessageCompileError::ContestedCarrier {
                    first: a.rule_id.clone(),
                    second: b.rule_id.clone(),
                    carrier: carrier.to_string(),
                });
            }
        }
    }

    Ok(MessagePlan { rules })
}

/// Every carrier name a rule may tag an observation with.
///
/// Total over the read forms on purpose. A function returning "the" carrier name had to pick one, and
/// each time a read form was added it silently returned nothing for it - reported as an empty carrier
/// twice, once for indexed families and once for ordered alternatives. A list forces every form to be
/// answered, and it is also the right input to the contested-carrier check, since a rule offering two
/// spellings can contest either of them.
fn declared_carriers(read: &ReadSpec) -> Vec<&str> {
    let mut names = Vec::new();
    if let Some(attribute) = read.attribute.as_deref() {
        names.push(attribute);
    }
    if let Some(event) = read.event.as_deref() {
        names.push(event);
    }
    if let Some(family) = read.indexed_family.as_deref() {
        names.push(family);
    }
    names.extend(read.attribute_any_of.iter().map(String::as_str));
    names
}

/// Whether two rules read the same carrier in the same way.
fn same_kind(a: &ReadSpec, b: &ReadSpec) -> bool {
    let attribute_like = |r: &ReadSpec| r.attribute.is_some() || !r.attribute_any_of.is_empty();
    (attribute_like(a) && attribute_like(b))
        || (a.event.is_some() && b.event.is_some())
        || (a.indexed_family.is_some() && b.indexed_family.is_some())
}

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        for rule in &self.rules {
            if !gates_allow(rule, ctx) {
                continue;
            }
            if let Some(family) = rule.read.indexed_family.as_deref() {
                for (carrier, value) in indexed_entries(
                    ctx.span_attrs,
                    family,
                    rule.read.entry_member.as_deref(),
                    rule.require_members.as_ref(),
                ) {
                    out.push(Emission {
                        rule_id: &rule.rule_id,
                        carrier: EmittedCarrier::Owned(carrier),
                        target: rule.target,
                        value,
                    });
                }
                continue;
            }
            // Event carriers are read from a span's events, not its attributes; no declared rule needs
            // one yet, and probing the attribute map for an event name would silently match nothing.
            let Some((attribute, raw)) = resolve_attribute(&rule.read, ctx.span_attrs) else {
                continue;
            };
            if rule.require_non_empty && raw.is_empty() {
                continue;
            }
            let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
                continue;
            };
            for value in readings(&parsed, &rule.alternatives) {
                let value = match &rule.wrap {
                    Some(wrap) => wrapped(value, wrap, ctx),
                    None => value,
                };
                out.push(Emission {
                    rule_id: &rule.rule_id,
                    carrier: EmittedCarrier::Attribute(attribute),
                    target: rule.target,
                    value,
                });
            }
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

/// Whether a value has the shape a rule requires.
fn shape_holds(value: &JsonValue, requirement: Option<&ShapeRequirement>) -> bool {
    let Some(req) = requirement else {
        return true;
    };
    let present = |member: &String| value.get(member.as_str()).is_some();
    (req.all_of.is_empty() || req.all_of.iter().all(present))
        && (req.any_of.is_empty() || req.any_of.iter().any(present))
}

/// The observations one payload yields, under the first alternative that produces any.
///
/// An ordered coalesce over documented shapes. With no alternatives the payload is emitted as it stands,
/// which is what a carrier holding exactly one message needs. "The first that produces any" is the whole
/// control flow, and it is deliberately all there is: a shape that yields nothing is not an error, it is
/// evidence the payload is in a different one of its documented forms.
fn readings(parsed: &JsonValue, alternatives: &[Alternative]) -> Vec<JsonValue> {
    if alternatives.is_empty() {
        return vec![parsed.clone()];
    }
    for alternative in alternatives {
        let selected = match &alternative.select {
            Some(member) => match parsed.get(member.as_str()) {
                Some(value) => value,
                None => continue,
            },
            None => parsed,
        };
        let elements: Vec<&JsonValue> = if alternative.each {
            match selected.as_array() {
                Some(items) => items.iter().collect(),
                // Declared as a list and is not one: this is not the shape, so try the next.
                None => continue,
            }
        } else {
            vec![selected]
        };

        let mut produced = Vec::new();
        for element in elements {
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
                None => element.clone(),
            };
            if shape_holds(&candidate, alternative.require.as_ref()) {
                produced.push(candidate);
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
) -> Vec<(String, JsonValue)> {
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
        // The subject's own members, unprefixed.
        let subject_dot = format!("{subject_prefix}.");
        for (key, value) in attrs {
            if let Some(member) = key.strip_prefix(subject_dot.as_str()) {
                object.insert(member.to_string(), sniffed_value(value));
            }
        }
        // Where the message is nested, the entry's *other* members come too: they belong to the same
        // observation, and the sub-level's own keys are already in, so they are skipped here.
        if let Some(nested_member) = entry_member {
            let entry_dot = format!("{entry_prefix}.");
            let skip = format!("{nested_member}.");
            for (key, value) in attrs {
                if let Some(member) = key.strip_prefix(entry_dot.as_str())
                    && !member.starts_with(skip.as_str())
                {
                    object.insert(member.to_string(), sniffed_value(value));
                }
            }
        }
        out.push((subject_prefix, JsonValue::Object(object)));
    }
    out
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
    if ctx.is_tool_span && !rule.reads_tool_spans {
        return false;
    }
    if let Some(gate) = &rule.when
        && !super::detect_rules::signals_hold(gate, ctx.span_name, ctx.span_attrs)
    {
        return false;
    }
    if let Some(gate) = &rule.unless
        && super::detect_rules::signals_hold(gate, ctx.span_name, ctx.span_attrs)
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
fn wrapped(value: JsonValue, wrap: &WrapSpec, ctx: &MessageContext<'_>) -> JsonValue {
    // A content block, when the carrier holds one part of a block rather than a whole message.
    let value = match &wrap.block {
        Some(block) => JsonValue::Array(vec![built_block(value, block, ctx)]),
        None => value,
    };
    let mut object = serde_json::Map::new();
    object.insert("role".to_string(), json!(wrap.role));
    for (member, literal) in &wrap.members {
        object.insert(member.clone(), literal.clone());
    }
    let content_member = wrap.content_as.as_deref().unwrap_or("content");
    // Before the content member, then the content, then after - because the order is observable: see
    // `AttachSpec::after_content`.
    for attach in wrap.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    object.insert(content_member.to_string(), value);
    for attach in wrap.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    JsonValue::Object(object)
}

/// The block a rule builds around its read value.
fn built_block(value: JsonValue, block: &BlockSpec, ctx: &MessageContext<'_>) -> JsonValue {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(block.block_type));
    for attach in block.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    object.insert(
        block.content_as.as_deref().unwrap_or("content").to_string(),
        value,
    );
    for attach in block.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    JsonValue::Object(object)
}

/// One attachment's value, or `None` where nothing supplied one.
fn attached_value(attach: &AttachSpec, ctx: &MessageContext<'_>) -> Option<JsonValue> {
    if let Some(raw) = ctx.span_attrs.get(&attach.from) {
        if let Some(expected) = &attach.when_equals {
            if raw != expected {
                return None;
            }
            // A flag: the literal is the point, not the string that proved it.
            return Some(attach.value.clone().unwrap_or(json!(true)));
        }
        return parse_value(raw, attach.parse.unwrap_or(ParseMode::Text));
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
