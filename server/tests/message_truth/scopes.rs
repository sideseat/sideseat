//! The scopes the placement checks run over: each call's span view, and the trace, session and feed views -
//! the conversation's, and each split-off trace's of its own (`propagation`).

use std::collections::BTreeSet;

use super::checks::{Assigned, Context, Placed, Scope};
use super::predicates::{Shows, shows};
use super::recon::ViewKind;
use super::truth::Fact;
use super::{Violation, ViolationView};

/// What one span's payload records of silent calls: the facts, and the calls they belong to.
type Recorded<'a> = (Vec<&'a Fact>, BTreeSet<&'a str>);

/// Each (trace, span)'s recorded facts, from each fact's homes. Identical content is owed once: two calls
/// answering the same words both name every span recording those words, and a span showing them once has shown
/// what it records.
fn records_by_span<'a>(
    homes: impl Iterator<Item = (&'a Fact, &'a BTreeSet<(String, String)>)>,
) -> std::collections::BTreeMap<(&'a str, &'a str), Recorded<'a>> {
    let mut carried: std::collections::BTreeMap<(&str, &str), Recorded<'_>> =
        std::collections::BTreeMap::new();
    for (fact, spans) in homes {
        for (trace, span) in spans {
            let entry = carried.entry((trace.as_str(), span.as_str())).or_default();
            if entry
                .0
                .iter()
                .any(|owed| owed.kind == fact.kind && owed.value == fact.value)
            {
                continue;
            }
            entry.0.push(fact);
            entry.1.extend(fact.call.as_deref());
        }
    }
    carried
}

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
        let sharing: Vec<&crate::message_truth::truth::Call> = truth
            .calls
            .iter()
            .filter(|c| context.matching.shared.get(&c.id) == Some(&index))
            .collect();
        let outputs: std::collections::BTreeSet<&str> = std::iter::once(call)
            .chain(sharing.iter().copied())
            .flat_map(|c| &c.outputs)
            .map(String::as_str)
            .collect();
        let calls: Vec<&str> = std::iter::once(call)
            .chain(sharing.iter().copied())
            .map(|c| c.id.as_str())
            .collect();
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
            own: Some(super::checks::ScopeOwn {
                calls,
                label: generation.label.as_str(),
                outputs,
            }),
        });
    }
    // The spans whose own payload records a part of a call whose request span is silent
    // (`call_span_records_no_output`): the call has no span to be checked on, so each span recording its parts
    // owes those parts in its own view, as a call's span owes its call's - grouped per span, once and in order.
    let carried = records_by_span(context.carried_spans.iter().filter_map(|(fact_id, homes)| {
        context
            .fact(fact_id)
            .filter(|f| context.asserted_in(f, "span"))
            .map(|f| (f, homes))
    }));
    for ((trace, span), (facts, calls)) in carried {
        let Some((view_index, view)) = recon.views.iter().enumerate().find(|(_, v)| {
            v.kind == ViewKind::Span && v.key == span && (v.trace.is_empty() || v.trace == trace)
        }) else {
            continue;
        };
        let label = recon
            .generations
            .iter()
            .find(|g| g.span == span && g.trace == trace)
            .map_or(span, |g| g.label.as_str());
        let outputs: BTreeSet<&str> = facts.iter().map(|f| f.id.as_str()).collect();
        scopes.push(Scope {
            kind: ViewKind::Span,
            blocks: view
                .blocks
                .iter()
                .enumerate()
                .map(|(i, b)| (view_index, i, b))
                .collect(),
            facts,
            by_trace: false,
            split: None,
            foreign: Vec::new(),
            own: Some(super::checks::ScopeOwn {
                calls: calls.into_iter().collect(),
                label,
                outputs,
            }),
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
            own: None,
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
                own: None,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::records_by_span;
    use crate::message_truth::truth::Fact;

    /// Two calls answering the same words, each on its own span: each span records the words once and owes them
    /// once - not both facts, which a span legitimately showing one answer could never satisfy.
    #[test]
    fn identical_answers_are_owed_once_per_span() {
        let (truth, _, _) = crate::message_truth::mutations::baseline("codex/native/tool_use");
        let template = truth
            .facts
            .iter()
            .find(|f| f.kind == "text")
            .expect("an answer")
            .clone();
        let answer = |id: &str, call: &str| -> Fact {
            let mut fact = template.clone();
            fact.id = id.to_string();
            fact.call = Some(call.to_string());
            fact.value = serde_json::json!({"text": "the same words"});
            fact
        };
        let (a, b) = (answer("fact-1", "call-1"), answer("fact-2", "call-2"));
        let spans: BTreeSet<(String, String)> = [("t", "s1"), ("t", "s2")]
            .into_iter()
            .map(|(t, s)| (t.to_string(), s.to_string()))
            .collect();
        let carried = records_by_span([(&a, &spans), (&b, &spans)].into_iter());
        assert_eq!(carried.len(), 2);
        for (facts, _) in carried.values() {
            assert_eq!(facts.len(), 1, "one answer owed per span");
        }
    }
}
