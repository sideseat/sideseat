//! Rubric v2: every reconstruction checked against what the model actually said.
//!
//! The goldens compare a reconstruction with an `expected.json` the parser wrote itself, so a defect
//! present at capture time is blessed, and `expected.json` keeps only a 240-character preview of each
//! block. The truth documents under `fixtures/truth/` are derived from the recorded model responses and
//! the scenario scripts by `python -m harness truth`, independently of the parser. These checks hold
//! every fixture's span, trace, session and feed views to them, on full content:
//!
//! 1. the truth documents are internally consistent and describe the repository as it is;
//! 2. each truth call is recorded by exactly one span, chosen injectively, with its model, response id,
//!    finish and usage (`matching`);
//! 3. assistant text is exact; 4. tool calls carry their id, name and arguments; 5. each result pairs
//!    with its call by id and carries its value; 6. visible reasoning is a thinking block, never
//!    assistant text, and withheld reasoning a signed thinking block with no text; 7. prompts, the
//!    system prompt and attachments are present (`predicates`, `checks`);
//! 8. order follows the truth's edges; 9. a fact is the same block in every view and stays in its
//!    trace (`checks`);
//! 10. what still fails is recorded, one entry per violation, in a shrink-only ledger (`ledger`);
//! 11. every mutation in the catalogue is caught (`mutations`).
//!
//! The comparison runs inside `message_goldens`, on the views that test already built, so the inner
//! loop pays for one ingestion pass rather than two.

mod absence;
mod adversarial;
mod checks;
mod explain;
mod invariance;
mod ledger;
mod matching;
mod mutate;
mod mutate_framework;
mod mutate_matching;
mod mutate_requests;
mod mutations;
mod order;
mod predicates;
mod recon;
mod request_context;
mod requests;
mod truth;

use std::collections::{BTreeMap, BTreeSet};

pub(crate) use recon::read;
use recon::{Recon, ViewKind};
use truth::Truth;

/// Where a violation was seen: one of the four views, the call's metadata, or the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ViolationView {
    Span,
    Trace,
    Session,
    Feed,
    Call,
    /// A delivery variation changed a fixture's views (`invariance`).
    Delivery,
    /// A call's recorded request against its span's input (`requests`).
    Request,
}

impl ViolationView {
    fn name(self) -> &'static str {
        match self {
            ViolationView::Span => "span",
            ViolationView::Trace => "trace",
            ViolationView::Session => "session",
            ViolationView::Feed => "feed",
            ViolationView::Call => "call",
            ViolationView::Delivery => "delivery",
            ViolationView::Request => "request",
        }
    }
}

impl From<ViewKind> for ViolationView {
    fn from(kind: ViewKind) -> Self {
        match kind {
            ViewKind::Span => ViolationView::Span,
            ViewKind::Trace => ViolationView::Trace,
            ViewKind::Session => ViolationView::Session,
            ViewKind::Feed => ViolationView::Feed,
        }
    }
}

/// One way a fixture's reconstruction disagrees with its truth.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Violation {
    pub fixture: String,
    pub view: ViolationView,
    pub assertion: String,
    /// A truth fact or call id, an edge, or a span's capture-stable label.
    pub subject: String,
    /// Stable across runs: labels and truth values only, never trace or span ids.
    pub detail: String,
}

impl Violation {
    fn new(view: ViolationView, assertion: &str, subject: &str, detail: String) -> Self {
        Violation {
            fixture: String::new(),
            view,
            assertion: assertion.to_string(),
            subject: subject.to_string(),
            detail,
        }
    }

    fn id(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.fixture,
            self.view.name(),
            self.assertion,
            self.subject
        )
    }

    fn fingerprint(&self) -> String {
        crate::content_digest(&serde_json::Value::String(self.detail.clone()))
    }
}

/// Every check the rubric runs, by assertion; `*` stands for the fact kind. A check the rubric emits
/// that is missing here fails the goldens, and one here that no mutation fires fails the catalogue.
pub(crate) const ASSERTION_FAMILIES: &[&str] = &[
    "call.unmatched",
    "call.span_shared",
    "call.ambiguous",
    "call.count",
    "call.failed_attempt_has_output",
    "generation.unexpected",
    "call.model",
    "call.response_model",
    "call.response_id",
    "call.finish",
    "call.usage.input",
    "call.usage.output",
    "call.usage.cache_read",
    "call.usage.cache_write",
    "call.usage.reasoning",
    "*.missing",
    "*.duplicated",
    "*.leaked",
    "*.wrong_trace",
    "*.claimed_twice",
    "tool_call.id_rewritten",
    "reasoning.as_text",
    "cross_view.differs",
    "order.parts",
    "order.calls",
    "order.prompt",
    "order.result",
    "order.inputs_after_previous",
    "order.inputs_before_next",
    "order.conversations",
    "order.frame",
    "order.sequence",
    "extra.unexplained",
    "gap.unused",
    "attribution.span",
    "attribution.call_order",
    "request.missing",
    "request.extra",
    "request.duplicated",
    "request.order",
    "request.role",
    "request.provenance",
    "request.thread_leak",
];

/// The delivery variations `invariance` checks, by assertion.
pub(crate) const DELIVERY_FAMILIES: &[&str] = &[
    "invariance.arrival_order",
    "invariance.re_delivery",
    "invariance.batch_splitting",
    "invariance.clock_offset",
    "invariance.framework_version",
];

/// The family an assertion belongs to: a per-kind check reads as `*.<check>`.
fn family(assertion: &str) -> String {
    const KINDS: &[&str] = &[
        "system",
        "user_text",
        "user_media",
        "text",
        "reasoning",
        "tool_call",
        "tool_result",
    ];
    const PER_KIND: &[&str] = &[
        "missing",
        "duplicated",
        "leaked",
        "wrong_trace",
        "claimed_twice",
    ];
    match assertion.split_once('.') {
        Some((kind, check)) if KINDS.contains(&kind) && PER_KIND.contains(&check) => {
            format!("*.{check}")
        }
        _ => assertion.to_string(),
    }
}

/// Checks 2-9 for one fixture.
fn check(truth: &Truth, recon: &Recon) -> Vec<Violation> {
    let mut out = Vec::new();
    // A fact only a request carries, whose content no payload holds, cannot be shown by any
    // reconstruction: the producer did not export it. Withdrawn from the payloads alone, and documented
    // as that framework's limitation - the rule every proven gap follows.
    let truth = &requests::without_unexported_facts(truth, recon);
    let matching = matching::match_calls(truth, recon, &mut out);
    matching::check_metadata(truth, recon, &matching, &mut out);
    let accounted = requests::check_requests(truth, recon, &matching, &mut out);
    request_context::check_composition(recon, &mut out);
    let context = checks::Context::new(truth, recon, &matching, accounted);
    checks::check_placement(&context, &mut out);
    for violation in &mut out {
        violation.fixture = recon.fixture.clone();
        assert!(
            ASSERTION_FAMILIES.contains(&family(&violation.assertion).as_str()),
            "{} is not a registered check; add it to ASSERTION_FAMILIES and the catalogue",
            violation.assertion
        );
    }
    out.sort();
    out.dedup();
    out
}

/// The truth documents, and which one describes each fixture.
pub(crate) struct Truths {
    documents: BTreeMap<String, Truth>,
    of_fixture: BTreeMap<String, String>,
}

impl Truths {
    pub(crate) fn load() -> Self {
        let documents = truth::load_all();
        let of_fixture = documents
            .iter()
            .flat_map(|(key, t)| t.fixtures.iter().map(move |f| (f.clone(), key.clone())))
            .collect();
        Truths {
            documents,
            of_fixture,
        }
    }

    /// The fixture's violations, or nothing when no truth describes it.
    pub(crate) fn check(
        &self,
        fixture: &str,
        paths: &[std::path::PathBuf],
        built: &crate::Built,
        spans: recon::Spans,
    ) -> Vec<Violation> {
        let Some(truth) = self
            .of_fixture
            .get(fixture)
            .and_then(|key| self.documents.get(key))
        else {
            return Vec::new();
        };
        check(
            &truth.for_fixture(fixture),
            &recon::from_built(fixture, paths, built, spans),
        )
    }
}

/// Compares a full run's violations with the ledger, or rewrites the ledger under
/// `UPDATE_TRUTH_LEDGER=1`. Returns what is wrong, for `message_goldens` to report beside its diffs.
///
/// `delivery` selects which part of the ledger the run owns: the truth comparison's, or the delivery
/// invariance test's (`view: delivery`). `scope` narrows it to the fixtures a selective run checked
/// (`MESSAGE_FIXTURES=only:...`); `None` is the whole corpus. Each run compares and rewrites only its own
/// part, and carries every other entry over untouched.
pub(crate) fn ledger_problems(
    observed: &[Violation],
    delivery: bool,
    scope: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let mut ids = std::collections::BTreeSet::new();
    let duplicates: Vec<String> = observed
        .iter()
        .map(Violation::id)
        .filter(|id| !ids.insert(id.clone()))
        .collect();
    assert!(
        duplicates.is_empty(),
        "the rubric produced one id for two violations, so the ledger could not tell them apart: \
         {duplicates:?}"
    );
    let whole = ledger::load();
    let owned = |e: &ledger::Entry| owns(e, delivery, scope);
    let current = ledger::Ledger {
        entries: whole.entries.iter().filter(|e| owned(e)).cloned().collect(),
        ..whole.clone()
    };
    if std::env::var("UPDATE_TRUTH_LEDGER").is_ok() {
        let mut next = ledger::rewritten(observed, &current);
        next.entries
            .extend(whole.entries.iter().filter(|e| !owned(e)).cloned());
        next.entries.sort_by(|a, b| a.id.cmp(&b.id));
        std::fs::write(ledger::path(), ledger::render(&next)).expect("write the ledger");
        eprintln!(
            "message_truth: wrote {} entries to {}; triage every {} entry",
            next.entries.len(),
            ledger::PATH,
            ledger::UNTRIAGED
        );
        return Vec::new();
    }
    ledger::compare(observed, &current)
}

/// Whether a run owns a ledger entry: its part of the ledger, and a fixture the run checked. A run that
/// checked only some fixtures has said nothing about the others, so their entries are neither "fixed" nor
/// rewritten by it.
fn owns(entry: &ledger::Entry, delivery: bool, scope: Option<&BTreeSet<String>>) -> bool {
    (entry.view == ViolationView::Delivery.name()) == delivery
        && scope.is_none_or(|fixtures| fixtures.contains(&entry.fixture))
}

/// The fixtures a run checked, when it was asked for only some: `None` for the whole corpus.
pub(crate) fn run_scope<'a>(labels: impl Iterator<Item = &'a String>) -> Option<BTreeSet<String>> {
    std::env::var("MESSAGE_FIXTURES")
        .is_ok()
        .then(|| labels.cloned().collect())
}

#[test]
fn a_selective_run_owns_only_the_fixtures_it_checked() {
    let entry = |fixture: &str, view: &str| ledger::Entry {
        id: format!("{fixture}:{view}:a:b"),
        fixture: fixture.to_string(),
        view: view.to_string(),
        assertion: "a".to_string(),
        subject: "b".to_string(),
        fingerprint: String::new(),
        reason: String::new(),
        issue: String::new(),
        introduced: String::new(),
    };
    let checked: BTreeSet<String> = ["google-genai/native/files".to_string()].into();
    let run = entry("google-genai/native/files", "trace");
    let other = entry("crewai/native/chat", "trace");
    let delivery = entry("google-genai/native/files", "delivery");
    assert!(owns(&run, false, Some(&checked)));
    assert!(!owns(&other, false, Some(&checked)));
    assert!(!owns(&delivery, false, Some(&checked)));
    assert!(owns(&delivery, true, Some(&checked)));
    // The whole corpus owns every entry of its part, including one whose fixture is gone.
    assert!(owns(&other, false, None));
}

#[test]
fn truth_documents_are_internally_consistent() {
    let documents = truth::load_all();
    assert!(
        !documents.is_empty(),
        "no truth documents under {}",
        truth::truth_root().display()
    );
    let mut defects = Vec::new();
    let mut described = BTreeMap::new();
    for (key, truth) in &documents {
        defects.extend(truth::document_defects(key, truth));
        defects.extend(truth::repository_defects(key, truth));
        for fixture in &truth.fixtures {
            if let Some(other) = described.insert(fixture.clone(), key.clone()) {
                defects.push(format!("{fixture} is described by both {other} and {key}"));
            }
        }
    }
    for (fixture, _) in crate::discover_fixtures() {
        if !described.contains_key(&fixture) && truth::reason_without_truth(&fixture).is_none() {
            defects.push(format!(
                "{fixture} has no truth and no stated reason; run `python -m harness truth <producer>`"
            ));
        }
    }
    assert!(
        defects.is_empty(),
        "truth documents:\n  {}",
        defects.join("\n  ")
    );
}

#[test]
fn truth_violation_ledger_is_well_formed() {
    let current = ledger::load();
    let mut problems = ledger::hygiene(&current, &truth::load_all());
    let on_disk = std::fs::read_to_string(ledger::path()).unwrap_or_default();
    if !on_disk.is_empty() && on_disk != ledger::render(&current) {
        problems
            .push("not in its committed form; rewrite it with UPDATE_TRUTH_LEDGER=1".to_string());
    }
    assert!(
        problems.is_empty(),
        "{}:\n  {}",
        ledger::PATH,
        problems.join("\n  ")
    );
}

/// The entries added since the base that are regressions: an entry for a fixture that existed at the
/// base, about a check that existed at the base and a truth fact the base already stated, that the base
/// did not record.
///
/// Five kinds of addition are baselines instead. A check introduced since the base describes defects
/// the rubric could not see before; a fixture introduced since the base had no entries to keep; a fact
/// the truth gained since the base - content a request carried that the truth then learned to state - is
/// an obligation the base never imposed; and a request entry whose request part - the one its subject
/// names, or the call's whole request - the base did not send, because the fixture's transcript was
/// recorded or re-recorded since with other content there; and an entry about a subject the base recorded
/// as missing in the same view, which is that defect changing form as its content starts to be shown. All still need a triaged backlog issue, which
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
    let added = regressions(
        &current_entries,
        &before,
        |family| registry.contains(&format!("\"{family}\"")),
        |fixture| fixtures_at_base.contains(fixture),
        |entry| {
            if changes_of_form.contains(&entry.id) {
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

/// Prints one fixture's views and violations, for triaging a ledger entry:
/// `TRUTH_FIXTURE=strands/native/tool_use cargo test --locked -p sideseat-server --test message_goldens
/// -- --ignored --nocapture truth_explain`.
#[test]
#[ignore]
fn truth_explain() {
    let Ok(fixture) = std::env::var("TRUTH_FIXTURE") else {
        eprintln!("set TRUTH_FIXTURE=<producer>/<mode>/<scenario>");
        return;
    };
    let truths = Truths::load();
    let fixtures = crate::discover_fixtures();
    let Some(paths) = fixtures
        .iter()
        .find(|(label, _)| *label == fixture)
        .map(|(_, paths)| paths.clone())
    else {
        // A prefix (`haystack/`, `adk/native/`) prints only the violations of every fixture under it,
        // so one producer's ledger entries can be checked without the whole corpus.
        let mut matched = 0;
        for (label, paths) in fixtures.iter().filter(|(l, _)| l.starts_with(&fixture)) {
            let Some(key) = truths.of_fixture.get(label) else {
                continue;
            };
            matched += 1;
            for violation in check(
                &truths.documents[key].for_fixture(label),
                &recon::build(label, paths),
            ) {
                eprintln!("{} - {}", violation.id(), violation.detail);
            }
        }
        assert!(matched > 0, "no fixture {fixture} or fixture under it");
        return;
    };
    let key = truths
        .of_fixture
        .get(&fixture)
        .expect("a truth describes it");
    let recon = recon::build(&fixture, &paths);
    for generation in &recon.generations {
        eprintln!(
            "span {} typed={} model={:?}/{:?} id={:?} finish={:?} usage={}/{}/{}/{}/{}",
            generation.label,
            generation.typed,
            generation.request_model,
            generation.response_model,
            generation.response_id,
            generation.finish,
            generation.input,
            generation.output,
            generation.cache_read,
            generation.cache_write,
            generation.reasoning
        );
    }
    for view in &recon.views {
        let name = recon
            .generations
            .iter()
            .find(|g| g.span == view.key)
            .map_or(view.key.as_str(), |g| g.label.as_str());
        eprintln!("--- {} {name}", view.kind.name());
        for block in &view.blocks {
            let content = super::canonical_json(&block.content);
            let head: String = content.chars().take(150).collect();
            eprintln!(
                "  {}{}/{} {head}",
                if block.output { ">" } else { " " },
                block.role,
                block.kind
            );
        }
    }
    for violation in check(&truths.documents[key].for_fixture(&fixture), &recon) {
        eprintln!("{} - {}", violation.id(), violation.detail);
    }
}
