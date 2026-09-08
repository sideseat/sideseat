//! Resolving a stored span field from whichever key its producer wrote it under.
//!
//! The counterpart of `message_rules` for the *scalar* side of a span. A message is a payload a carrier
//! holds; a field is one value, and the only thing a framework contributes is **which key** carries it and
//! in what order the spellings are preferred. As an ordered `&[&str]` in Rust that list was framework
//! knowledge in the code, and adding a producer meant editing a chain.
//!
//! Three things it deliberately does *not* do. It does not **claim** a key: a message carrier is claimed
//! because two dialects reading one payload report a conversation twice, while a key naming a model is
//! evidence about the model whoever else reads it. It does no **arithmetic**: a total synthesised from its
//! parts, and every pricing decision, stay in Rust, because those are statements about our own accounting
//! rather than about a producer's spelling. And it does not decide **presence** by value: an absent counter
//! and a genuine zero are different facts, so a resolution reports which it found.

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use super::schema::{
    DetectMatch, FieldCombine, FieldSource, FieldTarget, FieldType, JsonFieldSource, SpanFieldRule,
};

/// What reading one source produced.
///
/// `Absent` and `Empty` are separate because a chain steps over both and a *caller* may not: a producer
/// writing `session.id=""` beside a real conversation id is stepping over an empty value, while a counter
/// that is absent and one that is `0` are different statements about the call. `Malformed` is separate from
/// both because it is evidence the key was meant to carry this field and the payload cannot be read - a
/// silence there hides a producer bug behind a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// No key in this source's position carried anything.
    Absent,
    /// The key exists and holds nothing - an empty string, or an empty list.
    Empty,
    /// The key exists and does not hold this field's type.
    Malformed {
        detail: String,
    },
    /// A value of the field's own type.
    Text(String),
    Integer(i64),
    StringList(Vec<String>),
}

impl Reading {
    fn yielded(&self) -> bool {
        matches!(self, Self::Text(_) | Self::Integer(_) | Self::StringList(_))
    }
}

/// One field's resolution, and the source that answered it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub target: FieldTarget,
    pub reading: Reading,
    /// Which rule answered, for the explain trace.
    pub rule_id: String,
    /// Every source consulted that did not answer, and why. Kept because "no producer wrote this" and
    /// "three producers wrote it malformed" are different diagnoses of an empty column.
    pub refused: Vec<(String, Reading)>,
}

#[derive(Debug, thiserror::Error)]
pub enum FieldCompileError {
    #[error("span field rules in `{path}` are malformed: {message}")]
    Parse { path: String, message: String },
    #[error("span field rule `{rule}` in `{file}` declares no source")]
    NoSources { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` has a source that is neither an attribute nor a JSON member"
    )]
    SourceReadsNothing { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` has a source that is both an attribute and a JSON member"
    )]
    SourceReadsTwoThings { file: String, rule: String },
    #[error("span field rule `{rule}` in `{file}` names an empty attribute")]
    EmptyAttribute { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` merges into a field that holds one value - only a list field may merge"
    )]
    MergeIntoScalar { file: String, rule: String },
    #[error(
        "span field target `{target:?}` is resolved by `{first}` and `{second}` - one field, one resolver"
    )]
    DuplicateTarget {
        target: FieldTarget,
        first: String,
        second: String,
    },
    #[error("span field rule id `{rule}` is declared twice, in `{first}` and `{second}`")]
    DuplicateId {
        rule: String,
        first: String,
        second: String,
    },
    #[error(
        "span field rule `{rule}` in `{file}` gates a source in a way that never holds: {detail}"
    )]
    DeadGate {
        file: String,
        rule: String,
        detail: &'static str,
    },
    #[error(
        "span field rule `{rule}` in `{file}` gates a source on `{dimension}`, which this stage is never given"
    )]
    UnavailableGate {
        file: String,
        rule: String,
        dimension: &'static str,
    },
}

struct CompiledSource {
    spec: FieldSource,
    when: Option<super::detect_rules::CompiledDetect>,
    unless: Option<super::detect_rules::CompiledDetect>,
}

struct CompiledRule {
    rule_id: String,
    target: FieldTarget,
    combine: FieldCombine,
    sources: Vec<CompiledSource>,
}

/// Every declared field resolver, one per target.
pub struct SpanFieldPlan {
    rules: Vec<CompiledRule>,
}

impl SpanFieldPlan {
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn targets(&self) -> impl Iterator<Item = FieldTarget> + '_ {
        self.rules.iter().map(|rule| rule.target)
    }

    /// Resolve every declared field for one span.
    ///
    /// The JSON cache is shared across rules and across targets, because one attribute (`metadata`) carries
    /// several fields and parsing it per source made the cost quadratic in how many producers spell them.
    pub fn resolve(&self, span_name: &str, attrs: &HashMap<String, String>) -> Vec<Resolved> {
        let mut parsed: HashMap<&str, Option<JsonValue>> = HashMap::new();
        self.rules
            .iter()
            .map(|rule| self.resolve_rule(rule, span_name, attrs, &mut parsed))
            .collect()
    }

    fn resolve_rule<'a>(
        &self,
        rule: &'a CompiledRule,
        span_name: &str,
        attrs: &HashMap<String, String>,
        parsed: &mut HashMap<&'a str, Option<JsonValue>>,
    ) -> Resolved {
        let field_type = rule.target.field_type();
        let mut refused = Vec::new();
        let mut merged: Vec<String> = Vec::new();
        let mut answer = Reading::Absent;

        for source in &rule.sources {
            if !source_applies(source, span_name, attrs) {
                continue;
            }
            let reading = read_source(&source.spec, field_type, attrs, parsed);
            if let Reading::Malformed { .. } = &reading {
                // A present value that does not convert **ends** the search. The key exists and holds the
                // wrong shape, which is evidence the producer meant it to carry this field: stepping over it
                // reports a *later* spelling's value as this one, and `http.status_code = "OK"` beside another
                // key's `503` then answered 503 for a call whose own status attribute says otherwise. An empty
                // value is the opposite case, and stepping over that is what a chain is for.
                refused.push((source_label(&source.spec), reading));
                break;
            }
            if reading == Reading::Empty && source.spec.accept_empty {
                // An empty value the producer wrote, kept as one. Text becomes the empty string, which is
                // what the retired direct read stored.
                answer = match field_type {
                    FieldType::Text => Reading::Text(String::new()),
                    _ => reading,
                };
                if rule.combine == FieldCombine::FirstWins {
                    break;
                }
                continue;
            }
            if !reading.yielded() {
                if reading != Reading::Absent {
                    refused.push((source_label(&source.spec), reading));
                }
                continue;
            }
            match rule.combine {
                FieldCombine::FirstWins => {
                    answer = reading;
                    break;
                }
                FieldCombine::MergeAll => {
                    if let Reading::StringList(items) = reading {
                        for item in items {
                            if !merged.contains(&item) {
                                merged.push(item);
                            }
                        }
                    }
                }
            }
        }

        if rule.combine == FieldCombine::MergeAll {
            answer = if merged.is_empty() {
                Reading::Absent
            } else {
                Reading::StringList(merged)
            };
        }

        Resolved {
            target: rule.target,
            reading: answer,
            rule_id: rule.rule_id.clone(),
            refused,
        }
    }
}

/// Whether a source's gates permit it on this span.
fn source_applies(
    source: &CompiledSource,
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> bool {
    if let Some(gate) = &source.when
        && !super::detect_rules::compiled_signals_hold(gate, span_name, attrs)
    {
        return false;
    }
    if let Some(gate) = &source.unless
        && super::detect_rules::compiled_signals_hold(gate, span_name, attrs)
    {
        return false;
    }
    true
}

fn source_label(spec: &FieldSource) -> String {
    match (&spec.attribute, &spec.json) {
        (Some(attribute), _) => attribute.clone(),
        (_, Some(json)) => format!("{}{}", json.attribute, json.path),
        _ => String::new(),
    }
}

fn read_source<'a>(
    spec: &'a FieldSource,
    field_type: FieldType,
    attrs: &HashMap<String, String>,
    parsed: &mut HashMap<&'a str, Option<JsonValue>>,
) -> Reading {
    if let Some(attribute) = &spec.attribute {
        let Some(raw) = attrs.get(attribute) else {
            return Reading::Absent;
        };
        return from_text(raw, field_type);
    }
    let Some(json) = &spec.json else {
        return Reading::Absent;
    };
    read_json(json, field_type, attrs, parsed)
}

fn read_json<'a>(
    json: &'a JsonFieldSource,
    field_type: FieldType,
    attrs: &HashMap<String, String>,
    parsed: &mut HashMap<&'a str, Option<JsonValue>>,
) -> Reading {
    // Keyed by the attribute name so one `metadata` payload is parsed once per span, not once per field that
    // reads it. A failed parse is cached as a failure for the same reason - re-parsing junk per field is the
    // same waste as re-parsing a payload.
    let key = json.attribute.as_str();
    if !attrs.contains_key(key) {
        return Reading::Absent;
    }
    let value = parsed.entry(key).or_insert_with(|| {
        attrs
            .get(key)
            .and_then(|raw| serde_json::from_str(raw).ok())
    });
    let Some(value) = value.as_ref() else {
        return Reading::Malformed {
            detail: format!("`{}` is not JSON", json.attribute),
        };
    };
    let Some(found) = json.path.query(value).first() else {
        return Reading::Absent;
    };
    from_json(found, field_type)
}

/// A flat attribute's text, read as the field's type.
fn from_text(raw: &str, field_type: FieldType) -> Reading {
    match field_type {
        FieldType::Text => {
            if raw.is_empty() {
                Reading::Empty
            } else {
                Reading::Text(raw.to_string())
            }
        }
        FieldType::Integer => {
            if raw.is_empty() {
                Reading::Empty
            } else {
                match raw.parse::<i64>() {
                    Ok(value) => Reading::Integer(value),
                    Err(error) => Reading::Malformed {
                        detail: error.to_string(),
                    },
                }
            }
        }
        FieldType::StringList => {
            let items = crate::utils::string::parse_string_array(raw);
            if items.is_empty() {
                Reading::Empty
            } else {
                Reading::StringList(items)
            }
        }
    }
}

/// A JSON value, read as the field's type.
///
/// A number is accepted for a text field and a numeric string for an integer one, because a producer writing
/// its state as JSON chooses the encoding and both spellings are the same fact. A structure is not: an object
/// where a session id belongs is a producer bug, and reporting it as absent hides it.
fn from_json(value: &JsonValue, field_type: FieldType) -> Reading {
    match field_type {
        FieldType::Text => match value {
            JsonValue::String(text) if text.is_empty() => Reading::Empty,
            JsonValue::String(text) => Reading::Text(text.clone()),
            JsonValue::Null => Reading::Absent,
            // Deliberately *not* coerced. A number or a boolean where a text field belongs is a producer
            // mistake, and rendering it invents an identifier no other span will match - which for a session
            // id means a conversation of one.
            other => Reading::Malformed {
                detail: format!("expected a string, found {}", kind_of(other)),
            },
        },
        FieldType::Integer => match value {
            JsonValue::Number(number) => match number.as_i64() {
                Some(found) => Reading::Integer(found),
                None => Reading::Malformed {
                    detail: format!("{number} is not a whole number"),
                },
            },
            JsonValue::String(text) if text.is_empty() => Reading::Empty,
            JsonValue::String(text) => match text.parse::<i64>() {
                Ok(found) => Reading::Integer(found),
                Err(error) => Reading::Malformed {
                    detail: error.to_string(),
                },
            },
            JsonValue::Null => Reading::Absent,
            other => Reading::Malformed {
                detail: format!("expected a number, found {}", kind_of(other)),
            },
        },
        FieldType::StringList => match value {
            JsonValue::Array(items) => {
                let texts: Vec<String> = items
                    .iter()
                    .filter_map(|item| match item {
                        JsonValue::String(text) => Some(text.clone()),
                        JsonValue::Number(number) => Some(number.to_string()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    Reading::Empty
                } else {
                    Reading::StringList(texts)
                }
            }
            JsonValue::String(text) if text.is_empty() => Reading::Empty,
            JsonValue::String(text) => Reading::StringList(vec![text.clone()]),
            JsonValue::Null => Reading::Absent,
            other => Reading::Malformed {
                detail: format!("expected a list, found {}", kind_of(other)),
            },
        },
    }
}

fn kind_of(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "a boolean",
        JsonValue::Number(_) => "a number",
        JsonValue::String(_) => "a string",
        JsonValue::Array(_) => "a list",
        JsonValue::Object(_) => "an object",
    }
}

/// Compile every asset's field rules into one plan.
pub fn compile(
    sources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<SpanFieldPlan, FieldCompileError> {
    let mut rules: Vec<CompiledRule> = Vec::new();
    // Both indices exist to refuse a *silent* mistake: a duplicate id makes an explain trace ambiguous, and
    // a duplicate target means one field has two resolvers and which one fills it depends on load order.
    let mut by_id: HashMap<String, String> = HashMap::new();
    let mut by_target: HashMap<FieldTarget, String> = HashMap::new();

    for (file_id, bytes) in sources {
        let file: super::schema::RuleFile =
            serde_json::from_slice(bytes).map_err(|error| FieldCompileError::Parse {
                path: file_id.clone(),
                message: error.to_string(),
            })?;
        for rule in &file.span_fields {
            if let Some(first) = by_id.get(&rule.id) {
                return Err(FieldCompileError::DuplicateId {
                    rule: rule.id.clone(),
                    first: first.clone(),
                    second: file_id.clone(),
                });
            }
            if let Some(first) = by_target.get(&rule.target) {
                return Err(FieldCompileError::DuplicateTarget {
                    target: rule.target,
                    first: first.clone(),
                    second: rule.id.clone(),
                });
            }
            rules.push(compile_rule(file_id, rule)?);
            by_id.insert(rule.id.clone(), file_id.clone());
            by_target.insert(rule.target, rule.id.clone());
        }
    }
    Ok(SpanFieldPlan { rules })
}

fn compile_rule(file_id: &str, rule: &SpanFieldRule) -> Result<CompiledRule, FieldCompileError> {
    if rule.sources.is_empty() {
        return Err(FieldCompileError::NoSources {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    if rule.combine == FieldCombine::MergeAll && rule.target.field_type() != FieldType::StringList {
        return Err(FieldCompileError::MergeIntoScalar {
            file: file_id.to_string(),
            rule: rule.id.clone(),
        });
    }
    let mut sources = Vec::with_capacity(rule.sources.len());
    for spec in &rule.sources {
        match (&spec.attribute, &spec.json) {
            (None, None) => {
                return Err(FieldCompileError::SourceReadsNothing {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            (Some(_), Some(_)) => {
                return Err(FieldCompileError::SourceReadsTwoThings {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            _ => {}
        }
        if spec.attribute.as_deref().is_some_and(str::is_empty)
            || spec
                .json
                .as_ref()
                .is_some_and(|json| json.attribute.is_empty())
        {
            return Err(FieldCompileError::EmptyAttribute {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // The same refusal message rules carry: this stage sees a span's own name and attributes, so a gate
        // needing resource attributes would compile and never hold.
        for gate in [&spec.when, &spec.unless].into_iter().flatten() {
            if let Some(dimension) = unavailable_field_gate(gate) {
                return Err(FieldCompileError::UnavailableGate {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                    dimension,
                });
            }
            // And a gate that could never hold whatever it is given. `compile_signals` validates nothing, so
            // an empty gate - which is `false`, since signals are ORed - compiled as a dead source.
            if let Some(detail) = super::detect_rules::gate_defect(gate) {
                return Err(FieldCompileError::DeadGate {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                    detail,
                });
            }
        }
        sources.push(CompiledSource {
            spec: spec.clone(),
            when: spec.when.as_ref().map(super::detect_rules::compile_signals),
            unless: spec
                .unless
                .as_ref()
                .map(super::detect_rules::compile_signals),
        });
    }
    Ok(CompiledRule {
        rule_id: rule.id.clone(),
        target: rule.target,
        combine: rule.combine,
        sources,
    })
}

fn unavailable_field_gate(spec: &DetectMatch) -> Option<&'static str> {
    super::detect_rules::unavailable_gate_dimension(spec)
}
