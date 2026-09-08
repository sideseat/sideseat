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
    DetectMatch, FieldCombine, FieldSource, FieldTarget, FieldType, JsonFieldSource,
    MalformedPolicy, Reduction, SpanFieldRule,
};

/// What reading one source produced.
///
/// `Absent` and `Empty` are separate because a chain steps over both and a *caller* may not: a producer
/// writing `session.id=""` beside a real conversation id is stepping over an empty value, while a counter
/// that is absent and one that is `0` are different statements about the call. `Malformed` is separate from
/// both because it is evidence the key was meant to carry this field and the payload cannot be read - a
/// silence there hides a producer bug behind a fallback.
/// `PartialEq` only, no `Eq`: a float has no total equality, and deriving one would make `NaN` compare equal
/// to itself here and not elsewhere. A non-finite value never becomes a `Float` anyway - it is `Malformed`.
#[derive(Debug, Clone, PartialEq)]
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
    Float(f64),
    StringList(Vec<String>),
}

impl Reading {
    fn yielded(&self) -> bool {
        matches!(
            self,
            Self::Text(_) | Self::Integer(_) | Self::Float(_) | Self::StringList(_)
        )
    }
}

/// One field's resolution, and the source that answered it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub target: FieldTarget,
    pub reading: Reading,
    /// Which rule answered, for the explain trace.
    pub rule_id: String,
    /// Which **source** supplied the answer, and for a merge every source that contributed.
    ///
    /// The rule id alone was not enough: a chain like the session id's names five spellings, and knowing that
    /// `span-fields.session_id` answered says nothing about *which* producer's attribute was believed. The
    /// `refused` list beside this records the sources that were present and unreadable, which is a different
    /// question - it never named the winner.
    ///
    /// `None` where nothing answered, so an unset field is distinguishable from one set by a source nobody can
    /// name.
    pub evidence: Option<super::expr::EvidenceSet>,
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
    #[error(
        "span field rule `{rule}` in `{file}` states a literal with no gate, which answers on every span and \
         makes every source after it dead"
    )]
    UngatedLiteral { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` has a JSON source that names either no member or two ways of \
         naming one"
    )]
    JsonNamesNoMember { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` reduces a witness or a field that does not hold a number - a \
         witness asks only whether a member is there, and a sum is a number"
    )]
    ReductionThatCannotYield { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` reduces a first-present group, which names several paths and \
         takes one of them - a reduction combines the matches of one path"
    )]
    ReductionWithoutAPath { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` folds a field that holds no text - `lowercase` says a \
         producer's casing is not information, and a number has no casing"
    )]
    FoldWithoutText { file: String, rule: String },
    #[error(
        "span field rule `{rule}` in `{file}` declares `scalar_only` where it cannot apply - it says each \
         match of **one path** is a single string, so it means nothing on a witness, nothing beside a \
         reduction that is already per match, nothing on a first-present group that selects a whole member, \
         and nothing on a field that does not hold a list"
    )]
    ScalarOnlyWithoutAPath { file: String, rule: String },
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

    /// Every flat attribute any source of these targets reads.
    ///
    /// So a caller asking "which keys are already accounted for" reads the answer off the assets instead of
    /// keeping a list beside them. A hand-maintained mirror of a declaration is the hole this engine exists to
    /// close: adding a spelling to an asset would otherwise silently change what a *leftovers* set contains.
    pub fn attributes_read(&self, targets: &[FieldTarget]) -> std::collections::BTreeSet<&str> {
        self.rules
            .iter()
            .filter(|rule| targets.contains(&rule.target))
            .flat_map(|rule| &rule.sources)
            .flat_map(|source| {
                source
                    .spec
                    .attribute
                    .as_deref()
                    .into_iter()
                    .chain(
                        source
                            .spec
                            .attribute_first_present_of
                            .iter()
                            .map(String::as_str),
                    )
                    // The carrier a JSON source parses is read too, so a counter taken from inside one is
                    // accounted for. `when_json` is *not*: a witness is evidence about the span rather than
                    // the place the value came from, and counting it would hide a member nothing reads.
                    .chain(
                        source
                            .spec
                            .json
                            .as_ref()
                            .map(|json| json.attribute.as_str()),
                    )
            })
            .collect()
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
        // The sources that actually supplied the answer: one for a first-wins chain, every contributor for a
        // merge.
        let mut witnesses: Vec<super::expr::ClausePath> = Vec::new();
        let witness_of = |source: &CompiledSource| {
            super::expr::ClausePath::root(rule.rule_id.clone()).then(source.spec.id.clone())
        };

        for source in &rule.sources {
            if !source_applies(source, span_name, attrs) {
                continue;
            }
            // A JSON witness admits the source or not, and asking it here rather than in `source_applies` is
            // what lets it share the parse cache with the reads.
            if let Some(witness) = &source.spec.when_json
                && !json_member_present(witness, attrs, parsed)
            {
                continue;
            }
            let reading = folded_if_declared(
                read_source(&source.spec, field_type, span_name, attrs, parsed),
                source.spec.lowercase,
            );
            if let Reading::Malformed { .. } = &reading
                && source.spec.on_malformed == MalformedPolicy::Stop
            {
                // A present value that does not convert **ends** the search. The key exists and holds the
                // wrong shape, which is evidence the producer meant it to carry this field: stepping over it
                // reports a *later* spelling's value as this one, and `http.status_code = "OK"` beside another
                // key's `503` then answered 503 for a call whose own status attribute says otherwise. An empty
                // value is the opposite case, and stepping over that is what a chain is for.
                refused.push((source_label(&source.spec), reading));
                // A **merge** is a union, so one unreadable source does not invalidate the others - which is
                // also what the retired `merge_tags` did, since an unparseable value contributed nothing and
                // the loop went on. Only a first-wins chain stops, where continuing would substitute a later
                // spelling's value for the one this key was meant to carry.
                match rule.combine {
                    FieldCombine::FirstWins => break,
                    FieldCombine::MergeAll => continue,
                }
            }
            if reading == Reading::Empty && source.spec.accept_empty {
                // An empty value the producer wrote, kept as one. Text becomes the empty string, which is
                // what the retired direct read stored.
                answer = match field_type {
                    FieldType::Text => Reading::Text(String::new()),
                    _ => reading,
                };
                // An empty value a producer wrote *is* an answer, so the source that wrote it is the witness.
                witnesses.push(witness_of(source));
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
                    witnesses.push(witness_of(source));
                    break;
                }
                FieldCombine::MergeAll => {
                    if let Reading::StringList(items) = reading {
                        // A contributor, not the winner: a merge has as many witnesses as sources that
                        // supplied something, and reporting one of them would misdescribe the answer.
                        let mut contributed = false;
                        for item in items {
                            if !merged.contains(&item) {
                                merged.push(item);
                                contributed = true;
                            }
                        }
                        if contributed {
                            witnesses.push(witness_of(source));
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
            evidence: super::expr::EvidenceSet::of(witnesses),
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
    if let Some(attribute) = &spec.attribute {
        return attribute.clone();
    }
    if let [first, ..] = spec.attribute_first_present_of.as_slice() {
        return format!("first of {first} ...");
    }
    if let Some(json) = &spec.json {
        return match (&json.path, json.first_present_of.as_slice()) {
            (Some(path), _) => format!("{}{}", json.attribute, path),
            (_, [first, ..]) => format!("{} (first of {first} ...)", json.attribute),
            _ => json.attribute.clone(),
        };
    }
    if spec.raw_span_name {
        return "the span's own name".to_string();
    }
    if let Some(prefix) = &spec.span_name_strip_prefix {
        return format!("the span name past `{prefix}`");
    }
    if let Some(value) = &spec.value {
        return format!("the literal `{value}`");
    }
    String::new()
}

/// The reading with its text folded to lower case, where the source declared the producer's casing is not
/// information.
///
/// Applied once, where a source's reading is produced, rather than inside each read: the flag is a statement
/// about the *field's* vocabulary, so it must mean the same thing whether the value came from a flat
/// attribute, a JSON path or a first-present group. A number is left alone, having no case.
fn folded_if_declared(reading: Reading, lowercase: bool) -> Reading {
    if !lowercase {
        return reading;
    }
    match reading {
        Reading::Text(text) => Reading::Text(text.to_lowercase()),
        Reading::StringList(items) => {
            Reading::StringList(items.iter().map(|item| item.to_lowercase()).collect())
        }
        other => other,
    }
}

fn read_source<'a>(
    spec: &'a FieldSource,
    field_type: FieldType,
    span_name: &str,
    attrs: &HashMap<String, String>,
    parsed: &mut HashMap<&'a str, Option<JsonValue>>,
) -> Reading {
    if let Some(attribute) = &spec.attribute {
        let Some(raw) = attrs.get(attribute) else {
            return Reading::Absent;
        };
        return from_text(raw, field_type);
    }
    // Several spellings of one value: the **first present** one answers, and is then converted. Selecting by
    // conversion instead would answer from a later alias when the first is written badly or empty, where the
    // retired chain let the first one end this group and a *different carrier* answer.
    if !spec.attribute_first_present_of.is_empty() {
        return match spec
            .attribute_first_present_of
            .iter()
            .find_map(|key| attrs.get(key))
        {
            Some(raw) => from_text(raw, field_type),
            None => Reading::Absent,
        };
    }
    if spec.raw_span_name {
        return from_text(span_name, field_type);
    }
    if let Some(prefix) = &spec.span_name_strip_prefix {
        // An **empty** suffix is kept as an empty reading rather than dropped, which is what the retired
        // `strip_prefix` produced: a span named exactly the prefix has no name past it.
        return match span_name.strip_prefix(prefix.as_str()) {
            Some(rest) => from_text(rest, field_type),
            None => Reading::Absent,
        };
    }
    if let Some(value) = &spec.value {
        return from_text(value, field_type);
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
    // Several aliases of one member: the **first present** one answers, and then it is converted. Selecting by
    // conversion instead would answer from a *later alias* when the first is written badly, where the retired
    // code let the badly written one end this carrier and the next carrier answer.
    if !json.first_present_of.is_empty() {
        for path in &json.first_present_of {
            if let Some(found) = path.query(value).first() {
                return from_json(found, field_type);
            }
        }
        return Reading::Absent;
    }
    let Some(path) = &json.path else {
        return Reading::Absent;
    };
    // Every match kept, in the order the path found them.
    if let Some(Reduction::CollectAll) = json.reduce {
        let mut items: Vec<String> = Vec::new();
        for found in path.query(value).iter() {
            // Only a **scalar string** - implied here, whatever `scalar_only` says, because collecting is per
            // match and a match that is a list is the same malformed member. A match that is not one
            // contributes nothing, as `Sum`'s non-numeric match does and as the retired `as_str()` did; read
            // as a string *list* instead, a member holding `["stop", "length"]` contributed two reasons the
            // producer never stated.
            //
            // An **empty** string is a value, not an absence: `as_str()` returned `Some("")` and the retired
            // reader pushed it, which also *ended* the chain - so discarding it here both lost the value and
            // let a later producer's reason answer in its place.
            match from_json(found, FieldType::Text) {
                Reading::Text(text) => items.push(text),
                Reading::Empty => items.push(String::new()),
                _ => {}
            }
        }
        return if items.is_empty() {
            Reading::Absent
        } else {
            Reading::StringList(items)
        };
    }
    // Every match combined into one value. `Absent` for no matches, so "the span has no such shape" stays
    // distinguishable from a call that genuinely used no tokens.
    if let Some(Reduction::Sum) = json.reduce {
        let matched = path.query(value);
        if matched.is_empty() {
            return Reading::Absent;
        }
        let mut total = 0_i64;
        for found in matched.iter() {
            // A non-numeric match contributes nothing, which is what the retired reduction's `unwrap_or(0)`
            // did - one message without a usage object does not invalidate the others' counts.
            if let Reading::Integer(value) = from_json(found, FieldType::Integer) {
                // `checked_add`: an overflow is invalid telemetry, and saturating would report `i64::MAX` as a
                // believable count - an enormous bill from a number nobody sent. The retired reduction wrapped
                // in release and panicked in debug, so neither answer is the one to reproduce.
                match total.checked_add(value) {
                    Some(sum) => total = sum,
                    None => {
                        return Reading::Malformed {
                            detail: "the counts sum past what a count can hold".to_string(),
                        };
                    }
                }
            }
        }
        return match field_type {
            FieldType::Integer => Reading::Integer(total),
            _ => Reading::Malformed {
                detail: "a sum is a number, and this field does not hold one".to_string(),
            },
        };
    }
    // The first match that **yields**, not the first match. A dialect writes its agents as a list and the model
    // sits on whichever one declared it, so stopping at element 0's empty or absent member reported no model at
    // all - the same reason a flat chain steps over an empty value. Only the first match's outcome is carried
    // out as the diagnosis, since that is the one a reader would look at.
    let matched = path.query(value);
    let mut first = Reading::Absent;
    // Where the source says each match is a scalar string, read it as text and lift it into the field's list -
    // so an array *at the match* is malformed and the chain moves on, which is what the retired `as_str()`
    // readers did. Reading it as the field's own list type accepted `["stop"]` and answered here instead of
    // falling through to the next producer's statement.
    let read_as = if json.scalar_only {
        FieldType::Text
    } else {
        field_type
    };
    for (position, found) in matched.iter().enumerate() {
        let reading = match (json.scalar_only, from_json(found, read_as)) {
            (true, Reading::Text(text)) if field_type == FieldType::StringList => {
                Reading::StringList(vec![text])
            }
            // Present and empty is a **value** here, for the reason the collecting path gives: the retired
            // `as_str()` returned `Some("")`, pushed it, and stopped looking - so treating it as an absence
            // both lost the value and let the next producer's reason answer instead.
            (true, Reading::Empty) if field_type == FieldType::StringList => {
                Reading::StringList(vec![String::new()])
            }
            (_, other) => other,
        };
        if reading.yielded() {
            return reading;
        }
        if position == 0 {
            first = reading;
        }
    }
    first
}

/// Whether a JSON member exists at all, whatever it holds.
///
/// Presence, not a value: the retired test was `req.get("system").is_some()`, which a `null` member satisfies.
fn json_member_present<'a>(
    witness: &'a JsonFieldSource,
    attrs: &HashMap<String, String>,
    parsed: &mut HashMap<&'a str, Option<JsonValue>>,
) -> bool {
    let key = witness.attribute.as_str();
    if !attrs.contains_key(key) {
        return false;
    }
    let value = parsed.entry(key).or_insert_with(|| {
        attrs
            .get(key)
            .and_then(|raw| serde_json::from_str(raw).ok())
    });
    let Some(value) = value.as_ref() else {
        return false;
    };
    // A witness names one path, or several of which any is enough.
    match (&witness.path, witness.first_present_of.as_slice()) {
        (Some(path), _) => !path.query(value).is_empty(),
        (_, paths) => paths.iter().any(|path| !path.query(value).is_empty()),
    }
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
        FieldType::Float => {
            if raw.is_empty() {
                return Reading::Empty;
            }
            match raw.parse::<f64>() {
                // A non-finite value is **not** a number this can store: it survives no JSON round trip and
                // reads back as null, so it is a producer's mistake rather than a measurement.
                Ok(value) if value.is_finite() => Reading::Float(value),
                Ok(_) => Reading::Malformed {
                    detail: "not a finite number".to_string(),
                },
                Err(error) => Reading::Malformed {
                    detail: error.to_string(),
                },
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

/// A JSON value, read as the field's type - **strictly**, in both directions.
///
/// A number is not a text value and a quoted number is not a number. Both coercions were tried and both were
/// divergences: rendering a numeric `metadata.thread_id` invented a session identifier no other span will
/// match, and accepting a quoted count took a value the retired `as_i64` skipped, letting one carrier answer
/// where the next used to. A structure is refused for the same reason it always was: an object where a scalar
/// belongs is a producer bug, and reporting it as absent hides it.
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
            JsonValue::Null => Reading::Absent,
            // A quoted count is not a count. Every retired read of a JSON numeric member went through
            // `as_i64`, so coercing text here would accept a value that chain skipped.
            other => Reading::Malformed {
                detail: format!("expected a number, found {}", kind_of(other)),
            },
        },
        FieldType::Float => match value {
            JsonValue::Number(number) => match number.as_f64() {
                Some(found) if found.is_finite() => Reading::Float(found),
                _ => Reading::Malformed {
                    detail: format!("{number} is not a finite number"),
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
        // A declaration that could not take effect reads as one that does.
        if spec.lowercase
            && !matches!(
                rule.target.field_type(),
                FieldType::Text | FieldType::StringList
            )
        {
            return Err(FieldCompileError::FoldWithoutText {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // Exactly one form. Two would make the read ambiguous and none makes the source dead, and both used to
        // be expressible - so this counts rather than pattern-matching a pair, which is what stopped covering
        // the forms as they were added.
        let forms = usize::from(spec.attribute.is_some())
            + usize::from(!spec.attribute_first_present_of.is_empty())
            + usize::from(spec.json.is_some())
            + usize::from(spec.span_name_strip_prefix.is_some())
            + usize::from(spec.value.is_some())
            + usize::from(spec.raw_span_name);
        if forms == 0 {
            return Err(FieldCompileError::SourceReadsNothing {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        if forms > 1 {
            return Err(FieldCompileError::SourceReadsTwoThings {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        // Both the read and the **witness**, which had no such check: a witness naming no member always
        // answers false, so its source is permanently dead, and one naming both silently ignores the second.
        for json in [&spec.json, &spec.when_json].into_iter().flatten() {
            // A reduction combines the matches of *one* path, so there is nothing for it to do over a
            // first-present group - which names several paths and takes one of them.
            if json.reduce.is_some() && json.path.is_none() {
                return Err(FieldCompileError::ReductionWithoutAPath {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            // And nothing for it to do on a **witness**, which asks only whether a member is there, nor on a
            // target that holds anything but a number - a sum is a number, so such a source compiles and can
            // only ever be malformed.
            let is_witness = spec
                .when_json
                .as_ref()
                .is_some_and(|w| std::ptr::eq(w, json));
            // `scalar_only` says each match of one path is a single string. Its whole domain is an unreduced
            // read through a `path` into a list-valued field: a witness only asks whether a member is there, a
            // reduction is already per match, and a first-present group selects a *member* rather than
            // matching many - so on any of those the declaration was accepted and did nothing, which reads as
            // protection that is not there.
            if json.scalar_only
                && (is_witness
                    || json.reduce.is_some()
                    || json.path.is_none()
                    || rule.target.field_type() != FieldType::StringList)
            {
                return Err(FieldCompileError::ScalarOnlyWithoutAPath {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            let reduction_fits = match json.reduce {
                None => true,
                Some(Reduction::Sum) => rule.target.field_type() == FieldType::Integer,
                Some(Reduction::CollectAll) => rule.target.field_type() == FieldType::StringList,
            };
            if json.reduce.is_some() && (is_witness || !reduction_fits) {
                return Err(FieldCompileError::ReductionThatCannotYield {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
            let ways =
                usize::from(json.path.is_some()) + usize::from(!json.first_present_of.is_empty());
            if ways != 1 {
                return Err(FieldCompileError::JsonNamesNoMember {
                    file: file_id.to_string(),
                    rule: rule.id.clone(),
                });
            }
        }
        // A literal with no gate is not a source: it answers on every span, so every source after it is dead
        // and the field is a constant.
        if spec.value.is_some()
            && spec.when.is_none()
            && spec.unless.is_none()
            && spec.when_json.is_none()
        {
            return Err(FieldCompileError::UngatedLiteral {
                file: file_id.to_string(),
                rule: rule.id.clone(),
            });
        }
        if spec.attribute.as_deref().is_some_and(str::is_empty)
            || spec
                .json
                .as_ref()
                .is_some_and(|json| json.attribute.is_empty())
            || spec
                .span_name_strip_prefix
                .as_deref()
                .is_some_and(str::is_empty)
            || spec.value.as_deref().is_some_and(str::is_empty)
            || spec.attribute_first_present_of.iter().any(String::is_empty)
            || spec
                .when_json
                .as_ref()
                .is_some_and(|witness| witness.attribute.is_empty())
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
