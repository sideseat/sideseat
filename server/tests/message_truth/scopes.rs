//! The scopes the placement checks run over: each call's span view, and the trace, session and feed views -
//! the conversation's, and each split-off trace's of its own (`propagation`).

use std::collections::BTreeSet;

use super::checks::{Assigned, Context, Placed, Scope};
use super::predicates::{Shows, shows};
use super::recon::ViewKind;
use super::truth::Fact;
use super::{Violation, ViolationView};

pub(super) fn scopes<'a>(context: &'a Context<'a>) -> Vec<Scope<'a>> {
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
        // The calls recorded on this span too (`call_span_not_exported`): their parts are owed here, once and in
        // order, like its own call's.
        let sharing = truth
            .calls
            .iter()
            .filter(|c| context.matching.shared.get(&c.id) == Some(&index));
        let facts = std::iter::once(call)
            .chain(sharing)
            .flat_map(|c| &c.outputs)
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
            split: None,
            foreign: Vec::new(),
        });
    }
    let every_trace_in_a_session = recon
        .views
        .iter()
        .filter(|v| v.kind == ViewKind::Trace)
        .all(|v| recon.session_of_trace.contains_key(&v.key));
    let splits = &context.splits;
    let split_off = |trace: &str| splits.traces.iter().any(|t| t.trace == trace);
    // What a split-off trace's view owes, which the conversation's views leave to it: what only its raw
    // carriers hold, and what is at home there - its call's answer.
    let left_to_split = |f: &Fact| {
        splits.only_there.contains(&f.id)
            || context
                .home_trace
                .get(f.id.as_str())
                .is_some_and(|t| split_off(t))
    };
    for kind in [ViewKind::Trace, ViewKind::Session, ViewKind::Feed] {
        let all: Vec<Placed<'a>> = recon
            .views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.kind == kind)
            .flat_map(|(vi, v)| v.blocks.iter().enumerate().map(move |(i, b)| (vi, i, b)))
            .collect();
        let blocks = all
            .iter()
            .copied()
            .filter(|(_, _, b)| kind == ViewKind::Session || !split_off(&b.trace))
            .collect();
        let facts = truth
            .facts
            .iter()
            .filter(|f| context.asserted_in(f, kind.name()))
            .filter(|f| !left_to_split(f))
            .filter(|f| {
                // A session view exists only for a trace that belongs to a session.
                kind != ViewKind::Session
                    || match context.home_trace.get(f.id.as_str()) {
                        Some(trace) => recon.session_of_trace.contains_key(trace),
                        None => every_trace_in_a_session,
                    }
            })
            .collect();
        let foreign = truth
            .facts
            .iter()
            .filter(|f| context.asserted_in(f, kind.name()) && left_to_split(f))
            .collect();
        scopes.push(Scope {
            kind,
            blocks,
            facts,
            by_trace: true,
            split: None,
            foreign,
        });
        if kind == ViewKind::Session {
            continue;
        }
        for split in &splits.traces {
            let (facts, foreign): (Vec<&Fact>, Vec<&Fact>) = truth
                .facts
                .iter()
                .filter(|f| context.asserted_in(f, kind.name()))
                .partition(|f| {
                    split.history.contains(&f.id)
                        || context.home_trace.get(f.id.as_str()) == Some(&split.trace)
                });
            scopes.push(Scope {
                kind,
                blocks: all
                    .iter()
                    .copied()
                    .filter(|(_, _, b)| b.trace == split.trace)
                    .collect(),
                facts,
                by_trace: false,
                split: Some(split),
                foreign,
            });
        }
    }
    scopes
}

/// A block showing a fact another scope of this kind owes, which no fact here claims: a copy of a split-off
/// trace's own content in the conversation's views, or of the conversation's in the split-off trace's.
pub(super) fn report_foreign(scope: &Scope<'_>, assigned: &Assigned, out: &mut Vec<Violation>) {
    let claimed: BTreeSet<usize> = assigned
        .block_of
        .values()
        .copied()
        .chain(assigned.consumed.iter().copied())
        .collect();
    for fact in &scope.foreign {
        let call_id = fact.value.get("call_id").and_then(|v| v.as_str());
        let copies = (0..scope.blocks.len())
            .filter(|b| !claimed.contains(b))
            .filter(|&b| shows(fact, scope.blocks[b].2, call_id) != Shows::No)
            .count();
        if copies > 0 {
            out.push(Violation::new(
                ViolationView::from(scope.kind),
                &format!("{}.leaked", fact.kind),
                &fact.id,
                format!("also shown in {copies} block(s) of a view that does not own it"),
            ));
        }
    }
}
