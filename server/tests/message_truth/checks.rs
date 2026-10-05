//! Where each truth fact must appear, how many times, in which order, and identically in every view.
//!
//! A fact is assigned to a block by maximum bipartite matching within each scope, so two identical
//! legitimate facts need two blocks and one block cannot satisfy two facts. Extra reconstructed content
//! is not a failure by itself - the truth is a lower bound - but a second block showing an assigned fact
//! is a duplicate, and one in another trace is a leak.

use std::collections::{BTreeMap, BTreeSet};

use super::matching::Matching;
use super::predicates::{Shows, shows};
use super::recon::{Block, Recon, ViewKind};
use super::truth::{Fact, Truth};
use super::{Violation, ViolationView};

/// One block in a scope: its view, its index in that view, and the block.
pub(super) type Placed<'a> = (usize, usize, &'a Block);

/// A set of views checked together, and the facts they must show.
pub(super) struct Scope<'a> {
    pub kind: ViewKind,
    pub blocks: Vec<Placed<'a>>,
    pub facts: Vec<&'a Fact>,
    /// Restrict a fact's blocks to its home trace (every scope but a span's).
    pub by_trace: bool,
}

/// What the assignment found for one scope: fact id -> its block's position in `scope.blocks`.
#[derive(Default)]
pub(super) struct Assigned {
    pub block_of: BTreeMap<String, usize>,
    pub rewritten: BTreeMap<String, String>,
    /// Blocks a segmented answer covers beyond its first.
    pub consumed: BTreeSet<usize>,
}

pub(super) struct Context<'a> {
    pub truth: &'a Truth,
    pub recon: &'a Recon,
    pub matching: &'a Matching,
    facts: BTreeMap<&'a str, &'a Fact>,
    pub home_trace: BTreeMap<&'a str, String>,
}

impl<'a> Context<'a> {
    pub fn new(truth: &'a Truth, recon: &'a Recon, matching: &'a Matching) -> Self {
        let facts = truth.facts.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut context = Context {
            truth,
            recon,
            matching,
            facts,
            home_trace: BTreeMap::new(),
        };
        for fact in &truth.facts {
            if let Some(call) = context.home_call(fact)
                && let Some(&span) = matching.span_of.get(call)
            {
                let trace = recon.generations[span].trace.clone();
                context.home_trace.insert(fact.id.as_str(), trace);
            }
        }
        context
    }

    pub fn fact(&self, id: &str) -> Option<&'a Fact> {
        self.facts.get(id).copied()
    }

    /// The call a fact belongs to: its own; the call a prompt was sent to; the call a result answers;
    /// for anything else, the nearest call before it in its conversation, then after it.
    pub fn home_call(&self, fact: &'a Fact) -> Option<&'a str> {
        if let Some(call) = &fact.call {
            return Some(call);
        }
        for edge in &self.truth.edges {
            if edge.from.as_deref() == Some(fact.id.as_str()) {
                match edge.kind.as_str() {
                    "prompt_of" => return edge.to.as_deref(),
                    "result_of" => {
                        return edge
                            .to
                            .as_deref()
                            .and_then(|t| self.fact(t))
                            .and_then(|t| t.call.as_deref());
                    }
                    _ => {}
                }
            }
        }
        let conversation = self
            .truth
            .conversations
            .iter()
            .find(|c| c.id == fact.conversation)?;
        let at = conversation.sequence.iter().position(|id| *id == fact.id)?;
        if fact.kind == "user_media" {
            let prompt = conversation.sequence[..at]
                .iter()
                .rev()
                .filter_map(|id| self.fact(id))
                .find(|f| f.kind == "user_text")?;
            return self.home_call(prompt);
        }
        if fact.kind == "system" {
            return self
                .truth
                .calls
                .iter()
                .find(|c| c.conversation == fact.conversation && c.succeeded())
                .map(|c| c.id.as_str());
        }
        let before = conversation.sequence[..at].iter().rev();
        let after = conversation.sequence[at + 1..].iter();
        before
            .chain(after)
            .filter_map(|id| self.fact(id))
            .find_map(|f| f.call.as_deref())
    }

    fn asserted_in(&self, fact: &Fact, view: &str) -> bool {
        fact.require
            .as_ref()
            .is_some_and(|r| r.views.iter().any(|v| v == view))
    }
}

pub(super) fn check_placement(context: &Context<'_>, out: &mut Vec<Violation>) {
    let mut digests: BTreeMap<String, BTreeMap<ViewKind, BTreeSet<String>>> = BTreeMap::new();
    for scope in scopes(context) {
        let assigned = assign(context, &scope);
        for (fact, shown) in report_assignment(context, &scope, &assigned, out) {
            digests.entry(fact).or_default().insert(scope.kind, shown);
        }
        check_reasoning_kind(&scope, out);
        super::order::check(context, &scope, &assigned, out);
        super::explain::check(context, &scope, &assigned, out);
    }
    // The same fact, the same blocks: what a span shows is what its trace, session and feed show.
    for (fact, by_view) in &digests {
        let Some((reference_kind, reference)) = by_view.iter().next() else {
            continue;
        };
        for (kind, digest) in by_view {
            if digest != reference {
                out.push(Violation::new(
                    ViolationView::from(*kind),
                    "cross_view.differs",
                    fact,
                    format!(
                        "the {} view shows it differently from the {} view",
                        kind.name(),
                        reference_kind.name()
                    ),
                ));
            }
        }
    }
}

fn scopes<'a>(context: &Context<'a>) -> Vec<Scope<'a>> {
    let recon = context.recon;
    let truth = context.truth;
    let mut scopes = Vec::new();
    for call in truth.calls.iter().filter(|c| c.succeeded()) {
        let Some(&index) = context.matching.span_of.get(&call.id) else {
            continue;
        };
        let generation = &recon.generations[index];
        let Some((view_index, view)) = recon
            .views
            .iter()
            .enumerate()
            .find(|(_, v)| v.kind == ViewKind::Span && v.key == generation.span)
        else {
            continue;
        };
        let facts = call
            .outputs
            .iter()
            .filter_map(|id| context.fact(id))
            .filter(|f| context.asserted_in(f, "span"))
            .collect();
        scopes.push(Scope {
            kind: ViewKind::Span,
            blocks: view
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.output)
                .map(|(i, b)| (view_index, i, b))
                .collect(),
            facts,
            by_trace: false,
        });
    }
    let every_trace_in_a_session = recon
        .views
        .iter()
        .filter(|v| v.kind == ViewKind::Trace)
        .all(|v| recon.session_of_trace.contains_key(&v.key));
    for kind in [ViewKind::Trace, ViewKind::Session, ViewKind::Feed] {
        let blocks: Vec<Placed<'a>> = recon
            .views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.kind == kind)
            .flat_map(|(vi, v)| v.blocks.iter().enumerate().map(move |(i, b)| (vi, i, b)))
            .collect();
        let facts = truth
            .facts
            .iter()
            .filter(|f| context.asserted_in(f, kind.name()))
            .filter(|f| {
                // A session view exists only for a trace that belongs to a session.
                kind != ViewKind::Session
                    || match context.home_trace.get(f.id.as_str()) {
                        Some(trace) => recon.session_of_trace.contains_key(trace),
                        None => every_trace_in_a_session,
                    }
            })
            .collect();
        scopes.push(Scope {
            kind,
            blocks,
            facts,
            by_trace: true,
        });
    }
    scopes
}

/// How many consecutive blocks from `at` show a segmented answer one segment each, if they do.
fn segment_run(fact: &Fact, scope: &Scope<'_>, at: usize) -> Option<usize> {
    let segments = fact.value.get("segments")?.as_array()?;
    if segments.len() < 2 || at + segments.len() > scope.blocks.len() {
        return None;
    }
    let (view, start, _) = scope.blocks[at];
    let run = &scope.blocks[at..at + segments.len()];
    let consecutive = run
        .iter()
        .enumerate()
        .all(|(k, (v, i, b))| *v == view && *i == start + k && b.is("assistant", "text"));
    let equal = run
        .iter()
        .zip(segments)
        .all(|((_, _, b), s)| b.text() == s.as_str());
    (consecutive && equal).then_some(segments.len())
}

fn block_shows(fact: &Fact, scope: &Scope<'_>, at: usize, call_id: Option<&str>) -> Shows {
    match shows(fact, scope.blocks[at].2, call_id) {
        Shows::No if segment_run(fact, scope, at).is_some() => Shows::Yes,
        other => other,
    }
}

fn assign(context: &Context<'_>, scope: &Scope<'_>) -> Assigned {
    let mut assigned = Assigned::default();
    let (calls, results): (Vec<&Fact>, Vec<&Fact>) =
        scope.facts.iter().partition(|f| f.kind != "tool_result");
    let mut taken: BTreeMap<usize, String> = BTreeMap::new();
    // Results second: a result pairs with the id its call carries in this scope, which the first
    // phase decides.
    for phase in [calls, results] {
        let adjacency: Vec<Vec<usize>> = phase
            .iter()
            .map(|fact| {
                let call_id = paired_call_id(context, fact, &assigned);
                (0..scope.blocks.len())
                    .filter(|&at| !taken.contains_key(&at))
                    .filter(|&at| block_shows(fact, scope, at, call_id.as_deref()) != Shows::No)
                    .filter(|&at| in_home(context, scope, fact, at))
                    .collect()
            })
            .collect();
        let matched = maximum_matching(&adjacency);
        for (index, block) in matched.into_iter().enumerate() {
            let Some(at) = block else { continue };
            let fact = phase[index];
            if let Shows::WithRewrittenId(id) = shows(fact, scope.blocks[at].2, None) {
                assigned.rewritten.insert(fact.id.clone(), id);
            }
            assigned.block_of.insert(fact.id.clone(), at);
            taken.insert(at, fact.id.clone());
            if let Some(run) = segment_run(fact, scope, at) {
                for extra in at + 1..at + run {
                    taken.insert(extra, fact.id.clone());
                    assigned.consumed.insert(extra);
                }
            }
        }
    }
    assigned
}

/// Whether a block sits where the fact belongs: in the trace of its call and, in a session view, in
/// the session view of that trace's session - pooled session views must not trade conversations.
pub(super) fn in_home(context: &Context<'_>, scope: &Scope<'_>, fact: &Fact, at: usize) -> bool {
    let (view, _, block) = scope.blocks[at];
    let Some(trace) = context
        .home_trace
        .get(fact.id.as_str())
        .filter(|_| scope.by_trace)
    else {
        return true;
    };
    block.trace == *trace
        && (scope.kind != ViewKind::Session
            || context.recon.session_of_trace.get(trace) == Some(&context.recon.views[view].key))
}

/// The id a result must carry: its call's, as the call appears in this scope.
fn paired_call_id(context: &Context<'_>, fact: &Fact, assigned: &Assigned) -> Option<String> {
    if fact.kind != "tool_result" {
        return None;
    }
    let call = context
        .truth
        .edges
        .iter()
        .find(|e| e.kind == "result_of" && e.from.as_deref() == Some(fact.id.as_str()))
        .and_then(|e| e.to.clone())?;
    assigned.rewritten.get(&call).cloned().or_else(|| {
        context
            .fact(&call)
            .and_then(|c| c.value.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_owned)
    })
}

/// Kuhn's augmenting paths: for each left vertex, the right vertex it is matched to.
fn maximum_matching(adjacency: &[Vec<usize>]) -> Vec<Option<usize>> {
    fn augment(
        left: usize,
        adjacency: &[Vec<usize>],
        owner: &mut BTreeMap<usize, usize>,
        visited: &mut BTreeSet<usize>,
    ) -> bool {
        for &right in &adjacency[left] {
            if !visited.insert(right) {
                continue;
            }
            let free = match owner.get(&right).copied() {
                None => true,
                Some(other) => augment(other, adjacency, owner, visited),
            };
            if free {
                owner.insert(right, left);
                return true;
            }
        }
        false
    }
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    for left in 0..adjacency.len() {
        augment(left, adjacency, &mut owner, &mut BTreeSet::new());
    }
    let mut out = vec![None; adjacency.len()];
    for (right, left) in owner {
        out[left] = Some(right);
    }
    out
}

/// Reports what the assignment left unassigned or doubled; returns, per assigned fact, the digests of
/// every block in its trace that shows it.
fn report_assignment(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut shown_by = BTreeMap::new();
    let view = ViolationView::from(scope.kind);
    let claimed: BTreeSet<usize> = assigned.block_of.values().copied().collect();
    let consumed = &assigned.consumed;
    for fact in &scope.facts {
        let call_id = paired_call_id(context, fact, assigned);
        let anywhere: Vec<usize> = (0..scope.blocks.len())
            .filter(|&at| block_shows(fact, scope, at, call_id.as_deref()) != Shows::No)
            .collect();
        let kind = fact.kind.as_str();
        let Some(&at) = assigned.block_of.get(&fact.id) else {
            let home: Vec<&usize> = anywhere
                .iter()
                .filter(|&&at| in_home(context, scope, fact, at))
                .collect();
            let (assertion, detail) = if anywhere.is_empty() {
                (format!("{kind}.missing"), missing_hint(fact, scope))
            } else if home.is_empty() {
                (
                    format!("{kind}.wrong_trace"),
                    "shown only in a trace other than its call's".to_string(),
                )
            } else {
                (
                    format!("{kind}.claimed_twice"),
                    "its only blocks are assigned to identical facts".to_string(),
                )
            };
            out.push(Violation::new(view, &assertion, &fact.id, detail));
            continue;
        };
        if let Some(id) = assigned.rewritten.get(&fact.id) {
            out.push(Violation::new(
                view,
                "tool_call.id_rewritten",
                &fact.id,
                format!("the call is shown under the id {id:?}"),
            ));
        }
        shown_by.insert(
            fact.id.clone(),
            anywhere
                .iter()
                .filter(|&&b| in_home(context, scope, fact, b) && !consumed.contains(&b))
                .map(|&b| scope.blocks[b].2.digest.clone())
                .collect(),
        );
        // A copy is another block with the same content: a framework that wraps the prompt in a new
        // template each step shows it in several different blocks, which is not a duplicate.
        let digest = scope.blocks[at].2.identity.clone();
        let extra: Vec<usize> = anywhere
            .into_iter()
            .filter(|b| *b != at && !claimed.contains(b) && !consumed.contains(b))
            .filter(|&b| scope.blocks[b].2.identity == digest)
            .collect();
        let (here, elsewhere): (Vec<usize>, Vec<usize>) = extra
            .into_iter()
            .partition(|&b| in_home(context, scope, fact, b));
        if !here.is_empty() {
            out.push(Violation::new(
                view,
                &format!("{kind}.duplicated"),
                &fact.id,
                format!("shown {} times", here.len() + 1),
            ));
        }
        if !elsewhere.is_empty() {
            out.push(Violation::new(
                view,
                &format!("{kind}.leaked"),
                &fact.id,
                format!("also shown in {} other trace(s)", elsewhere.len()),
            ));
        }
    }
    shown_by
}

/// A stable one-line description of the nearest thing the view does show, for the ledger.
fn missing_hint(fact: &Fact, scope: &Scope<'_>) -> String {
    let blocks = || scope.blocks.iter().map(|(_, _, b)| *b);
    let text = fact.text();
    match fact.kind.as_str() {
        "tool_call" => {
            let name = fact
                .value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let id = fact.value.get("id").and_then(|n| n.as_str());
            if blocks().any(|b| b.kind == "tool_use" && b.call_id() == id && id.is_some()) {
                "a call with its id has another name or arguments".to_string()
            } else if blocks().any(|b| {
                b.kind == "tool_use"
                    && b.content
                        .get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| super::predicates::same_tool(n, name))
            }) {
                format!("{name} is called with other arguments")
            } else {
                format!("no {name} call")
            }
        }
        "tool_result" => {
            let id = fact.value.get("call_id").and_then(|n| n.as_str());
            match blocks().find(|b| b.is_tool_result() && b.result_call_id() == id) {
                Some(b) => format!(
                    "its result is shown as {}",
                    preview(&super::super::canonical_json(&b.content["content"]))
                ),
                None => "no result for its call id".to_string(),
            }
        }
        "user_media" => {
            let modality = fact
                .value
                .get("modality")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if blocks().any(|b| b.kind == modality) {
                format!("the {modality} has other bytes or media type")
            } else {
                format!("no {modality}")
            }
        }
        _ if !text.is_empty() => {
            match blocks().find(|b| b.text().is_some_and(|t| t == text || t.contains(text))) {
                Some(b) => format!("shown as {}/{}", b.role, b.kind),
                None if blocks()
                    .any(|b| b.text().is_some_and(|t| !t.is_empty() && text.contains(t))) =>
                {
                    "shown only in part".to_string()
                }
                None => "not shown".to_string(),
            }
        }
        _ => "not shown".to_string(),
    }
}

pub(super) fn preview(text: &str) -> String {
    let head: String = text.chars().take(60).collect();
    if head.len() < text.len() {
        format!("{head}...")
    } else {
        head
    }
}

/// Visible reasoning is a thinking block, never assistant text.
fn check_reasoning_kind(scope: &Scope<'_>, out: &mut Vec<Violation>) {
    for fact in scope.facts.iter().filter(|f| f.kind == "reasoning") {
        let text = fact.text();
        if text.is_empty() {
            continue;
        }
        if scope.blocks.iter().any(|(_, _, b)| {
            b.is("assistant", "text") && b.text().is_some_and(|t| t.contains(text))
        }) {
            out.push(Violation::new(
                ViolationView::from(scope.kind),
                "reasoning.as_text",
                &fact.id,
                "visible reasoning is shown as assistant text".to_string(),
            ));
        }
    }
}
