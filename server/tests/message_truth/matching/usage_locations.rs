//! Where producers state each usage count, as the usage vocabulary declares it, read from a raw span.
//!
//! The vocabulary (`assets/rules/vocabulary/span-fields-usage.json`) is the declaration of where a producer writes
//! each count: a counter attribute, or a JSON path inside an attribute's body, each perhaps limited to some
//! spans. A gapped count is the producer's statement only where the raw span holds it at one of those
//! locations; this reads the declared locations, not the engine's code. The candidate counts the engine weighs
//! against each other are locations too; sums over a list are not, since no one location states them.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;

/// One declared location of a count.
enum Location {
    /// A counter attribute, under any of its spellings.
    Attribute(Vec<String>, Option<Where>),
    /// A JSON path into an attribute's body, under any of its alternatives.
    Json(String, Vec<Vec<Step>>, Option<Where>),
}

/// The spans a location applies to, as the vocabulary's conditions state it.
enum Where {
    SpanNameStartsWith(String),
    AttributeEquals(String, Value),
    AttributeExists(String, bool),
    Any(Vec<Where>),
    All(Vec<Where>),
    Not(Box<Where>),
    /// A condition this reader does not evaluate: the location is not consulted.
    Unread,
}

enum Step {
    Member(String),
    Every,
    From(usize),
    At(usize),
}

/// The vocabulary targets that state each call usage assertion's count.
fn targets(assertion: &str) -> &'static [&'static str] {
    match assertion {
        "call.usage.input" => &["usage_input_tokens", "usage_candidate_input"],
        "call.usage.output" => &["usage_output_tokens", "usage_candidate_output"],
        "call.usage.cache_read" => &["usage_cache_read_tokens", "usage_candidate_cache_read"],
        "call.usage.cache_write" => &["usage_cache_write_tokens"],
        "call.usage.reasoning" => &["usage_reasoning_tokens"],
        _ => &[],
    }
}

fn locations() -> &'static BTreeMap<String, Vec<Location>> {
    static LOCATIONS: OnceLock<BTreeMap<String, Vec<Location>>> = OnceLock::new();
    LOCATIONS.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/rules/vocabulary/span-fields-usage.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("the usage vocabulary {}: {e}", path.display()));
        let document: Value = serde_json::from_str(&text).expect("the usage vocabulary is JSON");
        let mut by_target: BTreeMap<String, Vec<Location>> = BTreeMap::new();
        for field in document["span_fields"].as_array().into_iter().flatten() {
            let target = field["target"].as_str().unwrap_or_default().to_string();
            for source in field["sources"].as_array().into_iter().flatten() {
                let condition = source.get("where").map(read_where);
                if let Some(attribute) = source.get("attribute") {
                    by_target
                        .entry(target.clone())
                        .or_default()
                        .push(Location::Attribute(alternatives(attribute), condition));
                } else if let Some(json) = source.get("json") {
                    let attribute = json["attribute"].as_str().unwrap_or_default().to_string();
                    let paths = alternatives(&json["path"])
                        .iter()
                        .filter_map(|path| steps(path))
                        .collect();
                    by_target
                        .entry(target.clone())
                        .or_default()
                        .push(Location::Json(attribute, paths, condition));
                }
            }
        }
        by_target
    })
}

/// A name or a path, or `first_of` several.
fn alternatives(value: &Value) -> Vec<String> {
    match value {
        Value::String(one) => vec![one.clone()],
        Value::Object(map) => map
            .get("first_of")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn read_where(condition: &Value) -> Where {
    let list = |key: &str| {
        condition[key]
            .as_array()
            .map(|items| items.iter().map(read_where).collect::<Vec<_>>())
    };
    if let Some(any) = list("any") {
        return Where::Any(any);
    }
    if let Some(all) = list("all") {
        return Where::All(all);
    }
    if let Some(not) = condition.get("not") {
        return Where::Not(Box::new(read_where(not)));
    }
    let source = condition["source"].as_str().unwrap_or_default();
    if source == "span_name"
        && let Some(prefix) = condition["starts_with"].as_str()
    {
        return Where::SpanNameStartsWith(prefix.to_string());
    }
    if let Some(name) = source.strip_prefix("attr:") {
        if let Some(expected) = condition.get("equals") {
            return Where::AttributeEquals(name.to_string(), expected.clone());
        }
        if let Some(exists) = condition["exists"].as_bool() {
            return Where::AttributeExists(name.to_string(), exists);
        }
    }
    Where::Unread
}

/// Whether a condition holds for a span, read from its name and its own attributes.
fn holds(
    condition: &Where,
    span_name: &str,
    attributes: Option<&serde_json::Map<String, Value>>,
) -> bool {
    let attribute = |name: &str| attributes.and_then(|a| a.get(name));
    match condition {
        Where::SpanNameStartsWith(prefix) => span_name.starts_with(prefix.as_str()),
        Where::AttributeEquals(name, expected) => attribute(name) == Some(expected),
        Where::AttributeExists(name, exists) => attribute(name).is_some() == *exists,
        Where::Any(all) => all.iter().any(|c| holds(c, span_name, attributes)),
        Where::All(all) => all.iter().all(|c| holds(c, span_name, attributes)),
        Where::Not(inner) => !holds(inner, span_name, attributes),
        Where::Unread => false,
    }
}

/// `$.a.b[*].c`, `$.replies[0:]`, `$.x[2]` as steps; anything else is no path this reader follows.
fn steps(path: &str) -> Option<Vec<Step>> {
    let mut rest = path.strip_prefix('$')?;
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            out.push(Step::Member(after[..end].to_string()));
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']')?;
            let index = &after[..end];
            out.push(if index == "*" {
                Step::Every
            } else if let Some(from) = index.strip_suffix(':') {
                Step::From(from.parse().ok()?)
            } else {
                Step::At(index.parse().ok()?)
            });
            rest = &after[end + 1..];
        } else {
            return None;
        }
    }
    Some(out)
}

/// A body as JSON: the value itself, or the JSON a string holds.
fn body(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => serde_json::from_str(text).ok(),
        other => Some(other.clone()),
    }
}

/// A count as the payload states it: an integer, or a float with no fraction small enough to be one exactly -
/// never a figure a conversion would saturate or round into another.
fn integer(value: &Value) -> Option<i64> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    match value {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < EXACT)
                .map(|f| f as i64)
        }),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn follow<'a>(value: &'a Value, steps: &[Step], out: &mut Vec<&'a Value>) {
    let Some((step, rest)) = steps.split_first() else {
        out.push(value);
        return;
    };
    match (step, value) {
        (Step::Member(name), Value::Object(map)) => {
            if let Some(member) = map.get(name) {
                follow(member, rest, out);
            }
        }
        (Step::Every, Value::Array(items)) => items.iter().for_each(|v| follow(v, rest, out)),
        (Step::From(from), Value::Array(items)) => {
            items.iter().skip(*from).for_each(|v| follow(v, rest, out));
        }
        (Step::At(at), Value::Array(items)) => {
            if let Some(item) = items.get(*at) {
                follow(item, rest, out);
            }
        }
        _ => {}
    }
}

/// Whether `raw` - a span as the reader renders it - states `value` as `assertion`'s count at a declared
/// location: on the span's own attributes or an event's, never a link's or the resource's.
pub(super) fn states(raw: &Value, assertion: &str, value: i64) -> bool {
    let span_name = raw["name"].as_str().unwrap_or_default();
    let span_attributes = raw["attributes"].as_object();
    let mut scopes: Vec<&serde_json::Map<String, Value>> = span_attributes.into_iter().collect();
    for event in raw["events"].as_array().into_iter().flatten() {
        if let Some(attributes) = event["attributes"].as_object() {
            scopes.push(attributes);
        }
    }
    // Conditions read the span, whichever of its attribute sets holds the count.
    let applies = |condition: &Option<Where>| {
        condition
            .as_ref()
            .is_none_or(|c| holds(c, span_name, span_attributes))
    };
    targets(assertion)
        .iter()
        .filter_map(|target| locations().get(*target))
        .flatten()
        .any(|location| {
            scopes.iter().any(|attributes| match location {
                Location::Attribute(names, condition) => {
                    applies(condition)
                        && names
                            .iter()
                            .any(|name| attributes.get(name).and_then(integer) == Some(value))
                }
                Location::Json(attribute, paths, condition) => {
                    applies(condition)
                        && attributes
                            .get(attribute)
                            .and_then(body)
                            .is_some_and(|body| {
                                paths.iter().any(|path| {
                                    let mut found = Vec::new();
                                    follow(&body, path, &mut found);
                                    found.into_iter().any(|v| integer(v) == Some(value))
                                })
                            })
                }
            })
        })
}
