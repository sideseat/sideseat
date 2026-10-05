//! The shrink-only record of rubric violations the current corpus still has.
//!
//! Every entry names one fixture, one view, one assertion and one subject - never a pattern or a count -
//! with the reason it fails and the backlog item that fixes it. A violation not recorded here fails the
//! goldens; so does an entry that no longer occurs, so fixing a defect forces its entry out, and
//! `truth_violation_ledger_only_shrinks_against_main` refuses entries added after the reviewed baseline.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Violation;
use super::truth::Truth;

pub(super) const FORMAT: &str = "sideseat.truth-ledger/1";
/// The issue a freshly written entry carries until someone triages it; the ledger refuses it.
pub(super) const UNTRIAGED: &str = "UNTRIAGED";
/// Every issue names an item of the rule-language program's backlog: `<section>/<assertion>/<producer>`.
pub(super) const ISSUE_PREFIX: &str = "rule-language-program#";
pub(super) const PATH: &str = "server/tests/fixtures/truth/known-violations.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Ledger {
    pub format: String,
    pub description: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub id: String,
    pub fixture: String,
    pub view: String,
    pub assertion: String,
    pub subject: String,
    pub fingerprint: String,
    pub reason: String,
    pub issue: String,
    pub introduced: String,
}

pub(super) fn path() -> PathBuf {
    super::truth::repo_root().join(PATH)
}

pub(super) fn parse(text: &str) -> Ledger {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{PATH} is not {FORMAT}: {e}"))
}

pub(super) fn load() -> Ledger {
    match std::fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(_) => Ledger {
            format: FORMAT.to_string(),
            description: String::new(),
            entries: Vec::new(),
        },
    }
}

/// What the ledger and the observed violations disagree about.
pub(super) fn compare(observed: &[Violation], ledger: &Ledger) -> Vec<String> {
    let recorded: BTreeMap<&str, &Entry> =
        ledger.entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let seen: BTreeMap<String, &Violation> = observed.iter().map(|v| (v.id(), v)).collect();
    let mut problems = Vec::new();
    for (id, violation) in &seen {
        match recorded.get(id.as_str()) {
            None => problems.push(format!("unledgered: {id} - {}", violation.detail)),
            Some(entry) if entry.fingerprint != violation.fingerprint() => problems.push(format!(
                "changed: {id} now fails differently ({}) - fix it, or re-triage the entry",
                violation.detail
            )),
            Some(_) => {}
        }
    }
    for id in recorded.keys() {
        if !seen.contains_key(*id) {
            problems.push(format!("fixed: {id} no longer occurs - remove the entry"));
        }
    }
    problems
}

/// Every rule the ledger's own shape must keep, independent of any reconstruction.
pub(super) fn hygiene(ledger: &Ledger, truths: &BTreeMap<String, Truth>) -> Vec<String> {
    let mut problems = Vec::new();
    if ledger.format != FORMAT {
        problems.push(format!("format {:?}, expected {FORMAT}", ledger.format));
    }
    let unasserted: BTreeSet<(String, String)> = truths
        .values()
        .flat_map(|t| {
            t.fixtures.iter().flat_map(move |fixture| {
                t.facts
                    .iter()
                    .filter(|f| f.require.is_none())
                    .map(move |f| (fixture.clone(), f.id.clone()))
            })
        })
        .collect();
    let mut ids = BTreeSet::new();
    for entry in &ledger.entries {
        let composed = format!(
            "{}:{}:{}:{}",
            entry.fixture, entry.view, entry.assertion, entry.subject
        );
        if entry.id != composed {
            problems.push(format!(
                "{}: id is not fixture:view:assertion:subject",
                entry.id
            ));
        }
        if !ids.insert(entry.id.as_str()) {
            problems.push(format!("{}: recorded twice", entry.id));
        }
        if entry.id.contains(['*', '?']) || entry.subject.is_empty() {
            problems.push(format!(
                "{}: an entry names one subject, never a pattern",
                entry.id
            ));
        }
        let item = entry
            .issue
            .strip_prefix(ISSUE_PREFIX)
            .map(|i| i.split('/').collect::<Vec<_>>());
        if !item.is_some_and(|parts| parts.len() == 3 && parts.iter().all(|p| !p.is_empty())) {
            problems.push(format!(
                "{}: the issue must name a backlog item, `{ISSUE_PREFIX}<section>/<assertion>/<producer>`",
                entry.id
            ));
        }
        if entry.reason.trim().is_empty() || entry.reason.contains('\n') {
            problems.push(format!("{}: the reason must be one line", entry.id));
        }
        if unasserted.contains(&(entry.fixture.clone(), entry.subject.clone())) {
            problems.push(format!(
                "{}: {} is an oracle gap, which is never a violation",
                entry.id, entry.subject
            ));
        }
    }
    problems
}

/// The committed form: one entry per line, so a fix is a one-line deletion in review.
pub(super) fn render(ledger: &Ledger) -> String {
    let entries: Vec<String> = ledger
        .entries
        .iter()
        .map(|e| {
            format!(
                "    {}",
                serde_json::to_string(e).expect("an entry serialises")
            )
        })
        .collect();
    format!(
        "{{\n  \"format\": {},\n  \"description\": {},\n  \"entries\": [\n{}\n  ]\n}}\n",
        serde_json::to_string(&ledger.format).expect("a string serialises"),
        serde_json::to_string(&ledger.description).expect("a string serialises"),
        entries.join(",\n")
    )
}

/// The ledger the observed violations call for, keeping every surviving entry's triage.
pub(super) fn rewritten(observed: &[Violation], ledger: &Ledger) -> Ledger {
    let previous: BTreeMap<&str, &Entry> =
        ledger.entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let today = chrono::Utc::now().date_naive().to_string();
    let mut entries: Vec<Entry> = observed
        .iter()
        .map(|v| {
            let id = v.id();
            let kept = previous.get(id.as_str());
            Entry {
                fixture: v.fixture.clone(),
                view: v.view.name().to_string(),
                assertion: v.assertion.clone(),
                subject: v.subject.clone(),
                fingerprint: v.fingerprint(),
                reason: kept.map_or_else(|| v.detail.clone(), |e| e.reason.clone()),
                issue: kept.map_or_else(|| UNTRIAGED.to_string(), |e| e.issue.clone()),
                introduced: kept.map_or(today.clone(), |e| e.introduced.clone()),
                id,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries.dedup_by(|a, b| a.id == b.id);
    Ledger {
        format: FORMAT.to_string(),
        description: ledger.description.clone(),
        entries,
    }
}
