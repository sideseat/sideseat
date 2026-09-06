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
    Alternative, DetectMatch, EmitTarget, MessageRule, ParseMode, ReadSpec, RuleFile,
    ShapeRequirement,
};

/// What an ingestion knows when it asks which carriers to read.
#[derive(Debug, Clone, Copy)]
pub struct MessageContext<'a> {
    pub span_name: &'a str,
    pub span_attrs: &'a HashMap<String, String>,
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
    pub require_member: Option<String>,
    pub wrap_role: Option<String>,
    pub target: EmitTarget,
    pub when: Option<DetectMatch>,
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
                require_member,
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
            if carrier_name(read).is_empty() {
                return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
            }
            rules.push(CompiledMessageRule {
                rule_file: file.id.clone(),
                rule_id: id.clone(),
                doc: doc.clone(),
                read: read.clone(),
                parse: *parse,
                require_member: require_member.clone(),
                wrap_role: wrap.as_ref().map(|w| w.role.clone()),
                target: *emit,
                when: when.clone(),
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
            let same_kind = (a.read.attribute.is_some() && b.read.attribute.is_some())
                || (a.read.event.is_some() && b.read.event.is_some())
                || (a.read.indexed_family.is_some() && b.read.indexed_family.is_some());
            if same_kind && carrier_name(&a.read) == carrier_name(&b.read) {
                return Err(MessageCompileError::ContestedCarrier {
                    first: a.rule_id.clone(),
                    second: b.rule_id.clone(),
                    carrier: carrier_name(&a.read).to_string(),
                });
            }
        }
    }

    Ok(MessagePlan { rules })
}

fn carrier_name(read: &ReadSpec) -> &str {
    read.attribute
        .as_deref()
        .or(read.event.as_deref())
        .or(read.indexed_family.as_deref())
        .unwrap_or_default()
}

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        for rule in &self.rules {
            if let Some(family) = rule.read.indexed_family.as_deref() {
                if let Some(gate) = &rule.when
                    && !super::detect_rules::signals_hold(gate, ctx.span_name, ctx.span_attrs)
                {
                    continue;
                }
                for (carrier, value) in
                    indexed_entries(ctx.span_attrs, family, rule.require_member.as_deref())
                {
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
            let Some(attribute) = rule.read.attribute.as_deref() else {
                continue;
            };
            let Some(raw) = ctx.span_attrs.get(attribute) else {
                continue;
            };
            if let Some(gate) = &rule.when
                && !super::detect_rules::signals_hold(gate, ctx.span_name, ctx.span_attrs)
            {
                continue;
            }
            let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
                continue;
            };
            for value in readings(&parsed, &rule.alternatives) {
                let value = match &rule.wrap_role {
                    Some(role) => json!({ "role": role, "content": value }),
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
    require_member: Option<&str>,
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
        // An index exists as soon as any key mentions it, and a family holds keys that are not messages.
        if let Some(member) = require_member {
            let exact = format!("{entry_prefix}.{member}");
            let nested = format!("{exact}.");
            let present =
                attrs.contains_key(&exact) || attrs.keys().any(|k| k.starts_with(nested.as_str()));
            if !present {
                continue;
            }
        }
        let member_prefix = format!("{entry_prefix}.");
        let mut object = serde_json::Map::new();
        for (key, value) in attrs {
            if let Some(member) = key.strip_prefix(member_prefix.as_str()) {
                object.insert(member.to_string(), sniffed_value(value));
            }
        }
        out.push((entry_prefix, JsonValue::Object(object)));
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
