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
//!    assistant text; 7. prompts, the system prompt and attachments are present (`predicates`, `checks`);
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
mod mutations;
mod order;
mod predicates;
mod recon;
mod truth;

use std::collections::BTreeMap;

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
    "order.sequence",
    "extra.unexplained",
    "attribution.span",
    "attribution.call_order",
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
    let matching = matching::match_calls(truth, recon, &mut out);
    matching::check_metadata(truth, recon, &matching, &mut out);
    let context = checks::Context::new(truth, recon, &matching);
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
        check(truth, &recon::from_built(fixture, built, spans))
    }
}

/// Compares a full run's violations with the ledger, or rewrites the ledger under
/// `UPDATE_TRUTH_LEDGER=1`. Returns what is wrong, for `message_goldens` to report beside its diffs.
///
/// `delivery` selects which part of the ledger the run owns: the truth comparison's, or the delivery
/// invariance test's (`view: delivery`). Each run compares and rewrites only its own part.
pub(crate) fn ledger_problems(observed: &[Violation], delivery: bool) -> Vec<String> {
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
    let owned = |e: &ledger::Entry| (e.view == ViolationView::Delivery.name()) == delivery;
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
/// base, about a check that existed at the base, that the base did not record.
///
/// Two kinds of addition are baselines instead. A check introduced since the base describes defects
/// the rubric could not see before; a fixture introduced since the base had no entries to keep. Both
/// still need a triaged backlog issue, which `truth_violation_ledger_is_well_formed` enforces.
fn regressions(
    entries: &[ledger::Entry],
    recorded_at_base: &std::collections::BTreeSet<String>,
    check_existed: impl Fn(&str) -> bool,
    fixture_existed: impl Fn(&str) -> bool,
) -> Vec<String> {
    entries
        .iter()
        .filter(|e| !recorded_at_base.contains(&e.id))
        .filter(|e| check_existed(&family(&e.assertion)) && fixture_existed(&e.fixture))
        .map(|e| e.id.clone())
        .collect()
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
    let before: std::collections::BTreeSet<String> = ledger::parse(&before)
        .entries
        .into_iter()
        .map(|e| e.id)
        .collect();
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
    let added = regressions(
        &ledger::load().entries,
        &before,
        |family| registry.contains(&format!("\"{family}\"")),
        |fixture| fixtures_at_base.contains(fixture),
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
    );
    // An existing fixture under an existing check gains nothing; a new fixture or a new check brings
    // its own baseline; what the base recorded stays allowed.
    assert_eq!(refused, vec![entries[0].id.clone()]);
    let none_new = regressions(&entries, &base, |_| true, |_| true);
    assert_eq!(
        none_new.len(),
        3,
        "with nothing new, every addition is a regression"
    );
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
            for violation in check(&truths.documents[key], &recon::build(label, paths)) {
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
    for violation in check(&truths.documents[key], &recon) {
        eprintln!("{} - {}", violation.id(), violation.detail);
    }
}
