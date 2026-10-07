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
    /// For a fact a request re-sent, every trace it was sent in: its home is wherever the model was told it,
    /// so a client's preamble is at home in each trace of a session rather than in one of them.
    pub home_traces: BTreeMap<String, BTreeSet<String>>,
    /// The spans a fact a request re-sent may be shown on: those of the calls that were sent it.
    pub sent_spans: BTreeMap<String, BTreeSet<String>>,
    /// What this fixture's recorded requests account for in its views (`requests`).
    pub accounted: super::requests::Accounted,
    /// Each trace's capture-stable label (`trace-2`), for naming a per-trace obligation.
    pub trace_label: BTreeMap<String, String>,
}

impl<'a> Context<'a> {
    pub fn new(
        truth: &'a Truth,
        recon: &'a Recon,
        matching: &'a Matching,
        accounted: super::requests::Accounted,
    ) -> Self {
        let facts = truth.facts.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut context = Context {
            truth,
            recon,
            matching,
            facts,
            home_trace: BTreeMap::new(),
            home_traces: BTreeMap::new(),
            sent_spans: BTreeMap::new(),
            accounted,
            trace_label: BTreeMap::new(),
        };
        for (fact, calls) in super::requests::sent_to(truth, recon) {
            let spans: Vec<usize> = calls
                .iter()
                .filter_map(|call| super::requests::span_sent(truth, matching, call))
                .collect();
            // No call it was sent to has a span: where it belongs is unknown, and the matcher has already
            // reported the call it could not find. The fact keeps the ordinary single home instead.
            if spans.is_empty() {
                continue;
            }
            context.home_traces.insert(
                fact.clone(),
                spans
                    .iter()
                    .map(|&s| recon.generations[s].trace.clone())
                    .collect(),
            );
            // Only the spans the matcher found: a neighbour's span stands in for an unfound call's trace,
            // never for the span that call was shown on.
            context.sent_spans.insert(
                fact,
                calls
                    .iter()
                    .filter_map(|call| matching.span_of.get(call))
                    .map(|&s| recon.generations[s].span.clone())
                    .collect(),
            );
        }
        for generation in &recon.generations {
            if let Some((label, _)) = generation.label.split_once('/') {
                context
                    .trace_label
                    .entry(generation.trace.clone())
                    .or_insert_with(|| label.to_string());
            }
        }
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
        if let Some(call) = fact.call.as_deref().filter(|c| !self.unexported(c)) {
            return Some(call);
        }
        for edge in &self.truth.edges {
            if edge.from.as_deref() == Some(fact.id.as_str()) {
                match edge.kind.as_str() {
                    "prompt_of" => return edge.to.as_deref(),
                    "result_of" => {
                        let call = edge
                            .to
                            .as_deref()
                            .and_then(|t| self.fact(t))
                            .and_then(|t| t.call.as_deref());
                        if call.is_some_and(|c| self.unexported(c)) {
                            break;
                        }
                        return call;
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
        // An unexported response's parts belong to the nearest call that has a span: the run that
        // carried them. Searched forward first for them, since the run's span is the one answering.
        if fact.call.as_deref().is_some_and(|c| self.unexported(c)) || self.answers_unexported(fact)
        {
            return after
                .chain(before)
                .filter_map(|id| self.fact(id))
                .filter_map(|f| f.call.as_deref())
                .find(|c| !self.unexported(c));
        }
        before
            .chain(after)
            .filter_map(|id| self.fact(id))
            .filter_map(|f| f.call.as_deref())
            .find(|c| !self.unexported(c))
    }

    /// Whether a call's response is proven absent from the telemetry (`call_not_exported`).
    pub fn unexported(&self, call: &str) -> bool {
        self.truth
            .gaps
            .iter()
            .any(|g| g.reason == "call_not_exported" && g.subject.as_deref() == Some(call))
    }

    /// Whether a fact is the result of a call an unexported response made.
    fn answers_unexported(&self, fact: &Fact) -> bool {
        self.truth.edges.iter().any(|e| {
            e.kind == "result_of"
                && e.from.as_deref() == Some(fact.id.as_str())
                && e.to
                    .as_deref()
                    .and_then(|t| self.fact(t))
                    .and_then(|t| t.call.as_deref())
                    .is_some_and(|c| self.unexported(c))
        })
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
                // Oldest first, as the conversation happened: the feed lists newest first, so a fact
                // more than one block shows - a prompt the framework restates every step - prefers the
                // copy the conversation sent first in every view alike.
                // A block showing the call under its own id is tried before one under another id: a
                // rewrite is the explanation of last resort, never a rival to the call's own block.
                let mut candidates: Vec<(bool, usize)> = chronological(scope)
                    .filter(|&at| !taken.contains_key(&at))
                    .filter_map(
                        |at| match block_shows(fact, scope, at, call_id.as_deref()) {
                            Shows::No => None,
                            Shows::WithRewrittenId(_) => Some((true, at)),
                            Shows::Yes | Shows::Assigned(_) => Some((false, at)),
                        },
                    )
                    .filter(|&(_, at)| in_home(context, scope, fact, at))
                    .collect();
                candidates.sort_by_key(|&(rewritten, _)| rewritten);
                candidates.into_iter().map(|(_, at)| at).collect()
            })
            .collect();
        let matched = in_sequence(
            context,
            scope,
            &phase,
            &adjacency,
            maximum_matching(&adjacency),
        );
        for (index, block) in matched.into_iter().enumerate() {
            let Some(at) = block else { continue };
            let fact = phase[index];
            if let Shows::WithRewrittenId(id) | Shows::Assigned(id) =
                shows(fact, scope.blocks[at].2, None)
            {
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
    if !scope.by_trace {
        return true;
    }
    let at_home = |trace: &String| {
        block.trace == *trace
            && (scope.kind != ViewKind::Session
                || context.recon.session_of_trace.get(trace)
                    == Some(&context.recon.views[view].key))
    };
    // A fact the requests re-sent is at home in every trace that was sent it.
    if let Some(traces) = context.home_traces.get(fact.id.as_str()) {
        return traces.iter().any(at_home);
    }
    let Some(trace) = context.home_trace.get(fact.id.as_str()) else {
        return true;
    };
    at_home(trace)
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

/// The scope's blocks in the order the conversation happened: view order, reversed in the feed.
fn chronological(scope: &Scope<'_>) -> Box<dyn Iterator<Item = usize>> {
    if scope.kind == ViewKind::Feed {
        Box::new((0..scope.blocks.len()).rev())
    } else {
        Box::new(0..scope.blocks.len())
    }
}

/// A matching in which interchangeable facts take their blocks in the truth's order.
///
/// Two facts with the same kind, value and matcher and exactly the same candidate blocks are
/// indistinguishable to every check - two identical calls a framework names itself, say - so any
/// assignment of those blocks between them is as valid as another, and Kuhn's order would decide which one
/// the order checks see first. The truth's sequence decides instead: the earlier fact takes the earlier
/// block. Facts whose candidates differ are never exchanged, so no fact can receive a block it does not
/// show or that sits outside its home.
fn in_sequence(
    context: &Context<'_>,
    scope: &Scope<'_>,
    phase: &[&Fact],
    adjacency: &[Vec<usize>],
    mut matched: Vec<Option<usize>>,
) -> Vec<Option<usize>> {
    let position: BTreeMap<&str, usize> = context
        .truth
        .conversations
        .iter()
        .flat_map(|c| &c.sequence)
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    type Interchangeable<'f> = (String, String, Option<&'f str>, Vec<usize>);
    let mut groups: BTreeMap<Interchangeable<'_>, Vec<usize>> = BTreeMap::new();
    for (index, fact) in phase.iter().enumerate() {
        if matched[index].is_some() {
            let mut candidates = adjacency[index].clone();
            candidates.sort_unstable();
            groups
                .entry((
                    fact.kind.clone(),
                    super::super::canonical_json(&fact.value),
                    fact.require.as_ref().map(|r| r.matcher.as_str()),
                    candidates,
                ))
                .or_default()
                .push(index);
        }
    }
    for members in groups.values().filter(|m| m.len() > 1) {
        let mut facts = members.clone();
        facts.sort_by_key(|&i| position.get(phase[i].id.as_str()).copied());
        let mut blocks: Vec<usize> = members.iter().filter_map(|&i| matched[i]).collect();
        let rank: BTreeMap<usize, usize> = chronological(scope)
            .enumerate()
            .map(|(r, at)| (at, r))
            .collect();
        blocks.sort_by_key(|at| rank.get(at).copied());
        for (fact, block) in facts.into_iter().zip(blocks) {
            matched[fact] = Some(block);
        }
    }
    matched
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
        // A call the wire gave no id is named by the framework, which is no rewrite - unless the name is
        // another call's too, when the two results can no longer tell their calls apart.
        let framework_named = fact.value.get("id").is_some_and(serde_json::Value::is_null);
        if framework_named
            && let Some(id) = assigned.rewritten.get(&fact.id).filter(|id| !id.is_empty())
            && let Some(other) = assigned
                .rewritten
                .iter()
                .find(|(other, shared)| *other != &fact.id && *shared == id)
                .map(|(other, _)| other)
        {
            out.push(Violation::new(
                view,
                "tool_call.id_rewritten",
                &fact.id,
                format!("the framework names it {id:?}, as it names {other}"),
            ));
        }
        if let Some(id) = assigned.rewritten.get(&fact.id)
            && !framework_named
        {
            out.push(Violation::new(
                view,
                "tool_call.id_rewritten",
                &fact.id,
                format!("the call is shown under the id {id:?}"),
            ));
        }
        // Every block showing the fact but one another fact owns: two identical calls each show the
        // other under a different id, and the other's block is that call, not a second showing of this.
        shown_by.insert(
            fact.id.clone(),
            anywhere
                .iter()
                .filter(|&&b| in_home(context, scope, fact, b) && !consumed.contains(&b))
                .filter(|&&b| b == at || !claimed.contains(&b))
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
        // A fact the requests re-sent is shown once per trace that was sent it: a client's preamble goes
        // with every request of a session, so the session view holds one copy per trace and a second copy
        // *in one trace* is the duplicate.
        let per_trace = context.home_traces.contains_key(fact.id.as_str());
        let shown_trace = scope.blocks[at].2.trace.as_str();
        let (here, elsewhere): (Vec<usize>, Vec<usize>) = extra.into_iter().partition(|&b| {
            in_home(context, scope, fact, b)
                && (!per_trace || scope.blocks[b].2.trace == shown_trace)
        });
        let elsewhere: Vec<usize> = elsewhere
            .into_iter()
            .filter(|&b| !in_home(context, scope, fact, b))
            .collect();
        if !here.is_empty() {
            out.push(Violation::new(
                view,
                &format!("{kind}.duplicated"),
                &fact.id,
                format!("shown {} times", here.len() + 1),
            ));
        }
        if per_trace {
            report_other_home_traces(context, scope, fact, at, &claimed, out);
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

/// A fact the requests sent in several traces is owed once in each of them, not once overall: the assignment
/// places it in one, and here every other home trace this scope holds must show exactly one unclaimed copy.
/// Named `<fact>@<trace>`, so each trace's obligation is its own entry.
fn report_other_home_traces(
    context: &Context<'_>,
    scope: &Scope<'_>,
    fact: &Fact,
    at: usize,
    claimed: &BTreeSet<usize>,
    out: &mut Vec<Violation>,
) {
    let Some(traces) = context.home_traces.get(fact.id.as_str()) else {
        return;
    };
    let assigned_trace = scope.blocks[at].2.trace.as_str();
    let held: BTreeSet<&str> = scope
        .blocks
        .iter()
        .map(|(_, _, b)| b.trace.as_str())
        .collect();
    let kind = fact.kind.as_str();
    for trace in traces.iter().filter(|t| t.as_str() != assigned_trace) {
        // A trace this scope does not hold - one outside any session, in a session scope - owes nothing here.
        if !held.contains(trace.as_str())
            || (scope.kind == ViewKind::Session
                && !context.recon.session_of_trace.contains_key(trace))
        {
            continue;
        }
        let copies = (0..scope.blocks.len())
            .filter(|&b| scope.blocks[b].2.trace == *trace && !claimed.contains(&b))
            .filter(|&b| in_home(context, scope, fact, b))
            .filter(|&b| block_shows(fact, scope, b, None) != Shows::No)
            .count();
        let label = context
            .trace_label
            .get(trace)
            .map_or("an unlabelled trace", String::as_str);
        let subject = format!("{}@{label}", fact.id);
        match copies {
            1 => {}
            0 => out.push(Violation::new(
                ViolationView::from(scope.kind),
                &format!("{kind}.missing"),
                &subject,
                format!("{label} was sent it and does not show it"),
            )),
            n => out.push(Violation::new(
                ViolationView::from(scope.kind),
                &format!("{kind}.duplicated"),
                &subject,
                format!("shown {n} times in {label}"),
            )),
        }
    }
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
