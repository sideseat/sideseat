//! Which repeats the truth accounts for: a block whose copies each have a fact of their own.
//!
//! The goldens' duplicate invariant (`assert_no_duplicates`) reads it to tell a true repeat from a duplicate. A
//! framework that quotes each earlier step behind the same preamble sent the preamble once per step: four copies
//! in a trace whose truth holds four such facts are four facts. Where the truth holds no fact a block shows, or too
//! few, it cannot decide, and the invariant keeps its own rule.
//!
//! The facts are allocated, not counted: in each view, the blocks are matched to the facts they show, one fact to a
//! block (maximum bipartite matching), and a block is accounted for only where every block linked to it through a
//! fact they could both be - a prompt, and a text containing it; a segmented answer's whole and its run of
//! segments - gets one of its own. A count per content let two contents spend the same facts.
//!
//! A trace's views - its trace view, the session and feed views it is part of - may show what the trace holds. A
//! span view shows a call's conversation up to that call: what had been said by the last call recorded on the
//! viewed span or below it, in its own trace, since history only ever re-sends what came before.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::checks::{Context, maximum_matching};
use super::predicates::{Shows, shows};
use super::recon::{Block, Generation, View, ViewKind};
use super::truth::{Fact, UNSIGNED_ON_SPAN};

/// Per view, in the order `Built::invariants` lists them, and per block of that view: what the truth allows it.
pub(crate) type Allowances = Vec<Vec<Allowance>>;

/// What the truth allows one block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Allowance {
    /// How many of the truth's facts in the view's scope the block shows as what they are, not merely as text
    /// containing them: the most copies of this content the truth can account for.
    pub facts: usize,
    /// Whether the allocation gave this block a fact of its own, as it did every block it competes with -
    /// containment included, since a text containing a prompt could be that prompt's block.
    pub accounted: bool,
}

/// What the rendered views leave out and the allowance needs, per view in `Built::invariants` order.
#[derive(Default)]
pub(super) struct ViewRows<'a> {
    /// Per block, the SHA-256 of a reasoning block's signature: a signed block shows only the facts sealed with
    /// it, so two copies of one signature are never two turns.
    pub seals: Vec<Vec<Option<&'a str>>>,
    /// For a span view, its span's trace: the trace its reach is measured in.
    pub span_traces: Vec<Option<&'a str>>,
    /// Per block, the group of copies the duplicate invariant compares it within (`duplicate_group`): the copies
    /// the allocation must give exact facts of their own.
    pub groups: Vec<Vec<String>>,
}

pub(super) fn allowances(context: &Context<'_>, rows: &ViewRows<'_>) -> Allowances {
    let recon = context.recon;
    // Each fact with the start of the call it belongs to, where the matcher found that call's span.
    let facts: Vec<(&Fact, Option<DateTime<Utc>>)> = context
        .truth
        .facts
        .iter()
        .map(|fact| {
            let start = context
                .home_call(fact)
                .and_then(|call| context.matching.span_showing(call))
                .map(|at| recon.generations[at].start);
            (fact, start)
        })
        .collect();
    recon
        .views
        .iter()
        .enumerate()
        .map(|(at, view)| {
            let span_view = view.kind == ViewKind::Span;
            // The viewed span's own reach, applied to every block it shows - one composed from another trace's
            // history too, which is bounded by when this call was made, not by that trace's later calls.
            let horizon = span_view
                .then(|| {
                    horizon(
                        &recon.generations,
                        rows.span_traces.get(at).copied().flatten(),
                        &view.key,
                    )
                })
                .flatten();
            let in_scope = |fact: &Fact, start: &Option<DateTime<Utc>>, trace: &str| {
                at_home(context, fact, trace)
                    && match (horizon, start) {
                        (Some(horizon), Some(start)) => *start <= horizon,
                        _ => true,
                    }
            };
            let seals = rows.seals.get(at);
            // Each block's facts, read once per identity: blocks with one identity show the same facts.
            let mut by_identity: HashMap<(&str, &str, Option<&str>), Showing> = HashMap::new();
            let showings: Vec<Showing> = view
                .blocks
                .iter()
                .enumerate()
                .map(|(index, block)| {
                    let seal = seals.and_then(|view| view.get(index)).copied().flatten();
                    by_identity
                        .entry((block.trace.as_str(), block.identity.as_str(), seal))
                        .or_insert_with(|| {
                            let mut showing = Showing::default();
                            for f in (0..facts.len())
                                .filter(|&f| in_scope(facts[f].0, &facts[f].1, &block.trace))
                                // A signed block shows only the turn sealed with its signature.
                                .filter(|&f| {
                                    seal.is_none_or(|seal| facts[f].0.seal.as_deref() == Some(seal))
                                })
                            {
                                if let Some(exact) = shown(facts[f].0, block, span_view) {
                                    showing.all.push(f);
                                    if exact && !loosely(facts[f].0) {
                                        showing.exact.push(f);
                                    }
                                }
                            }
                            // A block that could be either of two different facts is exactly neither: whichever
                            // matcher let it - containment, a substring of an error, a modality without a
                            // digest, a namespaced name - it does not say which it is.
                            let values: std::collections::BTreeSet<(&str, String)> = showing
                                .all
                                .iter()
                                .map(|&f| {
                                    (
                                        facts[f].0.kind.as_str(),
                                        crate::canonical_json(&facts[f].0.value),
                                    )
                                })
                                .collect();
                            if values.len() > 1 {
                                showing.exact.clear();
                            }
                            showing
                        })
                        .clone()
                })
                .collect();
            let mut adjacency: Vec<Vec<usize>> = showings.iter().map(|s| s.all.clone()).collect();
            // A segmented answer's run of segments is a showing of its own, competing with a whole copy.
            adjacency.extend(segment_runs(view, &facts, &in_scope));
            // The copies of one content in this view, as the duplicate invariant groups them - by content, whatever
            // side-channel id a copy carries; without its groups, by trace, digest and seal.
            let groups = rows.groups.get(at);
            let mut copies: HashMap<String, Vec<usize>> = HashMap::new();
            for (index, block) in view.blocks.iter().enumerate() {
                let key = match groups.and_then(|groups| groups.get(index)) {
                    Some(group) => group.clone(),
                    None => {
                        let seal = seals.and_then(|view| view.get(index)).copied().flatten();
                        format!("{}|{}|{seal:?}", block.trace, block.digest)
                    }
                };
                copies.entry(key).or_default().push(index);
            }
            let mut copies: Vec<Vec<usize>> =
                copies.into_values().filter(|m| m.len() > 1).collect();
            copies.sort();
            let exact: Vec<Vec<usize>> = showings.iter().map(|s| s.exact.clone()).collect();
            let accounted = standing(&adjacency, &exact, &copies);
            (0..view.blocks.len())
                .map(|index| Allowance {
                    facts: showings[index].exact.len(),
                    accounted: accounted[index],
                })
                .collect()
        })
        .collect()
}

/// Which showings the allocation accounts for (`accounted`), where every set of `copies` of one content must stand
/// on facts each shows exactly (`exact`, indexed like the first showings of `adjacency`) - all of them at once, since
/// two sets can reach for the same facts - while every other showing still competes with all it could be: a call
/// under another's id, a text containing a prompt.
pub(super) fn standing(
    adjacency: &[Vec<usize>],
    exact: &[Vec<usize>],
    copies: &[Vec<usize>],
) -> Vec<bool> {
    let mut exact_only = adjacency.to_vec();
    for &member in copies.iter().flatten() {
        exact_only[member] = exact[member].clone();
    }
    // Matched on the restricted edges, linked on the broad ones: restricting an edge must not cut a showing off
    // from the rivals it still competes with for the same facts.
    accounted_with(&exact_only, adjacency)
}

/// For each left node of `adjacency` (a showing, and the facts it could be), whether the allocation accounts for it:
/// it is matched, and so is every showing linked to it through a fact they share. A maximum matching's size is
/// unique, so where a linked set has as many matches as members every maximum matching matches them all.
pub(super) fn accounted(adjacency: &[Vec<usize>]) -> Vec<bool> {
    accounted_with(adjacency, adjacency)
}

/// `accounted`, with the matching run on `matching` and the linking on `linking`.
fn accounted_with(matching: &[Vec<usize>], linking: &[Vec<usize>]) -> Vec<bool> {
    let matched = maximum_matching(matching);
    // Union the showings that share a fact.
    let mut parent: Vec<usize> = (0..linking.len()).collect();
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    let mut first_of_fact: HashMap<usize, usize> = HashMap::new();
    for (left, facts) in linking.iter().enumerate() {
        for &fact in facts {
            let other = *first_of_fact.entry(fact).or_insert(left);
            let (a, b) = (root(&mut parent, left), root(&mut parent, other));
            parent[a] = b;
        }
    }
    let mut unmatched_in: HashMap<usize, bool> = HashMap::new();
    for (left, matched) in matched.iter().enumerate() {
        let group = root(&mut parent, left);
        *unmatched_in.entry(group).or_default() |= matched.is_none();
    }
    (0..matching.len())
        .map(|left| !matching[left].is_empty() && !unmatched_in[&root(&mut parent, left)])
        .collect()
}

/// Whether a fact, with its call's start, is in a view's scope for a block of the given trace.
type InScope<'a> = dyn Fn(&Fact, &Option<DateTime<Utc>>, &str) -> bool + 'a;

/// Every run of consecutive assistant texts in `view` that shows a segmented answer one segment each, with the
/// facts in scope whose segments they are.
fn segment_runs(
    view: &View,
    facts: &[(&Fact, Option<DateTime<Utc>>)],
    in_scope: &InScope<'_>,
) -> Vec<Vec<usize>> {
    let mut by_segments: HashMap<String, Vec<usize>> = HashMap::new();
    // Only facts a block can show at all: an unasserted fact is shown by nothing, so a run is no showing of it.
    for (f, (fact, _)) in facts
        .iter()
        .enumerate()
        .filter(|(_, (fact, _))| fact.require.is_some())
    {
        if let Some(segments) = fact.value.get("segments").and_then(|s| s.as_array())
            && segments.len() >= 2
        {
            by_segments
                .entry(crate::canonical_json(&serde_json::Value::Array(
                    segments.clone(),
                )))
                .or_default()
                .push(f);
        }
    }
    let mut runs = Vec::new();
    #[expect(
        clippy::iter_over_hash_type,
        reason = "the runs are a set of showings; order does not matter"
    )]
    for candidates in by_segments.values() {
        let segments = facts[candidates[0]].0.value["segments"]
            .as_array()
            .expect("segments");
        let blocks = &view.blocks;
        for start in 0..blocks.len().saturating_sub(segments.len() - 1) {
            let run = &blocks[start..start + segments.len()];
            let shows_run = run
                .iter()
                .zip(segments)
                .all(|(b, s)| b.is("assistant", "text") && b.text() == s.as_str());
            if shows_run {
                runs.push(
                    candidates
                        .iter()
                        .copied()
                        .filter(|&f| {
                            run.iter()
                                .all(|b| in_scope(facts[f].0, &facts[f].1, &b.trace))
                        })
                        .collect(),
                );
            }
        }
    }
    runs
}

/// Whether a fact is shown by any text containing it: a block showing it so could be its block, and is not
/// thereby a copy of it.
fn loosely(fact: &Fact) -> bool {
    fact.require
        .as_ref()
        .is_some_and(|require| require.matcher == "contains")
}

/// The facts a block could be, and those it shows as exactly what they are.
#[derive(Clone, Default)]
struct Showing {
    all: Vec<usize>,
    exact: Vec<usize>,
}

/// Whether `block` shows `fact`, as the placement checks read it - withheld reasoning whose producing span does not
/// carry its signature is shown unsigned there - and whether exactly: a call shown under an id the wire did not give
/// it could be the block of either of two identical calls, and is a copy of neither.
fn shown(fact: &Fact, block: &Block, span_view: bool) -> Option<bool> {
    let read = |fact: &Fact| match shows(fact, block, None) {
        Shows::No => None,
        Shows::Yes => Some(true),
        Shows::WithRewrittenId(_) | Shows::Assigned(_) => Some(false),
    };
    read(fact)
        .or_else(|| {
            (span_view && fact.value.get(UNSIGNED_ON_SPAN).is_some())
                .then(|| {
                    let mut unsigned = fact.clone();
                    unsigned.value["signed"] = serde_json::Value::Bool(false);
                    read(&unsigned)
                })
                .flatten()
        })
        .or_else(|| {
            // A result rendered under an id the wire never gave - one the placement assigned through
            // `result_of` - still shows its fact's content, so it competes for the fact; never exactly, since
            // the id is not the wire's. Without this edge the rendered copies left the facts free for a
            // duplicate's copies to spend.
            (fact.kind == "tool_result" && shows(fact, block, block.result_call_id()) != Shows::No)
                .then_some(false)
        })
}

/// The start of the last span recorded on or below `span`, in the span's own trace: how far its conversation
/// reaches. Without the trace - a reconstruction built without its views' scopes - every trace holding that span id.
fn horizon(generations: &[Generation], trace: Option<&str>, span: &str) -> Option<DateTime<Utc>> {
    let traces: Vec<&str> = match trace {
        Some(trace) => vec![trace],
        None => generations
            .iter()
            .filter(|g| g.span == span)
            .map(|g| g.trace.as_str())
            .collect(),
    };
    generations
        .iter()
        .filter(|g| traces.contains(&g.trace.as_str()))
        .filter(|g| g.span == span || g.ancestors.iter().any(|a| a == span))
        .map(|g| g.start)
        .max()
}

/// Whether a fact belongs to a trace: one of those it was sent in, or its call's. A fact neither names is at home
/// anywhere.
fn at_home(context: &Context<'_>, fact: &Fact, trace: &str) -> bool {
    if let Some(traces) = context.home_traces.get(&fact.id) {
        return traces.contains(trace);
    }
    context
        .home_trace
        .get(fact.id.as_str())
        .is_none_or(|home| home == trace)
}

/// The allowance of the first block of `view` showing `text`, given the truth and each view's seals.
fn allowance_of(
    truth: &super::truth::Truth,
    recon: &super::recon::Recon,
    rows: &ViewRows<'_>,
    view: usize,
    kind: &str,
) -> usize {
    let (_, allowed) = super::checked(truth, recon, rows);
    let at = recon.views[view]
        .blocks
        .iter()
        .position(|b| b.kind == kind)
        .expect("the block");
    allowed[view][at].facts
}

/// Different facts a block shows are not copies of one: four preambles the truth holds four times allow four, and
/// when one of those facts is another prompt that merely occurs inside the preamble's text, a preamble could be
/// either and is exactly neither.
#[test]
fn different_facts_one_block_shows_allow_one_copy_between_them() {
    let (mut truth, recon, _) = super::mutations::baseline("adk/native/multi_agent");
    let view = recon
        .views
        .iter()
        .position(|v| {
            v.kind == ViewKind::Trace
                && v.blocks
                    .iter()
                    .any(|b| b.text().is_some_and(|t| t.starts_with("For context:")))
        })
        .expect("the trace quoting the steps");
    let preamble = |b: &Block| b.text().is_some_and(|t| t.starts_with("For context:"));
    let at = recon.views[view]
        .blocks
        .iter()
        .position(preamble)
        .expect("a preamble");
    let (_, allowed) = super::checked(&truth, &recon, &ViewRows::default());
    assert_eq!(allowed[view][at].facts, 4, "four identical facts");
    let fact = truth
        .facts
        .iter_mut()
        .find(|f| f.text().starts_with("For context:"))
        .expect("a preamble fact");
    fact.value["text"] = serde_json::Value::String("For context:".to_string());
    fact.require.as_mut().expect("asserted").matcher = "contains".to_string();
    let (_, allowed) = super::checked(&truth, &recon, &ViewRows::default());
    assert_eq!(
        allowed[view][at].facts, 0,
        "a preamble that could be a preamble or the prompt it contains is exactly neither"
    );
}

/// A signed block shows only the turn sealed with its signature: none, where the truth sealed no turn with it.
#[test]
fn a_signed_block_counts_only_the_turn_sealed_with_its_signature() {
    let (mut truth, recon, _) = super::mutations::baseline("pydantic-ai/native/multi_turn");
    let span = recon
        .generations
        .iter()
        .find(|g| g.label.ends_with("invoke_agent agent/span-6"))
        .expect("the last turn's agent span")
        .span
        .clone();
    let view = recon
        .views
        .iter()
        .position(|v| v.kind == ViewKind::Span && v.key == span)
        .expect("its view");
    let seal_of = |seal: Option<&'static str>| -> ViewRows<'static> {
        let seals = recon
            .views
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.blocks
                    .iter()
                    .map(|b| {
                        (i == view && b.kind == "thinking")
                            .then_some(seal)
                            .flatten()
                    })
                    .collect()
            })
            .collect();
        ViewRows {
            seals,
            ..ViewRows::default()
        }
    };
    let sealed: Vec<String> = truth.facts.iter().filter_map(|f| f.seal.clone()).collect();
    assert_eq!(sealed.len(), 2, "two withheld turns, each sealed");
    let first: &'static str = Box::leak(sealed[0].clone().into_boxed_str());
    assert_eq!(
        allowance_of(&truth, &recon, &seal_of(None), view, "thinking"),
        2
    );
    assert_eq!(
        allowance_of(&truth, &recon, &seal_of(Some(first)), view, "thinking"),
        1
    );
    assert_eq!(
        allowance_of(&truth, &recon, &seal_of(Some("00")), view, "thinking"),
        0
    );
    for fact in &mut truth.facts {
        fact.seal = None;
    }
    assert_eq!(
        allowance_of(&truth, &recon, &seal_of(Some(first)), view, "thinking"),
        0,
        "no turn the truth sealed with it, so none it shows"
    );
}

/// The allocation accounts for a showing only where every showing it competes with gets a fact too: two contents
/// sharing two facts - a prompt, and a text containing it - are four showings for two facts; a duplicate prompt
/// beside texts that contain it and other prompts is one showing too many for the facts they all compete for.
#[test]
fn the_allocation_accounts_for_a_showing_only_where_its_rivals_are_too() {
    // Two showings of two identical facts: both stand.
    assert_eq!(accounted(&[vec![0, 1], vec![0, 1]]), [true, true]);
    // Two contents, two copies each, all of the same two facts: none stands.
    assert_eq!(accounted(&vec![vec![0, 1]; 4]), [false; 4]);
    // `alpha` twice, `alpha beta`, and three `beta ...`, for two alpha facts (0, 1) and three beta facts (2, 3, 4).
    let mixed = [
        vec![0, 1],
        vec![0, 1, 2, 3, 4],
        vec![2, 3, 4],
        vec![2, 3, 4],
        vec![2, 3, 4],
        vec![0, 1],
    ];
    assert_eq!(accounted(&mixed), [false; 6]);
    // A showing of no fact stands for nothing, and does not spoil its neighbours.
    assert_eq!(accounted(&[vec![0], vec![]]), [true, false]);
}

/// A segmented answer's run of segments is a showing of its own: two whole copies beside two complete runs are four
/// showings of two answers.
#[test]
fn a_run_of_segments_competes_with_whole_copies() {
    let fact = |id: &str| -> Fact {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": "text", "role": "assistant", "conversation": "conv-1", "evidence": "wire",
            "value": {"text": "AB", "segments": ["A", "B"]},
            "require": {"anchor": "model_call", "views": ["span"], "cardinality": "exactly_once", "match": "exact"},
        }))
        .expect("a fact")
    };
    let (a, b) = (fact("fact-001"), fact("fact-002"));
    let facts = [(&a, None), (&b, None)];
    let block = |text: &str| super::mutate::text_block("assistant", text, "t", "s", true);
    let view = View::plain_span(
        "t".to_string(),
        "s".to_string(),
        vec![
            block("A"),
            block("B"),
            block("A"),
            block("B"),
            block("AB"),
            block("AB"),
        ],
    );
    let runs = segment_runs(&view, &facts, &|_, _, _| true);
    assert_eq!(runs, [vec![0, 1], vec![0, 1]], "two complete runs");
    let mut showings = vec![vec![0, 1], vec![0, 1]];
    showings.extend(runs);
    assert_eq!(accounted(&showings), [false; 4]);
}

/// A span view composed from another trace's history counts only what had been said by the viewed call: a later
/// call's prompt in that trace is no fact the view can show, however its block got there.
#[test]
fn a_block_from_another_trace_counts_only_what_the_viewed_call_could_have_been_sent() {
    let (truth, mut recon, _) = super::mutations::baseline("claude-code/native/multi_turn");
    let first = recon
        .generations
        .iter()
        .filter(|g| g.typed)
        .min_by_key(|g| g.start)
        .expect("a model call")
        .clone();
    let last_trace = recon
        .generations
        .iter()
        .max_by_key(|g| g.start)
        .expect("a later call")
        .trace
        .clone();
    assert_ne!(first.trace, last_trace, "turns in traces of their own");
    let (_, before) = super::checked(&truth, &recon, &ViewRows::default());
    let in_its_trace = recon
        .views
        .iter()
        .position(|v| v.kind == ViewKind::Trace && v.key == last_trace)
        .expect("its trace view");
    let at = (0..recon.views[in_its_trace].blocks.len())
        .rev()
        .find(|&i| {
            let b = &recon.views[in_its_trace].blocks[i];
            b.is("user", "text") && b.trace == last_trace && before[in_its_trace][i].accounted
        })
        .expect("the later turn's prompt, a fact of the truth");
    let later_prompt = recon.views[in_its_trace].blocks[at].clone();
    let view = recon
        .views
        .iter()
        .position(|v| v.kind == ViewKind::Span && v.key == first.span)
        .expect("the first call's span view");
    recon.views[view].blocks.push(later_prompt);
    let (_, allowed) = super::checked(&truth, &recon, &ViewRows::default());
    let injected = recon.views[view].blocks.len() - 1;
    assert_eq!(
        allowed[view][injected],
        Allowance::default(),
        "the first call could not have been sent a later turn's prompt"
    );
}

/// A span id another trace reuses does not stretch the viewed span's reach: it is measured in the span's own trace.
#[test]
fn a_reused_span_id_does_not_stretch_the_reach() {
    let (_, recon, _) = super::mutations::baseline("claude-code/native/multi_turn");
    let first = recon
        .generations
        .iter()
        .min_by_key(|g| g.start)
        .expect("a span")
        .clone();
    let mut reused = recon
        .generations
        .iter()
        .max_by_key(|g| g.start)
        .expect("a later span")
        .clone();
    assert!(reused.start > first.start && reused.trace != first.trace);
    reused.span = first.span.clone();
    reused.ancestors.clear();
    let generations = [first.clone(), reused.clone()];
    assert_eq!(
        horizon(&generations, Some(&first.trace), &first.span),
        Some(first.start),
        "the viewed span's own trace"
    );
    assert_eq!(
        horizon(&generations, None, &first.span),
        Some(reused.start),
        "without the trace, every trace holding the id"
    );
}

/// A call shown under an id the wire did not give it is no copy of the call it is: two identical calls' blocks,
/// one replaced by the other, are one call shown twice, which the truth's two facts do not excuse.
#[test]
fn a_call_under_another_call_s_id_is_no_copy_of_it() {
    let (truth, mut recon, _) = super::mutations::baseline("langgraph/native/error");
    let named = |b: &Block| b.kind == "tool_use" && b.content["name"] == "book_flight";
    let view = recon
        .views
        .iter()
        .position(|v| {
            v.kind == ViewKind::Trace && v.blocks.iter().filter(|b| named(b)).count() == 2
        })
        .expect("a trace showing both identical calls");
    let calls: Vec<usize> = (0..recon.views[view].blocks.len())
        .filter(|&i| named(&recon.views[view].blocks[i]))
        .collect();
    let (first, second) = (calls[0], calls[1]);
    assert_ne!(
        recon.views[view].blocks[first].tool_use_id, recon.views[view].blocks[second].tool_use_id,
        "two calls, two ids"
    );
    recon.views[view].blocks[second] = recon.views[view].blocks[first].clone();
    let (_, after) = super::checked(&truth, &recon, &ViewRows::default());
    assert_eq!(
        (after[view][first], after[view][second]),
        (Allowance::default(), Allowance::default()),
        "either copy could be either call, the second only under the first call's id: exactly neither"
    );
}

/// A run of segments shows an answer only where every segment is in the answer's scope: one split across traces is
/// no showing of an answer one trace holds, and leaves that trace's whole copies standing.
#[test]
fn a_run_across_traces_is_no_showing_of_one_trace_s_answer() {
    let fact = |id: &str| -> Fact {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": "text", "role": "assistant", "conversation": "conv-1", "evidence": "wire",
            "value": {"text": "AB", "segments": ["A", "B"]},
            "require": {"anchor": "model_call", "views": ["span"], "cardinality": "exactly_once", "match": "exact"},
        }))
        .expect("a fact")
    };
    let (a, b) = (fact("fact-001"), fact("fact-002"));
    let facts = [(&a, None), (&b, None)];
    let block =
        |text: &str, trace: &str| super::mutate::text_block("assistant", text, trace, "s", true);
    let view = View::plain_span(
        "t".to_string(),
        "s".to_string(),
        vec![
            block("AB", "t1"),
            block("AB", "t1"),
            block("A", "t1"),
            block("B", "t2"),
        ],
    );
    let runs = segment_runs(&view, &facts, &|_, _, trace| trace == "t1");
    assert_eq!(
        runs,
        [Vec::<usize>::new()],
        "the run reaches outside the answer's trace"
    );
    let mut showings = vec![vec![0, 1], vec![0, 1]];
    showings.extend(runs);
    assert_eq!(accounted(&showings), [true, true, false]);
}

/// Copies of one content stand only on facts each shows exactly: two copies of a call under an id that is exactly
/// two facts' and another's by rewrite, beside a third call that is exactly those two, cannot all have one.
#[test]
fn copies_stand_only_on_facts_they_show_exactly() {
    // Facts 0 and 1 are `foo` with id x; fact 2 is `left__foo` with id y. The copies are `left__foo` with id x:
    // exactly 0 and 1, and 2 only under a rewritten id. The third showing is exactly 0 and 1.
    let adjacency = [vec![0, 1, 2], vec![0, 1, 2], vec![0, 1]];
    let exact = [vec![0, 1], vec![0, 1], vec![0, 1]];
    assert_eq!(
        accounted(&adjacency),
        [true, true, true],
        "broadly, all three find one"
    );
    assert_eq!(
        standing(&adjacency, &exact, &[vec![0, 1]])[..2],
        [false, false],
        "exactly, three showings for two facts"
    );
    // Without the third, the copies stand on the two facts they are.
    assert_eq!(
        standing(&adjacency[..2], &exact[..2], &[vec![0, 1]]),
        [true, true]
    );
}

/// A run of segments is no showing of a fact nothing can show: an unasserted fact with the same segments is not
/// one more answer for the run to be.
#[test]
fn a_run_spends_no_unasserted_fact() {
    let fact = |id: &str, asserted: bool| -> Fact {
        let mut fact = serde_json::json!({
            "id": id, "kind": "text", "role": "assistant", "conversation": "conv-1", "evidence": "wire",
            "value": {"text": "AB", "segments": ["A", "B"]},
            "require": {"anchor": "model_call", "views": ["span"], "cardinality": "exactly_once", "match": "exact"},
        });
        if !asserted {
            fact["require"] = serde_json::Value::Null;
        }
        serde_json::from_value(fact).expect("a fact")
    };
    let (a, b, c) = (
        fact("fact-001", true),
        fact("fact-002", true),
        fact("fact-003", false),
    );
    let facts = [(&a, None), (&b, None), (&c, None)];
    let block = |text: &str| super::mutate::text_block("assistant", text, "t", "s", true);
    let view = View::plain_span(
        "t".to_string(),
        "s".to_string(),
        vec![block("AB"), block("AB"), block("A"), block("B")],
    );
    let runs = segment_runs(&view, &facts, &|_, _, _| true);
    assert_eq!(
        runs,
        [vec![0, 1]],
        "the run is either asserted answer, never the third"
    );
    let mut showings = vec![vec![0, 1], vec![0, 1]];
    showings.extend(runs);
    assert_eq!(accounted(&showings), [false; 3]);
}

/// Two sets of copies standing on the same exact facts stand together or not at all: four copies of a call that is
/// exactly two facts are not excused by two more facts each set could reach only under a rewritten id.
#[test]
fn sets_of_copies_stand_on_their_exact_facts_together() {
    // Facts 0 and 1 are `foo` with id x, 2 and 3 `foo` with id y. Both sets show x: exactly 0 and 1, the rest by
    // rewrite.
    let adjacency = vec![vec![0, 1, 2, 3]; 4];
    let exact = vec![vec![0, 1]; 4];
    assert_eq!(
        standing(&adjacency, &exact, &[vec![0, 1]]),
        [true; 4],
        "one set alone"
    );
    assert_eq!(
        standing(&adjacency, &exact, &[vec![0, 1], vec![2, 3]]),
        [false; 4]
    );
}

/// Copies are the invariant's: one content, whatever side-channel id each carries. Two copies of one call, one of
/// them without the id beside its content, are still two copies that must stand on facts they show exactly.
#[test]
fn copies_are_one_content_whatever_side_channel_id_they_carry() {
    let (truth, mut recon, _) = super::mutations::baseline("langgraph/native/error");
    let named = |b: &Block| b.kind == "tool_use" && b.content["name"] == "book_flight";
    let view = recon
        .views
        .iter()
        .position(|v| {
            v.kind == ViewKind::Trace && v.blocks.iter().filter(|b| named(b)).count() == 2
        })
        .expect("a trace showing both identical calls");
    let calls: Vec<usize> = (0..recon.views[view].blocks.len())
        .filter(|&i| named(&recon.views[view].blocks[i]))
        .collect();
    let mut copy = recon.views[view].blocks[calls[0]].clone();
    copy.tool_use_id = None;
    copy.refresh();
    assert_ne!(copy.identity, recon.views[view].blocks[calls[0]].identity);
    recon.views[view].blocks[calls[1]] = copy;
    let groups: Vec<Vec<String>> = recon
        .views
        .iter()
        .map(|v| {
            v.blocks
                .iter()
                .map(|b| format!("{}|{}", b.trace, b.digest))
                .collect()
        })
        .collect();
    let rows = ViewRows {
        groups,
        ..ViewRows::default()
    };
    let (_, allowed) = super::checked(&truth, &recon, &rows);
    assert_eq!(
        (
            allowed[view][calls[0]].accounted,
            allowed[view][calls[1]].accounted
        ),
        (false, false),
        "one call twice, whatever id rides beside it"
    );
}

/// Whichever matcher lets a block show two different facts - a substring of an error, a modality without a digest
/// - the block is exactly neither, so copies of it stand on no fact.
#[test]
fn a_block_showing_two_different_facts_is_exactly_neither() {
    let (mut truth, recon, _) = super::mutations::baseline("adk/native/multi_agent");
    let view = recon
        .views
        .iter()
        .position(|v| {
            v.kind == ViewKind::Trace
                && v.blocks
                    .iter()
                    .any(|b| b.text().is_some_and(|t| t.starts_with("For context:")))
        })
        .expect("the trace quoting the steps");
    let at = recon.views[view]
        .blocks
        .iter()
        .position(|b| b.text().is_some_and(|t| t.starts_with("For context:")))
        .expect("a preamble");
    let (_, before) = super::checked(&truth, &recon, &ViewRows::default());
    assert_eq!(before[view][at].facts, 4, "four identical facts");
    // One preamble fact now states a different text the preamble's still shows exactly: a second value.
    let fact = truth
        .facts
        .iter_mut()
        .find(|f| f.text().starts_with("For context:"))
        .expect("a preamble fact");
    fact.value["note"] = serde_json::json!("another value");
    let (_, after) = super::checked(&truth, &recon, &ViewRows::default());
    assert_eq!(after[view][at].facts, 0);
}

/// Restricting copies to their exact edges does not cut them off from the rivals their content still competes
/// with: two copies exactly two facts, beside two texts containing the same value's third fact, are four showings
/// of three facts, not two of two.
#[test]
fn restricted_copies_still_compete_with_their_broad_rivals() {
    let adjacency = [vec![0, 1, 2], vec![0, 1, 2], vec![2], vec![2]];
    let exact = [vec![0, 1], vec![0, 1], Vec::new(), Vec::new()];
    assert_eq!(
        standing(&adjacency, &exact, &[vec![0, 1]]),
        [false; 4],
        "four showings in one component, three facts"
    );
}

/// A result rendered under an assigned id still competes for its fact: two idless copies of an error two facts
/// hold cannot spend those facts while the rendered results stand on them.
#[test]
fn a_result_under_an_assigned_id_still_competes_for_its_fact() {
    let (truth, mut recon, _) = super::mutations::baseline("browser-use/native/error");
    let identical: Vec<&Fact> = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_result" && f.value["name"] == "book_flight")
        .collect();
    assert_eq!(identical.len(), 2, "two identical result facts");
    let view = recon
        .views
        .iter()
        .position(|v| {
            v.kind == ViewKind::Trace
                && v.blocks
                    .iter()
                    .filter(|b| b.kind == "tool_result" && b.result_call_id().is_some())
                    .count()
                    >= 2
        })
        .expect("the trace showing both results under assigned ids");
    let rendered = recon.views[view]
        .blocks
        .iter()
        .position(|b| {
            b.kind == "tool_result"
                && b.content["content"]
                    .as_str()
                    .is_some_and(|t| t.contains("booking system is offline"))
        })
        .expect("a rendered result");
    // Two idless copies of the same error, each at a place of its own.
    let mut copy = recon.views[view].blocks[rendered].clone();
    copy.tool_use_id = None;
    copy.content["tool_use_id"] = serde_json::Value::Null;
    copy.refresh();
    for position in ["998.0", "999.0"] {
        let mut copy = copy.clone();
        copy.position = position.to_string();
        recon.views[view].blocks.push(copy);
    }
    let (_, allowed) = super::checked(&truth, &recon, &ViewRows::default());
    let added = recon.views[view].blocks.len() - 2;
    assert_eq!(allowed[view][added].facts, 2, "the copies show both facts");
    assert!(
        !allowed[view][added].accounted && !allowed[view][added + 1].accounted,
        "four results cannot spend two facts: {:?}",
        (allowed[view][added], allowed[view][added + 1])
    );
}
