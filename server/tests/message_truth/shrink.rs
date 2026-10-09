//! The ledger only shrinks: an entry added since the merge base with main is a regression unless it is one of
//! the baselines a change may legitimately reveal, each bounded so it cannot admit anything else.

use std::collections::{BTreeMap, BTreeSet};

use super::truth::{self, Truth};
use super::{ViolationView, family, ledger};

/// The entries added since the base that are regressions: an entry for a fixture that existed at the
/// base, about a check that existed at the base and a truth fact the base already stated, that the base
/// did not record.
///
/// Five kinds of addition are baselines instead. A check introduced since the base describes defects
/// the rubric could not see before; a fixture introduced since the base had no entries to keep; a fact
/// the truth gained since the base - content a request carried that the truth then learned to state - is
/// an obligation the base never imposed; and a request entry whose request part - the one its subject
/// names, or the call's whole request - the base did not send, because the fixture's transcript was
/// recorded or re-recorded since with other content there; an entry about a subject the base recorded
/// as missing in the same view, which is that defect changing form as its content starts to be shown; and a
/// request entry about a call the base could not tie to a span (`call.unmatched`), whose request no check
/// could reach until the call was found. All still need a triaged backlog issue, which
/// `truth_violation_ledger_is_well_formed` enforces.
fn regressions(
    entries: &[ledger::Entry],
    recorded_at_base: &std::collections::BTreeSet<String>,
    check_existed: impl Fn(&str) -> bool,
    fixture_existed: impl Fn(&str) -> bool,
    fact_existed: impl Fn(&ledger::Entry) -> bool,
) -> Vec<String> {
    entries
        .iter()
        .filter(|e| !recorded_at_base.contains(&e.id))
        .filter(|e| check_existed(&family(&e.assertion)) && fixture_existed(&e.fixture))
        .filter(|e| fact_existed(e))
        .map(|e| e.id.clone())
        .collect()
}

/// The facts an entry is about: its subject, or both ends of an order entry (`fact-009 before fact-003`).
fn entry_facts(entry: &ledger::Entry) -> Vec<&str> {
    entry.subject.split(" before ").collect()
}

/// The added request entries about a call the base recorded as `call.unmatched` and the current ledger no
/// longer does. The request check runs only on a call tied to its span, so the base said nothing about that
/// call's request; the entries are what the call's defect is now that it is found, in the same fixture.
///
/// Per part, not 1:1 as a change of form is: an unmatched call hides its whole request check, not one fact,
/// so the first run that finds it may rightly surface several defects of that one request at once. The
/// bound keeps it to that request: only the found call's entries, none naming a part its request does not
/// have, one entry per subject, and no more entries than the request has parts. `parts` lists a call's
/// request parts (`system.0`, `m1.2`) by fixture and call; a call with no recorded request admits nothing.
fn unreached_requests(
    base: &[ledger::Entry],
    current: &[ledger::Entry],
    parts: impl Fn(&str, &str) -> Option<BTreeSet<String>>,
) -> BTreeSet<String> {
    let unmatched = |entries: &[ledger::Entry]| -> BTreeSet<(String, String)> {
        entries
            .iter()
            .filter(|e| e.view == ViolationView::Call.name() && e.assertion == "call.unmatched")
            .map(|e| (e.fixture.clone(), e.subject.clone()))
            .collect()
    };
    let in_base: BTreeSet<&str> = base.iter().map(|e| e.id.as_str()).collect();
    let mut added: Vec<&ledger::Entry> = current
        .iter()
        .filter(|e| e.view == ViolationView::Request.name() && !in_base.contains(e.id.as_str()))
        .collect();
    // In id order, so which entries a bound admits is deterministic.
    added.sort_by(|a, b| a.id.cmp(&b.id));
    let mut admitted = BTreeSet::new();
    for (fixture, call) in unmatched(base).difference(&unmatched(current)) {
        let Some(positions) = parts(fixture, call) else {
            continue;
        };
        let mut subjects: BTreeSet<&str> = BTreeSet::new();
        for entry in added.iter().filter(|e| &e.fixture == fixture) {
            let Some(position) = entry
                .subject
                .strip_prefix(call.as_str())
                .and_then(|rest| rest.strip_prefix(':'))
            else {
                continue;
            };
            // A subject naming a part (`system.2`, `m0.1`) must name one of this request's; a shown block no
            // part explains (`in3:system/text`) is about the request as a whole.
            let names_part = position.starts_with("system.")
                || position
                    .strip_prefix('m')
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
            if names_part && !positions.contains(position) {
                continue;
            }
            if subjects.len() == positions.len() || !subjects.insert(entry.subject.as_str()) {
                continue;
            }
            admitted.insert(entry.id.clone());
        }
    }
    admitted
}

/// The added entries that are the same defect as one the base recorded, changing form 1:1 as its content
/// starts to be shown.
///
/// The base recorded `<kind>.missing` for one of the entry's facts, with the same subject - trace scope
/// included - in the same fixture and view; that base entry is gone now; the number of entries about that
/// fact in that view has not grown; and each such base entry admits one replacement, never two. Anything
/// else - another fact, another view, the base entry still there, a second replacement - is a new defect.
fn changes_of_form(base: &[ledger::Entry], current: &[ledger::Entry]) -> BTreeSet<String> {
    let in_base: BTreeSet<&str> = base.iter().map(|e| e.id.as_str()).collect();
    let in_current: BTreeSet<&str> = current.iter().map(|e| e.id.as_str()).collect();
    let count = |entries: &[ledger::Entry], fixture: &str, view: &str, fact: &str| {
        entries
            .iter()
            .filter(|e| e.fixture == fixture && e.view == view)
            .filter(|e| entry_facts(e).contains(&fact))
            .count()
    };
    let mut used: BTreeSet<&str> = BTreeSet::new();
    let mut admitted = BTreeSet::new();
    // In id order, so which addition takes a base entry's one allowance is deterministic.
    let mut added: Vec<&ledger::Entry> = current
        .iter()
        .filter(|e| !in_base.contains(e.id.as_str()))
        .collect();
    added.sort_by(|a, b| a.id.cmp(&b.id));
    for entry in added {
        let replaced = entry_facts(entry).into_iter().find_map(|fact| {
            base.iter()
                .find(|m| {
                    m.fixture == entry.fixture
                        && m.view == entry.view
                        && m.assertion.ends_with(".missing")
                        && m.subject == fact
                        && !in_current.contains(m.id.as_str())
                        && !used.contains(m.id.as_str())
                })
                .filter(|_| {
                    count(current, &entry.fixture, &entry.view, fact)
                        <= count(base, &entry.fixture, &entry.view, fact)
                })
        });
        if let Some(missing) = replaced {
            used.insert(missing.id.as_str());
            admitted.insert(entry.id.clone());
        }
    }
    admitted
}

/// Whether the base already sent what a request entry is about, so the entry is a regression.
///
/// A request check compares a span with what its call was sent. The obligation is the request part the
/// subject names (`call-002:m3.0`, `call-001:system.2`), or for content a span shows that no part
/// explains (`call-002:in4:...`) the call's whole request: an entry is new only where that obligation is,
/// because the fixture's transcript was recorded or re-recorded since the base with other content there.
fn request_at_base(
    current: &BTreeMap<String, Truth>,
    base_truth: impl Fn(&str) -> Option<serde_json::Value>,
    fixture: &str,
    subject: &str,
) -> bool {
    let Some((key, truth)) = current
        .iter()
        .find(|(_, t)| t.fixtures.iter().any(|f| f == fixture))
    else {
        return true;
    };
    let mut parts = subject.split(':');
    let (Some(call), Some(position)) = (parts.next(), parts.next()) else {
        return true;
    };
    let Some(now) = truth.requests.get(fixture).and_then(|r| r.calls.get(call)) else {
        return true;
    };
    let Some(base) = base_truth(key) else {
        return false;
    };
    let then = &base["requests"][fixture]["calls"][call];
    if then.is_null() {
        return false;
    }
    let part_now = |system: bool, message: usize, index: usize| {
        if system {
            now.system.get(index).map(|o| o.part.clone())
        } else {
            now.messages
                .get(message)
                .and_then(|m| m.parts.get(index))
                .map(|o| o.part.clone())
        }
    };
    let part_then = |system: bool, message: usize, index: usize| {
        let occurrence = if system {
            &then["system"][index]
        } else {
            &then["messages"][message]["parts"][index]
        };
        Some(occurrence["part"].clone()).filter(|p| !p.is_null())
    };
    let named = position
        .strip_prefix("system.")
        .and_then(|i| i.parse().ok())
        .map(|i| (true, 0, i))
        .or_else(|| {
            let (m, i) = position.strip_prefix('m')?.split_once('.')?;
            Some((false, m.parse().ok()?, i.parse().ok()?))
        });
    match named {
        Some((system, message, index)) => {
            part_now(system, message, index) == part_then(system, message, index)
        }
        // A shown block no part explains: the obligation is the whole request.
        None => {
            let every = |value: &serde_json::Value| -> Vec<serde_json::Value> {
                value["system"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(
                        value["messages"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|m| m["parts"].as_array().into_iter().flatten()),
                    )
                    .map(|o| o["part"].clone())
                    .collect()
            };
            let parts_now: Vec<serde_json::Value> = now
                .system
                .iter()
                .chain(now.messages.iter().flat_map(|m| m.parts.iter()))
                .map(|o| o.part.clone())
                .collect();
            parts_now == every(then)
        }
    }
}

/// Whether the truth describing `fixture` stated the fact `subject` names at the base, with the same kind
/// and value. A subject that is no fact - a call, an edge, a span - always existed: only a fact is new.
fn fact_at_base(
    current: &BTreeMap<String, Truth>,
    base_truth: impl Fn(&str) -> Option<serde_json::Value>,
    fixture: &str,
    subject: &str,
) -> bool {
    let Some((key, truth)) = current
        .iter()
        .find(|(_, t)| t.fixtures.iter().any(|f| f == fixture))
    else {
        return true;
    };
    // A per-trace obligation (`fact-010@trace-2`) is its fact's.
    let subject = subject.split_once('@').map_or(subject, |(fact, _)| fact);
    let Some(fact) = truth.facts.iter().find(|f| f.id == subject) else {
        return true;
    };
    let Some(base) = base_truth(key) else {
        return false;
    };
    base["facts"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|f| f["id"] == subject && f["kind"] == fact.kind.as_str() && f["value"] == fact.value)
}

/// The reviewed baseline is the ledger's only expansion: an entry added after it is a regression
/// being recorded instead of fixed.
#[test]
fn truth_violation_ledger_only_shrinks_against_main() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(truth::repo_root())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let Some(base) = git(&["merge-base", "HEAD", "main"]) else {
        // A checkout without history cannot run the check; CI must never be one.
        assert!(
            std::env::var_os("CI").is_none(),
            "no merge base with main: the shrink check needs history (fetch main)"
        );
        eprintln!("message_truth: no merge base with main; the shrink check needs git history");
        return;
    };
    let base = base.trim().to_string();
    let Some(before) = git(&["show", &format!("{base}:{}", ledger::PATH)]) else {
        // The commit that introduces the ledger is its reviewed baseline.
        return;
    };
    let base_entries = ledger::parse(&before).entries;
    let before: std::collections::BTreeSet<String> =
        base_entries.iter().map(|e| e.id.clone()).collect();
    let registry =
        git(&["show", &format!("{base}:server/tests/message_truth/mod.rs")]).unwrap_or_default();
    let fixtures_at_base: std::collections::BTreeSet<String> = git(&[
        "ls-tree",
        "-r",
        "--name-only",
        &base,
        "--",
        "server/tests/fixtures/messages",
    ])
    .unwrap_or_default()
    .lines()
    .filter_map(|path| path.strip_prefix("server/tests/fixtures/messages/"))
    .filter_map(|path| path.rsplit_once('/').map(|(dir, _)| dir.to_string()))
    .collect();
    let current = truth::load_all();
    let base_truths: std::cell::RefCell<BTreeMap<String, Option<serde_json::Value>>> =
        Default::default();
    let base_truth = |key: &str| {
        base_truths
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(|| {
                git(&[
                    "show",
                    &format!("{base}:server/tests/fixtures/truth/{key}.json"),
                ])
                .and_then(|text| serde_json::from_str(&text).ok())
            })
            .clone()
    };
    let current_entries = ledger::load().entries;
    let changes_of_form = changes_of_form(&base_entries, &current_entries);
    let unreached = unreached_requests(&base_entries, &current_entries, |fixture, call| {
        let truth = current
            .values()
            .find(|t| t.fixtures.iter().any(|f| f == fixture))?;
        let request = truth.requests.get(fixture)?.calls.get(call)?;
        Some(
            (0..request.system.len())
                .map(|i| format!("system.{i}"))
                .chain(
                    request
                        .messages
                        .iter()
                        .enumerate()
                        .flat_map(|(m, message)| {
                            (0..message.parts.len()).map(move |i| format!("m{m}.{i}"))
                        }),
                )
                .collect(),
        )
    });
    let added = regressions(
        &current_entries,
        &before,
        |family| registry.contains(&format!("\"{family}\"")),
        |fixture| fixtures_at_base.contains(fixture),
        |entry| {
            if changes_of_form.contains(&entry.id) || unreached.contains(&entry.id) {
                return false;
            }
            if entry.view == ViolationView::Request.name() {
                return request_at_base(&current, base_truth, &entry.fixture, &entry.subject);
            }
            fact_at_base(&current, base_truth, &entry.fixture, &entry.subject)
        },
    );
    assert!(
        added.is_empty(),
        "{} gained entries since {}; fix the violation instead:\n  {}",
        ledger::PATH,
        &base[..base.len().min(10)],
        added.join("\n  ")
    );
}

#[test]
fn only_a_new_check_or_a_new_fixture_may_add_ledger_entries() {
    let entry = |fixture: &str, assertion: &str| ledger::Entry {
        id: format!("{fixture}:trace:{assertion}:fact-001"),
        fixture: fixture.to_string(),
        view: "trace".to_string(),
        assertion: assertion.to_string(),
        subject: "fact-001".to_string(),
        fingerprint: String::new(),
        reason: "a test entry".to_string(),
        issue: "rule-language-program#rv2/text.missing/p".to_string(),
        introduced: String::new(),
    };
    let entries = [
        entry("p/native/old", "text.missing"),
        entry("p/native/new", "text.missing"),
        entry("p/native/old", "order.new_check"),
        entry("p/native/kept", "text.missing"),
    ];
    let base: std::collections::BTreeSet<String> = [entries[3].id.clone()].into();
    let refused = regressions(
        &entries,
        &base,
        |family| family != "order.new_check",
        |fixture| fixture != "p/native/new",
        |_| true,
    );
    // An existing fixture under an existing check gains nothing; a new fixture or a new check brings
    // its own baseline; what the base recorded stays allowed.
    assert_eq!(refused, vec![entries[0].id.clone()]);
    let none_new = regressions(&entries, &base, |_| true, |_| true, |_| true);
    assert_eq!(
        none_new.len(),
        3,
        "with nothing new, every addition is a regression"
    );
    // A fact the truth gained since the base brings its own baseline, as a new check does.
    let new_fact = regressions(
        &entries,
        &base,
        |_| true,
        |_| true,
        |e| e.fixture != "p/native/old",
    );
    assert_eq!(new_fact, vec![entries[1].id.clone()]);
}

#[test]
fn a_defect_changing_form_from_missing_is_not_a_new_one() {
    let entry = |view: &str, assertion: &str, subject: &str| ledger::Entry {
        id: format!("p/native/s:{view}:{assertion}:{subject}"),
        fixture: "p/native/s".to_string(),
        view: view.to_string(),
        assertion: assertion.to_string(),
        subject: subject.to_string(),
        fingerprint: String::new(),
        reason: "a test entry".to_string(),
        issue: "rule-language-program#rv3b/system.missing/p".to_string(),
        introduced: String::new(),
    };
    let base = [
        entry("trace", "system.missing", "fact-010"),
        entry("trace", "system.missing", "fact-012"),
        entry("request", "request.missing", "call-002:m1.0"),
    ];
    let leaked = entry("trace", "system.leaked", "fact-010");
    let ordered = entry("trace", "order.sequence", "fact-010 before fact-003");
    let moved = entry("request", "request.order", "call-002:m1.0");
    let admits =
        |now: &[ledger::Entry], e: &ledger::Entry| changes_of_form(&base, now).contains(&e.id);
    // The fact is shown now, its missing entry is gone, and one entry replaces it: a change of form.
    assert!(admits(
        &[base[1].clone(), base[2].clone(), leaked.clone()],
        &leaked
    ));
    assert!(admits(
        &[base[1].clone(), base[2].clone(), ordered.clone()],
        &ordered
    ));
    assert!(admits(
        &[base[0].clone(), base[1].clone(), moved.clone()],
        &moved
    ));
    // Two replacements for one missing entry: the second is a new defect.
    let both = [
        base[1].clone(),
        base[2].clone(),
        leaked.clone(),
        ordered.clone(),
    ];
    assert_eq!(
        changes_of_form(&base, &both).len(),
        0,
        "the fact's count grew"
    );
    let duplicated = entry("trace", "system.duplicated", "fact-010");
    let swap_base = [
        entry("trace", "system.missing", "fact-010"),
        entry("trace", "system.as_text", "fact-010"),
    ];
    let swapped = [leaked.clone(), duplicated.clone()];
    assert_eq!(
        changes_of_form(&swap_base, &swapped).len(),
        1,
        "one missing entry admits one replacement, whatever the count"
    );
    // A different fact, or the same fact in another view: refused.
    let other = entry("trace", "system.leaked", "fact-011");
    assert!(!admits(
        &[base[1].clone(), base[2].clone(), other.clone()],
        &other
    ));
    let elsewhere = entry("session", "system.leaked", "fact-010");
    assert!(!admits(
        &[base[1].clone(), base[2].clone(), elsewhere.clone()],
        &elsewhere
    ));
    // The base's missing entry is still there: the new entry is a second defect, refused.
    assert!(!admits(
        &[
            base[0].clone(),
            base[1].clone(),
            base[2].clone(),
            leaked.clone()
        ],
        &leaked
    ));
    // A per-trace obligation keeps its scope: `fact-010@trace-2` missing admits nothing about `fact-010`.
    let scoped = [entry("trace", "system.missing", "fact-010@trace-2")];
    assert!(changes_of_form(&scoped, std::slice::from_ref(&leaked)).is_empty());
}

#[test]
fn a_request_no_check_reached_is_a_baseline_once_its_call_is_found() {
    let entry = |view: &str, assertion: &str, subject: &str| ledger::Entry {
        id: format!("p/native/s:{view}:{assertion}:{subject}"),
        fixture: "p/native/s".to_string(),
        view: view.to_string(),
        assertion: assertion.to_string(),
        subject: subject.to_string(),
        fingerprint: String::new(),
        reason: "a test entry".to_string(),
        issue: "rule-language-program#rv3/request.missing/p".to_string(),
        introduced: String::new(),
    };
    let base = [entry("call", "call.unmatched", "call-002")];
    let request = |positions: &[&str]| {
        let positions: BTreeSet<String> = positions.iter().map(|p| p.to_string()).collect();
        move |fixture: &str, call: &str| {
            (fixture == "p/native/s" && call == "call-002").then(|| positions.clone())
        }
    };
    let three = request(&["system.0", "m0.0", "m0.1"]);
    let missing = entry("request", "request.missing", "call-002:m0.1");
    let extra = entry("request", "request.extra", "call-002:in3:system/text");
    let system = entry("request", "request.missing", "call-002:system.0");
    // The call is found now: its request's own defects are admitted.
    assert_eq!(
        unreached_requests(
            &base,
            &[missing.clone(), extra.clone(), system.clone()],
            &three
        ),
        BTreeSet::from([missing.id.clone(), extra.id.clone(), system.id.clone()])
    );
    // Another call's request, or a part this request does not have: refused.
    let other_call = entry("request", "request.missing", "call-003:m0.1");
    let no_such_part = entry("request", "request.missing", "call-002:m7.0");
    assert!(unreached_requests(&base, &[other_call, no_such_part], &three).is_empty());
    // A second entry about one subject is a second defect of one part: refused.
    let reordered = entry("request", "request.order", "call-002:m0.1");
    assert_eq!(
        unreached_requests(&base, &[missing.clone(), reordered], &three),
        BTreeSet::from([missing.id.clone()])
    );
    // No more entries than the request has parts.
    let one = request(&["m0.1"]);
    assert_eq!(
        unreached_requests(&base, &[missing.clone(), extra.clone()], &one).len(),
        1
    );
    // Still unmatched, or no recorded request: nothing reached the request, so nothing is admitted on that
    // ground.
    assert!(unreached_requests(&base, &[base[0].clone(), missing.clone()], &three).is_empty());
    assert!(
        unreached_requests(&base, std::slice::from_ref(&missing), |_: &str, _: &str| {
            None
        })
        .is_empty()
    );
}
