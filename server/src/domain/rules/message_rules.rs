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

use super::schema::{EmitTarget, MessageRule, ParseMode, ReadSpec, RuleFile};

/// What an ingestion knows when it asks which carriers to read.
#[derive(Debug, Clone, Copy)]
pub struct MessageContext<'a> {
    pub span_name: &'a str,
    pub span_attrs: &'a HashMap<String, String>,
}

/// Where an emitted observation came from, in the vocabulary the ingestion types use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmittedCarrier<'a> {
    Attribute(&'a str),
    Event(&'a str),
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
    pub parse: ParseMode,
    pub wrap_role: Option<String>,
    pub target: EmitTarget,
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
                wrap_role: wrap.as_ref().map(|w| w.role.clone()),
                target: *emit,
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
                || (a.read.event.is_some() && b.read.event.is_some());
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
        .unwrap_or_default()
}

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        for rule in &self.rules {
            // Event carriers are read from a span's events, not its attributes; no declared rule needs
            // one yet, and probing the attribute map for an event name would silently match nothing.
            let Some(attribute) = rule.read.attribute.as_deref() else {
                continue;
            };
            let Some(raw) = ctx.span_attrs.get(attribute) else {
                continue;
            };
            let Some(parsed) = parse_value(raw, rule.parse) else {
                continue;
            };
            let value = match &rule.wrap_role {
                Some(role) => json!({ "role": role, "content": parsed }),
                None => parsed,
            };
            out.push(Emission {
                rule_id: &rule.rule_id,
                carrier: EmittedCarrier::Attribute(attribute),
                target: rule.target,
                value,
            });
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
