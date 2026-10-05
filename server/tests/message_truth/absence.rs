//! Absence proofs: a truth may withdraw a fact as "not in the telemetry" only when the telemetry
//! demonstrably does not carry it.
//!
//! A `not_exported` gap says the framework never exported a fact the model call produced (Codex's text
//! between tool calls, a second tool result an instrumentor drops). Declared by hand, such a gap is the
//! cheapest way to make a violation disappear, so it is accepted only with a proof: every captured
//! payload of every fixture the truth describes - trace requests and log exports, every attribute,
//! name, body and status, decoded through every encoding the engine can read (JSON in a string,
//! Python renderings, base64, data URIs, backslash escapes) - is searched for the fact's identifying
//! content, and the gap is refused when it is found.
//!
//! What counts as "found" is a compound witness in one carrier (a span with its events, a log record,
//! a resource, a scope), because a fact's parts legitimately occur apart: a tool call's id is repeated
//! by its result, and a result's value may be quoted by a later answer. Content found without its
//! witness is `Partial` - neither present nor proven absent - and is refused too, as is a fact whose
//! content is too weak to search for (`Unprovable`). Proven gaps are framework limitations, listed in
//! the documentation from the truths themselves (`limitations_section`).

mod haystack;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

use super::predicates::{json_eq, same_tool, semantic_eq};
use super::truth::{self, Fact, Truth};
use haystack::{Carrier, Haystack, MAX_DEPTH, collapse_whitespace};

/// Shorter texts occur by coincidence, so their absence proves nothing.
const MIN_TEXT: usize = 12;
/// A fragment this long found on its own is a truncated or reworded copy, not a coincidence.
const FRAGMENT: usize = 24;
/// Ids shorter than this are counters, not identities.
const MIN_ID: usize = 6;

/// The outcome of searching one fixture for one fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Proof {
    Absent,
    /// The fact's compound witness, at this location.
    Present(String),
    /// Part of the fact, or its content without its witness, at this location.
    Partial(String),
    /// The fact cannot be searched for: its content is too weak, or the payload too deeply encoded.
    Unprovable(String),
}

/// Searches one fixture's payloads for a fact the truth says is not in them.
pub(super) fn prove(fact: &Fact, haystack: &Haystack) -> Proof {
    if let Some(at) = haystack.undecoded.first() {
        return Proof::Unprovable(format!(
            "{at} is encoded deeper than the search decodes ({MAX_DEPTH} layers)"
        ));
    }
    match fact.kind.as_str() {
        "text" | "system" | "user_text" | "reasoning" => prove_text(fact.text(), haystack),
        "tool_call" => prove_tool_call(&fact.value, haystack),
        "tool_result" => prove_tool_result(&fact.value, haystack),
        "user_media" => match fact.value.get("sha256").and_then(Value::as_str) {
            Some(digest) => haystack
                .carriers
                .iter()
                .flat_map(|c| &c.digests)
                .find(|(_, d)| d == digest)
                .map_or(Proof::Absent, |(at, _)| Proof::Present(at.clone())),
            None => Proof::Unprovable("the attachment states no digest".to_string()),
        },
        other => Proof::Unprovable(format!("no absence search for a {other} fact")),
    }
}

fn find_string<'a>(carrier: &'a Carrier, needle: &str) -> Option<&'a str> {
    carrier
        .strings
        .iter()
        .find(|(_, s)| s.contains(needle))
        .map(|(at, _)| at.as_str())
}

fn find_node(carrier: &Carrier, matches: impl Fn(&Value) -> bool) -> Option<&str> {
    carrier
        .nodes
        .iter()
        .find(|(_, node)| matches(node))
        .map(|(at, _)| at.as_str())
}

fn prove_text(text: &str, haystack: &Haystack) -> Proof {
    let needle = collapse_whitespace(text);
    if needle.chars().count() < MIN_TEXT {
        return Proof::Unprovable(format!(
            "{needle:?} is too short to tell apart from other content"
        ));
    }
    if let Some(at) = haystack
        .carriers
        .iter()
        .find_map(|c| find_string(c, &needle))
    {
        return Proof::Present(at.to_string());
    }
    // A structured answer may be carried as the JSON it encodes.
    if let Ok(parsed) = serde_json::from_str::<Value>(text)
        && (parsed.is_object() || parsed.is_array())
        && let Some(at) = haystack
            .carriers
            .iter()
            .find_map(|c| find_node(c, |node| json_eq(node, &parsed)))
    {
        return Proof::Present(at.to_string());
    }
    for fragment in fragments(&needle) {
        if let Some(at) = haystack
            .carriers
            .iter()
            .find_map(|c| find_string(c, &fragment))
        {
            return Proof::Partial(format!("{at} holds {fragment:?}"));
        }
    }
    Proof::Absent
}

/// The pieces of a text a truncated or re-wrapped copy would still hold: its first and last
/// `FRAGMENT` characters and every sentence at least that long.
fn fragments(needle: &str) -> Vec<String> {
    let chars: Vec<char> = needle.chars().collect();
    if chars.len() <= FRAGMENT {
        return Vec::new();
    }
    let mut out = vec![
        chars[..FRAGMENT].iter().collect::<String>(),
        chars[chars.len() - FRAGMENT..].iter().collect::<String>(),
    ];
    out.extend(
        needle
            .split(['.', '!', '?', ';', '\n'])
            .map(str::trim)
            .filter(|s| s.chars().count() >= FRAGMENT)
            .map(str::to_string),
    );
    out
}

/// Arguments or a value that identifies a fact on its own: a non-empty object or array, or a text.
fn identifying(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::String(text) => collapse_whitespace(text).chars().count() >= MIN_TEXT,
        _ => false,
    }
}

fn prove_tool_call(value: &Value, haystack: &Haystack) -> Proof {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.len() >= MIN_ID);
    let name = value.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = &value["arguments"];
    let by_arguments = identifying(arguments);
    if id.is_none() && !by_arguments {
        return Proof::Unprovable(
            "a call with neither an id nor arguments to search for".to_string(),
        );
    }
    let mut partial = None;
    for carrier in &haystack.carriers {
        let has_id = id.and_then(|id| find_string(carrier, id));
        let has_arguments = by_arguments
            .then(|| find_node(carrier, |node| json_eq(node, arguments)))
            .flatten();
        let has_name = (!name.is_empty())
            .then(|| {
                carrier
                    .strings
                    .iter()
                    .find(|(_, s)| same_tool(s, name))
                    .map(|(at, _)| at.as_str())
            })
            .flatten();
        // The id with the call's arguments or name, or the arguments under the call's name: an id
        // alone is also its result's, and arguments alone may be another call's.
        let witness = match (has_id, has_arguments, has_name) {
            (Some(at), Some(_), _) | (Some(at), _, Some(_)) | (None, Some(at), Some(_)) => Some(at),
            _ => None,
        };
        if let Some(at) = witness {
            return Proof::Present(at.to_string());
        }
        if partial.is_none() {
            partial = has_arguments.map(str::to_string);
        }
    }
    partial.map_or(Proof::Absent, |at| {
        Proof::Partial(format!("{at} holds the arguments"))
    })
}

fn prove_tool_result(value: &Value, haystack: &Haystack) -> Proof {
    let id = value
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let result = &value["value"];
    if !identifying(result) {
        return Proof::Unprovable(format!(
            "the result {result} is too short to tell apart from other content"
        ));
    }
    let text = result.as_str().map(collapse_whitespace);
    let mut partial = None;
    for carrier in &haystack.carriers {
        let has_value = match &text {
            Some(needle) => find_string(carrier, needle),
            None => find_node(carrier, |node| semantic_eq(node, result)),
        };
        let Some(at) = has_value else {
            continue;
        };
        // The value under its call's id; a value with no id to pair by is the result itself.
        if id.is_none_or(|id| find_string(carrier, id).is_some()) {
            return Proof::Present(at.to_string());
        }
        partial.get_or_insert_with(|| at.to_string());
    }
    partial.map_or(Proof::Absent, |at| {
        Proof::Partial(format!("{at} holds the value without its call id"))
    })
}

/// Every absence gap of one truth with the fact it withdraws.
pub(super) fn absence_gaps(truth: &Truth) -> Vec<(&truth::Gap, &Fact)> {
    truth
        .gaps
        .iter()
        .filter(|gap| truth::gap_effects(&gap.reason).is_some_and(|e| e.needs_absence_proof))
        .filter_map(|gap| {
            let subject = gap.subject.as_deref()?;
            Some((gap, truth.facts.iter().find(|f| f.id == subject)?))
        })
        .collect()
}

/// Where the documentation lists the proven limitations, between these markers.
pub(super) const LIMITATIONS_DOC: &str =
    "docs/src/content/docs/docs/reference/production-readiness.mdx";
const BEGIN: &str = "{/* BEGIN framework-limitations */}";
const END: &str = "{/* END framework-limitations */}";

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "text" => "assistant text",
        "system" => "system prompt",
        "user_text" => "user prompt",
        "user_media" => "attachment",
        "reasoning" => "reasoning",
        "tool_call" => "tool call",
        "tool_result" => "tool result",
        _ => "other content",
    }
}

/// The documentation's table of proven limitations: per framework and per kind of content, why it is
/// missing and the captures it was proven missing from.
pub(super) fn limitations_section(truths: &BTreeMap<String, Truth>) -> String {
    // (producer, kind, detail) -> fixture -> facts withdrawn there.
    let mut rows: BTreeMap<(String, String, String), BTreeMap<String, usize>> = BTreeMap::new();
    for truth in truths.values() {
        for (gap, fact) in absence_gaps(truth) {
            let fixtures = rows
                .entry((
                    truth.producer.clone(),
                    kind_label(&fact.kind).to_string(),
                    gap.detail.clone(),
                ))
                .or_default();
            for fixture in &truth.fixtures {
                *fixtures.entry(fixture.clone()).or_default() += 1;
            }
        }
    }
    let mut out = vec![BEGIN.to_string(), String::new()];
    if rows.is_empty() {
        out.push("No framework currently omits content from its telemetry.".to_string());
    } else {
        out.push("| Framework | Missing | Why | Proven absent from |".to_string());
        out.push("| --- | --- | --- | --- |".to_string());
        for ((producer, kind, detail), fixtures) in &rows {
            // mode -> scenarios, so a native-only or one-release limitation reads as one.
            let mut by_mode: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for fixture in fixtures.keys() {
                let mut parts = fixture.splitn(3, '/');
                let (_, mode, scenario) = (parts.next(), parts.next(), parts.next());
                by_mode
                    .entry(mode.unwrap_or(""))
                    .or_default()
                    .insert(scenario.unwrap_or(""));
            }
            let facts: usize = fixtures.values().sum();
            let captured = by_mode
                .iter()
                .map(|(mode, scenarios)| {
                    let scenarios: Vec<&str> = scenarios.iter().copied().collect();
                    format!("`{mode}`: {}", scenarios.join(", "))
                })
                .collect::<Vec<_>>()
                .join("; ");
            out.push(format!(
                "| {producer} | {kind} ({facts} in {} captures) | {} | {captured} |",
                fixtures.len(),
                detail.replace('|', "\\|")
            ));
        }
    }
    out.push(String::new());
    out.push(END.to_string());
    out.join("\n")
}

/// Every absence gap holds in every fixture its truth describes.
#[test]
fn every_absence_gap_is_proven() {
    let truths = truth::load_all();
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let mut defects = Vec::new();
    let mut proven = 0;
    for (key, truth) in &truths {
        let gaps = absence_gaps(truth);
        if gaps.is_empty() {
            continue;
        }
        let mut searched = 0;
        for fixture in &truth.fixtures {
            let Some(paths) = fixtures.get(fixture) else {
                continue;
            };
            searched += 1;
            let haystack = Haystack::of_fixture(paths);
            for (gap, fact) in &gaps {
                match prove(fact, &haystack) {
                    Proof::Absent => proven += 1,
                    refused => defects.push(format!(
                        "{key}: {} {} ({}) is declared {} but {fixture}: {refused:?}",
                        fact.kind, fact.id, gap.detail, gap.reason
                    )),
                }
            }
        }
        if searched == 0 {
            defects.push(format!(
                "{key}: declares absence gaps but none of its fixtures is in the corpus"
            ));
        }
    }
    assert!(
        defects.is_empty(),
        "absence gaps without a proof ({proven} proven):\n  {}",
        defects.join("\n  ")
    );
}

/// The documentation lists exactly the proven limitations.
#[test]
fn framework_limitations_are_documented() {
    let path = truth::repo_root().join(LIMITATIONS_DOC);
    let doc = std::fs::read_to_string(&path).expect("the limitations page is readable");
    let section = limitations_section(&truth::load_all());
    let (Some(start), Some(end)) = (doc.find(BEGIN), doc.find(END)) else {
        panic!("{LIMITATIONS_DOC} has no generated framework-limitations section");
    };
    let current = &doc[start..end + END.len()];
    if current == section {
        return;
    }
    if std::env::var_os("UPDATE_LIMITATIONS").is_some() {
        let rewritten = format!("{}{section}{}", &doc[..start], &doc[end + END.len()..]);
        std::fs::write(&path, rewritten).expect("write the limitations page");
        return;
    }
    panic!(
        "{LIMITATIONS_DOC}'s framework limitations are stale; rewrite them with UPDATE_LIMITATIONS=1"
    );
}

/// Prints the proof for facts of one truth over each of its fixtures, for deciding whether a gap could
/// be declared: `ABSENCE_PROBE=spring-ai/tool_use:fact-007,fact-008 cargo test --locked -p
/// sideseat-server --test message_goldens -- --ignored --nocapture absence_probe` (no fact list probes
/// every fact).
#[test]
#[ignore]
fn absence_probe() {
    let Ok(probe) = std::env::var("ABSENCE_PROBE") else {
        eprintln!("set ABSENCE_PROBE=<producer>/<scenario>[:fact-001,fact-002]");
        return;
    };
    let (key, ids) = probe.split_once(':').unwrap_or((probe.as_str(), ""));
    let truths = truth::load_all();
    let truth = truths.get(key).unwrap_or_else(|| panic!("no truth {key}"));
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let wanted: BTreeSet<&str> = ids.split(',').filter(|s| !s.is_empty()).collect();
    for fixture in &truth.fixtures {
        let Some(paths) = fixtures.get(fixture) else {
            continue;
        };
        let haystack = Haystack::of_fixture(paths);
        for fact in truth
            .facts
            .iter()
            .filter(|f| wanted.is_empty() || wanted.contains(f.id.as_str()))
        {
            eprintln!(
                "{fixture} {} {}: {:?}",
                fact.id,
                fact.kind,
                prove(fact, &haystack)
            );
        }
    }
}
