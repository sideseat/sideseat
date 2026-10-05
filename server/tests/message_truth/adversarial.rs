//! Adversarial fixtures: for every check, a committed fixture that makes it fire.
//!
//! The mutation catalogue edits reconstructions in memory; these run the whole path - OTLP through
//! ingestion, the views, the rubric - on hand-written `_synthetic/adversarial_*` captures and on
//! hand-written truths (`fixtures/truth-adversarial/`). Most cases pair a clean capture with a truth
//! that disagrees with it in one way, written as a JSON Patch (RFC 6902: `add`, `replace`, `remove`)
//! over a base truth; the rest are captures that are themselves wrong. Each case records exactly which
//! checks fire, so a check that stops firing, or starts firing where it should not, fails here.
//!
//! A check is trusted only when something makes it fire: an adversarial case, a captured fixture whose
//! ledger entry proves it (`FIRED_BY_CAPTURES`), or - where no committed capture can hold the shape - the
//! mutation catalogue alone, with the reason (`FIRED_ONLY_BY_MUTATIONS`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use super::truth::Truth;

/// Checks whose firing fixture is a captured one, named with the fixture whose ledger entry shows it.
const FIRED_BY_CAPTURES: &[(&str, &str)] = &[
    // The feed of a LangGraph error run shows a prompt by another block than its trace does; a
    // hand-written payload would have to reproduce the feed's own pipeline divergence to show it.
    ("cross_view.differs", "langgraph/native/error"),
    // The pipeline orders a result before the response that consumed it in every hand-written shape
    // tried; a captured LangGraph error run holds the defect.
    ("order.inputs_before_next", "langgraph/native/error"),
];

/// Checks no committed fixture can make fire, with the reason; the mutation catalogue still does.
const FIRED_ONLY_BY_MUTATIONS: &[(&str, &str)] = &[(
    "order.result",
    "`assert_tool_causality` already rejects every fixture whose result precedes its call by id, so \
     no committed capture can hold the shape",
)];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    format: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    truth: String,
    patch: Vec<Value>,
    fires: Option<Vec<String>>,
    #[serde(default, rename = "why")]
    _why: Option<String>,
}

fn root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/truth-adversarial")
}

/// RFC 6902 `add`, `replace` and `remove` over RFC 6901 pointers; `-` appends to an array.
fn apply(document: &mut Value, patch: &[Value]) -> Result<(), String> {
    for operation in patch {
        let op = operation["op"].as_str().ok_or("an operation names no op")?;
        let path = operation["path"]
            .as_str()
            .ok_or("an operation names no path")?;
        let (parent, last) = path.rsplit_once('/').ok_or(format!("bad pointer {path}"))?;
        let unescape = |token: &str| token.replace("~1", "/").replace("~0", "~");
        let target = document
            .pointer_mut(parent)
            .ok_or(format!("{path}: no parent"))?;
        let value = operation.get("value").cloned();
        match (target, op) {
            (Value::Array(items), _) => {
                let index = if last == "-" {
                    items.len()
                } else {
                    last.parse::<usize>()
                        .map_err(|_| format!("{path}: not an index"))?
                };
                match op {
                    "add" if index <= items.len() => items.insert(index, value.ok_or("no value")?),
                    "replace" if index < items.len() => items[index] = value.ok_or("no value")?,
                    "remove" if index < items.len() => {
                        items.remove(index);
                    }
                    _ => return Err(format!("{op} {path}: out of range")),
                }
            }
            (Value::Object(map), "add" | "replace") => {
                map.insert(unescape(last), value.ok_or("no value")?);
            }
            (Value::Object(map), "remove") => {
                map.remove(&unescape(last))
                    .ok_or(format!("{path}: nothing to remove"))?;
            }
            _ => return Err(format!("{op} {path}: unsupported")),
        }
    }
    Ok(())
}

#[test]
fn truth_adversarial_fixtures_fire_their_checks() {
    let text = std::fs::read_to_string(root().join("cases.json")).expect("adversarial cases");
    let cases: Cases = serde_json::from_str(&text).expect("adversarial cases parse");
    assert_eq!(cases.format, "sideseat.truth-adversarial/1");
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let mut recons = BTreeMap::new();
    let mut failures = Vec::new();
    let mut fired: BTreeSet<String> = BTreeSet::new();
    for case in &cases.cases {
        let base = std::fs::read_to_string(root().join(format!("{}.json", case.truth)))
            .unwrap_or_else(|e| panic!("{}: truth {}: {e}", case.name, case.truth));
        let mut document: Value = serde_json::from_str(&base).expect("a base truth is JSON");
        if let Err(why) = apply(&mut document, &case.patch) {
            failures.push(format!("{}: patch: {why}", case.name));
            continue;
        }
        let truth: Truth = match serde_json::from_value(document) {
            Ok(truth) => truth,
            Err(e) => {
                failures.push(format!("{}: not {}: {e}", case.name, super::truth::FORMAT));
                continue;
            }
        };
        // A patched truth is still a sound document: the case disagrees with the capture, not with
        // the format.
        let key = format!("{}/{}", truth.producer, truth.scenario);
        let mut defects = super::truth::document_defects(&key, &truth);
        defects.extend(super::truth::repository_defects(&key, &truth));
        if !defects.is_empty() {
            failures.push(format!("{}: unsound truth: {defects:?}", case.name));
            continue;
        }
        let fixture = &truth.fixtures[0];
        let recon = recons.entry(fixture.clone()).or_insert_with(|| {
            let paths = fixtures
                .get(fixture)
                .unwrap_or_else(|| panic!("{}: no fixture {fixture}", case.name));
            super::recon::build(fixture, paths)
        });
        // Exactly which view, check and subject: a check that stops firing in one view, or fires on
        // another subject, changes the record.
        let violations = super::check(&truth, recon);
        fired.extend(violations.iter().map(|v| super::family(&v.assertion)));
        let observed: BTreeSet<String> = violations
            .iter()
            .map(|v| format!("{}:{}:{}", v.view.name(), v.assertion, v.subject))
            .collect();
        let expected: Option<BTreeSet<String>> =
            case.fires.as_ref().map(|f| f.iter().cloned().collect());
        if expected.as_ref() != Some(&observed) {
            failures.push(format!(
                "{}: fires {:?}, the case records {:?}",
                case.name, observed, case.fires
            ));
        }
    }

    let ledger = super::ledger::load();
    for (family, fixture) in FIRED_BY_CAPTURES {
        let proven = ledger
            .entries
            .iter()
            .any(|e| e.fixture == *fixture && super::family(&e.assertion) == *family);
        if proven {
            fired.insert((*family).to_string());
        } else {
            failures.push(format!(
                "{family}: {fixture} no longer shows it; add an adversarial case"
            ));
        }
    }
    fired.extend(
        FIRED_ONLY_BY_MUTATIONS
            .iter()
            .map(|(f, _)| (*f).to_string()),
    );
    for family in super::ASSERTION_FAMILIES {
        if !fired.contains(*family) {
            failures.push(format!(
                "{family}: no adversarial fixture makes this check fire"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "adversarial cases:\n  {}",
        failures.join("\n  ")
    );
}
